//! Context compaction: checkpoint discovery, cut-point selection, token
//! accounting, conversation serialization, config, and the pluggable
//! compactor seam.
//!
//! Design: `docs/superpowers/specs/2026-07-20-context-compaction-design.md`.
//! The session log is never truncated; a `CompactionCheckpoint` event is
//! appended and provider context rebuilds as summary + verbatim kept tail.

use std::sync::Arc;

use crate::provider::NativeRequestEnvelope;
use crate::rig_adapter::{RigProviderAdapterConfig, RigProviderConfig};

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::session::{
    CompactionReason, EntryId, Role, SessionEvent, SessionLog, TurnId, TurnOutcome,
};

pub const COMPACTION_DEFAULT_RESERVE_TOKENS: u64 = 16_384;
pub const COMPACTION_DEFAULT_KEEP_RECENT_TOKENS: u64 = 20_000;
pub const COMPACTION_DEFAULT_AUTO_THRESHOLD_PERCENT: u8 = 90;
/// Tool-result bodies are bounded to this many characters inside the
/// serialized conversation handed to the summarizer.
pub const COMPACTION_SERIALIZED_TOOL_RESULT_MAX_CHARS: usize = 2_000;

/// `compaction` section of `.yach/config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    pub enabled: bool,
    pub compactor: String,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
    pub auto_threshold_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactorKind {
    Auto,
    Summary,
    OpenAiResponses,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compactor: String::from("auto"),
            reserve_tokens: COMPACTION_DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: COMPACTION_DEFAULT_KEEP_RECENT_TOKENS,
            auto_threshold_percent: COMPACTION_DEFAULT_AUTO_THRESHOLD_PERCENT,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct CompactionConfigFile {
    compaction: CompactionConfig,
}

impl CompactionConfig {
    /// Load from user (`~/.yach/config.json`) then project scope; project
    /// values win. Unreadable or invalid config fails closed to defaults.
    #[must_use]
    pub fn load_for_project(project_root: Option<&Path>) -> Self {
        let user = user_config_path().and_then(|path| load_compaction_config(&path));
        let project = project_root
            .map(|root| root.join(".yach").join("config.json"))
            .and_then(|path| load_compaction_config(&path));
        project.or(user).unwrap_or_default()
    }

    #[must_use]
    pub fn compactor_kind(&self) -> Option<CompactorKind> {
        match self.compactor.as_str() {
            "auto" => Some(CompactorKind::Auto),
            "summary" => Some(CompactorKind::Summary),
            "openai-responses" => Some(CompactorKind::OpenAiResponses),
            _ => None,
        }
    }

    /// Percent of the usable window at which auto-compaction fires,
    /// clamped to a sane range.
    #[must_use]
    pub fn auto_threshold_percent_clamped(&self) -> u8 {
        self.auto_threshold_percent.clamp(10, 100)
    }
}

fn user_config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".yach")
            .join("config.json"),
    )
}

fn load_compaction_config(path: &Path) -> Option<CompactionConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<CompactionConfigFile>(&raw)
        .ok()
        .map(|file| file.compaction)
}

/// Rough token estimate for accounting purposes. Precision is deliberately
/// loose; the trigger threshold carries slack by design.
#[must_use]
pub fn estimate_text_tokens(text: &str) -> u64 {
    (text.len() as u64).div_ceil(4)
}

/// Provider-visible token estimate for one session event.
#[must_use]
pub fn estimate_event_tokens(event: &SessionEvent) -> u64 {
    match event {
        SessionEvent::EntryAppended { text, .. } => estimate_text_tokens(text),
        SessionEvent::ToolRequestRecorded {
            argument_content, ..
        } => argument_content.as_deref().map_or(0, estimate_text_tokens),
        SessionEvent::ToolExecutionFinished { result_content, .. } => {
            result_content.as_deref().map_or(0, estimate_text_tokens)
        }
        SessionEvent::CompactionCheckpoint { summary, .. } => estimate_text_tokens(summary),
        SessionEvent::TurnFinished { .. }
        | SessionEvent::MetricRecorded { .. }
        | SessionEvent::StaticContextIncluded { .. }
        | SessionEvent::PermissionDecisionRecorded { .. }
        | SessionEvent::EditTraceRecorded { .. }
        | SessionEvent::EditTransactionPrepared { .. }
        | SessionEvent::EditTransactionFinished { .. } => 0,
    }
}

/// Context accounting inputs shared by the auto-compaction trigger and the
/// TUI context meter: `usable = context_window − max_output_tokens −
/// reserve_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub reserve_tokens: u64,
}

impl ContextBudget {
    #[must_use]
    pub fn usable_tokens(&self) -> u64 {
        self.context_window
            .saturating_sub(self.max_output_tokens)
            .saturating_sub(self.reserve_tokens)
    }

    /// Estimated percent of the usable window in use; saturates at 255.
    #[must_use]
    pub fn used_percent(&self, used_tokens: u64) -> u8 {
        let usable = self.usable_tokens().max(1);
        u8::try_from(used_tokens.saturating_mul(100) / usable).unwrap_or(u8::MAX)
    }
}

/// Turn id of a token-bearing turn-scoped event. Checkpoints are
/// deliberately excluded: the newest summary feeds provider context
/// regardless of how the turn that produced it ended.
fn turn_scoped_event_turn_id(event: &SessionEvent) -> Option<&TurnId> {
    match event {
        SessionEvent::EntryAppended { turn_id, .. }
        | SessionEvent::ToolRequestRecorded { turn_id, .. }
        | SessionEvent::ToolExecutionFinished { turn_id, .. } => Some(turn_id),
        _ => None,
    }
}

/// Turns whose events feed provider context: completed turns plus the
/// newest turn while it is still in flight (no `TurnFinished` yet).
/// Mirrors the filter `provider_messages_from_log` applies so the
/// meter counts what a request would actually contain — failed and
/// cancelled turns are excluded from both.
fn context_turns(log: &SessionLog) -> std::collections::HashSet<&TurnId> {
    let mut finished = std::collections::HashSet::new();
    let mut context = std::collections::HashSet::new();
    for event in &log.events {
        if let SessionEvent::TurnFinished {
            turn_id, outcome, ..
        } = event
        {
            finished.insert(turn_id);
            if matches!(outcome, TurnOutcome::Completed) {
                context.insert(turn_id);
            }
        }
    }
    if let Some(turn_id) = log.events.iter().rev().find_map(turn_scoped_event_turn_id)
        && !finished.contains(turn_id)
    {
        context.insert(turn_id);
    }
    context
}

