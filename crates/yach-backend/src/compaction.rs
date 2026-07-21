//! Context compaction: checkpoint discovery, cut-point selection, token
//! accounting, conversation serialization, config, and the pluggable
//! compactor seam.
//!
//! Design: `docs/superpowers/specs/2026-07-20-context-compaction-design.md`.
//! The session log is never truncated; a `CompactionCheckpoint` event is
//! appended and provider context rebuilds as summary + verbatim kept tail.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use serde::Deserialize;

use crate::session::{
    NativeCompactionReason, NativeEntryId, NativeRole, NativeSessionEvent, NativeSessionLog,
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
pub struct NativeCompactionConfig {
    pub enabled: bool,
    pub compactor: String,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
    pub auto_threshold_percent: u8,
}

impl Default for NativeCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compactor: String::from("summary"),
            reserve_tokens: COMPACTION_DEFAULT_RESERVE_TOKENS,
            keep_recent_tokens: COMPACTION_DEFAULT_KEEP_RECENT_TOKENS,
            auto_threshold_percent: COMPACTION_DEFAULT_AUTO_THRESHOLD_PERCENT,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct NativeCompactionConfigFile {
    compaction: NativeCompactionConfig,
}

impl NativeCompactionConfig {
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

fn load_compaction_config(path: &Path) -> Option<NativeCompactionConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<NativeCompactionConfigFile>(&raw)
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
pub fn estimate_event_tokens(event: &NativeSessionEvent) -> u64 {
    match event {
        NativeSessionEvent::EntryAppended { text, .. } => estimate_text_tokens(text),
        NativeSessionEvent::ToolRequestRecorded {
            argument_content, ..
        } => argument_content.as_deref().map_or(0, estimate_text_tokens),
        NativeSessionEvent::ToolExecutionFinished { result_content, .. } => {
            result_content.as_deref().map_or(0, estimate_text_tokens)
        }
        NativeSessionEvent::CompactionCheckpoint { summary, .. } => estimate_text_tokens(summary),
        NativeSessionEvent::TurnFinished { .. }
        | NativeSessionEvent::MetricRecorded { .. }
        | NativeSessionEvent::StaticContextIncluded { .. }
        | NativeSessionEvent::PermissionDecisionRecorded { .. }
        | NativeSessionEvent::EditTraceRecorded { .. }
        | NativeSessionEvent::EditTransactionPrepared { .. }
        | NativeSessionEvent::EditTransactionFinished { .. } => 0,
    }
}

/// Context accounting inputs shared by the auto-compaction trigger and the
/// TUI context meter: `usable = context_window − max_output_tokens −
/// reserve_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeContextBudget {
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub reserve_tokens: u64,
}

impl NativeContextBudget {
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

/// Estimated tokens the log currently contributes to provider context:
/// the newest checkpoint's summary plus everything from its kept boundary
/// forward, or the whole log when no checkpoint exists. Feeds the TUI
/// context meter with the same accounting family as the trigger.
#[must_use]
pub fn estimate_current_context_tokens(log: &NativeSessionLog) -> u64 {
    newest_compaction_checkpoint(log).map_or_else(
        || log.events.iter().map(estimate_event_tokens).sum(),
        |view| {
            // The newest checkpoint event sits inside the kept slice; skip
            // it so its summary is not counted twice.
            estimate_text_tokens(view.summary).saturating_add(
                log.events[view.kept_start_index.min(log.events.len())..]
                    .iter()
                    .filter(|event| {
                        !matches!(event, NativeSessionEvent::CompactionCheckpoint { .. })
                    })
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
    pub first_kept_entry_id: &'a NativeEntryId,
    /// Event index the kept transcript resumes from: the position of the
    /// `first_kept_entry_id` entry, falling back to the event after the
    /// checkpoint itself when that entry is not found (Pi's fallback).
    pub kept_start_index: usize,
}

#[must_use]
pub fn newest_compaction_checkpoint(log: &NativeSessionLog) -> Option<NewestCheckpointView<'_>> {
    let (checkpoint_index, summary, details, first_kept_entry_id) = log
        .events
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, event)| match event {
            NativeSessionEvent::CompactionCheckpoint {
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
            NativeSessionEvent::EntryAppended { entry_id, .. }
                if entry_id == first_kept_entry_id =>
            {
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
    pub first_kept_entry_id: NativeEntryId,
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
pub fn select_compaction_cut(
    log: &NativeSessionLog,
    keep_recent_tokens: u64,
) -> Option<CompactionCut> {
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
    if budget_start == events.len() {
        // Even the newest content-bearing event exceeds the budget by
        // itself; keep the minimal mandatory tail (the newest entry) and
        // fold everything before it.
        budget_start = events
            .iter()
            .rposition(|event| matches!(event, NativeSessionEvent::EntryAppended { .. }))?;
    }

    // Preferred cut: the first turn boundary (user entry) at or after the
    // budget point. Fallback for one oversized turn: any entry boundary.
    let cut =
        events
            .iter()
            .enumerate()
            .skip(budget_start)
            .find_map(|(index, event)| match event {
                NativeSessionEvent::EntryAppended {
                    entry_id,
                    role: NativeRole::User,
                    ..
                } => Some((index, entry_id.clone())),
                _ => None,
            })
            .or_else(|| {
                events.iter().enumerate().skip(budget_start).find_map(
                    |(index, event)| match event {
                        NativeSessionEvent::EntryAppended { entry_id, .. } => {
                            Some((index, entry_id.clone()))
                        }
                        _ => None,
                    },
                )
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
pub fn serialize_events_for_summary(events: &[NativeSessionEvent]) -> String {
    let mut lines = Vec::new();
    for event in events {
        match event {
            NativeSessionEvent::EntryAppended { role, text, .. } => {
                let label = match role {
                    NativeRole::User => "[User]",
                    NativeRole::Assistant => "[Assistant]",
                    NativeRole::Tool => "[Tool]",
                    NativeRole::System => "[System]",
                };
                lines.push(format!("{label}: {text}"));
            }
            NativeSessionEvent::ToolRequestRecorded {
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
            NativeSessionEvent::ToolExecutionFinished {
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
            NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTraceRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. }
            | NativeSessionEvent::CompactionCheckpoint { .. } => {}
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
    folded_events: &[NativeSessionEvent],
) -> serde_json::Value {
    let mut read_files = details_string_set(previous_details, "read_files");
    let mut modified_files = details_string_set(previous_details, "modified_files");
    for event in folded_events {
        let NativeSessionEvent::ToolRequestRecorded {
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

/// Everything a compactor needs to produce a checkpoint. Owned values so
/// implementations can move work across await points freely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPreparation {
    pub serialized_conversation: String,
    pub previous_summary: Option<String>,
    pub previous_details: Option<serde_json::Value>,
    pub first_kept_entry_id: NativeEntryId,
    pub tokens_before: u64,
    pub reason: NativeCompactionReason,
    pub focus_instructions: Option<String>,
}

/// A compactor's product: the summary that becomes the checkpoint plus
/// compactor-specific state carried to the next checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionOutcome {
    pub summary: String,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionError {
    SummaryFailed(String),
}

pub type CompactionFuture =
    Pin<Box<dyn Future<Output = Result<CompactionOutcome, CompactionError>> + Send>>;

/// Compactor seam: core owns cut selection, token accounting, and log
/// writes; the compactor only produces the summary. Selected by config
/// (`compaction.compactor`); unknown names fail closed with an actionable
/// error, mirroring `shell.executor`.
pub trait NativeCompactor: Send + Sync {
    fn compact(&self, preparation: CompactionPreparation) -> CompactionFuture;
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
    use super::*;
    use crate::session::{
        NativeCompactionCheckpointId, NativeRole, NativeSessionId, NativeToolOutcome,
        NativeToolPayloadSummary, NativeToolRequestId, NativeTurnId,
    };

    fn entry(entry_id: &str, turn_id: &str, role: NativeRole, text: &str) -> NativeSessionEvent {
        NativeSessionEvent::EntryAppended {
            session_id: NativeSessionId(String::from("session-compaction")),
            entry_id: NativeEntryId(String::from(entry_id)),
            parent_entry_id: None,
            turn_id: NativeTurnId(String::from(turn_id)),
            role,
            text: String::from(text),
            provider: None,
        }
    }

    fn tool_pair(turn_id: &str, request_id: &str, result: &str) -> [NativeSessionEvent; 2] {
        [
            NativeSessionEvent::ToolRequestRecorded {
                session_id: NativeSessionId(String::from("session-compaction")),
                turn_id: NativeTurnId(String::from(turn_id)),
                tool_request_id: NativeToolRequestId(String::from(request_id)),
                tool_name: String::from("read_text_file"),
                provider_call_id: None,
                validation: Ok(()),
                permission: crate::NativeToolPermissionState::Allowed,
                argument_summary: NativeToolPayloadSummary {
                    summary: String::from("tool payload redacted"),
                    byte_count: 2,
                    redacted: true,
                    truncated: false,
                },
                argument_content: Some(String::from("{\"path\":\"src/lib.rs\"}")),
            },
            NativeSessionEvent::ToolExecutionFinished {
                session_id: NativeSessionId(String::from("session-compaction")),
                turn_id: NativeTurnId(String::from(turn_id)),
                tool_request_id: NativeToolRequestId(String::from(request_id)),
                outcome: NativeToolOutcome::Completed,
                reason: None,
                result_summary: None,
                result_content: Some(String::from(result)),
            },
        ]
    }

    fn checkpoint(turn_id: &str, summary: &str, first_kept_entry_id: &str) -> NativeSessionEvent {
        NativeSessionEvent::CompactionCheckpoint {
            session_id: NativeSessionId(String::from("session-compaction")),
            turn_id: NativeTurnId(String::from(turn_id)),
            checkpoint_id: NativeCompactionCheckpointId(String::from("checkpoint-1")),
            summary: String::from(summary),
            first_kept_entry_id: NativeEntryId(String::from(first_kept_entry_id)),
            tokens_before: 1_000,
            tokens_after_estimate: 100,
            reason: NativeCompactionReason::Threshold,
            compactor: String::from("summary"),
            details: serde_json::json!({}),
        }
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
        let decoded = serde_json::from_str::<NativeSessionEvent>(&encoded);
        assert_eq!(decoded.ok(), Some(event));
    }

    #[test]
    fn newest_checkpoint_resolves_kept_start_with_fallback() {
        let mut log = NativeSessionLog::default();
        log.push(entry("entry-1", "turn-1", NativeRole::User, "old"));
        log.push(checkpoint("turn-2", "summary", "entry-2"));
        log.push(entry("entry-2", "turn-2", NativeRole::User, "kept"));

        let view = newest_compaction_checkpoint(&log);
        assert_eq!(view.as_ref().map(|view| view.kept_start_index), Some(2));

        // Missing kept entry falls back to the event after the checkpoint.
        let mut fallback_log = NativeSessionLog::default();
        fallback_log.push(entry("entry-1", "turn-1", NativeRole::User, "old"));
        fallback_log.push(checkpoint("turn-2", "summary", "entry-missing"));
        fallback_log.push(entry("entry-3", "turn-2", NativeRole::User, "kept"));
        let fallback = newest_compaction_checkpoint(&fallback_log);
        assert_eq!(fallback.map(|view| view.kept_start_index), Some(2));
    }

    #[test]
    fn cut_selection_prefers_turn_boundaries_and_keeps_budget() {
        let mut log = NativeSessionLog::default();
        log.push(entry(
            "entry-1",
            "turn-1",
            NativeRole::User,
            &"a".repeat(4_000),
        ));
        log.push(entry(
            "entry-2",
            "turn-1",
            NativeRole::Assistant,
            &"b".repeat(4_000),
        ));
        log.push(entry(
            "entry-3",
            "turn-2",
            NativeRole::User,
            &"c".repeat(400),
        ));
        log.push(entry(
            "entry-4",
            "turn-2",
            NativeRole::Assistant,
            &"d".repeat(400),
        ));

        // Budget of 500 tokens keeps turn-2 (200 tokens) but not turn-1.
        let cut = select_compaction_cut(&log, 500);
        assert_eq!(
            cut,
            Some(CompactionCut {
                first_kept_entry_id: NativeEntryId(String::from("entry-3")),
                kept_start_index: 2,
                fold_range: 0..2,
            })
        );
    }

    #[test]
    fn cut_selection_returns_none_when_everything_fits() {
        let mut log = NativeSessionLog::default();
        log.push(entry("entry-1", "turn-1", NativeRole::User, "small"));
        log.push(entry("entry-2", "turn-1", NativeRole::Assistant, "reply"));
        assert_eq!(select_compaction_cut(&log, 20_000), None);
    }

    #[test]
    fn cut_selection_falls_back_to_entry_boundary_inside_oversized_turn() {
        let mut log = NativeSessionLog::default();
        log.push(entry("entry-1", "turn-1", NativeRole::User, "start"));
        log.push(entry(
            "entry-2",
            "turn-1",
            NativeRole::Assistant,
            &"x".repeat(40_000),
        ));
        log.push(entry(
            "entry-3",
            "turn-1",
            NativeRole::Assistant,
            &"y".repeat(2_000),
        ));

        // One turn far over budget: no user-entry boundary after the budget
        // point, so the cut lands at an assistant entry inside the turn.
        let cut = select_compaction_cut(&log, 1_000);
        assert_eq!(
            cut.map(|cut| cut.first_kept_entry_id),
            Some(NativeEntryId(String::from("entry-3")))
        );
    }

    #[test]
    fn cut_selection_resumes_from_previous_kept_boundary() {
        let mut log = NativeSessionLog::default();
        log.push(entry(
            "entry-1",
            "turn-1",
            NativeRole::User,
            &"a".repeat(4_000),
        ));
        log.push(checkpoint("turn-2", "summary", "entry-2"));
        log.push(entry(
            "entry-2",
            "turn-2",
            NativeRole::User,
            &"b".repeat(4_000),
        ));
        log.push(entry(
            "entry-3",
            "turn-3",
            NativeRole::User,
            &"c".repeat(400),
        ));

        // The fold starts at the previous kept boundary (entry-2), so the
        // previously-kept message folds into the next summary instead of
        // dropping; the checkpoint event itself is never folded.
        let cut = select_compaction_cut(&log, 500);
        assert_eq!(
            cut,
            Some(CompactionCut {
                first_kept_entry_id: NativeEntryId(String::from("entry-3")),
                kept_start_index: 3,
                fold_range: 2..3,
            })
        );
    }

    #[test]
    fn serialization_flattens_conversation_and_bounds_tool_results() {
        let mut events = vec![entry("entry-1", "turn-1", NativeRole::User, "please read")];
        events.extend(tool_pair("turn-1", "tool-request-1", &"z".repeat(3_000)));
        events.push(entry("entry-2", "turn-1", NativeRole::Assistant, "done"));

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
            first_kept_entry_id: NativeEntryId(String::from("entry-9")),
            tokens_before: 90_000,
            reason: NativeCompactionReason::Manual,
            focus_instructions: Some(String::from("keep the migration plan")),
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
        let mut events = vec![entry("entry-1", "turn-1", NativeRole::User, "work")];
        events.extend(tool_pair("turn-1", "tool-request-1", "contents"));
        events.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: NativeSessionId(String::from("session-compaction")),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_request_id: NativeToolRequestId(String::from("tool-request-2")),
            tool_name: String::from("edit_text_file"),
            provider_call_id: None,
            validation: Ok(()),
            permission: crate::NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
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
        let config = NativeCompactionConfig::default();
        assert!(config.enabled);
        assert_eq!(config.compactor, "summary");
        assert_eq!(config.reserve_tokens, 16_384);
        assert_eq!(config.keep_recent_tokens, 20_000);
        assert_eq!(config.auto_threshold_percent_clamped(), 90);

        let extreme = NativeCompactionConfig {
            auto_threshold_percent: 3,
            ..NativeCompactionConfig::default()
        };
        assert_eq!(extreme.auto_threshold_percent_clamped(), 10);
    }
}