/// Estimated tokens the log currently contributes to provider context:
/// the newest checkpoint's summary plus everything from its kept boundary
/// forward, or the whole log when no checkpoint exists. Events from
/// failed or cancelled turns are excluded, matching the provider context
/// rebuild (dogfood finding 2026-07-24: counting them inflated the meter
/// by the failed turn's weight while every actual request excluded it).
/// Feeds the TUI context meter with the same accounting family as the
/// trigger.
#[must_use]
pub fn estimate_current_context_tokens(log: &SessionLog) -> u64 {
    let context_turns = context_turns(log);
    let in_context = |event: &&SessionEvent| {
        turn_scoped_event_turn_id(event).is_none_or(|turn_id| context_turns.contains(turn_id))
    };
    newest_compaction_checkpoint(log).map_or_else(
        || {
            log.events
                .iter()
                .filter(in_context)
                .map(estimate_event_tokens)
                .sum()
        },
        |view| {
            // The newest checkpoint event sits inside the kept slice; skip
            // it so its summary is not counted twice.
            estimate_text_tokens(view.summary).saturating_add(
                log.events[view.kept_start_index.min(log.events.len())..]
                    .iter()
                    .filter(|event| !matches!(event, SessionEvent::CompactionCheckpoint { .. }))
                    .filter(in_context)
                    .map(estimate_event_tokens)
                    .sum(),
            )
        },
    )
}

/// The newest checkpoint in a log, as the context-assembly view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewestCheckpointView<'a> {
    pub summary: &'a str,
    pub details: &'a serde_json::Value,
    pub first_kept_entry_id: &'a EntryId,
    /// Event index the kept transcript resumes from: the position of the
    /// `first_kept_entry_id` entry, falling back to the event after the
    /// checkpoint itself when that entry is not found (Pi's fallback).
    pub kept_start_index: usize,
}

#[must_use]
pub fn newest_compaction_checkpoint(log: &SessionLog) -> Option<NewestCheckpointView<'_>> {
    let (checkpoint_index, summary, details, first_kept_entry_id) = log
        .events
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, event)| match event {
            SessionEvent::CompactionCheckpoint {
                summary,
                details,
                first_kept_entry_id,
                ..
            } => Some((index, summary.as_str(), details, first_kept_entry_id)),
            _ => None,
        })?;
    let kept_start_index = log
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            SessionEvent::EntryAppended { entry_id, .. } if entry_id == first_kept_entry_id => {
                Some(index)
            }
            _ => None,
        })
        .unwrap_or(checkpoint_index.saturating_add(1));
    Some(NewestCheckpointView {
        summary,
        details,
        first_kept_entry_id,
        kept_start_index,
    })
}

/// A selected compaction cut: which events fold into the summary and where
/// the verbatim kept tail begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionCut {
    pub first_kept_entry_id: EntryId,
    pub kept_start_index: usize,
    /// Events to fold into the summary: from the previous checkpoint's
    /// kept boundary (or session start) up to the kept tail.
    pub fold_range: std::ops::Range<usize>,
}

/// Select the cut for a new compaction. The kept tail targets
/// `keep_recent_tokens`, cut at a turn boundary (a user entry); when a
/// single turn exceeds the whole budget the cut falls back to any entry
/// boundary inside it. Entry boundaries never separate a tool call from
/// its result (tool activity is recorded as adjacent non-entry events).
/// Returns `None` when there is nothing to fold.
#[must_use]
pub fn select_compaction_cut(log: &SessionLog, keep_recent_tokens: u64) -> Option<CompactionCut> {
    let fold_start = newest_compaction_checkpoint(log).map_or(0, |view| view.kept_start_index);
    let events = &log.events[fold_start..];

    // Walk backward accumulating estimates to find where the kept budget
    // is exhausted (relative index of the oldest event that still fits).
    let mut budget_start = events.len();
    let mut accumulated: u64 = 0;
    for (index, event) in events.iter().enumerate().rev() {
        accumulated = accumulated.saturating_add(estimate_event_tokens(event));
        if accumulated > keep_recent_tokens {
            break;
        }
        budget_start = index;
    }
    if budget_start == 0 {
        // Everything fits in the kept budget: nothing worth folding.
        return None;
    }
    if !events[budget_start..]
        .iter()
        .any(|event| matches!(event, SessionEvent::EntryAppended { .. }))
    {
        // An oversized newest entry can be followed by zero-token metadata.
        // Keep that entry as the mandatory tail rather than searching only
        // after it and concluding there is no valid cut.
        budget_start = events[..budget_start]
            .iter()
            .rposition(|event| matches!(event, SessionEvent::EntryAppended { .. }))?;
    }

    // Preferred cut: the first turn boundary (user entry) at or after the
    // budget point. Fallback for one oversized turn: any entry boundary.
    let cut = events
        .iter()
        .enumerate()
        .skip(budget_start)
        .find_map(|(index, event)| match event {
            SessionEvent::EntryAppended {
                entry_id,
                role: Role::User,
                ..
            } => Some((index, entry_id.clone())),
            _ => None,
        })
        .or_else(|| {
            events
                .iter()
                .enumerate()
                .skip(budget_start)
                .find_map(|(index, event)| match event {
                    SessionEvent::EntryAppended { entry_id, .. } => Some((index, entry_id.clone())),
                    _ => None,
                })
        });
    let (relative_cut_index, first_kept_entry_id) = cut?;
    if relative_cut_index == 0 {
        return None;
    }
    Some(CompactionCut {
        first_kept_entry_id,
        kept_start_index: fold_start + relative_cut_index,
        fold_range: fold_start..fold_start + relative_cut_index,
    })
}

/// Flatten events into text for the summarizer, so it treats the history
/// as material rather than a conversation to continue. Tool-result bodies
/// are bounded; non-conversation events are skipped.
#[must_use]
pub fn serialize_events_for_summary(events: &[SessionEvent]) -> String {
    let mut lines = Vec::new();
    for event in events {
        match event {
            SessionEvent::EntryAppended { role, text, .. } => {
                let label = match role {
                    Role::User => "[User]",
                    Role::Assistant => "[Assistant]",
                    Role::Tool => "[Tool]",
                    Role::System => "[System]",
                };
                lines.push(format!("{label}: {text}"));
            }
            SessionEvent::ToolRequestRecorded {
                tool_name,
                argument_content,
                ..
            } => {
                let arguments = argument_content.as_deref().unwrap_or("{}");
                lines.push(format!(
                    "[Tool call]: {tool_name}({})",
                    bounded_chars(arguments, COMPACTION_SERIALIZED_TOOL_RESULT_MAX_CHARS)
                ));
            }
            SessionEvent::ToolExecutionFinished {
                outcome,
                result_content,
                ..
            } => {
                let content = result_content.as_deref().unwrap_or("(not retained)");
                lines.push(format!(
                    "[Tool result {outcome:?}]: {}",
                    bounded_chars(content, COMPACTION_SERIALIZED_TOOL_RESULT_MAX_CHARS)
                ));
            }
            SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. }
            | SessionEvent::CompactionCheckpoint { .. } => {}
        }
    }
    lines.join("\n")
}

fn bounded_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_owned();
    }
    let bounded: String = text.chars().take(max_chars).collect();
    format!("{bounded}... [{} chars truncated]", char_count - max_chars)
}

/// Cumulative read/modified file tracking carried across checkpoints in
/// the summary compactor's `details` (Pi's pattern): merge the previous
/// checkpoint's lists with file paths from the events being folded.
#[must_use]
pub fn merge_compaction_file_details(
    previous_details: Option<&serde_json::Value>,
    folded_events: &[SessionEvent],
) -> serde_json::Value {
    let mut read_files = details_string_set(previous_details, "read_files");
    let mut modified_files = details_string_set(previous_details, "modified_files");
    for event in folded_events {
        let SessionEvent::ToolRequestRecorded {
            tool_name,
            argument_content: Some(arguments),
            ..
        } = event
        else {
            continue;
        };
        let Some(path) = serde_json::from_str::<serde_json::Value>(arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("path")
                    .and_then(|path| path.as_str())
                    .map(str::to_owned)
            })
        else {
            continue;
        };
        match tool_name.as_str() {
            "read_text_file" => {
                read_files.insert(path);
            }
            "edit_text_file" | "create_text_file" => {
                modified_files.insert(path);
            }
            _ => {}
        }
    }
    serde_json::json!({
        "read_files": read_files.into_iter().collect::<Vec<_>>(),
        "modified_files": modified_files.into_iter().collect::<Vec<_>>(),
    })
}

fn details_string_set(
    details: Option<&serde_json::Value>,
    key: &str,
) -> std::collections::BTreeSet<String> {
    details
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Authenticated provider context retained with a compaction request.
///
/// The adapter remains opaque in diagnostics: credentials are exposed only
/// while constructing the native HTTP request.
#[derive(Clone)]
pub struct CompactionProviderContext {
    pub provider: String,
    pub wire: String,
    pub model: String,
    /// Stable active connection/endpoint provenance. It is metadata only and
    /// deliberately excludes credentials.
    pub connection: String,
    pub responses_compact: Option<bool>,
    pub adapter: Arc<RigProviderAdapterConfig>,
}

/// Everything core needs to prepare one compaction checkpoint. Owned values
/// let the optional native provider call move work across await points.
#[derive(Clone)]
pub struct CompactionPreparation {
    pub serialized_conversation: String,
    pub previous_summary: Option<String>,
    pub previous_details: Option<serde_json::Value>,
    pub first_kept_entry_id: EntryId,
    pub tokens_before: u64,
    pub reason: CompactionReason,
    pub focus_instructions: Option<String>,
    pub provider: Arc<CompactionProviderContext>,
    /// Exact Responses envelope of the request that triggered compaction.
    pub native_request: Option<NativeRequestEnvelope>,
}

/// Versioned provider-native replacement window returned by `/responses/compact`.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeCompactionArtifact {
    pub version: u8,
    pub provider: String,
    pub wire: String,
    pub model: String,
    /// Provenance must match the active connection before replay can resume.
    pub connection: String,
    pub window: Vec<serde_json::Value>,
}
impl std::fmt::Debug for NativeCompactionArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let connection_digest = format!("{:x}", Sha256::digest(self.connection.as_bytes()));
        formatter
            .debug_struct("NativeCompactionArtifact")
            .field("version", &self.version)
            .field("provider", &self.provider)
            .field("wire", &self.wire)
            .field("model", &self.model)
            .field("connection_fingerprint", &&connection_digest[..12])
            .field("window_items", &self.window.len())
            .finish()
    }
}

/// A successful native compactor call. Core still owns the portable summary,
/// accounting, checkpoint detail merge, and persistence.
#[derive(Clone, PartialEq, Eq)]
pub struct NativeCompactionOutcome {
    pub artifact: NativeCompactionArtifact,
}

impl std::fmt::Debug for NativeCompactionOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeCompactionOutcome")
            .field("artifact", &self.artifact)
            .finish()
    }
}

/// The only native compact window accepted at a trust boundary.
///
/// A usable Responses replay window must contain the provider's compaction
/// item and every passthrough item must be a JSON object, which is the shape
/// Rig can serialize through `InputItem::Unknown`.
#[must_use]
pub fn native_window_is_replayable(window: &[serde_json::Value]) -> bool {
    !window.is_empty()
        && window.iter().all(serde_json::Value::is_object)
        && window
            .iter()
            .any(|item| item.get("type").and_then(serde_json::Value::as_str) == Some("compaction"))
}

/// Redacted native-compaction failures. No variant carries response bodies,
/// request data, or credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionError {
    UnsupportedProvider { provider: String },
    MissingNativeRequest,
    Timeout,
    Transport,
    HttpStatus { status: u16 },
    Decode,
    InvalidOutput,
}

pub type CompactionFuture =
    Pin<Box<dyn Future<Output = Result<NativeCompactionOutcome, CompactionError>> + Send>>;

/// Compactor seam: implementations produce a provider-native replacement
/// artifact only. Core owns cut selection, the mandatory portable summary
/// call, accounting, detail merging, checkpoint writes, and replay mutation.
pub trait Compactor: Send + Sync {
    fn compact(&self, preparation: CompactionPreparation) -> CompactionFuture;
}

/// OpenAI Responses implementation of `/responses/compact`.
pub struct OpenAiResponsesCompactor;

impl Compactor for OpenAiResponsesCompactor {
    fn compact(&self, preparation: CompactionPreparation) -> CompactionFuture {
        Box::pin(async move {
            let CompactionPreparation {
                provider,
                native_request,
                ..
            } = preparation;
            if provider.provider != "openai"
                || provider.wire != "openai-responses"
                || !matches!(provider.adapter.provider, RigProviderConfig::OpenAi { .. })
            {
                return Err(CompactionError::UnsupportedProvider {
                    provider: provider.provider.clone(),
                });
            }
            let Some(native_request) = native_request else {
                return Err(CompactionError::MissingNativeRequest);
            };
            let RigProviderConfig::OpenAi { api_key, base_url } = &provider.adapter.provider else {
                return Err(CompactionError::UnsupportedProvider {
                    provider: provider.provider.clone(),
                });
            };
            let base_url = base_url
                .as_deref()
                .unwrap_or("https://api.openai.com/v1")
                .trim_end_matches('/');
            let url = format!("{base_url}/responses/compact");
            let body = serde_json::json!({
                "model": provider.model,
                "input": native_request.input,
                "instructions": native_request.instructions,
            });
            let client = reqwest::Client::builder()
                .timeout(provider.adapter.timeout)
                .build()
                .map_err(|_| CompactionError::Transport)?;
            let request =
                api_key.with_exposed(|key| client.post(&url).bearer_auth(key).json(&body));
            let response = request
                .send()
                .await
                .map_err(|error| compaction_transport_error(&error))?;
            if !response.status().is_success() {
                return Err(CompactionError::HttpStatus {
                    status: response.status().as_u16(),
                });
            }
            let body = response
                .bytes()
                .await
                .map_err(|error| compaction_transport_error(&error))?;
            let value = serde_json::from_slice::<serde_json::Value>(&body)
                .map_err(|_| CompactionError::Decode)?;
            let Some(window) = value.get("output").and_then(serde_json::Value::as_array) else {
                return Err(CompactionError::InvalidOutput);
            };
            if !native_window_is_replayable(window) {
                return Err(CompactionError::InvalidOutput);
            }
            Ok(NativeCompactionOutcome {
                artifact: NativeCompactionArtifact {
                    version: 1,
                    provider: provider.provider.clone(),
                    wire: provider.wire.clone(),
                    model: provider.model.clone(),
                    connection: provider.connection.clone(),
                    window: window.clone(),
                },
            })
        })
    }
}

fn compaction_transport_error(error: &reqwest::Error) -> CompactionError {
    if error.is_timeout() {
        CompactionError::Timeout
    } else {
        CompactionError::Transport
    }
}

/// Fixed schema shared by the summary prompt and its anchored-iteration
/// variant. Section 2 restates user instructions verbatim: the measured
/// failure mode of compaction is silently dropping standing instructions.
pub const COMPACTION_SUMMARY_SCHEMA: &str = "\
1. Goal and intent
2. User instructions and constraints (restate every standing user \
instruction verbatim, word-for-word; these remain in effect)
3. Progress: done / in progress / blocked
4. Key decisions, with rationale
5. Files touched: read / modified (cumulative)
6. Errors encountered and how they were resolved
7. Next steps
8. Critical context (anything else needed to continue)";

/// Build the summarization prompt for a preparation. Pure so the prompt
/// shape is unit-testable without a provider.
#[must_use]
pub fn build_summary_prompt(preparation: &CompactionPreparation) -> String {
    let mut prompt = String::from(
        "You are summarizing the earlier part of a coding session so work \
can continue in a smaller context. The conversation below is material to \
summarize, not a conversation to continue. Do not answer it and do not \
mention that you are summarizing.\n\nProduce a summary with exactly these \
sections:\n",
    );
    prompt.push_str(COMPACTION_SUMMARY_SCHEMA);
    if let Some(previous_summary) = preparation.previous_summary.as_deref() {
        prompt.push_str("\n\n<previous-summary>\n");
        prompt.push_str(previous_summary);
        prompt.push_str(
            "\n</previous-summary>\n\nTreat the previous summary above as the \
current anchored summary: preserve still-true details, remove stale ones, \
and merge in new facts from the conversation below.",
        );
    }
    if let Some(focus) = preparation.focus_instructions.as_deref() {
        prompt.push_str("\n\nUser focus for this summary (in addition to the fixed sections): ");
        prompt.push_str(focus);
    }
    prompt.push_str("\n\n<conversation>\n");
    prompt.push_str(&preparation.serialized_conversation);
    prompt.push_str("\n</conversation>");
    prompt
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::time::Duration;

    use yach_connections::ProviderSecret;

    use crate::rig_adapter::{MaxTokensParam, RigProviderAdapterConfig, RigProviderConfig};
    use crate::session::{
        CompactionCheckpointId, Role, SessionId, ToolOutcome, ToolPayloadSummary, ToolRequestId,
        TurnId,
    };

    use super::*;

    #[test]
    fn native_artifact_debug_redacts_connection_and_window_content() {
        let artifact = NativeCompactionArtifact {
            version: 1,
            provider: String::from("openai"),
            wire: String::from("openai-responses"),
            model: String::from("gpt-fixture"),
            connection: String::from("endpoint-with-secret"),
            window: vec![
                serde_json::json!({"type":"compaction","encrypted_content":"opaque-secret"}),
            ],
        };

        let debug = format!("{artifact:?}");
        assert!(debug.contains("connection_fingerprint"));
        assert!(!debug.contains("endpoint-with-secret"));
        assert!(!debug.contains("opaque-secret"));
    }

    fn entry(entry_id: &str, turn_id: &str, role: Role, text: &str) -> SessionEvent {
        SessionEvent::EntryAppended {
            session_id: SessionId(String::from("session-compaction")),
            entry_id: EntryId(String::from(entry_id)),
            parent_entry_id: None,
            turn_id: TurnId(String::from(turn_id)),
            role,
            text: String::from(text),
            provider: None,
        }
    }

    fn tool_pair(turn_id: &str, request_id: &str, result: &str) -> [SessionEvent; 2] {
        [
            SessionEvent::ToolRequestRecorded {
                session_id: SessionId(String::from("session-compaction")),
                turn_id: TurnId(String::from(turn_id)),
                tool_request_id: ToolRequestId(String::from(request_id)),
                tool_name: String::from("read_text_file"),
                provider_call_id: None,
                validation: Ok(()),
                permission: crate::ToolPermissionState::Allowed,
                argument_summary: ToolPayloadSummary {
                    summary: String::from("tool payload redacted"),
                    byte_count: 2,
                    redacted: true,
                    truncated: false,
                },
                argument_content: Some(String::from("{\"path\":\"src/lib.rs\"}")),
            },
            SessionEvent::ToolExecutionFinished {
                session_id: SessionId(String::from("session-compaction")),
                turn_id: TurnId(String::from(turn_id)),
                tool_request_id: ToolRequestId(String::from(request_id)),
                outcome: ToolOutcome::Completed,
                reason: None,
                result_summary: None,
                result_content: Some(String::from(result)),
            },
        ]
    }

    fn turn_finished(turn_id: &str, outcome: TurnOutcome) -> SessionEvent {
        SessionEvent::TurnFinished {
            session_id: SessionId(String::from("session-compaction")),
            turn_id: TurnId(String::from(turn_id)),
            outcome,
            reason: None,
        }
    }

    fn checkpoint(turn_id: &str, summary: &str, first_kept_entry_id: &str) -> SessionEvent {
        SessionEvent::CompactionCheckpoint {
            session_id: SessionId(String::from("session-compaction")),
            turn_id: TurnId(String::from(turn_id)),
            checkpoint_id: CompactionCheckpointId(String::from("checkpoint-1")),
            summary: String::from(summary),
            first_kept_entry_id: EntryId(String::from(first_kept_entry_id)),
            tokens_before: 1_000,
            tokens_after_estimate: 100,
            reason: CompactionReason::Threshold,
            compactor: String::from("summary"),
            details: serde_json::json!({}),
        }
    }

    #[test]
    fn context_estimate_excludes_failed_and_cancelled_turns() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "ask"));
        log.push(entry("entry-2", "turn-1", Role::Assistant, "answer"));
        log.push(turn_finished("turn-1", TurnOutcome::Completed));
        let completed_only = estimate_current_context_tokens(&log);
        assert!(completed_only > 0);

        // A failed and a cancelled turn add events that every provider
        // request excludes; the estimate must exclude them too.
        log.push(entry("entry-3", "turn-2", Role::User, &"f".repeat(4_000)));
        for event in tool_pair("turn-2", "request-1", &"g".repeat(4_000)) {
            log.push(event);
        }
        log.push(turn_finished("turn-2", TurnOutcome::Failed));
        log.push(entry("entry-4", "turn-3", Role::User, &"h".repeat(4_000)));
        log.push(turn_finished("turn-3", TurnOutcome::Cancelled));
        assert_eq!(estimate_current_context_tokens(&log), completed_only);

        // The newest turn has no TurnFinished yet: in flight, so counted.
        log.push(entry("entry-5", "turn-4", Role::User, "next question"));
        assert_eq!(
            estimate_current_context_tokens(&log),
            completed_only + estimate_text_tokens("next question")
        );
    }

    #[test]
    fn context_estimate_after_checkpoint_excludes_failed_turns_in_kept_slice() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "old history"));
        log.push(turn_finished("turn-1", TurnOutcome::Completed));
        log.push(checkpoint("turn-2", "summary text", "entry-2"));
        log.push(entry("entry-2", "turn-2", Role::User, "kept ask"));
        log.push(entry("entry-3", "turn-2", Role::Assistant, "kept answer"));
        log.push(turn_finished("turn-2", TurnOutcome::Completed));
        let clean = estimate_current_context_tokens(&log);
        assert_eq!(
            clean,
            estimate_text_tokens("summary text")
                + estimate_text_tokens("kept ask")
                + estimate_text_tokens("kept answer")
        );

        log.push(entry("entry-4", "turn-3", Role::User, &"x".repeat(4_000)));
        log.push(turn_finished("turn-3", TurnOutcome::Failed));
        assert_eq!(estimate_current_context_tokens(&log), clean);
    }

    #[test]
    fn compaction_checkpoint_event_round_trips_as_json() {
        let event = checkpoint("turn-3", "the summary", "entry-7");
        let encoded = serde_json::to_string(&event);
        assert!(encoded.is_ok());
        let Ok(encoded) = encoded else {
            return;
        };
        assert!(encoded.contains("\"type\":\"compaction_checkpoint\""));
        assert!(encoded.contains("\"reason\":\"threshold\""));
        let decoded = serde_json::from_str::<SessionEvent>(&encoded);
        assert_eq!(decoded.ok(), Some(event));
    }

    #[test]
    fn newest_checkpoint_resolves_kept_start_with_fallback() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "old"));
        log.push(checkpoint("turn-2", "summary", "entry-2"));
        log.push(entry("entry-2", "turn-2", Role::User, "kept"));

        let view = newest_compaction_checkpoint(&log);
        assert_eq!(view.as_ref().map(|view| view.kept_start_index), Some(2));

        // Missing kept entry falls back to the event after the checkpoint.
        let mut fallback_log = SessionLog::default();
        fallback_log.push(entry("entry-1", "turn-1", Role::User, "old"));
        fallback_log.push(checkpoint("turn-2", "summary", "entry-missing"));
        fallback_log.push(entry("entry-3", "turn-2", Role::User, "kept"));
        let fallback = newest_compaction_checkpoint(&fallback_log);
        assert_eq!(fallback.map(|view| view.kept_start_index), Some(2));
    }

    #[test]
    fn cut_selection_prefers_turn_boundaries_and_keeps_budget() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, &"a".repeat(4_000)));
        log.push(entry(
            "entry-2",
            "turn-1",
            Role::Assistant,
            &"b".repeat(4_000),
        ));
        log.push(entry("entry-3", "turn-2", Role::User, &"c".repeat(400)));
        log.push(entry(
            "entry-4",
            "turn-2",
            Role::Assistant,
            &"d".repeat(400),
        ));

        // Budget of 500 tokens keeps turn-2 (200 tokens) but not turn-1.
        let cut = select_compaction_cut(&log, 500);
        assert_eq!(
            cut,
            Some(CompactionCut {
                first_kept_entry_id: EntryId(String::from("entry-3")),
                kept_start_index: 2,
                fold_range: 0..2,
            })
        );
    }

    #[test]
    fn cut_selection_returns_none_when_everything_fits() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "small"));
        log.push(entry("entry-2", "turn-1", Role::Assistant, "reply"));
        assert_eq!(select_compaction_cut(&log, 20_000), None);
    }

    #[test]
    fn cut_selection_falls_back_to_entry_boundary_inside_oversized_turn() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "start"));
        log.push(entry(
            "entry-2",
            "turn-1",
            Role::Assistant,
            &"x".repeat(40_000),
        ));
        log.push(entry(
            "entry-3",
            "turn-1",
            Role::Assistant,
            &"y".repeat(2_000),
        ));

        // One turn far over budget: no user-entry boundary after the budget
        // point, so the cut lands at an assistant entry inside the turn.
        let cut = select_compaction_cut(&log, 1_000);
        assert_eq!(
            cut.map(|cut| cut.first_kept_entry_id),
            Some(EntryId(String::from("entry-3")))
        );
    }

    #[test]
    fn cut_selection_keeps_oversized_newest_entry_before_zero_token_event() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, "old request"));
        log.push(entry("entry-2", "turn-1", Role::Assistant, "old response"));
        log.push(entry("entry-3", "turn-2", Role::User, &"x".repeat(100_000)));
        log.record_static_context_included(
            SessionId(String::from("session-compaction")),
            TurnId(String::from("turn-2")),
            crate::StaticContextSummary::default(),
            Vec::new(),
        );

        assert_eq!(
            select_compaction_cut(&log, 20_000),
            Some(CompactionCut {
                first_kept_entry_id: EntryId(String::from("entry-3")),
                kept_start_index: 2,
                fold_range: 0..2,
            })
        );
    }

    #[test]
    fn cut_selection_resumes_from_previous_kept_boundary() {
        let mut log = SessionLog::default();
        log.push(entry("entry-1", "turn-1", Role::User, &"a".repeat(4_000)));
        log.push(checkpoint("turn-2", "summary", "entry-2"));
        log.push(entry("entry-2", "turn-2", Role::User, &"b".repeat(4_000)));
        log.push(entry("entry-3", "turn-3", Role::User, &"c".repeat(400)));

        // The fold starts at the previous kept boundary (entry-2), so the
        // previously-kept message folds into the next summary instead of
        // dropping; the checkpoint event itself is never folded.
        let cut = select_compaction_cut(&log, 500);
        assert_eq!(
            cut,
            Some(CompactionCut {
                first_kept_entry_id: EntryId(String::from("entry-3")),
                kept_start_index: 3,
                fold_range: 2..3,
            })
        );
    }

    #[test]
    fn serialization_flattens_conversation_and_bounds_tool_results() {
        let mut events = vec![entry("entry-1", "turn-1", Role::User, "please read")];
        events.extend(tool_pair("turn-1", "tool-request-1", &"z".repeat(3_000)));
        events.push(entry("entry-2", "turn-1", Role::Assistant, "done"));

        let serialized = serialize_events_for_summary(&events);
        assert!(serialized.starts_with("[User]: please read"));
        assert!(serialized.contains("[Tool call]: read_text_file({\"path\":\"src/lib.rs\"})"));
        assert!(serialized.contains("[Tool result Completed]:"));
        assert!(serialized.contains("[1000 chars truncated]"));
        assert!(serialized.ends_with("[Assistant]: done"));
    }

    #[test]
    fn summary_prompt_threads_anchor_and_focus() {
        let preparation = CompactionPreparation {
            serialized_conversation: String::from("[User]: hi"),
            previous_summary: Some(String::from("prior anchored summary")),
            previous_details: None,
            first_kept_entry_id: EntryId(String::from("entry-9")),
            tokens_before: 90_000,
            reason: CompactionReason::Manual,
            focus_instructions: Some(String::from("keep the migration plan")),
            provider: Arc::new(CompactionProviderContext {
                provider: String::from("fixture"),
                wire: String::from("fixture-wire"),
                model: String::from("fixture-model"),
                connection: String::from("fixture-connection"),
                responses_compact: None,
                adapter: Arc::new(RigProviderAdapterConfig {
                    provider: RigProviderConfig::Anthropic {
                        api_key: ProviderSecret::new(String::from("fixture-secret")),
                        base_url: None,
                    },
                    timeout: Duration::from_secs(1),
                    max_tokens: 1,
                    context_window: 1,
                    max_tokens_param: MaxTokensParam::MaxTokens,
                }),
            }),
            native_request: None,
        };
        let prompt = build_summary_prompt(&preparation);
        assert!(prompt.contains("verbatim"));
        assert!(prompt.contains("<previous-summary>\nprior anchored summary"));
        assert!(prompt.contains("anchored summary: preserve still-true details"));
        assert!(prompt.contains("keep the migration plan"));
        assert!(prompt.ends_with("<conversation>\n[User]: hi\n</conversation>"));
    }

    #[test]
    fn file_details_merge_cumulatively_across_checkpoints() {
        let previous = serde_json::json!({
            "read_files": ["docs/old.md"],
            "modified_files": ["src/old.rs"],
        });
        let mut events = vec![entry("entry-1", "turn-1", Role::User, "work")];
        events.extend(tool_pair("turn-1", "tool-request-1", "contents"));
        events.push(SessionEvent::ToolRequestRecorded {
            session_id: SessionId(String::from("session-compaction")),
            turn_id: TurnId(String::from("turn-1")),
            tool_request_id: ToolRequestId(String::from("tool-request-2")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: None,
            validation: Ok(()),
            permission: crate::ToolPermissionState::Allowed,
            argument_summary: ToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 2,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(String::from("{\"path\":\"src/new.rs\"}")),
        });

        let details = merge_compaction_file_details(Some(&previous), &events);
        assert_eq!(
            details,
            serde_json::json!({
                "read_files": ["docs/old.md", "src/lib.rs"],
                "modified_files": ["src/new.rs", "src/old.rs"],
            })
        );
    }

    #[test]
    fn compaction_config_defaults_and_clamps() {
        let config = CompactionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.compactor, "auto");
        assert_eq!(config.compactor_kind(), Some(CompactorKind::Auto));
        assert_eq!(config.reserve_tokens, 16_384);
        assert_eq!(config.keep_recent_tokens, 20_000);
        assert_eq!(config.auto_threshold_percent_clamped(), 90);

        let extreme = CompactionConfig {
            auto_threshold_percent: 3,
            ..CompactionConfig::default()
        };
        assert_eq!(extreme.auto_threshold_percent_clamped(), 10);
    }

    #[test]
    fn compaction_config_parses_project_override_and_rejects_unknown_compactors() {
        for (compactor, expected) in [
            ("auto", Some(CompactorKind::Auto)),
            ("summary", Some(CompactorKind::Summary)),
            ("openai-responses", Some(CompactorKind::OpenAiResponses)),
            ("not-a-compactor", None),
        ] {
            let json = format!(r#"{{"compaction":{{"compactor":"{compactor}"}}}}"#);
            let Ok(file) = serde_json::from_str::<CompactionConfigFile>(&json) else {
                unreachable!("project config fixture must parse");
            };
            assert_eq!(file.compaction.compactor_kind(), expected);
        }
    }

    #[tokio::test]
    async fn native_compactor_posts_exact_envelope_and_retains_raw_window() {
        let fixture = native_compaction_fixture(
            "HTTP/1.1 200 OK",
            r#"{"output":[{"type":"compaction","encrypted_content":"opaque-window"}]}"#,
        );
        assert!(fixture.is_some(), "fixture listener should initialize");
        let Some((base_url, received)) = fixture else {
            return;
        };
        let secret = "native-compactor-fixture-secret";
        let compactor = OpenAiResponsesCompactor;
        let preparation = native_preparation(base_url, secret);

        let outcome = compactor.compact(preparation).await;

        assert!(
            outcome.is_ok(),
            "fixture compact response should be accepted"
        );
        let Ok(outcome) = outcome else {
            return;
        };
        assert_eq!(outcome.artifact.version, 1);
        assert_eq!(outcome.artifact.provider, "openai");
        assert_eq!(outcome.artifact.wire, "openai-responses");
        assert_eq!(outcome.artifact.model, "gpt-fixture");
        assert_eq!(outcome.artifact.connection, "fixture-connection");
        assert_eq!(outcome.artifact.window.len(), 1);
        let request = received.recv();
        assert!(request.is_ok(), "fixture received request");
        let Ok(request) = request else {
            return;
        };
        assert_eq!(request.path, "/v1/responses/compact");
        assert!(
            request
                .authorization
                .as_deref()
                .is_some_and(|authorization| authorization.starts_with("Bearer "))
        );
        assert!(
            request
                .body
                .get("input")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|input| input.len() == 1)
        );
        assert!(
            request
                .body
                .get("instructions")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|instructions| !instructions.is_empty())
        );
        let debug = format!("{outcome:?}");
        assert!(!debug.contains(secret));
        assert!(!debug.contains("opaque-window"));
    }

    #[tokio::test]
    async fn native_compactor_maps_transport_failures_to_redacted_errors() {
        let cases = [
            (
                "HTTP/1.1 500 Internal Server Error",
                "provider body sentinel",
                CompactionError::HttpStatus { status: 500 },
            ),
            ("HTTP/1.1 200 OK", "{not-json", CompactionError::Decode),
            (
                "HTTP/1.1 200 OK",
                r#"{"output":{"not":"an array"}}"#,
                CompactionError::InvalidOutput,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"output":[]}"#,
                CompactionError::InvalidOutput,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"output":[null]}"#,
                CompactionError::InvalidOutput,
            ),
            (
                "HTTP/1.1 200 OK",
                r#"{"output":[{"type":"message"}]}"#,
                CompactionError::InvalidOutput,
            ),
        ];
        for (status, body, expected) in cases {
            let fixture = native_compaction_fixture(status, body);
            assert!(fixture.is_some(), "fixture listener should initialize");
            let Some((base_url, _received)) = fixture else {
                continue;
            };
            let error = OpenAiResponsesCompactor
                .compact(native_preparation(
                    base_url,
                    "native-compactor-fixture-secret",
                ))
                .await;
            assert_eq!(error, Err(expected));
            assert!(!format!("{error:?}").contains("native-compactor-fixture-secret"));
            assert!(!format!("{error:?}").contains("provider body sentinel"));
        }
    }

    #[tokio::test]
    async fn native_compactor_maps_missing_envelope_and_transport_without_secrets() {
        let mut missing = native_preparation(
            String::from("http://127.0.0.1:1"),
            "native-compactor-fixture-secret",
        );
        missing.native_request = None;
        assert_eq!(
            OpenAiResponsesCompactor.compact(missing).await,
            Err(CompactionError::MissingNativeRequest)
        );

        let transport = OpenAiResponsesCompactor
            .compact(native_preparation(
                String::from("http://127.0.0.1:1"),
                "native-compactor-fixture-secret",
            ))
            .await;
        assert_eq!(transport, Err(CompactionError::Transport));
        assert!(!format!("{transport:?}").contains("native-compactor-fixture-secret"));
    }

    #[tokio::test]
    async fn native_compactor_rejects_unsupported_provider_without_network() {
        let mut preparation = native_preparation(
            String::from("http://127.0.0.1:1"),
            "native-compactor-fixture-secret",
        );
        preparation.provider = Arc::new(CompactionProviderContext {
            provider: String::from("anthropic"),
            wire: String::from("anthropic-messages"),
            model: String::from("claude-fixture"),
            connection: String::from("fixture-connection"),
            responses_compact: Some(true),
            adapter: Arc::new(RigProviderAdapterConfig {
                provider: RigProviderConfig::Anthropic {
                    api_key: ProviderSecret::new(String::from("native-compactor-fixture-secret")),
                    base_url: None,
                },
                timeout: Duration::from_secs(1),
                max_tokens: 1,
                context_window: 1,
                max_tokens_param: MaxTokensParam::MaxTokens,
            }),
        });

        assert_eq!(
            OpenAiResponsesCompactor.compact(preparation).await,
            Err(CompactionError::UnsupportedProvider {
                provider: String::from("anthropic")
            })
        );
    }

    #[tokio::test]
    async fn native_compactor_maps_stalled_fixture_to_timeout_without_secret() {
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok(), "fixture listener should initialize");
        let Ok(listener) = listener else {
            return;
        };
        let address = listener.local_addr();
        assert!(address.is_ok(), "fixture address should initialize");
        let Ok(address) = address else {
            return;
        };
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut bytes = [0; 4096];
            let _ = stream.read(&mut bytes);
            std::thread::sleep(Duration::from_millis(50));
        });
        let secret = "native-compactor-fixture-secret";
        let mut preparation = native_preparation(format!("http://{address}/v1"), secret);
        let mut adapter = (*preparation.provider.adapter).clone();
        adapter.timeout = Duration::from_millis(1);
        let provider = Arc::get_mut(&mut preparation.provider);
        assert!(provider.is_some(), "preparation owns its provider context");
        let Some(provider) = provider else {
            return;
        };
        provider.adapter = Arc::new(adapter);

        let error = OpenAiResponsesCompactor.compact(preparation).await;

        assert_eq!(error, Err(CompactionError::Timeout));
        assert!(!format!("{error:?}").contains(secret));
    }

    #[tokio::test]
    async fn native_compactor_maps_headers_then_stalled_body_to_timeout_without_secret() {
        let listener = TcpListener::bind("127.0.0.1:0");
        assert!(listener.is_ok(), "fixture listener should initialize");
        let Ok(listener) = listener else {
            return;
        };
        let address = listener.local_addr();
        assert!(address.is_ok(), "fixture address should initialize");
        let Ok(address) = address else {
            return;
        };
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut bytes = [0; 4096];
            let _ = stream.read(&mut bytes);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 64\r\nConnection: keep-alive\r\n\r\n[",
            );
            std::thread::sleep(Duration::from_millis(500));
        });
        let secret = "native-compactor-fixture-secret";
        let mut preparation = native_preparation(format!("http://{address}/v1"), secret);
        let mut adapter = (*preparation.provider.adapter).clone();
        adapter.timeout = Duration::from_millis(100);
        let provider = Arc::get_mut(&mut preparation.provider);
        assert!(provider.is_some(), "preparation owns its provider context");
        let Some(provider) = provider else {
            return;
        };
        provider.adapter = Arc::new(adapter);

        let error = OpenAiResponsesCompactor.compact(preparation).await;

        assert_eq!(error, Err(CompactionError::Timeout));
        assert!(!format!("{error:?}").contains(secret));
    }

    fn native_preparation(base_url: String, secret: &str) -> CompactionPreparation {
        CompactionPreparation {
            serialized_conversation: String::new(),
            previous_summary: None,
            previous_details: None,
            first_kept_entry_id: EntryId(String::from("entry-1")),
            tokens_before: 100,
            reason: CompactionReason::Manual,
            focus_instructions: None,
            provider: Arc::new(CompactionProviderContext {
                provider: String::from("openai"),
                wire: String::from("openai-responses"),
                model: String::from("gpt-fixture"),
                connection: String::from("fixture-connection"),
                responses_compact: Some(true),
                adapter: Arc::new(RigProviderAdapterConfig {
                    provider: RigProviderConfig::OpenAi {
                        api_key: ProviderSecret::new(String::from(secret)),
                        base_url: Some(base_url),
                    },
                    timeout: Duration::from_secs(1),
                    max_tokens: 1,
                    context_window: 1,
                    max_tokens_param: MaxTokensParam::MaxTokens,
                }),
            }),
            native_request: Some(NativeRequestEnvelope {
                input: vec![serde_json::json!({
                    "type":"message",
                    "role":"user",
                    "content":"exact-input"
                })],
                instructions: String::from("exact-instructions"),
            }),
        }
    }

    struct CapturedNativeRequest {
        path: String,
        authorization: Option<String>,
        body: serde_json::Value,
    }

    fn native_compaction_fixture(
        status: &str,
        response_body: &str,
    ) -> Option<(String, std::sync::mpsc::Receiver<CapturedNativeRequest>)> {
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        let address = listener.local_addr().ok()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let status = status.to_owned();
        let response_body = response_body.to_owned();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut bytes = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let Ok(count) = stream.read(&mut chunk) else {
                    return;
                };
                if count == 0 {
                    return;
                }
                bytes.extend_from_slice(&chunk[..count]);
                let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                else {
                    continue;
                };
                let header_text = String::from_utf8_lossy(&bytes[..headers_end]);
                let content_length = header_text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if bytes.len() >= headers_end + 4 + content_length {
                    let mut lines = header_text.lines();
                    let path = lines
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or_default()
                        .to_owned();
                    let authorization = lines.find_map(|line| {
                        line.strip_prefix("authorization:")
                            .map(|value| value.trim().to_owned())
                    });
                    let body = serde_json::from_slice(&bytes[headers_end + 4..]).ok();
                    if let Some(body) = body {
                        let _ = sender.send(CapturedNativeRequest {
                            path,
                            authorization,
                            body,
                        });
                    }
                    let response = format!(
                        "{status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                        response_body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    return;
                }
            }
        });
        Some((format!("http://{address}/v1/"), receiver))
    }
}
