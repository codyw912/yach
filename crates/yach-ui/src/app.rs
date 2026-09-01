use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui_textarea::{CursorMove, Input, Key, TextArea, WrapMode};
use tokio::sync::mpsc;
use yach_proto::{
    ApprovalMode, BackendEvent, BackendState, Capability, ClientEvent, DialogKind, DialogRequest,
    DialogResponse, ExtensionDiagnosticRecord, ExtensionDiagnosticSnapshotOutcome,
    ExtensionLifecycleAction, ExtensionLifecycleOutcome, ForkMessage, ForkPosition,
    HarnessOutcomeKind, LocalEditDecision, LocalEditOperationInput, LocalEditReviewState,
    ModelInfo, NegotiatedCapabilities, PromptOutcome, RecentSession, ServerEvent, SessionMessage,
    SessionStats, ToolResultMetadata, ToolReviewDecision, ToolReviewResolution,
};
use zeroize::Zeroize;

use crate::layout;
use crate::lifecycle::{StatusLifecycle, is_lifecycle_status, status_lifecycle};
use crate::perf_metrics::PerfMetrics;
use crate::session_tree::{SessionTree, branch_summary_line, build_session_tree};
use crate::slash_commands::{
    SlashAction, SlashCommand, SlashParseResult, match_slash_commands, parse_slash_command,
};
use crate::theme::Theme;
use crate::thinking_level::ThinkingLevel;
use crate::transcript::{self, Transcript, TranscriptRenderCache};

#[derive(Debug, Clone)]
pub struct StartupTrace {
    path: PathBuf,
    start: Instant,
    marks: Arc<Mutex<Vec<StartupTraceMark>>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunTuiOptions {
    pub resume_session: bool,
    pub theme: Theme,
}

#[derive(Debug, Clone)]
struct StartupTraceMark {
    elapsed_micros: u128,
    label: String,
}

impl StartupTrace {
    #[must_use]
    pub fn from_env(name: &str) -> Option<Self> {
        let path = std::env::var_os(name).map(PathBuf::from)?;
        Some(Self {
            path,
            start: Instant::now(),
            marks: Arc::default(),
        })
    }

    pub fn mark(&self, label: &str) {
        let elapsed_micros = self.start.elapsed().as_micros();
        if let Ok(mut marks) = self.marks.lock() {
            marks.push(StartupTraceMark {
                elapsed_micros,
                label: label.to_string(),
            });
        }
    }

    pub fn flush(&self) {
        let Ok(marks) = self.marks.lock() else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
        {
            for mark in marks.iter() {
                let _ = writeln!(file, "{} {}", mark.elapsed_micros, mark.label);
            }
        }
    }
}

fn lifecycle_action_verb(action: ExtensionLifecycleAction) -> &'static str {
    match action {
        ExtensionLifecycleAction::Stop => "stopping",
        ExtensionLifecycleAction::Reload => "reloading",
    }
}

fn extension_diagnostic_snapshot_summary(
    outcome: ExtensionDiagnosticSnapshotOutcome,
    records: &[ExtensionDiagnosticRecord],
    message: Option<&str>,
) -> String {
    if let Some(message) = message
        && !message.is_empty()
    {
        return message.to_string();
    }
    match outcome {
        ExtensionDiagnosticSnapshotOutcome::Completed => {
            let active = records
                .iter()
                .filter(|record| record.activation_state == "active")
                .count();
            let failed = records
                .iter()
                .filter(|record| record.activation_state == "failed")
                .count();
            let stopped = records
                .iter()
                .filter(|record| record.activation_state == "stopped")
                .count();
            format!(
                "extensions: count={} active={} stopped={} failed={}",
                records.len(),
                active,
                stopped,
                failed
            )
        }
        ExtensionDiagnosticSnapshotOutcome::NotFound => String::from("extension not found"),
        ExtensionDiagnosticSnapshotOutcome::Failed => String::from("extension status unavailable"),
    }
}

fn render_extension_diagnostic_snapshot(records: &[ExtensionDiagnosticRecord]) -> String {
    if records.is_empty() {
        return String::from("no live extension diagnostics");
    }

    records
        .iter()
        .map(render_extension_diagnostic_record)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_extension_diagnostic_record(record: &ExtensionDiagnosticRecord) -> String {
    let id = record.id.as_deref().unwrap_or("none");
    let version = record.version.as_deref().unwrap_or("none");
    let selector = record
        .source_ref
        .as_deref()
        .or(record.id.as_deref())
        .unwrap_or(record.package_root.as_str());
    let provider_visible_tools = extension_tool_names_label(&record.provider_visible_tools);
    let registered_tools = extension_tool_names_label(&record.registered_tools);
    let error = record
        .last_error_kind
        .as_deref()
        .map(|kind| {
            let summary = record.last_error_summary.as_deref().unwrap_or("none");
            format!(" error={kind}:{summary}")
        })
        .unwrap_or_default();
    format!(
        "{id} state={} generation={} version={} scope={} selector={} provider_visible_tools={} registered_tools={}{}",
        record.activation_state,
        record.generation,
        version,
        record.scope,
        selector,
        provider_visible_tools,
        registered_tools,
        error
    )
}

fn extension_tool_names_label(names: &[String]) -> String {
    if names.is_empty() {
        String::from("none")
    } else {
        names.join(",")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveTool {
    id: Option<String>,
    name: String,
    preview: Option<String>,
}

impl ActiveTool {
    fn label(&self) -> String {
        match self.preview.as_deref() {
            Some(preview) if !preview.is_empty() => format!("{} {preview}", self.name),
            _ => self.name.clone(),
        }
    }
}

fn same_tool(left: &ActiveTool, right: &ActiveTool) -> bool {
    match (&left.id, &right.id) {
        (Some(left_id), Some(right_id)) => left_id == right_id,
        _ => left.name == right.name,
    }
}

fn tool_output_summary(
    output: &str,
    is_error: bool,
    metadata: Option<&ToolResultMetadata>,
    outcome_kind: Option<HarnessOutcomeKind>,
) -> String {
    if metadata.is_none() && is_tool_display_output(output) {
        return output.to_string();
    }
    let status = outcome_kind.map_or_else(
        || if is_error { "failed" } else { "completed" },
        HarnessOutcomeKind::label,
    );
    if output.is_empty() {
        return format!("{status} with no output");
    }

    let line_count = output.lines().count().max(1);
    let byte_count = metadata.map_or(output.len(), |metadata| metadata.byte_count);
    let line_label = if line_count == 1 { "line" } else { "lines" };
    let mut summary = format!("{status}: {line_count} {line_label}, {byte_count} bytes");
    if metadata.is_some_and(|metadata| metadata.truncated) {
        summary.push_str(", truncated");
    }
    if is_error && let Some(excerpt) = tool_error_excerpt(output) {
        summary.push_str("; ");
        summary.push_str(&excerpt);
    }
    summary
}

/// Refine a harness-authored turn-failure message into a display kind. The
/// backend only knows failed/cancelled at the turn level; denied/limit/blocked
/// are read from the structured reason labels embedded in the message text.
fn classify_turn_failure_text(error: &str) -> HarnessOutcomeKind {
    if error.contains("denied") {
        HarnessOutcomeKind::Denied
    } else if error.contains("too_many") || error.contains("limit") {
        HarnessOutcomeKind::Limit
    } else if error.contains("blocked") || error.contains("unavailable") {
        HarnessOutcomeKind::Blocked
    } else {
        HarnessOutcomeKind::Failed
    }
}

fn tool_error_excerpt(output: &str) -> Option<String> {
    let first_line = output.lines().find_map(|line| {
        let trimmed = line.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })?;

    let mut excerpt = String::new();
    for ch in first_line.chars().take(MAX_TOOL_ERROR_EXCERPT_CHARS) {
        excerpt.push(ch);
    }
    if first_line.chars().count() > MAX_TOOL_ERROR_EXCERPT_CHARS {
        excerpt.push_str("...");
    }
    Some(excerpt)
}

fn is_tool_display_output(output: &str) -> bool {
    // Backend-shaped tool summaries pass through untouched; anything else
    // (raw tool output) gets line/byte-counted by tool_output_summary.
    [
        "completed",
        "failed",
        "denied",
        "cancelled",
        "validation_failed",
    ]
    .iter()
    .any(|status| {
        output
            .strip_prefix(status)
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with(';'))
    })
}

fn clears_input(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::SUPER)
        || modifiers.contains(KeyModifiers::META)
        || modifiers.contains(KeyModifiers::CONTROL)
}

fn textarea_input(key: KeyCode, modifiers: KeyModifiers) -> Input {
    Input {
        key: textarea_key(key),
        ctrl: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::META),
        shift: modifiers.contains(KeyModifiers::SHIFT),
    }
}

fn textarea_key(key: KeyCode) -> Key {
    match key {
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab => Key::Tab,
        KeyCode::Delete => Key::Delete,
        KeyCode::Esc => Key::Esc,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::F(n) => Key::F(n),
        _ => Key::Null,
    }
}

fn textarea_from_text(text: &str) -> TextArea<'static> {
    let mut textarea = TextArea::new(text.split('\n').map(ToOwned::to_owned).collect());
    textarea.move_cursor(CursorMove::Bottom);
    textarea.move_cursor(CursorMove::End);
    textarea
}

fn is_selection_up_key(key: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(key, KeyCode::Up) || (matches!(key, KeyCode::Char('k')) && modifiers.is_empty())
}

fn is_selection_down_key(key: KeyCode, modifiers: KeyModifiers) -> bool {
    matches!(key, KeyCode::Down) || (matches!(key, KeyCode::Char('j')) && modifiers.is_empty())
}

fn accepts_plain_text_modifier(modifiers: KeyModifiers) -> bool {
    !(modifiers.contains(KeyModifiers::CONTROL)
        || modifiers.contains(KeyModifiers::ALT)
        || modifiers.contains(KeyModifiers::META)
        || modifiers.contains(KeyModifiers::SUPER)
        || modifiers.contains(KeyModifiers::HYPER))
}

fn local_edit_compose_accepts_multiline(step: LocalEditComposeStep) -> bool {
    matches!(
        step,
        LocalEditComposeStep::Find | LocalEditComposeStep::Replace | LocalEditComposeStep::Content
    )
}

fn local_edit_review_status_message(review_state: LocalEditReviewState) -> &'static str {
    match review_state {
        LocalEditReviewState::Allowed => "local edit pre-approved",
        LocalEditReviewState::NeedsUserApproval => "review local edit",
        LocalEditReviewState::AutoReviewUnavailable => {
            "auto-review unavailable; user approval required"
        }
    }
}
fn model_change_matches(
    pending: &ModelInfo,
    model: &str,
    connection_id: Option<&str>,
    provider: Option<&str>,
) -> bool {
    pending.id == model
        && pending.connection_id.as_deref() == connection_id
        && provider.is_none_or(|provider| provider == pending.provider)
}
fn pending_model_change_matches(
    pending: &PendingThinkingHandoff,
    request_id: Option<u64>,
    model: &str,
    connection_id: Option<&str>,
    provider: Option<&str>,
) -> bool {
    request_id == Some(pending.request_id)
        && model_change_matches(&pending.model, model, connection_id, provider)
}

fn state_model_label(state: &BackendState) -> Option<String> {
    state
        .model_name
        .clone()
        .or_else(|| match (&state.model_provider, &state.model_id) {
            (Some(provider), Some(id)) => Some(format!("{provider}/{id}")),
            _ => state.model_id.clone(),
        })
}

fn model_matches_query(model: &ModelInfo, needle: &str) -> bool {
    model.provider.to_lowercase().contains(needle)
        || model.id.to_lowercase().contains(needle)
        || model.name.to_lowercase().contains(needle)
        || model
            .connection_display
            .as_deref()
            .is_some_and(|display| display.to_lowercase().contains(needle))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Normal,
    SlashComplete {
        prefix: String,
        selected: usize,
    },
    ModelSelect {
        selected: usize,
        query: String,
    },
    SessionSelect {
        selected: usize,
    },
    ForkSelect {
        selected: usize,
    },
    ThinkingSelect {
        selected: usize,
    },
    ApprovalSelect {
        selected: usize,
    },
    FullAccessConfirm {
        selected: FullAccessConfirmationAction,
    },
    HelpOverlay,
    DialogConfirm,
    DialogInput,
    DialogSecretInput,
    DialogSelect,
    PerfOverlay,
    LocalEditCompose {
        step: LocalEditComposeStep,
        draft: LocalEditDraft,
    },
    LocalEditReview {
        preview: LocalEditReview,
        selected: LocalEditReviewAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FullAccessConfirmationAction {
    Enable,
    Cancel,
}

#[derive(Debug)]
struct PendingDialog {
    request: DialogRequest,
    input_buffer: String,
    cursor_pos: usize,
    secret_input: Option<SecretInput>,
    selected: usize,
    confirm_accepted: bool,
}

const MAX_SECRET_BYTES: usize = 8192;

struct SecretInput {
    value: Vec<u8>,
    len: usize,
    cursor_pos: usize,
}

impl SecretInput {
    fn new() -> Self {
        Self {
            value: vec![0; MAX_SECRET_BYTES],
            len: 0,
            cursor_pos: 0,
        }
    }

    fn text(&self) -> &str {
        match std::str::from_utf8(&self.value[..self.len]) {
            Ok(value) => value,
            Err(_) => unreachable!("secret buffer must remain valid UTF-8"),
        }
    }

    fn normalized_cursor(&self) -> usize {
        byte_boundary_at_or_before(self.text(), self.cursor_pos.min(self.len))
    }

    fn masked_value(&self) -> String {
        let scalar_count = self.text().chars().count();
        let mut masked = String::with_capacity(scalar_count.saturating_mul('•'.len_utf8()));
        for _ in 0..scalar_count {
            masked.push('•');
        }
        masked
    }

    fn masked_cursor_pos(&self) -> usize {
        let cursor_pos = self.normalized_cursor();
        self.text()[..cursor_pos]
            .chars()
            .count()
            .saturating_mul('•'.len_utf8())
    }

    fn insert(&mut self, value: char) {
        let width = value.len_utf8();
        let Some(new_len) = self.len.checked_add(width) else {
            return;
        };
        if new_len > MAX_SECRET_BYTES {
            return;
        }

        let cursor_pos = self.normalized_cursor();
        self.value
            .copy_within(cursor_pos..self.len, cursor_pos + width);
        let _ = value.encode_utf8(&mut self.value[cursor_pos..cursor_pos + width]);
        self.len = new_len;
        self.cursor_pos = cursor_pos + width;
    }

    /// Batch insert for bracketed paste. Line breaks are dropped: the field is
    /// single-line (Enter submits), so a pasted newline is unreachable state
    /// that would silently corrupt a credential.
    fn insert_str(&mut self, text: &str) {
        for value in text.chars() {
            if value == '\n' || value == '\r' {
                continue;
            }
            self.insert(value);
        }
    }

    fn backspace(&mut self) {
        let cursor_pos = self.normalized_cursor();
        if cursor_pos == 0 {
            return;
        }

        let previous = prev_char_boundary(self.text(), cursor_pos);
        let removed = cursor_pos - previous;
        self.value.copy_within(cursor_pos..self.len, previous);
        self.len -= removed;
        self.value[self.len..self.len + removed].fill(0);
        self.cursor_pos = previous;
    }

    fn delete(&mut self) {
        let cursor_pos = self.normalized_cursor();
        if cursor_pos >= self.len {
            return;
        }

        let next = next_char_boundary(self.text(), cursor_pos);
        let removed = next - cursor_pos;
        self.value.copy_within(next..self.len, cursor_pos);
        self.len -= removed;
        self.value[self.len..self.len + removed].fill(0);
        self.cursor_pos = cursor_pos;
    }

    fn move_left(&mut self) {
        self.cursor_pos = prev_char_boundary(self.text(), self.cursor_pos);
    }

    fn move_right(&mut self) {
        self.cursor_pos = next_char_boundary(self.text(), self.cursor_pos);
    }

    fn move_home(&mut self) {
        self.cursor_pos = 0;
    }

    fn move_end(&mut self) {
        self.cursor_pos = self.len;
    }

    fn wipe(&mut self) {
        self.value.as_mut_slice().zeroize();
        self.len = 0;
        self.cursor_pos = 0;
    }

    fn into_value(mut self) -> String {
        let mut value = std::mem::take(&mut self.value);
        value.truncate(self.len);
        self.len = 0;
        self.cursor_pos = 0;
        match String::from_utf8(value) {
            Ok(value) => value,
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.zeroize();
                unreachable!("secret buffer must remain valid UTF-8");
            }
        }
    }
}

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretInput")
            .field("value", &"[REDACTED]")
            .field("len", &self.len)
            .field("cursor_pos", &self.cursor_pos)
            .finish()
    }
}

impl Drop for SecretInput {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[derive(Clone)]
struct DialogRenderSnapshot {
    request: DialogRequest,
    input_buffer: String,
    cursor_pos: usize,
    selected: usize,
    confirm_accepted: bool,
}

impl PendingDialog {
    fn render_snapshot(&self) -> DialogRenderSnapshot {
        let (input_buffer, cursor_pos) = self.secret_input.as_ref().map_or_else(
            || (self.input_buffer.clone(), self.cursor_pos),
            |secret| (secret.masked_value(), secret.masked_cursor_pos()),
        );

        DialogRenderSnapshot {
            request: self.request.clone(),
            input_buffer,
            cursor_pos,
            selected: self.selected,
            confirm_accepted: self.confirm_accepted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalEditComposeStep {
    Kind,
    Path,
    ExpectedSha256,
    Find,
    Replace,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalEditOperationKind {
    Modify,
    Create,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LocalEditDraft {
    kind: Option<LocalEditOperationKind>,
    path: Option<String>,
    expected_sha256: Option<String>,
    find: Option<String>,
    replace: Option<String>,
    content: Option<String>,
    buffer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalEditReview {
    preview_id: String,
    permission_decision_id: String,
    path: String,
    operation: String,
    review_state: LocalEditReviewState,
    diff_summary: String,
    diff_summary_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalEditReviewAction {
    Apply,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalEditDecisionSubmission {
    Idle,
    Submitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionMessageHydration {
    None,
    ExplicitResume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelAvailabilityRefresh {
    Idle,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingModelConnectionId {
    NotPending,
    NoConnection,
    Connection(String),
}
struct PendingThinkingHandoff {
    request_id: u64,
    model: ModelInfo,
    activation_succeeded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamState {
    Idle,
    Streaming { session_id: String },
    LocallyCancelled { session_id: String },
}

impl StreamState {
    fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    fn is_display_streaming(&self) -> bool {
        matches!(self, Self::Streaming { .. })
    }
}

const MAX_QUEUED_DIALOGS: usize = 8;
const MAX_TOOL_ERROR_EXCERPT_CHARS: usize = 240;
const DEFAULT_TRANSCRIPT_VIEW_WIDTH: u16 = 80;
const DEFAULT_TRANSCRIPT_VIEW_HEIGHT: u16 = 20;
const EMPTY_ASSISTANT_RESPONSE_MESSAGE: &str = "assistant returned no text";

// Independent UI facts (connection, focus, streaming, estimate), not
// encodable states of one machine.
#[expect(clippy::struct_excessive_bools)]
pub struct App {
    transcript: Transcript,
    theme: Theme,
    transcript_cache: TranscriptRenderCache,
    scroll_offset: usize,
    scrollback_archive_count: usize,
    prompt: TextArea<'static>,
    active_tools: Vec<ActiveTool>,
    /// Estimated percent of the usable context window in use, from
    /// backend session stats (the compaction trigger's accounting).
    context_used_percent: Option<u8>,
    /// Last backend-owned counts and configured context capacity for `/status`.
    session_stats: Option<SessionStats>,
    /// Human-facing model label for the header and status surfaces.
    model: String,
    /// Raw protocol model identity used for exact picker-row matching.
    model_id: String,
    model_connection_id: Option<String>,
    available_models: Vec<ModelInfo>,
    discovered_models: Vec<ModelInfo>,
    model_availability_refresh: ModelAvailabilityRefresh,
    session_id: String,
    status_message: String,
    is_connected: bool,
    terminal_focused: bool,
    is_streaming: bool,
    stream_state: StreamState,
    should_quit: bool,
    mode: AppMode,
    sessions: Vec<String>,
    session_labels: Vec<String>,
    session_is_path: Vec<bool>,
    fork_messages: Vec<ForkMessage>,
    session_tree: Option<SessionTree>,
    thinking_level: ThinkingLevel,
    approval_mode: ApprovalMode,
    pending_model: Option<String>,
    pending_model_id: Option<String>,
    pending_model_connection_id: PendingModelConnectionId,
    pending_session_stats: Option<SessionStats>,
    pending_session_id: Option<String>,
    session_file: Option<String>,
    session_message_hydration: SessionMessageHydration,
    pending_thinking_level: Option<ThinkingLevel>,
    pending_thinking_handoff: Option<PendingThinkingHandoff>,
    perf_metrics: PerfMetrics,
    negotiated: Option<NegotiatedCapabilities>,
    active_dialog: Option<PendingDialog>,
    queued_dialogs: VecDeque<PendingDialog>,
    transcript_view_width: u16,
    transcript_view_height: u16,
    local_edit_request_counter: u64,
    model_change_request_counter: u64,
    extension_lifecycle_request_counter: u64,
    approval_mode_request_counter: u64,
    extension_diagnostic_request_counter: u64,
    pending_local_edit_request_id: Option<String>,
    pending_extension_lifecycle_request_id: Option<String>,
    pending_extension_diagnostic_request_id: Option<String>,
    active_local_edit_preview_id: Option<String>,
    local_edit_decision_submission: LocalEditDecisionSubmission,
    client_tx: mpsc::UnboundedSender<ClientEvent>,
}

impl App {
    fn new(client_tx: mpsc::UnboundedSender<ClientEvent>) -> Self {
        Self::new_with_theme(client_tx, Theme::default())
    }

    fn new_with_theme(client_tx: mpsc::UnboundedSender<ClientEvent>, theme: Theme) -> Self {
        Self {
            transcript: Transcript::new(),
            transcript_cache: TranscriptRenderCache::with_theme(theme),
            theme,
            scroll_offset: 0,
            scrollback_archive_count: 0,
            prompt: TextArea::default(),
            active_tools: Vec::new(),
            context_used_percent: None,
            session_stats: None,
            model: String::from("default"),
            model_id: String::from("default"),
            model_connection_id: None,
            available_models: Vec::new(),
            discovered_models: Vec::new(),
            model_availability_refresh: ModelAvailabilityRefresh::Idle,
            session_id: String::from("default"),
            status_message: String::from("connecting..."),
            is_connected: false,
            terminal_focused: true,
            is_streaming: false,
            stream_state: StreamState::Idle,
            should_quit: false,
            mode: AppMode::Normal,
            sessions: vec![String::from("default")],
            session_labels: vec![String::from("default")],
            session_is_path: vec![false],
            fork_messages: Vec::new(),
            session_tree: None,
            thinking_level: ThinkingLevel::Off,
            approval_mode: ApprovalMode::Review,
            pending_model: None,
            pending_model_id: None,
            pending_model_connection_id: PendingModelConnectionId::NotPending,
            pending_session_stats: None,
            pending_session_id: None,
            session_file: None,
            session_message_hydration: SessionMessageHydration::None,
            pending_thinking_level: None,
            pending_thinking_handoff: None,
            perf_metrics: PerfMetrics::new(),
            negotiated: None,
            active_dialog: None,
            queued_dialogs: VecDeque::new(),
            transcript_view_width: DEFAULT_TRANSCRIPT_VIEW_WIDTH,
            transcript_view_height: DEFAULT_TRANSCRIPT_VIEW_HEIGHT,
            model_change_request_counter: 0,
            local_edit_request_counter: 0,
            extension_lifecycle_request_counter: 0,
            approval_mode_request_counter: 0,
            extension_diagnostic_request_counter: 0,
            pending_local_edit_request_id: None,
            pending_extension_lifecycle_request_id: None,
            pending_extension_diagnostic_request_id: None,
            active_local_edit_preview_id: None,
            local_edit_decision_submission: LocalEditDecisionSubmission::Idle,
            client_tx,
        }
    }

    fn clear_model_context(&mut self) {
        self.context_used_percent = None;
        if let Some(stats) = self.session_stats.as_mut() {
            stats.context_window = None;
            stats.context_used_percent = None;
        }
    }

    fn apply_session_stats(&mut self, stats: SessionStats) {
        self.context_used_percent = stats.context_used_percent;
        let message_count = stats.message_count;
        self.session_stats = Some(stats);
        self.status_message = message_count.map_or_else(
            || String::from("session stats loaded"),
            |count| format!("session messages: {count}"),
        );
    }

    fn set_stream_state(&mut self, stream_state: StreamState) {
        self.is_streaming = stream_state.is_display_streaming();
        self.stream_state = stream_state;
        if matches!(self.stream_state, StreamState::Idle) {
            self.apply_pending_backend_state();
            self.maybe_open_pending_thinking_handoff();
        }
    }

    fn apply_pending_backend_state(&mut self) {
        if let Some(session_id) = self.pending_session_id.take() {
            self.session_id = session_id;
        }
        if let Some(model) = self.pending_model.take() {
            self.clear_model_context();
            self.model = model;
            self.model_id = self
                .pending_model_id
                .take()
                .unwrap_or_else(|| self.model.clone());
            self.model_connection_id = match std::mem::replace(
                &mut self.pending_model_connection_id,
                PendingModelConnectionId::NotPending,
            ) {
                PendingModelConnectionId::NotPending | PendingModelConnectionId::NoConnection => {
                    None
                }
                PendingModelConnectionId::Connection(connection_id) => Some(connection_id),
            };
        }
        if let Some(stats) = self.pending_session_stats.take() {
            self.apply_session_stats(stats);
        }
        if let Some(level) = self.pending_thinking_level.take() {
            self.thinking_level = level;
        }
    }
    fn maybe_open_pending_thinking_handoff(&mut self) {
        let ui_idle = matches!(self.mode, AppMode::Normal)
            && self.active_dialog.is_none()
            && self.queued_dialogs.is_empty()
            && self.pending_local_edit_request_id.is_none()
            && self.pending_extension_lifecycle_request_id.is_none()
            && self.pending_extension_diagnostic_request_id.is_none()
            && !self.transcript.has_unresolved_review()
            && matches!(
                self.local_edit_decision_submission,
                LocalEditDecisionSubmission::Idle
            );
        let ready = !self.backend_busy()
            && ui_idle
            && self
                .pending_thinking_handoff
                .as_ref()
                .is_some_and(|pending| pending.activation_succeeded);
        if ready {
            self.pending_thinking_handoff = None;
            self.open_thinking_selector();
        }
    }

    fn backend_busy(&self) -> bool {
        self.stream_state.is_busy()
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.transcript_cache.max_scroll_start(
            &self.transcript,
            self.transcript_view_width,
            self.transcript_view_height,
        );
    }

    fn at_transcript_bottom(&mut self) -> bool {
        self.scroll_offset
            >= self.transcript_cache.max_scroll_start(
                &self.transcript,
                self.transcript_view_width,
                self.transcript_view_height,
            )
    }

    fn set_transcript_viewport(&mut self, width: u16, height: u16) {
        let was_at_bottom = self.at_transcript_bottom();
        self.transcript_view_width = width.max(1);
        self.transcript_view_height = height.max(1);
        if was_at_bottom {
            self.scroll_to_bottom();
        } else {
            let max_start = self.transcript_cache.max_scroll_start(
                &self.transcript,
                self.transcript_view_width,
                self.transcript_view_height,
            );
            self.scroll_offset = self.scroll_offset.min(max_start);
        }
    }

    fn scroll_transcript_up(&mut self) {
        let page = usize::from(self.transcript_view_height.max(1));
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
    }

    fn scroll_transcript_lines_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    fn scroll_transcript_lines_down(&mut self, lines: usize) {
        let max_start = self.transcript_cache.max_scroll_start(
            &self.transcript,
            self.transcript_view_width,
            self.transcript_view_height,
        );
        self.scroll_offset = self.scroll_offset.saturating_add(lines).min(max_start);
    }

    fn scroll_transcript_down(&mut self) {
        let page = usize::from(self.transcript_view_height.max(1));
        let max_start = self.transcript_cache.max_scroll_start(
            &self.transcript,
            self.transcript_view_width,
            self.transcript_view_height,
        );
        self.scroll_offset = self.scroll_offset.saturating_add(page).min(max_start);
    }

    fn accepts_session_event(&self, session_id: &str) -> bool {
        session_id == "active" || session_id == self.session_id
    }

    fn should_accept_delta(&self, session_id: &str) -> bool {
        match &self.stream_state {
            StreamState::Idle => self.accepts_session_event(session_id),
            StreamState::Streaming {
                session_id: active_session,
            } => {
                session_id == "active"
                    || session_id == active_session
                    || self.pending_session_id.as_deref() == Some(session_id)
            }
            StreamState::LocallyCancelled { .. } => false,
        }
    }

    fn supports(&self, capability: Capability) -> bool {
        self.negotiated
            .as_ref()
            .is_some_and(|negotiated| negotiated.supports(capability))
    }
    fn request_connections(&mut self) {
        self.clear_input();
        if !self.is_connected || !self.supports(Capability::ProviderConnections) {
            self.status_message = String::from("provider connections unavailable");
            return;
        }

        if self.send_client_event(ClientEvent::ConnectionsRequested) {
            self.pending_thinking_handoff = None;
            self.status_message = String::from("loading provider connections");
        }
    }

    fn request_available_models(&mut self) -> bool {
        let requested = self.send_client_event(ClientEvent::AvailableModelsRequested);
        if requested {
            self.model_availability_refresh = ModelAvailabilityRefresh::Pending;
        }
        requested
    }

    fn mark_disconnected(&mut self, reason: String) {
        self.is_connected = false;
        self.pending_thinking_handoff = None;
        self.pending_model = None;
        self.pending_model_connection_id = PendingModelConnectionId::NotPending;
        self.pending_session_stats = None;
        self.pending_session_id = None;
        self.pending_thinking_level = None;
        self.pending_local_edit_request_id = None;
        self.pending_extension_lifecycle_request_id = None;
        self.pending_extension_diagnostic_request_id = None;
        self.active_local_edit_preview_id = None;
        self.transcript.interrupt_pending_reviews();
        self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
        self.active_tools.clear();
        self.active_dialog = None;
        self.queued_dialogs.clear();
        self.mode = AppMode::Normal;
        self.set_stream_state(StreamState::Idle);
        self.status_message = if reason.is_empty() {
            String::from("disconnected")
        } else {
            reason
        };
    }

    fn send_client_event(&mut self, event: ClientEvent) -> bool {
        if self.client_tx.send(event).is_ok() {
            true
        } else {
            self.mark_disconnected(String::from("backend disconnected"));
            false
        }
    }
    fn handle_terminal_focus_event(&mut self, event: &Event) {
        match event {
            Event::FocusGained => self.terminal_focused = true,
            Event::FocusLost => self.terminal_focused = false,
            _ => {}
        }
    }

    fn handle_backend_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Connected { negotiated } => {
                self.is_connected = true;
                self.negotiated = Some(negotiated.clone());
                self.status_message = format!("connected: {}", negotiated.adapter_agent_name);
            }
            BackendEvent::Server(event) => {
                self.handle_server_event(event);
                self.maybe_open_pending_thinking_handoff();
            }
            BackendEvent::Disconnected { reason } => self.mark_disconnected(reason),
        }
    }

    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Ready { .. } => {}
            ServerEvent::StateUpdated(state) => self.apply_backend_state(state),
            ServerEvent::ApprovalModeChanged { mode, .. } => {
                self.approval_mode = mode;
                self.status_message = format!("approval mode: {}", mode.as_str());
            }
            ServerEvent::ApprovalModeChangeFailed { message, .. } => {
                self.status_message = message;
            }
            ServerEvent::PromptDelta { session_id, delta } => {
                let was_at_bottom = self.at_transcript_bottom();
                if self.should_accept_delta(&session_id) {
                    if matches!(self.stream_state, StreamState::Idle) {
                        self.set_stream_state(StreamState::Streaming {
                            session_id: self.session_id.clone(),
                        });
                    }
                    self.transcript.append_delta(&delta);
                    if was_at_bottom {
                        self.scroll_to_bottom();
                    }
                }
            }
            ServerEvent::PromptFinished {
                outcome, message, ..
            } => {
                if matches!(outcome, PromptOutcome::Completed)
                    && self.transcript_ended_after_tool_result()
                {
                    self.transcript
                        .append_assistant_message(EMPTY_ASSISTANT_RESPONSE_MESSAGE);
                    self.scroll_to_bottom();
                }
                if matches!(outcome, PromptOutcome::Failed | PromptOutcome::Cancelled) {
                    let error = message
                        .clone()
                        .unwrap_or_else(|| String::from("turn failed"));
                    let kind = if matches!(outcome, PromptOutcome::Cancelled) {
                        HarnessOutcomeKind::Cancelled
                    } else {
                        classify_turn_failure_text(&error)
                    };
                    self.transcript.append_harness_outcome(kind, &error);
                    self.scroll_to_bottom();
                }
                self.set_stream_state(StreamState::Idle);
                self.active_tools.clear();
                self.transcript.interrupt_pending_reviews();
                self.status_message = message.unwrap_or_else(|| format!("prompt {outcome:?}"));
            }
            ServerEvent::ToolCallStarted {
                tool_call_id,
                tool_name,
                preview,
            } => {
                if matches!(self.stream_state, StreamState::LocallyCancelled { .. }) {
                    return;
                }
                if matches!(self.stream_state, StreamState::Idle) {
                    self.set_stream_state(StreamState::Streaming {
                        session_id: self.session_id.clone(),
                    });
                }
                self.transcript.append_tool_call(
                    tool_call_id.as_deref(),
                    &tool_name,
                    preview.as_deref(),
                );
                let active_tool = ActiveTool {
                    id: tool_call_id,
                    name: tool_name,
                    preview,
                };
                if !self
                    .active_tools
                    .iter()
                    .any(|tool| same_tool(tool, &active_tool))
                {
                    self.active_tools.push(active_tool);
                }
            }
            ServerEvent::ToolCallOutput {
                tool_call_id,
                chunk,
            } => {
                if matches!(self.stream_state, StreamState::LocallyCancelled { .. }) {
                    return;
                }
                self.transcript
                    .append_tool_call_output(&tool_call_id, &chunk);
                self.scroll_to_bottom();
            }
            ServerEvent::ToolCallFinished(result) => {
                if matches!(self.stream_state, StreamState::LocallyCancelled { .. }) {
                    return;
                }
                let active_tool =
                    self.take_active_tool(result.tool_call_id.as_deref(), &result.tool_name);
                let label = active_tool
                    .as_ref()
                    .map_or_else(|| result.tool_name.clone(), ActiveTool::label);
                let summary = tool_output_summary(
                    &result.output,
                    result.is_error,
                    result.metadata.as_ref(),
                    result.outcome_kind,
                );
                if !self.transcript.finish_tool_call_record(
                    result.tool_call_id.as_deref(),
                    &result.tool_name,
                    &label,
                    &summary,
                    &result.output,
                    result.is_error,
                    result.outcome_kind,
                    None,
                ) {
                    self.transcript.append_tool_result_record(
                        result.tool_call_id.as_deref(),
                        &label,
                        &summary,
                        &result.output,
                        result.is_error,
                        result.outcome_kind,
                        None,
                    );
                }
                self.scroll_to_bottom();
            }
            ServerEvent::StatusUpdated { message } => {
                match status_lifecycle(&message) {
                    Some(StatusLifecycle::Ended) => {
                        self.set_stream_state(StreamState::Idle);
                        self.active_tools.clear();
                    }
                    Some(StatusLifecycle::Started) => {
                        self.set_stream_state(StreamState::Streaming {
                            session_id: self.session_id.clone(),
                        });
                    }
                    Some(StatusLifecycle::Internal) | None => {}
                }
                if !is_lifecycle_status(&message) {
                    self.status_message.clone_from(&message);
                }
            }
            ServerEvent::ModelChangeFailed(target) => {
                if self
                    .pending_thinking_handoff
                    .as_ref()
                    .is_some_and(|pending| {
                        pending_model_change_matches(
                            pending,
                            target.request_id,
                            &target.model,
                            target.connection_id.as_deref(),
                            target.provider.as_deref(),
                        )
                    })
                {
                    self.pending_thinking_handoff = None;
                }
            }
            ServerEvent::SessionChanged { session_id } => {
                if !self.sessions.contains(&session_id) {
                    self.sessions.push(session_id.clone());
                    self.session_labels.push(session_id.clone());
                    self.session_is_path.push(false);
                }
                if self.backend_busy() {
                    self.pending_session_id = Some(session_id.clone());
                    self.status_message = format!("session pending: {session_id}");
                } else {
                    self.session_id.clone_from(&session_id);
                }
            }
            ServerEvent::ModelChanged(target) => {
                let completes_thinking_handoff = self
                    .pending_thinking_handoff
                    .as_ref()
                    .is_some_and(|pending| {
                        pending_model_change_matches(
                            pending,
                            target.request_id,
                            &target.model,
                            target.connection_id.as_deref(),
                            target.provider.as_deref(),
                        )
                    });
                let label = self.model_label_for(&target.model, target.connection_id.as_deref());
                if self.backend_busy() {
                    self.pending_model = Some(label);
                    self.pending_model_id = Some(target.model.clone());
                    self.pending_model_connection_id = target.connection_id.map_or(
                        PendingModelConnectionId::NoConnection,
                        PendingModelConnectionId::Connection,
                    );
                    self.status_message = format!("model pending: {}", target.model);
                } else {
                    // A model name must never render beside the previous model's
                    // capacity while the backend publishes replacement stats.
                    self.clear_model_context();
                    self.model = label;
                    self.model_id = target.model;
                    self.model_connection_id = target.connection_id;
                }
                if completes_thinking_handoff {
                    if let Some(pending) = self.pending_thinking_handoff.as_mut() {
                        pending.activation_succeeded = true;
                    }
                    self.maybe_open_pending_thinking_handoff();
                }
            }
            ServerEvent::ThinkingLevelApplied { level } => {
                self.thinking_level = level;
                self.status_message = format!("thinking: {}", level.as_str());
            }
            ServerEvent::DiscoveredModelsUpdated { models } => {
                self.discovered_models = models;
                self.clamp_model_select_selection();
            }
            ServerEvent::AvailableModelsUpdated { models } => {
                let refresh_was_pending = matches!(
                    std::mem::replace(
                        &mut self.model_availability_refresh,
                        ModelAvailabilityRefresh::Idle,
                    ),
                    ModelAvailabilityRefresh::Pending
                );
                self.available_models = models;
                if self.available_models.is_empty() {
                    self.status_message = String::from("no available models reported");
                } else if refresh_was_pending && self.status_message == "loading available models" {
                    self.status_message = String::from("available models loaded");
                }
                self.clamp_model_select_selection();
            }
            ServerEvent::ForkMessagesUpdated { messages } => {
                let count = messages.len();
                self.fork_messages = messages;
                if count == 0 {
                    self.mode = AppMode::Normal;
                    self.status_message = String::from("no fork points available");
                } else {
                    self.mode = AppMode::ForkSelect { selected: 0 };
                    self.status_message = format!("fork points loaded: {count}");
                }
            }
            ServerEvent::SessionMessagesUpdated { messages } => {
                let tree = build_session_tree(&messages);
                if self.session_message_hydration == SessionMessageHydration::ExplicitResume {
                    self.hydrate_transcript_from_session_messages(&messages);
                    self.session_message_hydration = SessionMessageHydration::None;
                }
                self.status_message = branch_summary_line(&tree);
                self.session_tree = Some(tree);
            }
            ServerEvent::SessionStatsUpdated(stats) => {
                if self.pending_model.is_some() {
                    self.pending_session_stats = Some(stats);
                } else {
                    self.apply_session_stats(stats);
                }
            }
            ServerEvent::RecentSessionsUpdated { sessions } => {
                self.apply_recent_sessions(sessions);
            }
            ServerEvent::DialogRequested(request) => self.open_dialog(request),
            ServerEvent::ToolReviewRequested {
                request_id,
                tool_name,
                payload,
            } => {
                if matches!(self.mode, AppMode::SlashComplete { .. }) {
                    self.mode = AppMode::Normal;
                }
                self.transcript
                    .begin_tool_review(&request_id, &tool_name, payload);
                self.status_message =
                    String::from("review pending · ↑/↓ or j/k select · Enter confirm");
                self.scroll_to_bottom();
            }
            ServerEvent::ToolReviewResolved {
                request_id,
                resolution,
            } => {
                if self.transcript.resolve_tool_review(&request_id, resolution) {
                    self.status_message = match resolution {
                        ToolReviewResolution::Approved => String::from("review approved"),
                        ToolReviewResolution::Rejected => String::from("review rejected"),
                        ToolReviewResolution::Interrupted => String::from("review interrupted"),
                    };
                    self.scroll_to_bottom();
                }
            }
            ServerEvent::LocalEditPreviewReady {
                request_id,
                preview,
            } => {
                if !self.should_accept_local_edit_preview(&request_id) {
                    return;
                }
                let status_message = local_edit_review_status_message(preview.review_state);
                self.pending_local_edit_request_id = None;
                self.active_local_edit_preview_id = Some(preview.preview_id.clone());
                self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
                self.status_message = String::from(status_message);
                self.mode = AppMode::LocalEditReview {
                    preview: LocalEditReview {
                        preview_id: preview.preview_id,
                        permission_decision_id: preview.permission_decision_id,
                        path: preview.path,
                        operation: preview.operation,
                        review_state: preview.review_state,
                        diff_summary: preview.diff_summary,
                        diff_summary_truncated: preview.diff_summary_truncated,
                    },
                    selected: LocalEditReviewAction::Apply,
                };
            }
            ServerEvent::LocalEditFinished {
                preview_id,
                outcome,
                message,
            } => {
                if !self.should_accept_local_edit_finish(preview_id.as_deref()) {
                    return;
                }
                self.pending_local_edit_request_id = None;
                self.active_local_edit_preview_id = None;
                self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
                self.mode = AppMode::Normal;
                self.status_message = if message.is_empty() {
                    format!("local edit {outcome:?}")
                } else {
                    message
                };
            }
            ServerEvent::ExtensionLifecycleFinished {
                request_id,
                outcome,
                message,
                selector,
                ..
            } => {
                if self.pending_extension_lifecycle_request_id.as_deref() != Some(&request_id) {
                    return;
                }
                self.pending_extension_lifecycle_request_id = None;
                let status_message = if message.is_empty() {
                    match outcome {
                        ExtensionLifecycleOutcome::Completed => String::from("extension updated"),
                        ExtensionLifecycleOutcome::NotFound => String::from("extension not found"),
                        ExtensionLifecycleOutcome::NotActive => {
                            String::from("extension is not active")
                        }
                        ExtensionLifecycleOutcome::Failed => {
                            String::from("extension lifecycle failed")
                        }
                    }
                } else {
                    message
                };
                self.status_message = status_message;
                if !selector.is_empty() {
                    self.request_extension_diagnostics(Some(&selector), false);
                }
            }
            ServerEvent::ExtensionDiagnosticSnapshotUpdated {
                request_id,
                outcome,
                records,
                message,
            } => {
                if self.pending_extension_diagnostic_request_id.as_deref() != Some(&request_id) {
                    return;
                }
                self.pending_extension_diagnostic_request_id = None;
                self.status_message =
                    extension_diagnostic_snapshot_summary(outcome, &records, message.as_deref());
                self.transcript.append_tool_result(
                    None,
                    "extension_status",
                    &render_extension_diagnostic_snapshot(&records),
                    matches!(outcome, ExtensionDiagnosticSnapshotOutcome::Failed),
                );
                self.scroll_to_bottom();
            }
            ServerEvent::NotificationRaised(notification) => {
                self.status_message = format!("[{}] {}", notification.level, notification.message);
            }
            ServerEvent::WidgetUpdated(widget) => {
                self.status_message = if widget.body.is_empty() {
                    format!("[widget: {}]", widget.title)
                } else {
                    format!("[widget: {}] {}", widget.title, widget.body)
                };
            }
            ServerEvent::TitleChanged { title } => {
                self.status_message = title;
            }
        }
    }

    fn supports_backend_cancel(&self) -> bool {
        self.negotiated
            .as_ref()
            .is_some_and(|negotiated| negotiated.supports(Capability::PromptCancellation))
    }

    fn cancel_streaming_prompt(&mut self) {
        let session_id = self.session_id.clone();
        self.set_stream_state(StreamState::LocallyCancelled {
            session_id: session_id.clone(),
        });
        self.active_tools.clear();
        if self.supports_backend_cancel() {
            let _sent = self.send_client_event(ClientEvent::PromptCancelled { session_id });
            self.status_message = String::from("cancelling prompt...");
        } else {
            self.status_message = String::from("cancelled locally; waiting for backend");
        }
    }

    fn has_local_edit_in_flight(&self) -> bool {
        self.pending_local_edit_request_id.is_some()
            || self.active_local_edit_preview_id.is_some()
            || matches!(
                self.local_edit_decision_submission,
                LocalEditDecisionSubmission::Submitted
            )
    }

    fn should_accept_local_edit_preview(&self, request_id: &str) -> bool {
        self.pending_local_edit_request_id.as_deref() == Some(request_id)
    }

    fn should_accept_local_edit_finish(&self, preview_id: Option<&str>) -> bool {
        match preview_id {
            Some(preview_id) => self.active_local_edit_preview_id.as_deref() == Some(preview_id),
            None => self.pending_local_edit_request_id.is_some(),
        }
    }

    fn apply_backend_state(&mut self, state: BackendState) {
        let busy = self.backend_busy();
        if let Some(model_id) = state.model_id.clone() {
            let model = state_model_label(&state).unwrap_or_else(|| model_id.clone());
            if busy {
                self.pending_model = Some(model);
                self.pending_model_id = Some(model_id);
                self.pending_model_connection_id = state.model_connection_id.clone().map_or(
                    PendingModelConnectionId::NoConnection,
                    PendingModelConnectionId::Connection,
                );
            } else {
                self.model = model;
                self.model_id = model_id;
                self.model_connection_id = state.model_connection_id.clone();
            }
        }

        if let Some(session_id) = state.session_id {
            if !self.sessions.contains(&session_id) {
                self.sessions.push(session_id.clone());
                self.session_labels.push(session_id.clone());
                self.session_is_path.push(false);
            }
            if busy {
                self.pending_session_id = Some(session_id);
            } else {
                self.session_id.clone_from(&session_id);
            }
        }

        if state.session_file.is_some() {
            self.session_file = state.session_file;
        }

        if let Some(level) = state.thinking_level {
            if busy {
                self.pending_thinking_level = Some(level);
            } else {
                self.thinking_level = level;
            }
        }

        if state.is_streaming && matches!(self.stream_state, StreamState::Idle) {
            self.set_stream_state(StreamState::Streaming {
                session_id: self.session_id.clone(),
            });
        } else if !state.is_streaming {
            self.set_stream_state(StreamState::Idle);
        }

        if state.is_compacting {
            self.status_message = String::from("compacting");
        } else if !self.status_message.starts_with("connected") {
            self.status_message = String::from("state loaded");
        }
    }
    fn model_label_for(&self, model_id: &str, connection_id: Option<&str>) -> String {
        self.available_models
            .iter()
            .find(|candidate| {
                candidate.id == model_id && candidate.connection_id.as_deref() == connection_id
            })
            .map_or_else(|| model_id.to_owned(), |candidate| candidate.name.clone())
    }

    fn take_active_tool(&mut self, id: Option<&str>, name: &str) -> Option<ActiveTool> {
        let index = self
            .active_tools
            .iter()
            .position(|tool| match (&tool.id, id) {
                (Some(tool_id), Some(id)) => tool_id == id,
                _ => tool.name == name,
            })?;

        Some(self.active_tools.remove(index))
    }

    fn open_dialog(&mut self, request: DialogRequest) {
        let pending = Self::pending_dialog(request);
        let replaces_active = self.active_dialog.as_ref().is_some_and(|active| {
            is_provider_connection_dialog(&active.request)
                && is_provider_connection_dialog(&pending.request)
        });
        if self.active_dialog.is_some() && !replaces_active {
            if self.queued_dialogs.len() >= MAX_QUEUED_DIALOGS {
                self.status_message = String::from("dialog queue full");
                let dialog_id = pending.request.id.clone().unwrap_or_default();
                self.send_client_event(ClientEvent::DialogResolved {
                    dialog_id,
                    response: DialogResponse::Cancelled,
                });
                return;
            }
            self.queued_dialogs.push_back(pending);
            self.status_message = String::from("dialog queued");
            return;
        }
        self.activate_dialog(pending);
    }

    fn pending_dialog(request: DialogRequest) -> PendingDialog {
        match &request.kind {
            DialogKind::Confirm | DialogKind::DeviceCode { .. } => PendingDialog {
                request,
                input_buffer: String::new(),
                cursor_pos: 0,
                secret_input: None,
                selected: 0,
                confirm_accepted: true,
            },
            DialogKind::Input { default } => {
                let input_buffer = default.clone().unwrap_or_default();
                let cursor_pos = input_buffer.len();
                PendingDialog {
                    request,
                    input_buffer,
                    cursor_pos,
                    secret_input: None,
                    selected: 0,
                    confirm_accepted: false,
                }
            }
            DialogKind::Editor { initial_text } => {
                let input_buffer = initial_text.clone().unwrap_or_default();
                let cursor_pos = input_buffer.len();
                PendingDialog {
                    request,
                    input_buffer,
                    cursor_pos,
                    secret_input: None,
                    selected: 0,
                    confirm_accepted: false,
                }
            }
            DialogKind::Select { .. } => PendingDialog {
                request,
                input_buffer: String::new(),
                cursor_pos: 0,
                secret_input: None,
                selected: 0,
                confirm_accepted: false,
            },
            DialogKind::SecretInput => PendingDialog {
                request,
                input_buffer: String::new(),
                cursor_pos: 0,
                secret_input: Some(SecretInput::new()),
                selected: 0,
                confirm_accepted: false,
            },
        }
    }

    fn activate_dialog(&mut self, pending: PendingDialog) {
        self.status_message = dialog_summary(&pending.request);
        self.mode = match &pending.request.kind {
            DialogKind::Confirm | DialogKind::DeviceCode { .. } => AppMode::DialogConfirm,
            DialogKind::Input { .. } | DialogKind::Editor { .. } => AppMode::DialogInput,
            DialogKind::Select { .. } => AppMode::DialogSelect,
            DialogKind::SecretInput => AppMode::DialogSecretInput,
        };
        self.active_dialog = Some(pending);
    }

    fn clear_dialog(&mut self) {
        self.active_dialog = None;
        if let Some(next) = self.queued_dialogs.pop_front() {
            self.activate_dialog(next);
        } else {
            self.mode = AppMode::Normal;
        }
    }

    fn submit_dialog_response(&mut self, response: DialogResponse) {
        let Some(dialog) = self.active_dialog.as_ref() else {
            self.mode = AppMode::Normal;
            return;
        };

        let dialog_id = dialog.request.id.clone().unwrap_or_default();
        if self.send_client_event(ClientEvent::DialogResolved {
            dialog_id,
            response,
        }) {
            self.status_message = String::from("dialog resolved");
        }
        self.clear_dialog();
    }

    fn cancel_dialog(&mut self) {
        self.submit_dialog_response(DialogResponse::Cancelled);
    }

    fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        if matches!(self.mode, AppMode::Normal | AppMode::SlashComplete { .. })
            && self.handle_inline_tool_review_key(key, modifiers)
        {
            self.maybe_open_pending_thinking_handoff();
            return;
        }
        match &self.mode {
            AppMode::Normal => self.handle_normal_key(key, modifiers),
            AppMode::SlashComplete { .. } => self.handle_slash_complete_key(key, modifiers),
            AppMode::ModelSelect { .. } => self.handle_model_select_key(key, modifiers),
            AppMode::SessionSelect { .. } => self.handle_session_select_key(key, modifiers),
            AppMode::ForkSelect { .. } => self.handle_fork_select_key(key, modifiers),
            AppMode::ThinkingSelect { .. } => self.handle_thinking_select_key(key, modifiers),
            AppMode::ApprovalSelect { .. } => self.handle_approval_select_key(key, modifiers),
            AppMode::FullAccessConfirm { .. } => {
                self.handle_full_access_confirm_key(key, modifiers);
            }
            AppMode::HelpOverlay => self.handle_help_overlay_key(key, modifiers),
            AppMode::DialogConfirm => self.handle_dialog_confirm_key(key, modifiers),
            AppMode::DialogInput => self.handle_dialog_input_key(key, modifiers),
            AppMode::DialogSecretInput => self.handle_secret_dialog_key(key, modifiers),
            AppMode::DialogSelect => self.handle_dialog_select_key(key, modifiers),
            AppMode::PerfOverlay => self.handle_perf_overlay_key(key, modifiers),
            AppMode::LocalEditCompose { .. } => self.handle_local_edit_compose_key(key, modifiers),
            AppMode::LocalEditReview { .. } => self.handle_local_edit_review_key(key, modifiers),
        }
        self.maybe_open_pending_thinking_handoff();
    }

    fn handle_inline_tool_review_key(&mut self, key: KeyCode, modifiers: KeyModifiers) -> bool {
        if !self.transcript.has_unresolved_review() {
            return false;
        }
        match (key, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.cancel_streaming_prompt(),
            (KeyCode::PageUp, _) => self.scroll_transcript_up(),
            (KeyCode::PageDown, _) => self.scroll_transcript_down(),
            (KeyCode::End, modifiers) if modifiers.is_empty() => self.scroll_to_bottom(),
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.transcript.toggle_tool_details();
                self.scroll_to_bottom();
            }
            (KeyCode::Up | KeyCode::Char('k'), modifiers)
                if modifiers.is_empty() && self.transcript.has_pending_review() =>
            {
                self.transcript
                    .select_pending_review(ToolReviewDecision::Approve);
                self.scroll_to_bottom();
            }
            (KeyCode::Down | KeyCode::Char('j'), modifiers)
                if modifiers.is_empty() && self.transcript.has_pending_review() =>
            {
                self.transcript
                    .select_pending_review(ToolReviewDecision::Reject);
                self.scroll_to_bottom();
            }
            (KeyCode::Enter, modifiers)
                if modifiers.is_empty() && self.transcript.has_pending_review() =>
            {
                self.submit_inline_tool_review(None);
            }
            (KeyCode::Esc, modifiers)
                if modifiers.is_empty() && self.transcript.has_pending_review() =>
            {
                self.submit_inline_tool_review(Some(ToolReviewDecision::Reject));
            }
            _ => {
                self.status_message = if self.transcript.has_pending_review() {
                    String::from("review pending · ↑/↓ or j/k select · Enter confirm")
                } else {
                    String::from("review decision submitted; waiting for tool result")
                };
            }
        }
        true
    }

    fn submit_inline_tool_review(&mut self, decision: Option<ToolReviewDecision>) {
        let submission = match decision {
            Some(decision) => self.transcript.submit_pending_review_as(decision),
            None => self.transcript.submit_pending_review(),
        };
        let Some((request_id, preview_id, permission_decision_id, decision)) = submission else {
            return;
        };
        self.send_client_event(ClientEvent::ToolReviewDecisionSubmitted {
            request_id,
            preview_id,
            permission_decision_id,
            decision,
        });
        self.status_message = match decision {
            ToolReviewDecision::Approve => String::from("review approval submitted"),
            ToolReviewDecision::Reject => String::from("review rejection submitted"),
        };
        self.scroll_to_bottom();
    }

    fn handle_normal_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match (key, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if matches!(self.stream_state, StreamState::Streaming { .. }) {
                    self.cancel_streaming_prompt();
                } else {
                    self.should_quit = true;
                }
            }
            (KeyCode::PageUp, _) => self.scroll_transcript_up(),
            (KeyCode::PageDown, _) => self.scroll_transcript_down(),
            (KeyCode::End, modifiers) if modifiers.is_empty() => self.scroll_to_bottom(),
            (KeyCode::Char('m'), modifiers)
                if modifiers.contains(KeyModifiers::ALT)
                    || modifiers.contains(KeyModifiers::META)
                    || modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.open_model_selector();
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => self.open_session_selector(),
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => self.request_session_tree(),
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => self.open_thinking_selector(),
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::PerfOverlay;
            }
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => self.fork_current_session(),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => self.clear_input(),
            (KeyCode::Esc, _) => {
                if matches!(self.stream_state, StreamState::Streaming { .. }) {
                    self.cancel_streaming_prompt();
                } else {
                    self.clear_input();
                }
            }
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => self.insert_input_newline(),
            (KeyCode::Enter, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    || (modifiers.contains(KeyModifiers::CONTROL) && self.prompt_has_text()) =>
            {
                self.insert_input_newline();
            }
            (KeyCode::Enter, _)
                if self.prompt_has_text()
                    && (!self.backend_busy() || self.prompt_is_approval_command()) =>
            {
                self.submit_input();
            }
            (KeyCode::Enter, _) if self.prompt_has_text() => {
                self.status_message = String::from("wait for current response before submitting");
            }
            (KeyCode::Enter, _) if !self.prompt.is_empty() => self.clear_input(),
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.transcript.toggle_tool_details();
                self.scroll_to_bottom();
            }
            (KeyCode::Backspace, modifiers) if clears_input(modifiers) => self.clear_input(),
            (KeyCode::Tab, _) => {
                if self.prompt_text().starts_with('/') {
                    self.enter_slash_complete();
                } else {
                    self.handle_prompt_input_key(key, modifiers);
                }
            }
            _ => {
                self.handle_prompt_input_key(key, modifiers);
                self.refresh_slash_completion(0);
            }
        }
    }

    fn refresh_slash_completion(&mut self, selected: usize) {
        let prefix = self.prompt_text();
        if !prefix.starts_with('/') || prefix.contains('\n') {
            if matches!(self.mode, AppMode::SlashComplete { .. }) {
                self.mode = AppMode::Normal;
            }
            return;
        }

        let matches = match_slash_commands(&prefix);
        if matches.is_empty() {
            self.mode = AppMode::Normal;
        } else {
            self.mode = AppMode::SlashComplete {
                prefix,
                selected: selected.min(matches.len().saturating_sub(1)),
            };
        }
    }

    fn insert_input_newline(&mut self) {
        self.prompt.insert_newline();
    }

    fn clear_input(&mut self) {
        self.prompt.clear();
    }

    /// Replace the transcript with the selected session's messages. Armed
    /// only for explicit resume (startup `--resume` or `/resume` selection
    /// of a different session), so switching sessions replaces stale
    /// scrollback instead of silently keeping it.
    fn hydrate_transcript_from_session_messages(&mut self, messages: &[SessionMessage]) {
        self.transcript.clear();
        self.scroll_offset = 0;
        self.scrollback_archive_count = 0;

        for message in messages {
            match message.role.as_str() {
                "user" => self.transcript.append_user_message(&message.text),
                "assistant" => {
                    let text = if message.text.trim().is_empty() {
                        EMPTY_ASSISTANT_RESPONSE_MESSAGE
                    } else {
                        &message.text
                    };
                    self.transcript.append_assistant_message(text);
                }
                "tool" => {
                    let tool_name = message.tool_name.as_deref().unwrap_or("tool");
                    let is_error = message.is_error.unwrap_or(false);
                    let summary = tool_output_summary(
                        &message.text,
                        is_error,
                        message.tool_result_metadata.as_ref(),
                        message.outcome_kind,
                    );
                    self.transcript.append_tool_result_record(
                        message.entry_id.as_deref(),
                        tool_name,
                        &summary,
                        &message.text,
                        is_error,
                        message.outcome_kind,
                        message.tool_review.clone(),
                    );
                }
                "harness" => {
                    let kind = match message.outcome_kind {
                        Some(HarnessOutcomeKind::Failed) | None => {
                            classify_turn_failure_text(&message.text)
                        }
                        Some(kind) => kind,
                    };
                    self.transcript.append_harness_outcome(kind, &message.text);
                }
                _ => {}
            }
        }
        if !messages.is_empty() {
            self.scroll_to_bottom();
        }
    }

    fn transcript_ended_after_tool_result(&self) -> bool {
        self.transcript
            .entries()
            .last()
            .is_some_and(|entry| matches!(entry.kind, transcript::EntryKind::ToolResult { .. }))
    }

    fn handle_prompt_input_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::SUPER) || modifiers.contains(KeyModifiers::HYPER) {
            return;
        }

        self.prompt.input(textarea_input(key, modifiers));
    }

    /// Mouse-wheel scrolling over the transcript, three lines per notch.
    fn handle_mouse(&mut self, kind: crossterm::event::MouseEventKind) {
        const WHEEL_LINES: usize = 3;
        match kind {
            crossterm::event::MouseEventKind::ScrollUp => {
                self.scroll_transcript_lines_up(WHEEL_LINES);
            }
            crossterm::event::MouseEventKind::ScrollDown => {
                self.scroll_transcript_lines_down(WHEEL_LINES);
            }
            _ => {}
        }
    }

    fn handle_paste(&mut self, text: &str) {
        if self.transcript.has_unresolved_review()
            && matches!(self.mode, AppMode::Normal | AppMode::SlashComplete { .. })
        {
            self.status_message = String::from("review active; paste ignored");
            return;
        }
        match self.mode {
            AppMode::Normal | AppMode::SlashComplete { .. } => {
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                self.prompt.insert_str(&normalized);
                self.refresh_slash_completion(0);
            }
            AppMode::DialogInput => {
                let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
                if let Some(dialog) = self.active_dialog.as_mut() {
                    dialog.cursor_pos =
                        byte_boundary_at_or_before(&dialog.input_buffer, dialog.cursor_pos);
                    dialog
                        .input_buffer
                        .insert_str(dialog.cursor_pos, &normalized);
                    dialog.cursor_pos += normalized.len();
                }
            }
            AppMode::DialogSecretInput => {
                if let Some(dialog) = self.active_dialog.as_mut()
                    && let Some(secret) = dialog.secret_input.as_mut()
                {
                    secret.insert_str(text);
                }
            }
            _ => {}
        }
    }

    fn set_prompt_text(&mut self, text: &str) {
        self.prompt = textarea_from_text(text);
    }

    fn prompt_text(&self) -> String {
        self.prompt.lines().join("\n")
    }

    fn prompt_has_text(&self) -> bool {
        !self.prompt_text().trim().is_empty()
    }

    fn enter_slash_complete(&mut self) {
        let input = self.prompt_text();
        let prefix = if input.starts_with('/') {
            input
        } else {
            String::from("/")
        };
        let matches = match_slash_commands(&prefix);
        if !matches.is_empty() {
            self.mode = AppMode::SlashComplete {
                prefix,
                selected: 0,
            };
        }
    }

    fn open_model_selector(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before changing model");
        } else {
            if self.request_available_models() {
                self.status_message = String::from("loading available models");
            }
            self.mode = AppMode::ModelSelect {
                selected: 0,
                query: String::new(),
            };
        }
    }

    fn open_session_selector(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before changing session");
        } else {
            if self.send_client_event(ClientEvent::RecentSessionsRequested) {
                self.sessions.clear();
                self.session_labels.clear();
                self.session_is_path.clear();
                self.status_message = String::from("loading recent sessions");
            }
            self.mode = AppMode::SessionSelect { selected: 0 };
        }
    }

    fn apply_recent_sessions(&mut self, recent_sessions: Vec<RecentSession>) {
        let mut sessions = recent_sessions
            .into_iter()
            .map(|session| {
                let path = session.path.clone();
                let label = recent_session_label(&session);
                (path, label)
            })
            .collect::<Vec<_>>();
        let has_current_path = sessions.iter().any(|(path, _)| path == &self.session_id);
        if !has_current_path {
            sessions.insert(0, (self.session_id.clone(), self.session_id.clone()));
        }
        let count = sessions.len();
        self.sessions = sessions.iter().map(|(path, _)| path.clone()).collect();
        self.session_labels = sessions.iter().map(|(_, label)| label.clone()).collect();
        self.session_is_path = sessions
            .iter()
            .map(|(path, _)| has_current_path || path != &self.session_id)
            .collect();
        self.status_message = format!("recent sessions: {count}");
    }

    fn open_thinking_selector(&mut self) {
        if self.backend_busy() {
            self.status_message =
                String::from("wait for current response before changing thinking");
        } else {
            let selected = ThinkingLevel::ALL
                .iter()
                .position(|level| *level == self.thinking_level)
                .unwrap_or_default();
            self.mode = AppMode::ThinkingSelect { selected };
        }
    }

    fn open_approval_selector(&mut self) {
        let selected = ApprovalMode::ALL
            .iter()
            .position(|mode| *mode == self.approval_mode)
            .unwrap_or_default();
        self.mode = AppMode::ApprovalSelect { selected };
    }

    fn open_local_edit_composer(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before editing");
            return;
        }

        if self.has_local_edit_in_flight() {
            self.status_message = String::from("wait for current local edit");
            return;
        }

        if !self.supports(Capability::LocalEdit) {
            self.status_message = String::from("local edit unavailable");
            return;
        }

        self.mode = AppMode::LocalEditCompose {
            step: LocalEditComposeStep::Kind,
            draft: LocalEditDraft::default(),
        };
        self.status_message = String::from("choose edit kind");
    }

    fn submit_extension_lifecycle(&mut self, action: ExtensionLifecycleAction, selector: &str) {
        if self.backend_busy() {
            self.status_message =
                String::from("wait for current response before changing extensions");
            return;
        }

        if self.pending_extension_lifecycle_request_id.is_some() {
            self.status_message = String::from("wait for current extension lifecycle request");
            return;
        }

        if !self.supports(Capability::ExtensionLifecycle) {
            self.status_message = String::from("extension lifecycle unavailable");
            return;
        }

        let selector = selector.trim();
        if selector.is_empty() {
            self.status_message = String::from("extension selector required");
            return;
        }

        let request_id = format!(
            "extension-lifecycle-request-{}",
            self.extension_lifecycle_request_counter
        );
        self.extension_lifecycle_request_counter =
            self.extension_lifecycle_request_counter.saturating_add(1);

        if self.send_client_event(ClientEvent::ExtensionLifecycleRequested {
            request_id: request_id.clone(),
            action,
            selector: selector.to_string(),
        }) {
            self.pending_extension_lifecycle_request_id = Some(request_id);
            self.clear_input();
            self.status_message = format!("{} extension {selector}", lifecycle_action_verb(action));
        }
    }

    fn submit_extension_diagnostics(&mut self, selector: Option<&str>) {
        self.request_extension_diagnostics(selector, true);
    }

    fn request_extension_diagnostics(&mut self, selector: Option<&str>, clear_input: bool) {
        if self.pending_extension_diagnostic_request_id.is_some() {
            self.status_message = String::from("wait for current extension status request");
            return;
        }

        if !self.supports(Capability::ExtensionLifecycle) {
            self.status_message = String::from("extension lifecycle unavailable");
            return;
        }

        let selector = selector
            .map(str::trim)
            .filter(|selector| !selector.is_empty())
            .map(str::to_string);
        let request_id = format!(
            "extension-diagnostic-request-{}",
            self.extension_diagnostic_request_counter
        );
        self.extension_diagnostic_request_counter =
            self.extension_diagnostic_request_counter.saturating_add(1);

        if self.send_client_event(ClientEvent::ExtensionDiagnosticSnapshotRequested {
            request_id: request_id.clone(),
            selector: selector.clone(),
        }) {
            self.pending_extension_diagnostic_request_id = Some(request_id);
            if clear_input {
                self.clear_input();
                self.status_message = selector.map_or_else(
                    || String::from("loading extension status"),
                    |selector| format!("loading extension status {selector}"),
                );
            }
        }
    }

    fn handle_slash_complete_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::SlashComplete { selected, .. } = &self.mode else {
            return;
        };
        let mut selected = *selected;
        let prefix = self.prompt_text();
        let matches = match_slash_commands(&prefix);

        match (key, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = AppMode::Normal;
            }
            (KeyCode::Tab, _) => {
                if let Some(cmd) = matches.get(selected) {
                    self.set_prompt_text(cmd.name);
                }
                self.mode = AppMode::Normal;
            }
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                selected = selected.saturating_sub(1);
                self.refresh_slash_completion(selected);
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                selected = (selected + 1).min(matches.len().saturating_sub(1));
                self.refresh_slash_completion(selected);
            }
            (KeyCode::Enter, _)
                if matches!(parse_slash_command(&prefix), SlashParseResult::Command(_)) =>
            {
                self.mode = AppMode::Normal;
                self.submit_input();
            }
            (KeyCode::Enter, _) => {
                if let Some(cmd) = matches.get(selected) {
                    self.set_prompt_text(cmd.name);
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.handle_prompt_input_key(key, modifiers);
                self.refresh_slash_completion(selected);
            }
        }
    }

    fn handle_model_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match (key, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = AppMode::Normal;
            }
            (KeyCode::Up, _) => {
                if let AppMode::ModelSelect { selected, .. } = &mut self.mode {
                    *selected = selected.saturating_sub(1);
                }
            }
            (KeyCode::Down, _) => {
                let row_count = match &self.mode {
                    AppMode::ModelSelect { query, .. } => self.model_rows_for_query(query).len(),
                    _ => return,
                };
                if let AppMode::ModelSelect { selected, .. } = &mut self.mode {
                    *selected = (*selected + 1).min(row_count.saturating_sub(1));
                }
            }
            (KeyCode::Backspace, _) => {
                if let AppMode::ModelSelect { query, .. } = &mut self.mode {
                    query.pop();
                }
                self.clamp_model_select_selection();
            }
            (KeyCode::Char(ch), modifiers) if accepts_plain_text_modifier(modifiers) => {
                if let AppMode::ModelSelect { query, .. } = &mut self.mode {
                    query.push(ch);
                }
                self.clamp_model_select_selection();
            }
            (KeyCode::Enter, _) => {
                if self.backend_busy() {
                    self.status_message =
                        String::from("wait for current response before changing model");
                    self.mode = AppMode::Normal;
                    return;
                }

                let selected_model = {
                    let AppMode::ModelSelect { selected, query } = &self.mode else {
                        return;
                    };
                    let rows = self.model_rows_for_query(query);
                    rows.get((*selected).min(rows.len().saturating_sub(1)))
                        .map(|model| (*model).clone())
                };

                if let Some(model) = selected_model {
                    self.model_change_request_counter =
                        self.model_change_request_counter.wrapping_add(1).max(1);
                    let request_id = self.model_change_request_counter;
                    if self.send_client_event(ClientEvent::ModelSelectedDetailed {
                        provider: model.provider.clone(),
                        model_id: model.id.clone(),
                        connection_id: model.connection_id.clone(),
                        request_id,
                    }) {
                        self.status_message = format!("model requested: {}", model.label());
                        self.pending_thinking_handoff = Some(PendingThinkingHandoff {
                            request_id,
                            model,
                            activation_succeeded: false,
                        });
                    }
                    self.mode = AppMode::Normal;
                } else {
                    let query_is_empty = matches!(
                        &self.mode,
                        AppMode::ModelSelect { query, .. } if query.is_empty()
                    );
                    if query_is_empty {
                        self.status_message = String::from("available models not loaded yet");
                        self.mode = AppMode::Normal;
                    } else {
                        let status_message = match &self.mode {
                            AppMode::ModelSelect { query, .. } => {
                                format!("no models match: {query}")
                            }
                            _ => return,
                        };
                        self.status_message = status_message;
                        if let AppMode::ModelSelect { selected, .. } = &mut self.mode {
                            *selected = 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_session_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::SessionSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::SessionSelect { selected };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                selected = (selected + 1).min(self.sessions.len().saturating_sub(1));
                self.mode = AppMode::SessionSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if self.backend_busy() {
                    self.status_message =
                        String::from("wait for current response before changing session");
                } else if !self.session_is_path.get(selected).copied().unwrap_or(false) {
                    self.status_message = String::from("recent sessions not loaded yet");
                } else if let Some(session_path) = self.sessions.get(selected).cloned() {
                    // Reselecting the current session is a no-op: the
                    // transcript must not be mutated.
                    if self.session_file.as_deref() == Some(session_path.as_str()) {
                        self.status_message = String::from("already on this session");
                    } else if self.send_client_event(ClientEvent::SessionPathSelected {
                        session_path: session_path.clone(),
                    }) {
                        self.session_message_hydration = SessionMessageHydration::ExplicitResume;
                        self.status_message = format!("switching session: {session_path}");
                    }
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.mode = AppMode::Normal;
            }
        }
    }

    fn handle_fork_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::ForkSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::ForkSelect { selected };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                selected = (selected + 1).min(self.fork_messages.len().saturating_sub(1));
                self.mode = AppMode::ForkSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if self.backend_busy() {
                    self.status_message = String::from("wait for current response before forking");
                } else if let Some(message) = self.fork_messages.get(selected).cloned()
                    && self.send_client_event(ClientEvent::SessionForkRequested {
                        session_id: self.session_id.clone(),
                        entry_id: Some(message.entry_id.clone()),
                        position: ForkPosition::Before,
                    })
                {
                    self.status_message = format!("forking from: {}", message.entry_id);
                }
                self.mode = AppMode::Normal;
            }
            (KeyCode::Esc, _) => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_thinking_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::ThinkingSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::ThinkingSelect { selected };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                selected = (selected + 1).min(ThinkingLevel::ALL.len().saturating_sub(1));
                self.mode = AppMode::ThinkingSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if self.backend_busy() {
                    self.status_message =
                        String::from("wait for current response before changing thinking");
                } else if let Some(level) = ThinkingLevel::ALL.get(selected)
                    && self.send_client_event(ClientEvent::ThinkingLevelSelected { level: *level })
                {
                    self.thinking_level = *level;
                    self.status_message = format!("thinking: {}", level.as_str());
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.mode = AppMode::Normal;
            }
        }
    }

    fn handle_approval_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::ApprovalSelect { selected } = self.mode else {
            return;
        };
        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                self.mode = AppMode::ApprovalSelect {
                    selected: selected.saturating_sub(1),
                };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                self.mode = AppMode::ApprovalSelect {
                    selected: (selected + 1).min(ApprovalMode::ALL.len().saturating_sub(1)),
                };
            }
            (KeyCode::Enter, _) => {
                if let Some(mode) = ApprovalMode::ALL.get(selected).copied() {
                    if mode == ApprovalMode::FullAccess {
                        self.open_full_access_confirmation();
                    } else {
                        self.request_approval_mode(mode);
                        self.mode = AppMode::Normal;
                    }
                }
            }
            (KeyCode::Esc, _) => self.mode = AppMode::Normal,
            _ => {}
        }
    }

    fn open_full_access_confirmation(&mut self) {
        self.mode = AppMode::FullAccessConfirm {
            selected: FullAccessConfirmationAction::Cancel,
        };
        self.status_message = String::from("full-access confirmation required");
    }

    fn handle_full_access_confirm_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::FullAccessConfirm { selected } = self.mode else {
            return;
        };
        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                self.mode = AppMode::FullAccessConfirm {
                    selected: FullAccessConfirmationAction::Enable,
                };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                self.mode = AppMode::FullAccessConfirm {
                    selected: FullAccessConfirmationAction::Cancel,
                };
            }
            (KeyCode::Enter, _) if selected == FullAccessConfirmationAction::Enable => {
                self.request_approval_mode(ApprovalMode::FullAccess);
                self.mode = AppMode::Normal;
            }
            (KeyCode::Enter | KeyCode::Esc, _) => {
                self.status_message = String::from("full-access cancelled");
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn request_approval_mode(&mut self, mode: ApprovalMode) {
        self.approval_mode_request_counter = self.approval_mode_request_counter.wrapping_add(1);
        let request_id = self.approval_mode_request_counter;
        if self.send_client_event(ClientEvent::ApprovalModeSelected { request_id, mode }) {
            self.status_message = format!("changing approval mode to {}", mode.as_str());
        }
    }

    fn handle_dialog_confirm_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        let Some(dialog) = self.active_dialog.as_mut() else {
            self.mode = AppMode::Normal;
            return;
        };

        if let DialogKind::DeviceCode {
            verification_uri,
            user_code,
        } = &dialog.request.kind
        {
            let copy_status = match key {
                KeyCode::Char('c' | 'C') => Some(if copy_to_clipboard(user_code) {
                    "copied device code"
                } else {
                    "could not copy device code"
                }),
                KeyCode::Char('u' | 'U') => Some(if copy_to_clipboard(verification_uri) {
                    "copied login URL"
                } else {
                    "could not copy login URL"
                }),
                _ => None,
            };
            if let Some(message) = copy_status {
                self.status_message = String::from(message);
                return;
            }
        }

        let mut response = None;
        let mut cancelled = false;

        match key {
            KeyCode::Esc => cancelled = true,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab
                if !matches!(dialog.request.kind, DialogKind::DeviceCode { .. }) =>
            {
                dialog.confirm_accepted = !dialog.confirm_accepted;
            }
            KeyCode::Char('y' | 'Y')
                if !matches!(dialog.request.kind, DialogKind::DeviceCode { .. }) =>
            {
                dialog.confirm_accepted = true;
            }
            KeyCode::Char('n' | 'N')
                if !matches!(dialog.request.kind, DialogKind::DeviceCode { .. }) =>
            {
                dialog.confirm_accepted = false;
            }
            KeyCode::Enter if matches!(dialog.request.kind, DialogKind::DeviceCode { .. }) => {}
            KeyCode::Enter => {
                response = Some(DialogResponse::Confirmed {
                    accepted: dialog.confirm_accepted,
                });
            }
            _ => {}
        }

        if cancelled {
            self.cancel_dialog();
        } else if let Some(response) = response {
            self.submit_dialog_response(response);
        }
    }

    fn handle_dialog_input_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let Some(dialog) = self.active_dialog.as_mut() else {
            self.mode = AppMode::Normal;
            return;
        };

        let is_editor = matches!(&dialog.request.kind, DialogKind::Editor { .. });
        let mut response = None;
        let mut cancelled = false;
        match (key, modifiers) {
            (KeyCode::Esc, _) => cancelled = true,
            (KeyCode::Char('j'), modifiers)
                if is_editor && modifiers.contains(KeyModifiers::CONTROL) =>
            {
                insert_dialog_newline(dialog);
            }
            (KeyCode::Enter, _) => {
                response = Some(DialogResponse::Text {
                    value: dialog.input_buffer.clone(),
                });
            }
            (KeyCode::Backspace, _) => {
                dialog.cursor_pos =
                    byte_boundary_at_or_before(&dialog.input_buffer, dialog.cursor_pos);
                if dialog.cursor_pos > 0 {
                    let previous = prev_char_boundary(&dialog.input_buffer, dialog.cursor_pos);
                    dialog.input_buffer.drain(previous..dialog.cursor_pos);
                    dialog.cursor_pos = previous;
                }
            }
            (KeyCode::Delete, _) => {
                dialog.cursor_pos =
                    byte_boundary_at_or_before(&dialog.input_buffer, dialog.cursor_pos);
                if dialog.cursor_pos < dialog.input_buffer.len() {
                    let next = next_char_boundary(&dialog.input_buffer, dialog.cursor_pos);
                    dialog.input_buffer.drain(dialog.cursor_pos..next);
                }
            }
            (KeyCode::Left, KeyModifiers::CONTROL) => {
                dialog.cursor_pos = prev_word_boundary(&dialog.input_buffer, dialog.cursor_pos);
            }
            (KeyCode::Right, KeyModifiers::CONTROL) => {
                dialog.cursor_pos = next_word_boundary(&dialog.input_buffer, dialog.cursor_pos);
            }
            (KeyCode::Left, _) => {
                dialog.cursor_pos = prev_char_boundary(&dialog.input_buffer, dialog.cursor_pos);
            }
            (KeyCode::Right, _) => {
                dialog.cursor_pos = next_char_boundary(&dialog.input_buffer, dialog.cursor_pos);
            }
            (KeyCode::Home, _) => {
                dialog.cursor_pos = 0;
            }
            (KeyCode::End, _) => {
                dialog.cursor_pos = dialog.input_buffer.len();
            }
            (KeyCode::Char(c), _) => {
                dialog.cursor_pos =
                    byte_boundary_at_or_before(&dialog.input_buffer, dialog.cursor_pos);
                dialog.input_buffer.insert(dialog.cursor_pos, c);
                dialog.cursor_pos += c.len_utf8();
            }
            _ => {}
        }

        if cancelled {
            self.cancel_dialog();
        } else if let Some(response) = response {
            self.submit_dialog_response(response);
        }
    }
    fn handle_secret_dialog_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let Some(dialog) = self.active_dialog.as_mut() else {
            self.mode = AppMode::Normal;
            return;
        };

        let mut cancelled = false;
        let response = match (key, modifiers) {
            (KeyCode::Esc, _) => {
                cancelled = true;
                None
            }
            (KeyCode::Enter, _) => {
                dialog
                    .secret_input
                    .take()
                    .map(|secret| DialogResponse::Secret {
                        value: yach_proto::SubmittedSecret::new(secret.into_value()),
                    })
            }
            (KeyCode::Backspace, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.backspace();
                }
                None
            }
            (KeyCode::Delete, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.delete();
                }
                None
            }
            (KeyCode::Left, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.move_left();
                }
                None
            }
            (KeyCode::Right, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.move_right();
                }
                None
            }
            (KeyCode::Home, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.move_home();
                }
                None
            }
            (KeyCode::End, _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.move_end();
                }
                None
            }
            (KeyCode::Char(value), _) => {
                if let Some(secret) = dialog.secret_input.as_mut() {
                    secret.insert(value);
                }
                None
            }
            _ => None,
        };

        if cancelled {
            self.cancel_dialog();
        } else if let Some(response) = response {
            self.submit_dialog_response(response);
        }
    }

    fn handle_dialog_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let Some(dialog) = self.active_dialog.as_mut() else {
            self.mode = AppMode::Normal;
            return;
        };
        let DialogKind::Select { options } = &dialog.request.kind else {
            self.mode = AppMode::Normal;
            return;
        };

        let mut response = None;
        let mut cancelled = false;

        match key {
            KeyCode::Esc => cancelled = true,
            key if is_selection_up_key(key, modifiers) => {
                dialog.selected = dialog.selected.saturating_sub(1);
            }
            key if is_selection_down_key(key, modifiers) => {
                dialog.selected = (dialog.selected + 1).min(options.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                if let Some(option) = options.get(dialog.selected) {
                    response = Some(DialogResponse::Selection {
                        value: option.value.clone(),
                    });
                } else {
                    cancelled = true;
                }
            }
            _ => {}
        }

        if cancelled {
            self.cancel_dialog();
        } else if let Some(response) = response {
            self.submit_dialog_response(response);
        }
    }

    fn handle_help_overlay_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | 'h' | '?') => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_perf_overlay_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Char('p') => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_local_edit_compose_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::LocalEditCompose { step, mut draft } = self.mode.clone() else {
            return;
        };

        match (key, modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = AppMode::Normal;
                self.status_message = String::from("local edit cancelled");
            }
            (KeyCode::Char('j'), modifiers)
                if modifiers == KeyModifiers::CONTROL
                    && local_edit_compose_accepts_multiline(step) =>
            {
                draft.buffer.push('\n');
                self.mode = AppMode::LocalEditCompose { step, draft };
            }
            (KeyCode::Enter, modifiers)
                if modifiers == KeyModifiers::SHIFT
                    && local_edit_compose_accepts_multiline(step) =>
            {
                draft.buffer.push('\n');
                self.mode = AppMode::LocalEditCompose { step, draft };
            }
            (KeyCode::Enter, modifiers) if modifiers.is_empty() => {
                self.advance_local_edit_compose();
            }
            (KeyCode::Backspace, modifiers) if modifiers.is_empty() => {
                draft.buffer.pop();
                self.mode = AppMode::LocalEditCompose { step, draft };
            }
            (KeyCode::Char('1'), modifiers)
                if modifiers.is_empty() && step == LocalEditComposeStep::Kind =>
            {
                draft.kind = Some(LocalEditOperationKind::Modify);
                draft.buffer.clear();
                self.mode = AppMode::LocalEditCompose {
                    step: LocalEditComposeStep::Path,
                    draft,
                };
                self.status_message = String::from("enter path");
            }
            (KeyCode::Char('2'), modifiers)
                if modifiers.is_empty() && step == LocalEditComposeStep::Kind =>
            {
                draft.kind = Some(LocalEditOperationKind::Create);
                draft.buffer.clear();
                self.mode = AppMode::LocalEditCompose {
                    step: LocalEditComposeStep::Path,
                    draft,
                };
                self.status_message = String::from("enter path");
            }
            (KeyCode::Char(ch), modifiers) if accepts_plain_text_modifier(modifiers) => {
                draft.buffer.push(ch);
                self.mode = AppMode::LocalEditCompose { step, draft };
            }
            _ => {}
        }
    }

    fn advance_local_edit_compose(&mut self) {
        let AppMode::LocalEditCompose { step, mut draft } = self.mode.clone() else {
            return;
        };

        match step {
            LocalEditComposeStep::Kind => {
                self.status_message = String::from("choose 1 modify or 2 create");
                self.mode = AppMode::LocalEditCompose { step, draft };
            }
            LocalEditComposeStep::Path => {
                let path = draft.buffer.trim().to_string();
                if path.is_empty() {
                    self.status_message = String::from("path required");
                    self.mode = AppMode::LocalEditCompose { step, draft };
                    return;
                }
                draft.path = Some(path);
                draft.buffer.clear();
                let next_step = match draft.kind {
                    Some(LocalEditOperationKind::Modify) => LocalEditComposeStep::ExpectedSha256,
                    Some(LocalEditOperationKind::Create) => LocalEditComposeStep::Content,
                    None => LocalEditComposeStep::Kind,
                };
                self.status_message = match next_step {
                    LocalEditComposeStep::ExpectedSha256 => String::from("enter expected sha256"),
                    LocalEditComposeStep::Content => String::from("enter file content"),
                    _ => String::from("choose edit kind"),
                };
                self.mode = AppMode::LocalEditCompose {
                    step: next_step,
                    draft,
                };
            }
            LocalEditComposeStep::ExpectedSha256 => {
                draft.expected_sha256 = Some(draft.buffer.trim().to_string());
                draft.buffer.clear();
                self.status_message = String::from("enter text to find");
                self.mode = AppMode::LocalEditCompose {
                    step: LocalEditComposeStep::Find,
                    draft,
                };
            }
            LocalEditComposeStep::Find => {
                draft.find = Some(draft.buffer.clone());
                draft.buffer.clear();
                self.status_message = String::from("enter replacement text");
                self.mode = AppMode::LocalEditCompose {
                    step: LocalEditComposeStep::Replace,
                    draft,
                };
            }
            LocalEditComposeStep::Replace => {
                draft.replace = Some(draft.buffer.clone());
                self.submit_local_edit_prepare(draft);
            }
            LocalEditComposeStep::Content => {
                draft.content = Some(draft.buffer.clone());
                self.submit_local_edit_prepare(draft);
            }
        }
    }

    fn submit_local_edit_prepare(&mut self, draft: LocalEditDraft) {
        let Some(path) = draft.path else {
            self.status_message = String::from("enter path");
            return;
        };
        let Some(kind) = draft.kind else {
            self.status_message = String::from("choose edit kind");
            return;
        };

        let operation = match kind {
            LocalEditOperationKind::Modify => LocalEditOperationInput::ModifyTextFile {
                path,
                expected_sha256: draft.expected_sha256.unwrap_or_default(),
                find: draft.find.unwrap_or_default(),
                replace: draft.replace.unwrap_or_default(),
            },
            LocalEditOperationKind::Create => LocalEditOperationInput::CreateTextFile {
                path,
                content: draft.content.unwrap_or_default(),
            },
        };
        let request_id = format!("local-edit-request-{}", self.local_edit_request_counter);
        self.local_edit_request_counter = self.local_edit_request_counter.saturating_add(1);

        if self.send_client_event(ClientEvent::LocalEditPrepareRequested {
            request_id: request_id.clone(),
            operation,
        }) {
            self.pending_local_edit_request_id = Some(request_id);
            self.active_local_edit_preview_id = None;
            self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
            self.mode = AppMode::Normal;
            self.status_message = String::from("preparing local edit");
        }
    }

    fn handle_local_edit_review_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::LocalEditReview { preview, selected } = self.mode.clone() else {
            return;
        };

        if matches!(
            self.local_edit_decision_submission,
            LocalEditDecisionSubmission::Submitted
        ) {
            if matches!(
                (key, modifiers),
                (KeyCode::Char('c'), KeyModifiers::CONTROL)
            ) {
                self.handle_normal_key(key, modifiers);
                return;
            }
            self.status_message = String::from("local edit decision already submitted");
            return;
        }

        match (key, modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('r'), KeyModifiers::NONE) => {
                self.submit_local_edit_review(LocalEditDecision::Reject);
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let decision = match selected {
                    LocalEditReviewAction::Apply => LocalEditDecision::Apply,
                    LocalEditReviewAction::Reject => LocalEditDecision::Reject,
                };
                self.submit_local_edit_review(decision);
            }
            (KeyCode::Char('a'), KeyModifiers::NONE) => {
                self.submit_local_edit_review(LocalEditDecision::Apply);
            }
            (KeyCode::Up | KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.mode = AppMode::LocalEditReview {
                    preview,
                    selected: LocalEditReviewAction::Apply,
                };
            }
            (KeyCode::Down | KeyCode::Char('j'), KeyModifiers::NONE) => {
                self.mode = AppMode::LocalEditReview {
                    preview,
                    selected: LocalEditReviewAction::Reject,
                };
            }
            _ => {}
        }
    }

    fn submit_local_edit_review(&mut self, decision: LocalEditDecision) {
        let AppMode::LocalEditReview { preview, .. } = self.mode.clone() else {
            return;
        };

        if self.send_client_event(ClientEvent::LocalEditDecisionSubmitted {
            preview_id: preview.preview_id,
            permission_decision_id: preview.permission_decision_id,
            decision,
        }) {
            self.pending_local_edit_request_id = None;
            self.local_edit_decision_submission = LocalEditDecisionSubmission::Submitted;
            self.status_message = String::from("submitting local edit decision");
        }
    }

    fn show_session_status(&mut self) {
        self.clear_input();
        let connection = match self.model_connection_id.as_deref() {
            Some(connection_id) => connection_id,
            None if self.is_connected => "default",
            None => "disconnected",
        };
        let mut lines = vec![
            String::from("Session status"),
            format!("session: {}", self.session_id),
            format!("model: {}", self.model),
            format!("thinking: {}", self.thinking_level.as_str()),
            format!("connection: {connection}"),
            format!("approval: {}", self.approval_mode.as_str()),
        ];
        if let Some(stats) = &self.session_stats {
            if let Some(percent) = stats.context_used_percent {
                lines.push(format!(
                    "context: {}",
                    crate::status_bar::format_context_meter(percent, stats.context_window)
                ));
            } else if let Some(context_window) = stats.context_window {
                lines.push(format!(
                    "context window: {}",
                    crate::status_bar::format_token_capacity(context_window)
                ));
            }
            if let Some(message_count) = stats.message_count {
                lines.push(format!(
                    "messages: {message_count} (user {}, assistant {}, tool {})",
                    stats.user_message_count.unwrap_or_default(),
                    stats.assistant_message_count.unwrap_or_default(),
                    stats.tool_message_count.unwrap_or_default(),
                ));
            }
        }
        lines.push(format!(
            "compactions: {}",
            self.transcript.compaction_count()
        ));
        self.transcript.append_status(&lines.join("\n"));
        self.scroll_to_bottom();
    }

    fn submit_input(&mut self) {
        let input = self.prompt_text();

        match parse_slash_command(&input) {
            SlashParseResult::Command(SlashAction::Quit) => {
                self.clear_input();
                self.should_quit = true;
                return;
            }
            SlashParseResult::Command(SlashAction::Clear) => {
                self.clear_input();
                self.transcript.clear();
                self.scroll_offset = 0;
                self.scrollback_archive_count = 0;
                return;
            }
            SlashParseResult::Command(SlashAction::Model) => {
                self.clear_input();
                self.open_model_selector();
                return;
            }
            SlashParseResult::Command(SlashAction::Connect) => {
                self.request_connections();
                return;
            }
            SlashParseResult::Command(SlashAction::Session | SlashAction::Resume) => {
                self.clear_input();
                self.open_session_selector();
                return;
            }
            SlashParseResult::Command(SlashAction::Status) => {
                self.show_session_status();
                return;
            }
            SlashParseResult::Command(SlashAction::Thinking) => {
                self.clear_input();
                self.open_thinking_selector();
                return;
            }
            SlashParseResult::Command(SlashAction::Approval) => {
                self.clear_input();
                self.open_approval_selector();
                return;
            }
            SlashParseResult::CommandWithArgs {
                action: SlashAction::Approval,
                args,
            } => {
                self.clear_input();
                match args.as_str() {
                    "review" => self.request_approval_mode(ApprovalMode::Review),
                    "accept-edits" => self.request_approval_mode(ApprovalMode::AcceptEdits),
                    "full-access" => self.open_full_access_confirmation(),
                    _ => {
                        self.status_message = String::from(
                            "approval mode must be review, accept-edits, or full-access",
                        );
                    }
                }
                return;
            }
            SlashParseResult::Command(SlashAction::Compact) => {
                self.clear_input();
                self.request_compaction(None);
                return;
            }
            SlashParseResult::CommandWithArgs {
                action: SlashAction::Compact,
                args,
            } => {
                self.clear_input();
                self.request_compaction(Some(args));
                return;
            }
            SlashParseResult::Command(SlashAction::Perf) => {
                self.clear_input();
                self.mode = AppMode::PerfOverlay;
                return;
            }
            SlashParseResult::Command(SlashAction::Edit) => {
                self.clear_input();
                self.open_local_edit_composer();
                return;
            }
            SlashParseResult::Command(
                SlashAction::ExtensionStop | SlashAction::ExtensionReload,
            ) => {
                self.clear_input();
                self.status_message = String::from("extension selector required");
                return;
            }
            SlashParseResult::Command(SlashAction::ExtensionStatus) => {
                self.submit_extension_diagnostics(None);
                return;
            }
            SlashParseResult::Command(SlashAction::Fork) => {
                self.clear_input();
                self.fork_current_session();
                return;
            }
            SlashParseResult::Command(SlashAction::Help) => {
                self.clear_input();
                self.mode = if matches!(self.mode, AppMode::HelpOverlay) {
                    AppMode::Normal
                } else {
                    AppMode::HelpOverlay
                };
                return;
            }
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionStop,
                args,
            } => {
                self.submit_extension_lifecycle(ExtensionLifecycleAction::Stop, &args);
                return;
            }
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionReload,
                args,
            } => {
                self.submit_extension_lifecycle(ExtensionLifecycleAction::Reload, &args);
                return;
            }
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionStatus,
                args,
            } => {
                self.submit_extension_diagnostics(Some(&args));
                return;
            }
            SlashParseResult::CommandWithArgs { .. } | SlashParseResult::ArgumentsUnsupported => {
                self.status_message = String::from("slash command arguments are not supported yet");
                return;
            }
            SlashParseResult::Unknown | SlashParseResult::NotSlash => {}
        }

        let session_id = self.session_id.clone();
        if self.send_client_event(ClientEvent::PromptSubmitted {
            session_id,
            prompt: input.clone(),
        }) {
            self.clear_input();
            // Inline rendering leaves completed turns in terminal-native
            // scrollback when the next real turn begins. The new user row
            // stays live in the ratatui viewport for streaming updates.
            self.scrollback_archive_count = self.transcript.entries().len();
            self.transcript.append_user_message(&input);
            self.scroll_to_bottom();
            self.status_message = String::from("sending...");
            self.set_stream_state(StreamState::Streaming {
                session_id: self.session_id.clone(),
            });
        }
    }

    fn take_scrollback_lines(&mut self, width: u16) -> Vec<ratatui::text::Line<'static>> {
        let count = std::mem::take(&mut self.scrollback_archive_count);
        let lines = self
            .transcript
            .drain_prefix_lines(count, width, &self.theme);
        if !lines.is_empty() {
            self.scroll_to_bottom();
        }
        lines
    }

    fn request_session_tree(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before loading branches");
        } else if self.send_client_event(ClientEvent::SessionMessagesRequested) {
            self.status_message = String::from("loading session tree");
        }
    }

    fn request_compaction(&mut self, instructions: Option<String>) {
        let session_id = self.session_id.clone();
        if self.send_client_event(ClientEvent::CompactionRequested {
            session_id,
            instructions,
        }) {
            self.status_message = String::from("compaction requested");
        }
    }

    fn fork_current_session(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before forking");
            return;
        }

        if !self.supports(Capability::SessionForking) {
            self.status_message = String::from("session forking unavailable");
            return;
        }

        if !self.has_forkable_history() {
            self.status_message = String::from("send a message before cloning the session");
            return;
        }

        if self.send_client_event(ClientEvent::ForkMessagesRequested) {
            self.status_message = String::from("loading fork points");
        }
    }

    #[cfg(test)]
    fn fork_session(&mut self, session_id: &str) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before forking");
            return;
        }

        if !self.supports(Capability::SessionForking) {
            self.status_message = String::from("session forking unavailable");
            return;
        }

        if !self.has_forkable_history() {
            self.status_message = String::from("send a message before cloning the session");
            return;
        }

        if self.send_client_event(ClientEvent::SessionForkRequested {
            session_id: session_id.to_string(),
            entry_id: None,
            position: ForkPosition::Before,
        }) {
            self.status_message = format!("cloning current branch from: {session_id}");
        }
    }

    fn has_forkable_history(&self) -> bool {
        self.transcript
            .entries()
            .iter()
            .any(|entry| matches!(entry.kind, transcript::EntryKind::UserMessage))
    }

    #[cfg(test)]
    fn model_select_index(&self) -> usize {
        if let AppMode::ModelSelect { selected, .. } = &self.mode {
            *selected
        } else {
            0
        }
    }

    /// Visible picker rows: an empty query shows the curated snapshot; a
    /// non-empty query searches the complete discovered snapshot.
    fn model_rows_for_query(&self, query: &str) -> Vec<&ModelInfo> {
        if query.is_empty() {
            return self.available_models.iter().collect();
        }
        let needle = query.to_lowercase();
        self.discovered_models
            .iter()
            .filter(|model| model_matches_query(model, &needle))
            .collect()
    }

    fn model_select_view(&self) -> (Vec<&ModelInfo>, usize, &str) {
        let AppMode::ModelSelect { selected, query } = &self.mode else {
            return (Vec::new(), 0, "");
        };
        let rows = self.model_rows_for_query(query);
        let selected = (*selected).min(rows.len().saturating_sub(1));
        (rows, selected, query)
    }

    /// Background snapshot updates must not close the picker or erase the
    /// query; only clamp the selection against the newly visible rows.
    fn clamp_model_select_selection(&mut self) {
        let row_count = match &self.mode {
            AppMode::ModelSelect { query, .. } => self.model_rows_for_query(query).len(),
            _ => return,
        };
        if let AppMode::ModelSelect { selected, .. } = &mut self.mode {
            *selected = (*selected).min(row_count.saturating_sub(1));
        }
    }

    fn session_select_index(&self) -> usize {
        if let AppMode::SessionSelect { selected } = &self.mode {
            *selected
        } else {
            0
        }
    }

    fn fork_select_index(&self) -> usize {
        if let AppMode::ForkSelect { selected } = &self.mode {
            *selected
        } else {
            0
        }
    }

    fn prompt_is_approval_command(&self) -> bool {
        matches!(
            parse_slash_command(&self.prompt_text()),
            SlashParseResult::Command(SlashAction::Approval)
                | SlashParseResult::CommandWithArgs {
                    action: SlashAction::Approval,
                    ..
                }
        )
    }

    fn approval_select_index(&self) -> usize {
        if let AppMode::ApprovalSelect { selected } = self.mode {
            selected
        } else {
            0
        }
    }

    fn thinking_select_index(&self) -> usize {
        if let AppMode::ThinkingSelect { selected } = &self.mode {
            *selected
        } else {
            0
        }
    }

    fn slash_completion(&self) -> Option<(String, usize, Vec<&SlashCommand>)> {
        if let AppMode::SlashComplete { prefix, selected } = &self.mode {
            Some((prefix.clone(), *selected, match_slash_commands(prefix)))
        } else {
            None
        }
    }
}

fn dialog_summary(request: &DialogRequest) -> String {
    request
        .title
        .clone()
        .or_else(|| request.prompt.clone())
        .unwrap_or_else(|| String::from("dialog pending"))
}

fn is_provider_connection_dialog(request: &DialogRequest) -> bool {
    request
        .id
        .as_deref()
        .is_some_and(|id| id.starts_with("provider-connection:"))
}

fn copy_to_clipboard(text: &str) -> bool {
    use std::io::{self, Write};

    let encoded = encode_osc52(text.as_bytes());
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes()).is_ok() && stdout.flush().is_ok()
}

fn encode_osc52(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let (chunks, remainder) = bytes.as_chunks::<3>();
    for chunk in chunks {
        let n = u32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]);
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        encoded.push(TABLE[(n & 0x3f) as usize] as char);
    }
    if remainder.len() == 1 {
        let n = u32::from(remainder[0]) << 16;
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        encoded.push('=');
        encoded.push('=');
    } else if remainder.len() == 2 {
        let n = (u32::from(remainder[0]) << 16) | (u32::from(remainder[1]) << 8);
        encoded.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        encoded.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        encoded.push('=');
    }
    encoded
}

fn recent_session_label(session: &RecentSession) -> String {
    if let Some(name) = session.name.as_ref().filter(|name| !name.is_empty()) {
        return format_session_label(name, session.message_count);
    }

    if let Some(first_message) = session
        .first_message
        .as_ref()
        .filter(|first_message| !first_message.is_empty())
    {
        return format_session_label(&truncate_label(first_message), session.message_count);
    }

    let fallback = session
        .id
        .as_ref()
        .filter(|id| !id.is_empty())
        .map_or_else(|| short_path(&session.path), std::clone::Clone::clone);
    format_session_label(&fallback, session.message_count)
}

fn format_session_label(title: &str, message_count: Option<u64>) -> String {
    message_count.map_or_else(
        || title.to_owned(),
        |count| format!("{title} ({count} messages)"),
    )
}

fn truncate_label(label: &str) -> String {
    let trimmed = label.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = trimmed.chars().take(72).collect();
    if trimmed.chars().count() > 72 {
        format!("{preview}...")
    } else {
        preview
    }
}

fn short_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .map_or_else(|| path.to_owned(), ToOwned::to_owned)
}

fn prev_char_boundary(s: &str, pos: usize) -> usize {
    let pos = byte_boundary_at_or_before(s, pos.min(s.len()));
    if pos == 0 {
        return 0;
    }
    s[..pos].char_indices().last().map_or(0, |(idx, _)| idx)
}

fn next_char_boundary(s: &str, pos: usize) -> usize {
    let pos = byte_boundary_at_or_before(s, pos.min(s.len()));
    if pos >= s.len() {
        return s.len();
    }
    let Some(ch) = s[pos..].chars().next() else {
        return s.len();
    };
    pos + ch.len_utf8()
}

fn byte_boundary_at_or_before(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut boundary = 0;
    for (idx, ch) in s.char_indices() {
        let next = idx + ch.len_utf8();
        if next > pos {
            return boundary;
        }
        boundary = next;
    }
    boundary
}

fn prev_word_boundary(s: &str, pos: usize) -> usize {
    let pos = byte_boundary_at_or_before(s, pos.min(s.len()));
    let before = &s[..pos];
    let mut chars = before.char_indices().rev().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch.is_alphanumeric() {
            while let Some((_next_idx, next_ch)) = chars.peek() {
                if !next_ch.is_alphanumeric() {
                    return idx + next_ch.len_utf8();
                }
                chars.next();
            }
            return idx;
        }
    }
    0
}

fn next_word_boundary(s: &str, pos: usize) -> usize {
    let pos = byte_boundary_at_or_before(s, pos.min(s.len()));
    let after = &s[pos..];
    let mut chars = after.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch.is_alphanumeric() {
            while let Some((_next_idx, next_ch)) = chars.peek() {
                if !next_ch.is_alphanumeric() {
                    return pos + idx + next_ch.len_utf8();
                }
                chars.next();
            }
            return s.len();
        }
    }
    s.len()
}

struct TerminalRestoreGuard {
    flags: u8,
}

impl TerminalRestoreGuard {
    const RAW_MODE: u8 = 1;
    const CURSOR_HIDDEN: u8 = 1 << 1;
    const BRACKETED_PASTE: u8 = 1 << 2;
    const FOCUS_CHANGE: u8 = 1 << 3;
    const RESTORED: u8 = 1 << 4;

    fn new() -> Self {
        Self { flags: 0 }
    }

    fn mark_raw_mode(&mut self) {
        self.flags |= Self::RAW_MODE;
    }

    fn mark_cursor_hidden(&mut self) {
        self.flags |= Self::CURSOR_HIDDEN;
    }

    fn mark_bracketed_paste(&mut self) {
        self.flags |= Self::BRACKETED_PASTE;
    }

    fn mark_focus_change(&mut self) {
        self.flags |= Self::FOCUS_CHANGE;
    }

    fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
    fn restore(&mut self) -> io::Result<()> {
        use crossterm::ExecutableCommand;
        use crossterm::cursor::Show;
        use crossterm::event::{DisableBracketedPaste, DisableFocusChange};
        use crossterm::terminal::disable_raw_mode;

        if self.has_flag(Self::RESTORED) {
            return Ok(());
        }
        self.flags |= Self::RESTORED;

        let mut first_error = None;
        if self.has_flag(Self::FOCUS_CHANGE)
            && let Err(error) = io::stdout().execute(DisableFocusChange)
        {
            first_error = Some(error);
        }
        if self.has_flag(Self::BRACKETED_PASTE)
            && let Err(error) = io::stdout().execute(DisableBracketedPaste)
        {
            first_error = Some(error);
        }
        if self.has_flag(Self::CURSOR_HIDDEN)
            && let Err(error) = io::stdout().execute(Show)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        if self.has_flag(Self::RAW_MODE)
            && let Err(error) = disable_raw_mode()
            && first_error.is_none()
        {
            first_error = Some(error);
        }

        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

impl Drop for TerminalRestoreGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// Narrow, unstable headless seam for benchmarks and tests.
///
/// This intentionally does not define a stable public UI API. It reuses the
/// production app event handlers and layout renderer so benchmark fixtures can
/// measure app/event/render costs without a real terminal.
pub struct BenchmarkApp {
    app: App,
    _client_rx: mpsc::UnboundedReceiver<ClientEvent>,
}

impl BenchmarkApp {
    #[must_use]
    pub fn new() -> Self {
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        Self {
            app: App::new(client_tx),
            _client_rx: client_rx,
        }
    }

    pub fn handle_backend_event(&mut self, event: BackendEvent) {
        self.app.handle_backend_event(event);
    }

    pub fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        self.app.handle_key(key, modifiers);
    }

    pub fn set_prompt_text(&mut self, text: &str) {
        self.app.set_prompt_text(text);
    }

    #[must_use]
    pub fn prompt_text(&self) -> String {
        self.app.prompt_text()
    }

    pub fn set_transcript(&mut self, transcript: Transcript) {
        self.app.transcript = transcript;
        self.app.scroll_to_bottom();
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.app.scroll_offset = self.app.scroll_offset.saturating_add(lines);
    }

    pub fn render_headless(&mut self, width: u16, height: u16) {
        let backend = TestBackend::new(width, height);
        let terminal_result = Terminal::new(backend);
        let Ok(mut terminal) = terminal_result;
        let _ = self.render_to_terminal(&mut terminal);
    }

    pub fn render_live_terminal(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> io::Result<()> {
        self.render_to_terminal(terminal)
    }

    fn render_to_terminal<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> io::Result<()>
    where
        B::Error: std::fmt::Debug,
    {
        let area = terminal
            .size()
            .map_err(|error| io::Error::other(format!("terminal size failed: {error:?}")))?;
        let (viewport_width, viewport_height) =
            layout::transcript_viewport_size(area.into(), &self.app.prompt);
        self.app
            .set_transcript_viewport(viewport_width, viewport_height);

        let render_params = layout::RenderParams {
            transcript: &self.app.transcript,
            transcript_cache: &mut self.app.transcript_cache,
            scroll_offset: self.app.scroll_offset,
            is_streaming: self.app.is_streaming,
            input: &mut self.app.prompt,
            model: &self.app.model,
            thinking_level: self.app.thinking_level.as_str(),
            approval_mode: self.app.approval_mode.as_str(),
            status_message: &self.app.status_message,
            is_connected: self.app.is_connected,
            compaction_count: self.app.transcript.compaction_count(),
            context_used_percent: self.app.context_used_percent,
            context_window: self
                .app
                .session_stats
                .as_ref()
                .and_then(|stats| stats.context_window),
            terminal_focused: self.app.terminal_focused,
            theme: &self.app.theme,
        };

        terminal
            .draw(|frame| {
                let mut render_params = render_params;
                layout::render(frame, &mut render_params);
            })
            .map_err(|error| io::Error::other(format!("terminal draw failed: {error:?}")))?;
        Ok(())
    }
}

impl Default for BenchmarkApp {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn run_tui(
    client_tx: mpsc::UnboundedSender<ClientEvent>,
    rx: mpsc::UnboundedReceiver<BackendEvent>,
) -> io::Result<()> {
    run_tui_with_startup_trace(client_tx, rx, None).await
}

pub async fn run_tui_with_startup_trace(
    client_tx: mpsc::UnboundedSender<ClientEvent>,
    rx: mpsc::UnboundedReceiver<BackendEvent>,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    run_tui_with_startup_trace_and_options(client_tx, rx, startup_trace, RunTuiOptions::default())
        .await
}

pub async fn run_tui_with_startup_trace_and_options(
    client_tx: mpsc::UnboundedSender<ClientEvent>,
    mut rx: mpsc::UnboundedReceiver<BackendEvent>,
    startup_trace: Option<StartupTrace>,
    options: RunTuiOptions,
) -> io::Result<()> {
    use crossterm::ExecutableCommand;
    use crossterm::cursor::Hide;
    use crossterm::event::{EnableBracketedPaste, EnableFocusChange};
    use crossterm::terminal::{enable_raw_mode, size};
    use ratatui::backend::CrosstermBackend;
    use ratatui::{Terminal, TerminalOptions, Viewport};
    use tokio_stream::StreamExt;

    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("run_tui_start");
    }
    let mut app = App::new_with_theme(client_tx, options.theme);
    if options.resume_session {
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;
    }
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_app_created");
    }
    let mut backend_open = true;

    let mut terminal_guard = TerminalRestoreGuard::new();
    enable_raw_mode()?;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_raw_mode_enabled");
    }
    terminal_guard.mark_raw_mode();
    io::stdout().execute(Hide)?;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_cursor_hidden");
    }
    terminal_guard.mark_cursor_hidden();
    io::stdout().execute(EnableBracketedPaste)?;
    terminal_guard.mark_bracketed_paste();
    io::stdout().execute(EnableFocusChange)?;
    terminal_guard.mark_focus_change();

    let backend = CrosstermBackend::new(io::stdout());
    let viewport_height = size()?.1.max(1);
    let mut terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(viewport_height),
        },
    )?;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_terminal_created");
    }

    let mut crossterm_stream = crossterm::event::EventStream::new();
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_event_stream_created");
    }
    let mut first_event_recorded = false;
    let mut first_render_recorded = false;

    loop {
        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_event = rx.recv(), if backend_open => {
                if let Some(event) = maybe_event {
                    if !first_event_recorded {
                        if let Some(trace) = startup_trace.as_ref() {
                            trace.mark("tui_first_backend_event_received");
                        }
                        first_event_recorded = true;
                    }
                    app.handle_backend_event(event);
                } else {
                    backend_open = false;
                    app.handle_backend_event(BackendEvent::Disconnected {
                        reason: String::from("backend disconnected"),
                    });
                }
            }
            Some(event) = crossterm_stream.next() => {
                if let Ok(event) = event {
                    app.handle_terminal_focus_event(&event);
                    match event {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            app.handle_key(key.code, key.modifiers);
                        }
                        Event::Paste(text) => {
                            app.handle_paste(&text);
                        }
                        Event::Mouse(mouse) => {
                            app.handle_mouse(mouse.kind);
                        }
                        _ => {}
                    }
                }
            }
            else => break,
        }
        if let Ok(area) = terminal.size() {
            let lines = app.take_scrollback_lines(area.width);
            if !lines.is_empty() {
                use ratatui::widgets::{Paragraph, Widget as _};

                let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
                terminal.insert_before(height, |buffer| {
                    Paragraph::new(lines).render(buffer.area, buffer);
                })?;
            }
        }

        if let Ok(area) = terminal.size() {
            let (width, height) = layout::transcript_viewport_size(area.into(), &app.prompt);
            app.set_transcript_viewport(width, height);
        }
        let session_idx = app.session_select_index();
        let fork_idx = app.fork_select_index();
        let slash_info = app.slash_completion().map(|(prefix, selected, matches)| {
            (prefix, selected, matches.into_iter().copied().collect())
        });
        let dialog = app
            .active_dialog
            .as_ref()
            .map(PendingDialog::render_snapshot);
        let model = app.model.clone();
        let model_id = app.model_id.clone();
        let model_connection_id = app.model_connection_id.clone();
        let approval_idx = app.approval_select_index();
        let sessions = app.sessions.clone();
        let session_labels = app.session_labels.clone();
        let fork_messages = app.fork_messages.clone();
        let session_id = app.session_id.clone();
        let status_message = app.status_message.clone();
        let thinking_level = app.thinking_level;
        let approval_mode = app.approval_mode;
        let thinking_idx = app.thinking_select_index();
        let perf_metrics = app.perf_metrics.clone();
        let show_fork_hint = app.supports(Capability::SessionForking);

        let render_start = std::time::Instant::now();
        if !first_render_recorded && let Some(trace) = startup_trace.as_ref() {
            trace.mark("tui_first_render_start");
        }

        terminal.draw(|frame| {
            let mut render_params = layout::RenderParams {
                transcript: &app.transcript,
                transcript_cache: &mut app.transcript_cache,
                scroll_offset: app.scroll_offset,
                is_streaming: app.is_streaming,
                input: &mut app.prompt,
                model: &model,
                thinking_level: thinking_level.as_str(),
                approval_mode: approval_mode.as_str(),
                status_message: &status_message,
                is_connected: app.is_connected,
                compaction_count: app.transcript.compaction_count(),
                terminal_focused: app.terminal_focused,
                context_used_percent: app.context_used_percent,
                context_window: app
                    .session_stats
                    .as_ref()
                    .and_then(|stats| stats.context_window),
                theme: &app.theme,
            };
            layout::render(frame, &mut render_params);
            match &app.mode {
                AppMode::ModelSelect { .. } => {
                    let (models, selected_index, query) = app.model_select_view();
                    let selector = crate::model_selector::ModelSelector {
                        models: &models,
                        current_model: &model_id,
                        current_connection_id: model_connection_id.as_deref(),
                        selected_index,
                        query,
                        theme: &app.theme,
                    };
                    frame.render_widget(selector, frame.area());
                }
                AppMode::SessionSelect { .. } => {
                    let picker = crate::session_picker::SessionPicker {
                        sessions: &sessions,
                        labels: &session_labels,
                        current_session: &session_id,
                        selected_index: session_idx,
                        show_fork_hint,
                        theme: &app.theme,
                    };
                    frame.render_widget(picker, frame.area());
                }
                AppMode::ForkSelect { .. } => {
                    let picker = crate::fork_picker::ForkPicker {
                        messages: &fork_messages,
                        selected_index: fork_idx,
                        theme: &app.theme,
                    };
                    frame.render_widget(picker, frame.area());
                }
                AppMode::SlashComplete { .. } => {
                    if let Some((_prefix, selected, matches)) = slash_info {
                        let popup = crate::slash_popup::SlashPopup {
                            selected,
                            matches,
                            theme: &app.theme,
                        };
                        frame.render_widget(popup, frame.area());
                    }
                }
                AppMode::Normal
                | AppMode::DialogConfirm
                | AppMode::DialogInput
                | AppMode::DialogSecretInput
                | AppMode::DialogSelect => {}
                AppMode::LocalEditCompose { step, draft } => {
                    render_local_edit_compose_overlay(frame, *step, draft, &app.theme);
                }
                AppMode::LocalEditReview { preview, selected } => {
                    render_local_edit_review_overlay(frame, preview, *selected, &app.theme);
                }
                AppMode::HelpOverlay => {
                    frame.render_widget(
                        crate::help_overlay::HelpOverlay { theme: &app.theme },
                        frame.area(),
                    );
                }
                AppMode::ThinkingSelect { .. } => {
                    let selector = crate::thinking_selector::ThinkingLevelSelector {
                        levels: &ThinkingLevel::ALL,
                        current_level: thinking_level,
                        selected_index: thinking_idx,
                        theme: &app.theme,
                    };
                    frame.render_widget(selector, frame.area());
                }
                AppMode::ApprovalSelect { .. } => {
                    frame.render_widget(
                        crate::approval_selector::ApprovalModeSelector {
                            current_mode: approval_mode,
                            selected_index: approval_idx,
                            theme: &app.theme,
                        },
                        frame.area(),
                    );
                }
                AppMode::FullAccessConfirm { selected } => {
                    frame.render_widget(
                        crate::approval_selector::FullAccessConfirmation {
                            enable_selected: *selected == FullAccessConfirmationAction::Enable,
                            theme: &app.theme,
                        },
                        frame.area(),
                    );
                }
                AppMode::PerfOverlay => {
                    let overlay = crate::perf_overlay::PerfMetricsOverlay {
                        metrics: &perf_metrics,
                        theme: &app.theme,
                    };
                    frame.render_widget(overlay, frame.area());
                }
            }

            if let Some(dialog) = dialog.as_ref() {
                render_dialog_overlay(frame, dialog, &app.theme);
            }
        })?;

        app.perf_metrics.record_render(render_start.elapsed());
        if !first_render_recorded {
            if let Some(trace) = startup_trace.as_ref() {
                trace.mark("tui_first_render_end");
                trace.flush();
            }
            if app.supports(Capability::FirstRenderEvents) {
                app.send_client_event(ClientEvent::FirstRenderCompleted);
            }
            first_render_recorded = true;
        }
    }

    terminal.clear()?;
    terminal_guard.restore()?;
    io::stdout().execute(crossterm::cursor::MoveToNextLine(1))?;

    Ok(())
}

fn render_dialog_overlay(
    frame: &mut ratatui::Frame<'_>,
    dialog: &DialogRenderSnapshot,
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(70, 50, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.colors.border))
        .title(dialog_summary(&dialog.request))
        .title_style(
            Style::new()
                .fg(theme.colors.accent)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let mut lines: Vec<Line<'_>> = Vec::new();
    if let Some(prompt) = dialog.request.prompt.as_deref()
        && !prompt.is_empty()
    {
        lines.push(Line::from(prompt.to_string()));
        lines.push(Line::raw(""));
    }

    match &dialog.request.kind {
        DialogKind::Confirm => {
            let yes_style = if dialog.confirm_accepted {
                Style::new()
                    .fg(theme.colors.selected_text)
                    .bg(theme.colors.success)
            } else {
                Style::new().fg(theme.colors.success)
            };
            let no_style = if dialog.confirm_accepted {
                Style::new().fg(theme.colors.error)
            } else {
                Style::new()
                    .fg(theme.colors.selected_text)
                    .bg(theme.colors.error)
            };
            lines.push(Line::from(vec![
                Span::styled(" Yes ", yes_style),
                Span::raw("  "),
                Span::styled(" No ", no_style),
            ]));
            lines.push(Line::raw(""));
            lines.push(Line::from("Enter to confirm, Esc to cancel"));
        }
        DialogKind::DeviceCode {
            verification_uri,
            user_code,
        } => {
            lines.push(Line::from(format!("URL: {verification_uri}")));
            lines.push(Line::from(format!("Code: {user_code}")));
            lines.push(Line::raw(""));
            lines.push(Line::from("Esc to cancel · c copies code · u copies URL"));
        }
        DialogKind::Input { .. } | DialogKind::SecretInput => {
            render_dialog_textarea(
                frame,
                inner,
                lines,
                dialog,
                "Enter to submit, Esc to cancel",
                theme,
            );
            return;
        }
        DialogKind::Editor { .. } => {
            render_dialog_textarea(
                frame,
                inner,
                lines,
                dialog,
                "Enter to submit, Ctrl+J for newline, Esc to cancel",
                theme,
            );
            return;
        }
        DialogKind::Select { options } => {
            for (idx, option) in options.iter().enumerate() {
                let is_selected = idx == dialog.selected;
                let style = if is_selected {
                    Style::new()
                        .fg(theme.colors.selected_text)
                        .bg(theme.colors.selected_background)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme.colors.muted)
                };
                let prefix = if is_selected { "▸ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(option.label.clone(), style),
                ]));
            }
            if options.is_empty() {
                lines.push(Line::from("No options available"));
            }
            lines.push(Line::raw(""));
            lines.push(Line::from("Enter to choose, Esc to cancel"));
        }
    }

    let paragraph = Paragraph::new(lines).style(Style::new().fg(theme.colors.text));
    Widget::render(paragraph, inner, frame.buffer_mut());
}

fn render_local_edit_compose_overlay(
    frame: &mut ratatui::Frame<'_>,
    step: LocalEditComposeStep,
    draft: &LocalEditDraft,
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(70, 50, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.colors.border))
        .title("local edit")
        .title_style(
            Style::new()
                .fg(theme.colors.accent)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let mut lines = Vec::new();
    if step == LocalEditComposeStep::Kind {
        lines.push(Line::from(vec![
            Span::styled("1", Style::new().fg(theme.colors.warning)),
            Span::raw(" Modify existing file"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("2", Style::new().fg(theme.colors.warning)),
            Span::raw(" Create new file"),
        ]));
        lines.push(Line::raw(""));
        lines.push(Line::from("Choose edit kind"));
    } else {
        lines.push(Line::from(local_edit_compose_prompt(step)));
        if let Some(path) = draft.path.as_deref() {
            lines.push(Line::from(format!("Path: {path}")));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(draft.buffer.clone()));
        lines.push(Line::raw(""));
        lines.push(Line::from(
            "Enter to continue, Ctrl+J newline, Esc to cancel",
        ));
    }

    Widget::render(
        Paragraph::new(lines).style(Style::new().fg(theme.colors.text)),
        inner,
        frame.buffer_mut(),
    );
}

fn local_edit_compose_prompt(step: LocalEditComposeStep) -> &'static str {
    match step {
        LocalEditComposeStep::Kind => "Choose edit kind",
        LocalEditComposeStep::Path => "Path",
        LocalEditComposeStep::ExpectedSha256 => "Expected SHA-256",
        LocalEditComposeStep::Find => "Find",
        LocalEditComposeStep::Replace => "Replace",
        LocalEditComposeStep::Content => "Content",
    }
}

fn render_local_edit_review_overlay(
    frame: &mut ratatui::Frame<'_>,
    preview: &LocalEditReview,
    selected: LocalEditReviewAction,
    theme: &Theme,
) {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(76, 62, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(theme.colors.border))
        .title("review local edit")
        .title_style(
            Style::new()
                .fg(theme.colors.accent)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let apply_style = if selected == LocalEditReviewAction::Apply {
        Style::new()
            .fg(theme.colors.selected_text)
            .bg(theme.colors.success)
    } else {
        Style::new().fg(theme.colors.success)
    };
    let reject_style = if selected == LocalEditReviewAction::Reject {
        Style::new()
            .fg(theme.colors.selected_text)
            .bg(theme.colors.error)
    } else {
        Style::new().fg(theme.colors.error)
    };
    let mut lines = vec![
        Line::from(format!("Path: {}", preview.path)),
        Line::from(format!("Operation: {}", preview.operation)),
        Line::from(format!("Review: {:?}", preview.review_state)),
        Line::raw(""),
    ];
    let action_lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            if selected == LocalEditReviewAction::Apply {
                "› Approve"
            } else {
                "  Approve"
            },
            apply_style,
        )),
        Line::from(Span::styled(
            if selected == LocalEditReviewAction::Reject {
                "› Reject"
            } else {
                "  Reject"
            },
            reject_style,
        )),
        Line::from("↑/↓ or j/k select, Enter submits, Esc rejects"),
    ];
    let diff_line_budget =
        usize::from(inner.height).saturating_sub(lines.len() + action_lines.len() + 1);
    let mut rendered_diff_lines = 0;
    for line in preview.diff_summary.lines().take(diff_line_budget) {
        lines.push(Line::from(Span::styled(
            line.to_owned(),
            review_diff_line_style(line, theme),
        )));
        rendered_diff_lines += 1;
    }
    let diff_was_line_truncated = preview.diff_summary.lines().count() > rendered_diff_lines;
    if preview.diff_summary_truncated || diff_was_line_truncated {
        lines.push(Line::from("[diff summary truncated]"));
    }
    lines.extend(action_lines);

    Widget::render(
        Paragraph::new(lines).style(Style::new().fg(theme.colors.text)),
        inner,
        frame.buffer_mut(),
    );
}

fn review_diff_line_style(line: &str, theme: &Theme) -> ratatui::style::Style {
    use ratatui::style::Style;

    if line.starts_with("+++ ") || line.starts_with("--- ") {
        return Style::new().fg(theme.colors.muted).bold();
    }
    if line.starts_with('+') {
        return Style::new().fg(theme.colors.diff_added);
    }
    if line.starts_with('-') {
        return Style::new().fg(theme.colors.diff_removed);
    }
    if line.starts_with("@@") {
        return Style::new().fg(theme.colors.accent);
    }
    Style::new().fg(theme.colors.diff_context)
}

fn insert_dialog_newline(dialog: &mut PendingDialog) {
    dialog.cursor_pos = byte_boundary_at_or_before(&dialog.input_buffer, dialog.cursor_pos);
    dialog.input_buffer.insert(dialog.cursor_pos, '\n');
    dialog.cursor_pos += '\n'.len_utf8();
}

fn render_dialog_textarea(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    prompt_lines: Vec<ratatui::text::Line<'_>>,
    dialog: &DialogRenderSnapshot,
    hint: &'static str,
    theme: &Theme,
) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph, Widget};

    let prompt_height = u16::try_from(prompt_lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(4));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(prompt_height),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    if prompt_height > 0 {
        let paragraph = Paragraph::new(prompt_lines).style(Style::new().fg(theme.colors.text));
        Widget::render(paragraph, chunks[0], frame.buffer_mut());
    }

    let mut textarea = dialog_textarea(&dialog.input_buffer, dialog.cursor_pos);
    textarea.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(theme.colors.border))
            .title("input")
            .title_style(Style::new().fg(theme.colors.warning)),
    );
    textarea.set_wrap_mode(WrapMode::Word);
    textarea.set_style(Style::new().fg(theme.colors.text));
    textarea.set_cursor_line_style(Style::default());
    textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

    Widget::render(&textarea, chunks[1], frame.buffer_mut());
    Widget::render(
        Paragraph::new(Line::from(hint)).style(Style::new().fg(theme.colors.dim)),
        chunks[2],
        frame.buffer_mut(),
    );
}

fn dialog_textarea(input: &str, cursor_pos: usize) -> TextArea<'static> {
    let mut textarea = TextArea::new(input.split('\n').map(ToOwned::to_owned).collect());
    let (row, col) = textarea_cursor_from_byte_pos(input, cursor_pos);
    textarea.move_cursor(CursorMove::Jump(row, col));
    textarea
}

fn textarea_cursor_from_byte_pos(input: &str, cursor_pos: usize) -> (u16, u16) {
    let cursor_pos = byte_boundary_at_or_before(input, cursor_pos.min(input.len()));
    let mut row: u16 = 0;
    let mut col: u16 = 0;

    for ch in input[..cursor_pos].chars() {
        if ch == '\n' {
            row = row.saturating_add(1);
            col = 0;
        } else {
            col = col.saturating_add(1);
        }
    }

    (row, col)
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout};

    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::{
        App, AppMode, EMPTY_ASSISTANT_RESPONSE_MESSAGE, FullAccessConfirmationAction,
        LocalEditComposeStep, LocalEditDraft, LocalEditReview, LocalEditReviewAction,
        MAX_TOOL_ERROR_EXCERPT_CHARS, SessionMessageHydration, StartupTrace, tool_output_summary,
    };
    use crate::thinking_level::ThinkingLevel;
    use crate::transcript::EntryKind;
    use crossterm::event::{Event, KeyCode, KeyModifiers};
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
    use std::sync::Arc;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;
    use yach_proto::{
        ApprovalMode, BackendEvent, BackendState, Capability, ClientEvent, DialogKind,
        DialogRequest, DialogResponse, ExtensionDiagnosticRecord,
        ExtensionDiagnosticSnapshotOutcome, ExtensionLifecycleAction, ExtensionLifecycleOutcome,
        ForkMessage, ForkPosition, Handshake, HarnessOutcomeKind, LocalEditDecision,
        LocalEditFinishedOutcome, LocalEditOperationInput, LocalEditPreviewSummary,
        LocalEditReviewState, ModelChangeTarget, ModelInfo, NegotiatedCapabilities, PromptOutcome,
        RecentSession, ServerEvent, SessionMessage, SessionStats, ToolResult, ToolResultMetadata,
        ToolReviewDecision, ToolReviewPayload, ToolReviewResolution, default_backend_handshake,
        default_ui_handshake,
    };

    fn connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &default_backend_handshake(),
            ),
        }
    }

    #[test]
    fn startup_trace_buffers_marks_until_flush() {
        let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos(),
            Err(_) => 0,
        };
        let trace_path = std::env::temp_dir().join(format!(
            "yach-startup-trace-test-{}-{timestamp}.log",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&trace_path);
        let trace = StartupTrace {
            path: trace_path.clone(),
            start: Instant::now(),
            marks: Arc::default(),
        };

        trace.mark("alpha");
        trace.mark("beta");

        assert!(!trace_path.exists());

        trace.flush();

        let contents = std::fs::read_to_string(&trace_path);
        assert!(contents.is_ok());
        let Ok(contents) = contents else {
            return;
        };
        let _ = std::fs::remove_file(&trace_path);
        assert!(contents.lines().any(|line| line.ends_with(" alpha")));
        assert!(contents.lines().any(|line| line.ends_with(" beta")));
    }

    #[test]
    fn terminal_focus_events_toggle_input_focus_state() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        assert!(app.terminal_focused);
        app.handle_terminal_focus_event(&Event::FocusLost);
        assert!(!app.terminal_focused);
        app.handle_terminal_focus_event(&Event::FocusGained);
        assert!(app.terminal_focused);
    }

    fn connected_event_without_capabilities() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native", vec![]),
            ),
        }
    }

    fn cancellable_native_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native", vec![Capability::PromptCancellation]),
            ),
        }
    }

    fn local_edit_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native", vec![Capability::LocalEdit]),
            ),
        }
    }

    fn extension_lifecycle_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native", vec![Capability::ExtensionLifecycle]),
            ),
        }
    }
    fn provider_connections_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new(
                    "provider-enabled-backend",
                    vec![Capability::ProviderConnections],
                ),
            ),
        }
    }

    fn model(provider: &str, id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            provider: provider.to_string(),
            id: id.to_string(),
            name: name.to_string(),
            connection_id: None,
            connection_display: None,
        }
    }

    fn model_change(
        model: &str,
        connection_id: Option<&str>,
        provider: Option<&str>,
        request_id: Option<u64>,
    ) -> ModelChangeTarget {
        ModelChangeTarget {
            model: String::from(model),
            connection_id: connection_id.map(String::from),
            provider: provider.map(String::from),
            request_id,
        }
    }

    fn session_message(role: &str, entry_id: &str, text: &str) -> SessionMessage {
        SessionMessage {
            role: role.to_string(),
            text: text.to_string(),
            entry_id: Some(entry_id.to_string()),
            tool_name: None,
            is_error: None,
            outcome_kind: None,
            tool_result_metadata: None,
            tool_review: None,
        }
    }

    fn tool_session_message(
        entry_id: &str,
        tool_name: &str,
        text: &str,
        is_error: bool,
    ) -> SessionMessage {
        SessionMessage {
            role: String::from("tool"),
            text: text.to_string(),
            entry_id: Some(entry_id.to_string()),
            tool_name: Some(tool_name.to_string()),
            is_error: Some(is_error),
            outcome_kind: None,
            tool_result_metadata: None,
            tool_review: None,
        }
    }

    fn local_edit_preview(review_state: LocalEditReviewState) -> LocalEditPreviewSummary {
        LocalEditPreviewSummary {
            preview_id: String::from("preview-1"),
            transaction_id: String::from("tx-1"),
            permission_decision_id: String::from("permission-1"),
            path: String::from("src/lib.rs"),
            operation: String::from("modify_text_file"),
            review_state,
            diff_summary: String::from("-old\n+new"),
            diff_summary_truncated: false,
        }
    }

    fn type_chars(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE);
        }
    }
    fn open_secret_dialog(app: &mut App, id: &str) {
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(id.to_string()),
            title: Some(String::from("provider connection")),
            prompt: Some(String::from("Enter the API key")),
            kind: DialogKind::SecretInput,
        }));
    }

    fn rendered_active_dialog(app: &App) -> String {
        assert!(app.active_dialog.is_some(), "dialog should be active");
        let Some(dialog) = app.active_dialog.as_ref() else {
            return String::new();
        };
        let dialog = dialog.render_snapshot();
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = match ratatui::Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(infallible) => match infallible {},
        };
        let result =
            terminal.draw(|frame| super::render_dialog_overlay(frame, &dialog, &app.theme));
        assert!(result.is_ok());
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    fn expect_local_edit_preview(app: &mut App, request_id: &str) {
        app.pending_local_edit_request_id = Some(request_id.to_string());
        app.local_edit_request_counter = app.local_edit_request_counter.max(1);
    }

    #[test]
    fn submit_input_emits_prompt_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.set_prompt_text("hello from tui");

        app.submit_input();

        assert_eq!(app.transcript.entries().len(), 1);
        assert_eq!(app.transcript.entries()[0].content, "hello from tui");
        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: String::from("hello from tui"),
            }
        );
    }

    #[test]
    fn ctrl_j_inserts_newline_without_submitting() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.set_prompt_text("hello");

        app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL);

        assert_eq!(app.prompt_text(), "hello\n");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn shift_enter_inserts_newline_without_submitting() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.set_prompt_text("hello");

        app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT);

        assert_eq!(app.prompt_text(), "hello\n");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn ctrl_u_clears_input_without_inserting_u() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.set_prompt_text("delete me");

        app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL);

        assert!(app.prompt.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn modified_backspace_clears_input() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("delete me");

        app.handle_key(KeyCode::Backspace, KeyModifiers::META);

        assert!(app.prompt.is_empty());
    }

    #[test]
    fn unknown_control_char_does_not_insert_literal_text() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_key(KeyCode::Char('x'), KeyModifiers::CONTROL);

        assert!(app.prompt.is_empty());
    }

    #[test]
    fn plain_enter_submits_multiline_input() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.set_prompt_text("hello\nworld");

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: String::from("hello\nworld"),
            }
        );
    }

    #[test]
    fn dialog_requests_are_resolved_inside_the_tui() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let request = DialogRequest {
            id: Some(String::from("dlg-1")),
            title: Some(String::from("Confirm action")),
            prompt: Some(String::from("Continue?")),
            kind: DialogKind::Confirm,
        };

        app.handle_server_event(ServerEvent::DialogRequested(request));
        assert!(matches!(app.mode, AppMode::DialogConfirm));

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::Normal));
        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-1"),
                response: DialogResponse::Confirmed { accepted: true },
            }
        );
    }

    #[test]
    fn device_code_enter_keeps_dialog_open() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("provider-connection:chatgpt:device")),
            title: Some(String::from("ChatGPT login")),
            prompt: Some(String::from("Waiting for authorization")),
            kind: DialogKind::DeviceCode {
                verification_uri: String::from("https://auth.openai.com/device"),
                user_code: String::from("ABCD-1234"),
            },
        }));
        assert!(matches!(app.mode, AppMode::DialogConfirm));

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::DialogConfirm));
        assert!(rx.try_recv().is_err());

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::Normal));
        let event = rx.try_recv();
        assert!(matches!(
            event,
            Ok(ClientEvent::DialogResolved {
                response: DialogResponse::Cancelled,
                ..
            })
        ));
    }

    #[test]
    fn connection_success_replaces_device_code_dialog() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("provider-connection:chatgpt:device")),
            title: Some(String::from("ChatGPT login")),
            prompt: Some(String::from("Waiting for authorization")),
            kind: DialogKind::DeviceCode {
                verification_uri: String::from("https://auth.openai.com/device"),
                user_code: String::from("ABCD-1234"),
            },
        }));
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("provider-connection:root")),
            title: Some(String::from("Provider connections")),
            prompt: Some(String::from("Choose a connection")),
            kind: DialogKind::Select {
                options: vec![yach_proto::DialogOption {
                    label: String::from("Add connection"),
                    value: String::from("add"),
                }],
            },
        }));

        assert!(matches!(app.mode, AppMode::DialogSelect));
        assert_eq!(
            app.active_dialog
                .as_ref()
                .and_then(|dialog| dialog.request.id.as_deref()),
            Some("provider-connection:root")
        );
        assert!(app.queued_dialogs.is_empty());
    }

    #[test]
    fn device_code_copy_keys_update_status() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("provider-connection:chatgpt:device")),
            title: Some(String::from("ChatGPT login")),
            prompt: Some(String::from("Waiting for authorization")),
            kind: DialogKind::DeviceCode {
                verification_uri: String::from("https://auth.openai.com/device"),
                user_code: String::from("ABCD-1234"),
            },
        }));

        app.handle_key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(
            app.status_message.contains("copied") || app.status_message.contains("could not copy")
        );
        assert!(matches!(app.mode, AppMode::DialogConfirm));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn forking_requires_negotiated_capability() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.fork_session("default");
        assert_eq!(app.status_message, "session forking unavailable");
        assert!(rx.try_recv().is_err());

        app.handle_backend_event(connected_event());
        app.fork_session("default");
        assert_eq!(
            app.status_message,
            "send a message before cloning the session"
        );
        assert!(rx.try_recv().is_err());

        app.transcript.append_user_message("hello");
        app.fork_current_session();

        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(event, ClientEvent::ForkMessagesRequested);
    }

    #[test]
    fn ctrl_b_requests_and_presents_session_tree_summary() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());

        app.handle_key(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::SessionMessagesRequested));
        assert_eq!(app.status_message, "loading session tree");

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![
                session_message("user", "u1", "Start"),
                session_message("assistant", "a1", "Answer"),
                session_message("user", "u2", "Next branch"),
                session_message("assistant", "a2", "Another answer"),
            ],
        });

        assert_eq!(app.status_message, "session tree: 2 branches · 4 messages");
        assert_eq!(
            app.session_tree.as_ref().map(|tree| tree.branches.len()),
            Some(2)
        );
    }

    #[test]
    fn session_messages_hydrate_empty_transcript_after_explicit_resume_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![
                session_message("user", "u1", "Start"),
                session_message("assistant", "a1", "Answer"),
            ],
        });

        assert_eq!(app.transcript.entries().len(), 2);
        assert_eq!(app.transcript.entries()[0].content, "Start");
        assert_eq!(app.transcript.entries()[1].content, "Answer");
        assert_eq!(app.session_message_hydration, SessionMessageHydration::None);
    }

    #[test]
    fn session_messages_hydrate_tool_results_after_explicit_resume_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![
                session_message("user", "u1", "Read README"),
                tool_session_message(
                    "tool-request-1",
                    "read_text_file",
                    "completed; bytes=56; content=redacted; truncated=false",
                    false,
                ),
                session_message("assistant", "a1", "Summary"),
                tool_session_message(
                    "tool-request-2",
                    "create_text_file",
                    "failed; reason=target_exists",
                    true,
                ),
            ],
        });

        assert_eq!(app.transcript.entries().len(), 4);
        assert!(matches!(
            app.transcript.entries()[1].kind,
            EntryKind::ToolResult {
                ref name,
                is_error: false,
                ..
            } if name == "read_text_file"
        ));
        assert!(matches!(
            app.transcript.entries()[3].kind,
            EntryKind::ToolResult {
                ref name,
                is_error: true,
                ..
            } if name == "create_text_file"
        ));
        assert_eq!(
            app.transcript.entries()[3].content,
            "failed; reason=target_exists"
        );
    }

    #[test]
    fn session_messages_hydrate_turn_outcomes_as_harness_rows() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;

        let mut failed = session_message("harness", "turn-1", "provider_error kind=rate_limited");
        failed.outcome_kind = Some(HarnessOutcomeKind::Failed);
        let mut cancelled = session_message("harness", "turn-2", "cancelled by user");
        cancelled.outcome_kind = Some(HarnessOutcomeKind::Cancelled);

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![session_message("user", "u1", "Start"), failed, cancelled],
        });

        assert_eq!(app.transcript.entries().len(), 3);
        assert!(matches!(
            app.transcript.entries()[1].kind,
            EntryKind::HarnessOutcome {
                kind: HarnessOutcomeKind::Limit
            }
        ));
        assert!(matches!(
            app.transcript.entries()[2].kind,
            EntryKind::HarnessOutcome {
                kind: HarnessOutcomeKind::Cancelled
            }
        ));
    }

    #[test]
    fn completed_prompt_after_tool_result_shows_empty_response_placeholder() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallFinished(ToolResult {
            tool_call_id: Some(String::from("tool-request-1")),
            tool_name: String::from("search_project"),
            output: String::from("completed; bytes=53; content=redacted; truncated=false"),
            is_error: false,
            outcome_kind: None,
            metadata: None,
        }));

        app.handle_server_event(ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome: PromptOutcome::Completed,
            message: Some(String::from("turn_end provider")),
        });

        assert_eq!(app.transcript.entries().len(), 2);
        assert_eq!(
            app.transcript.entries()[1].content,
            EMPTY_ASSISTANT_RESPONSE_MESSAGE
        );
    }

    #[test]
    fn session_tree_messages_do_not_hydrate_without_resume_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![session_message("user", "u1", "Start")],
        });

        assert!(app.transcript.entries().is_empty());
        assert_eq!(app.status_message, "session tree: 1 branches · 1 messages");
    }

    #[test]
    fn session_messages_replace_stale_transcript_after_explicit_resume() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.transcript.append_user_message("stale scrollback");
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![session_message("user", "u1", "resumed history")],
        });

        assert_eq!(app.transcript.entries().len(), 1);
        assert_eq!(app.transcript.entries()[0].content, "resumed history");
        assert_eq!(app.session_message_hydration, SessionMessageHydration::None);
    }

    #[test]
    fn selecting_the_current_session_is_a_no_op() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.transcript.append_user_message("keep me");
        app.sessions = vec![String::from("/tmp/current-session.jsonl")];
        app.session_labels = vec![String::from("current")];
        app.session_is_path = vec![true];
        app.session_file = Some(String::from("/tmp/current-session.jsonl"));
        app.mode = AppMode::SessionSelect { selected: 0 };

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.session_message_hydration, SessionMessageHydration::None);
        assert_eq!(app.status_message, "already on this session");
        assert_eq!(app.transcript.entries().len(), 1);
        assert_eq!(app.transcript.entries()[0].content, "keep me");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn fork_picker_sends_entry_id_fork() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.transcript.append_user_message("hello");

        app.fork_current_session();
        assert_eq!(rx.try_recv(), Ok(ClientEvent::ForkMessagesRequested));

        app.handle_server_event(ServerEvent::ForkMessagesUpdated {
            messages: vec![ForkMessage {
                entry_id: String::from("entry-1"),
                text: String::from("hello"),
            }],
        });
        assert!(matches!(app.mode, AppMode::ForkSelect { selected: 0 }));

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::SessionForkRequested {
                session_id: String::from("default"),
                entry_id: Some(String::from("entry-1")),
                position: ForkPosition::Before,
            }
        );
    }

    #[test]
    fn backend_state_updates_loaded_defaults() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: Some(String::from("gpt-5.4")),
            model_name: Some(String::from("GPT-5.4")),
            model_provider: Some(String::from("openai")),
            model_connection_id: None,
            session_id: Some(String::from("sess-1")),
            session_file: Some(String::from("/tmp/session.jsonl")),
            thinking_level: Some(ThinkingLevel::High),
            is_streaming: true,
            is_compacting: false,
            message_count: Some(3),
            pending_message_count: Some(1),
        }));

        assert_eq!(app.model, "GPT-5.4");
        assert_eq!(app.session_id, "sess-1");
        assert!(app.sessions.contains(&String::from("sess-1")));
        assert_eq!(app.thinking_level.as_str(), "high");
        assert!(app.is_streaming);
    }

    #[test]
    fn tool_events_update_active_tools_and_transcript() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("call-1")),
            tool_name: String::from("bash"),
            preview: Some(String::from("pwd")),
        });
        assert_eq!(app.active_tools.len(), 1);
        assert_eq!(app.active_tools[0].label(), "bash pwd");

        app.handle_server_event(ServerEvent::ToolCallFinished(ToolResult {
            tool_call_id: Some(String::from("call-1")),
            tool_name: String::from("bash"),
            output: String::from("done\n"),
            is_error: false,
            outcome_kind: None,
            metadata: None,
        }));

        assert!(app.active_tools.is_empty());
        assert_eq!(app.transcript.entries().len(), 1);
        assert_eq!(
            app.transcript.entries()[0].content,
            "completed: 1 line, 5 bytes"
        );
    }

    #[test]
    fn lifecycle_events_do_not_overwrite_status_text() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.status_message = String::from("state loaded");

        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("agent_end"),
        });

        assert_eq!(app.status_message, "state loaded");
        assert!(!app.is_streaming);
    }

    #[test]
    fn local_cancel_ignores_stale_deltas_until_lifecycle_end() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_server_event(ServerEvent::PromptDelta {
            session_id: String::from("active"),
            delta: String::from("stale"),
        });

        assert!(app.transcript.entries().is_empty());
        assert!(!app.is_streaming);
        assert!(app.backend_busy());

        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_end"),
        });
        assert!(!app.backend_busy());
    }

    #[test]
    fn backend_cancel_requires_prompt_cancellation_capability() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event_without_capabilities());
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(app.status_message, "cancelled locally; waiting for backend");
    }

    #[test]
    fn backend_cancel_sends_event_when_capability_is_negotiated() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(cancellable_native_connected_event());
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::PromptCancelled {
                session_id: String::from("default"),
            })
        );
        assert_eq!(app.status_message, "cancelling prompt...");
    }

    #[test]
    fn esc_cancels_streaming_prompt_when_capability_is_negotiated() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(cancellable_native_connected_event());
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });
        app.set_prompt_text("draft reply survives the interrupt");

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::PromptCancelled {
                session_id: String::from("default"),
            })
        );
        assert_eq!(app.status_message, "cancelling prompt...");
        assert!(app.prompt_has_text(), "esc-cancel keeps the drafted input");
    }

    #[test]
    fn esc_clears_input_when_not_streaming() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("draft");

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

        assert!(!app.prompt_has_text());
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn prompt_finished_returns_backend_to_idle() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });

        app.handle_server_event(ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome: PromptOutcome::Cancelled,
            message: Some(String::from("cancelled by backend")),
        });

        assert!(!app.backend_busy());
        assert_eq!(app.status_message, "cancelled by backend");
    }

    #[test]
    fn busy_backend_state_updates_apply_after_lifecycle_end() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });
        app.handle_server_event(ServerEvent::SessionStatsUpdated(SessionStats {
            message_count: None,
            user_message_count: None,
            assistant_message_count: None,
            tool_message_count: None,
            total_tokens: None,
            context_window: Some(120_000),
            context_used_percent: Some(42),
        }));

        app.handle_server_event(ServerEvent::SessionChanged {
            session_id: String::from("sess-2"),
        });
        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "model-2", None, None, None,
        )));
        app.handle_server_event(ServerEvent::SessionStatsUpdated(SessionStats {
            message_count: None,
            user_message_count: None,
            assistant_message_count: None,
            tool_message_count: None,
            total_tokens: None,
            context_window: Some(240_000),
            context_used_percent: Some(21),
        }));
        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: None,
            model_name: None,
            model_provider: None,
            model_connection_id: None,
            session_id: None,
            session_file: None,
            thinking_level: Some(ThinkingLevel::High),
            is_streaming: true,
            is_compacting: false,
            message_count: None,
            pending_message_count: None,
        }));

        assert_eq!(app.session_id, "default");
        assert_eq!(app.model, "default");
        assert_eq!(app.context_used_percent, Some(42));
        assert_eq!(
            app.session_stats
                .as_ref()
                .and_then(|stats| stats.context_window),
            Some(120_000)
        );
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_end"),
        });

        assert_eq!(app.session_id, "sess-2");
        assert_eq!(app.model, "model-2");
        assert_eq!(app.thinking_level.as_str(), "high");
        assert_eq!(app.context_used_percent, Some(21));
        assert_eq!(
            app.session_stats
                .as_ref()
                .and_then(|stats| stats.context_window),
            Some(240_000)
        );
    }

    #[test]
    fn local_cancel_finishes_when_backend_state_stops_streaming() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: None,
            model_name: None,
            model_provider: None,
            model_connection_id: None,
            session_id: None,
            session_file: None,
            thinking_level: None,
            is_streaming: false,
            is_compacting: false,
            message_count: None,
            pending_message_count: None,
        }));

        assert!(!app.backend_busy());
        assert!(!app.is_streaming);
    }

    #[test]
    fn local_cancel_ignores_stale_tool_results() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("call-1")),
            tool_name: String::from("bash"),
            preview: Some(String::from("pwd")),
        });
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        app.handle_server_event(ServerEvent::ToolCallFinished(ToolResult {
            tool_call_id: Some(String::from("call-1")),
            tool_name: String::from("bash"),
            output: String::from("done"),
            is_error: false,
            outcome_kind: None,
            metadata: None,
        }));

        assert_eq!(app.transcript.entries().len(), 1);
        assert!(app.active_tools.is_empty());
    }

    #[test]
    fn pending_session_deltas_are_accepted_during_stream() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });
        app.handle_server_event(ServerEvent::SessionChanged {
            session_id: String::from("sess-2"),
        });

        app.handle_server_event(ServerEvent::PromptDelta {
            session_id: String::from("sess-2"),
            delta: String::from("visible"),
        });

        assert_eq!(app.transcript.entries().len(), 1);
        assert_eq!(app.transcript.entries()[0].content, "visible");
    }

    #[test]
    fn blank_prompt_is_not_submitted() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(app.prompt.is_empty());
        assert!(app.transcript.entries().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn alt_m_opens_model_selector() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);

        assert!(matches!(app.mode, AppMode::ModelSelect { .. }));
        assert_eq!(app.status_message, "loading available models");
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.status_message, "available models not loaded yet");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn model_selector_requests_fresh_availability_while_retaining_stale_models() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let stale = model("anthropic", "stale-model", "Stale Model");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![stale.clone()],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);

        assert_eq!(app.available_models, vec![stale]);
        assert_eq!(app.status_message, "loading available models");
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));

        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "fresh-model", "Fresh Model")],
        });
        assert_eq!(
            app.available_models,
            vec![model("anthropic", "fresh-model", "Fresh Model")]
        );
        assert_eq!(app.status_message, "available models loaded");
    }

    #[test]
    fn discovered_models_event_replaces_complete_snapshot_without_changing_curated_models() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "curated", "Curated");
        let complete_only = model("anthropic", "complete-only", "Complete Only");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });

        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![curated.clone(), complete_only.clone()],
        });

        assert_eq!(app.available_models, vec![curated]);
        assert_eq!(
            app.discovered_models,
            vec![model("anthropic", "curated", "Curated"), complete_only]
        );
    }

    #[test]
    fn model_selector_clears_loading_after_an_active_only_reopen_response() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let active = model("anthropic", "active-model", "Active Model");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![active.clone()],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);

        assert_eq!(app.status_message, "loading available models");
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![active.clone()],
        });
        assert_eq!(app.available_models, vec![active]);
        assert_eq!(app.status_message, "available models loaded");
    }

    #[test]
    fn model_selector_uses_backend_models_without_optimistic_state_change() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model(
                "anthropic",
                "claude-sonnet-4-20250514",
                "Claude Sonnet 4",
            )],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.model, "default");
        assert_eq!(
            app.status_message,
            "model requested: anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ModelSelectedDetailed {
                provider: String::from("anthropic"),
                model_id: String::from("claude-sonnet-4-20250514"),
                connection_id: None,
                request_id: 1,
            })
        );

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "anthropic/claude-sonnet-4-20250514",
            None,
            None,
            None,
        )));
        assert_eq!(app.model, "anthropic/claude-sonnet-4-20250514");
    }

    #[test]
    fn successful_model_picker_activation_opens_thinking_selector() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "claude-sonnet-4", "Claude Sonnet 4")],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::Normal));
        app.handle_server_event(ServerEvent::ModelChangeFailed(model_change(
            "startup-restored-model",
            None,
            Some("anthropic"),
            None,
        )));
        assert!(matches!(app.mode, AppMode::Normal));

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            None,
            Some(1),
        )));

        assert!(matches!(app.mode, AppMode::ThinkingSelect { selected: 0 }));
    }

    #[test]
    fn failed_model_activation_cancels_thinking_handoff() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "claude-sonnet-4", "Claude Sonnet 4")],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_server_event(ServerEvent::ModelChangeFailed(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));
        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));

        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn stale_terminal_for_same_model_does_not_complete_new_handoff() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "claude-sonnet-4", "Claude Sonnet 4")],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        app.handle_server_event(ServerEvent::ModelChangeFailed(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));
        assert_eq!(
            app.pending_thinking_handoff
                .as_ref()
                .map(|pending| pending.request_id),
            Some(2)
        );

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));
        assert!(matches!(app.mode, AppMode::Normal));
        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(2),
        )));
        assert!(matches!(app.mode, AppMode::ThinkingSelect { selected: 0 }));
    }

    #[test]
    fn successful_model_handoff_waits_for_idle_before_opening_thinking() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "claude-sonnet-4", "Claude Sonnet 4")],
        });
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.set_stream_state(super::StreamState::Streaming {
            session_id: String::from("default"),
        });

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));
        assert!(matches!(app.mode, AppMode::Normal));

        app.handle_server_event(ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome: PromptOutcome::Completed,
            message: Some(String::from("prompt completed")),
        });
        assert!(matches!(app.mode, AppMode::ThinkingSelect { selected: 0 }));
    }

    #[test]
    fn successful_model_handoff_preserves_active_ui_mode_until_it_closes() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![model("anthropic", "claude-sonnet-4", "Claude Sonnet 4")],
        });
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        app.mode = AppMode::LocalEditCompose {
            step: super::LocalEditComposeStep::Path,
            draft: super::LocalEditDraft {
                buffer: String::from("draft path"),

                ..super::LocalEditDraft::default()
            },
        };

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            Some(1),
        )));
        assert!(matches!(
            &app.mode,
            AppMode::LocalEditCompose { draft, .. } if draft.buffer == "draft path"
        ));

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::ThinkingSelect { selected: 0 }));
    }
    #[test]
    fn thinking_level_application_replaces_optimistic_status_with_current_level() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());

        app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ThinkingLevelSelected {
                level: ThinkingLevel::Medium,
            })
        );
        assert_eq!(app.status_message, "thinking: medium");

        app.handle_server_event(ServerEvent::ThinkingLevelApplied {
            level: ThinkingLevel::Medium,
        });

        assert_eq!(app.thinking_level.as_str(), "medium");
        assert_eq!(app.status_message, "thinking: medium");
    }

    #[test]
    fn client_send_failure_cancels_deferred_thinking_handoff() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.pending_thinking_handoff = Some(super::PendingThinkingHandoff {
            request_id: 1,
            model: model("anthropic", "claude-sonnet-4", "Claude Sonnet 4"),
            activation_succeeded: true,
        });
        app.set_stream_state(super::StreamState::Streaming {
            session_id: String::from("default"),
        });
        drop(rx);

        assert!(!app.send_client_event(ClientEvent::AvailableModelsRequested));
        assert!(app.pending_thinking_handoff.is_none());
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn disconnect_clears_pending_extension_diagnostic_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.pending_extension_diagnostic_request_id =
            Some(String::from("extension-diagnostic-request-1"));

        app.handle_backend_event(BackendEvent::Disconnected {
            reason: String::from("offline"),
        });

        assert!(app.pending_extension_diagnostic_request_id.is_none());
    }

    #[test]
    fn model_identity_tracks_connection_from_state_and_model_changed() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: Some(String::from("gpt-5")),
            model_name: None,
            model_provider: Some(String::from("openai-compatible")),
            model_connection_id: Some(String::from("connection-a")),
            session_id: None,
            session_file: None,
            thinking_level: None,
            is_streaming: false,
            is_compacting: false,
            message_count: None,
            pending_message_count: None,
        }));
        assert_eq!(app.model, "openai-compatible/gpt-5");
        assert_eq!(app.model_connection_id.as_deref(), Some("connection-a"));

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "gpt-5",
            Some("connection-b"),
            Some("openai-compatible"),
            None,
        )));
        assert_eq!(app.model, "gpt-5");
        assert_eq!(app.model_connection_id.as_deref(), Some("connection-b"));
    }

    #[test]
    fn initial_backend_state_uses_raw_model_id_for_exact_current_row() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![
                ModelInfo {
                    id: String::from("gpt-5"),
                    name: String::from("GPT-5"),
                    provider: String::from("openai-compatible"),
                    connection_id: Some(String::from("connection-a")),
                    connection_display: Some(String::from("A")),
                },
                ModelInfo {
                    id: String::from("gpt-5"),
                    name: String::from("GPT-5"),
                    provider: String::from("openai-compatible"),
                    connection_id: Some(String::from("connection-b")),
                    connection_display: Some(String::from("B")),
                },
            ],
        });
        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: Some(String::from("gpt-5")),
            model_name: Some(String::from("Displayed GPT-5")),
            model_provider: Some(String::from("openai-compatible")),
            model_connection_id: Some(String::from("connection-b")),
            session_id: None,
            session_file: None,
            thinking_level: None,
            is_streaming: false,
            is_compacting: false,
            message_count: None,
            pending_message_count: None,
        }));

        assert_eq!(app.model, "Displayed GPT-5");
        assert_eq!(app.model_id, "gpt-5");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));
        crate::model_selector::ModelSelector {
            models: &app.available_models,
            current_model: &app.model_id,
            current_connection_id: app.model_connection_id.as_deref(),
            selected_index: 0,
            query: "",
            theme: &app.theme,
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("openai-compatible/gpt-5 [B] — GPT-5 (current)"));
        assert!(!rendered.contains("openai-compatible/gpt-5 [A] — GPT-5 (current)"));
    }

    #[test]
    fn model_selector_emits_selected_duplicate_row_connection_id() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![
                ModelInfo {
                    id: String::from("gpt-5"),
                    name: String::from("GPT-5"),
                    provider: String::from("openai-compatible"),
                    connection_id: Some(String::from("connection-a")),
                    connection_display: Some(String::from("A")),
                },
                ModelInfo {
                    id: String::from("gpt-5"),
                    name: String::from("GPT-5"),
                    provider: String::from("openai-compatible"),
                    connection_id: Some(String::from("connection-b")),
                    connection_display: Some(String::from("B")),
                },
            ],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ModelSelectedDetailed {
                provider: String::from("openai-compatible"),
                model_id: String::from("gpt-5"),
                connection_id: Some(String::from("connection-b")),
                request_id: 1,
            })
        );
    }
    #[test]
    fn model_selector_renders_cached_curated_rows_while_refresh_pending() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "curated-model", "Curated Model");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);

        assert_eq!(app.status_message, "loading available models");
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        let (rows, selected, query) = app.model_select_view();
        assert!(query.is_empty());
        assert_eq!(selected, 0);
        assert_eq!(rows, vec![&curated]);

        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 24));
        crate::model_selector::ModelSelector {
            models: &rows,
            current_model: &app.model_id,
            current_connection_id: app.model_connection_id.as_deref(),
            selected_index: selected,
            query,
            theme: &app.theme,
        }
        .render(Rect::new(0, 0, 100, 24), &mut buffer);
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("anthropic/curated-model — Curated Model"));
    }

    #[test]
    fn model_selector_hides_unknown_discovered_rows_at_empty_query() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "curated", "Curated");
        let unknown = model("opencode-zen", "unknown-model", "Unknown Model");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![curated.clone(), unknown],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);

        let (rows, _, query) = app.model_select_view();
        assert!(query.is_empty());
        assert_eq!(rows, vec![&curated]);
    }

    #[test]
    fn model_selector_typed_query_reveals_and_selects_unknown_row() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "claude-sonnet", "Claude Sonnet");
        let unknown = model("opencode-zen", "kimi-k3", "Kimi K3");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![curated, unknown],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        type_chars(&mut app, "KiMi");

        assert!(matches!(app.mode, AppMode::ModelSelect { .. }));
        let (rows, _, query) = app.model_select_view();
        assert_eq!(query, "KiMi");
        assert_eq!(rows, vec![&model("opencode-zen", "kimi-k3", "Kimi K3")]);

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "model requested: opencode-zen/kimi-k3");
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ModelSelectedDetailed {
                provider: String::from("opencode-zen"),
                model_id: String::from("kimi-k3"),
                connection_id: None,
                request_id: 1,
            })
        );
    }

    #[test]
    fn model_selector_backspace_restores_curated_list() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "claude-sonnet", "Claude Sonnet");
        let unknown = model("opencode-zen", "kimi-k3", "Kimi K3");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![curated.clone(), unknown],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        type_chars(&mut app, "kimi");
        let (rows, _, _) = app.model_select_view();
        assert_eq!(rows, vec![&model("opencode-zen", "kimi-k3", "Kimi K3")]);

        for _ in 0..4 {
            app.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        }

        assert!(matches!(app.mode, AppMode::ModelSelect { .. }));
        let (rows, _, query) = app.model_select_view();
        assert!(query.is_empty());
        assert_eq!(rows, vec![&curated]);
    }

    #[test]
    fn model_selector_query_matches_connection_display() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let work = ModelInfo {
            id: String::from("gpt-5"),
            name: String::from("GPT-5"),
            provider: String::from("openai-compatible"),
            connection_id: Some(String::from("connection-work")),
            connection_display: Some(String::from("Work")),
        };
        let home = ModelInfo {
            id: String::from("gpt-5"),
            name: String::from("GPT-5"),
            provider: String::from("openai-compatible"),
            connection_id: Some(String::from("connection-home")),
            connection_display: Some(String::from("Home")),
        };
        app.handle_server_event(ServerEvent::AvailableModelsUpdated { models: vec![] });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![work.clone(), home],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        type_chars(&mut app, "work");

        let (rows, _, _) = app.model_select_view();
        assert_eq!(rows, vec![&work]);
    }

    #[test]
    fn model_selector_zero_matches_cannot_select_and_esc_closes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let curated = model("anthropic", "claude-sonnet", "Claude Sonnet");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![curated.clone()],
        });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![curated],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        type_chars(&mut app, "zzz");

        let (rows, _, _) = app.model_select_view();
        assert!(rows.is_empty());

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::ModelSelect { .. }));
        assert_eq!(app.status_message, "no models match: zzz");
        assert!(rx.try_recv().is_err());

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn model_selector_background_updates_preserve_query_and_clamp_selection() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        let first = model("anthropic", "claude-a", "Claude A");
        let second = model("anthropic", "claude-b", "Claude B");
        let third = model("anthropic", "claude-c", "Claude C");
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![first.clone(), second.clone(), third.clone()],
        });
        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![first.clone(), second, third],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        type_chars(&mut app, "claude");
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::ModelSelect { selected: 2, .. }));

        app.handle_server_event(ServerEvent::DiscoveredModelsUpdated {
            models: vec![first.clone()],
        });

        let AppMode::ModelSelect { selected, query } = &app.mode else {
            unreachable!("picker must stay open after discovered snapshot update");
        };
        assert_eq!(*selected, 0);
        assert_eq!(query, "claude");
        let (rows, _, _) = app.model_select_view();
        assert_eq!(rows, vec![&first]);

        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![first],
        });

        let AppMode::ModelSelect { query, .. } = &app.mode else {
            unreachable!("picker must stay open after curated snapshot update");
        };
        assert_eq!(query, "claude");
    }

    #[test]
    fn slash_completion_opens_while_typing_and_exact_enter_executes() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_key(KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::SlashComplete { .. }));

        app.handle_key(KeyCode::Char('h'), KeyModifiers::NONE);
        assert_eq!(app.prompt_text(), "/h");
        assert!(matches!(app.mode, AppMode::SlashComplete { .. }));

        app.handle_key(KeyCode::Char('e'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('l'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('p'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::HelpOverlay));
    }

    #[test]
    fn resume_slash_command_opens_session_selector() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.set_prompt_text("/resume");
        app.submit_input();

        assert_eq!(app.prompt_text(), "");
        assert_eq!(rx.try_recv(), Ok(ClientEvent::RecentSessionsRequested));
        assert!(matches!(app.mode, AppMode::SessionSelect { selected: 0 }));
        assert_eq!(app.status_message, "loading recent sessions");
    }

    #[test]
    fn approval_picker_opens_and_switches_while_streaming() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.is_streaming = true;
        app.set_prompt_text("/approval");

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::ApprovalSelect { selected: 0 }));
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ApprovalModeSelected {
                request_id: 1,
                mode: ApprovalMode::AcceptEdits,
            })
        );
    }

    #[test]
    fn approval_picker_requires_confirmation_before_full_access() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("/approval");
        app.submit_input();
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(
            app.mode,
            AppMode::FullAccessConfirm {
                selected: FullAccessConfirmationAction::Cancel
            }
        ));
        assert!(rx.try_recv().is_err());

        app.handle_key(KeyCode::Up, KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ApprovalModeSelected {
                request_id: 1,
                mode: ApprovalMode::FullAccess,
            })
        );
    }

    #[test]
    fn direct_full_access_command_uses_same_confirmation_and_can_cancel() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.is_streaming = true;
        app.set_prompt_text("/approval full-access");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(
            app.mode,
            AppMode::FullAccessConfirm {
                selected: FullAccessConfirmationAction::Cancel
            }
        ));
        assert!(rx.try_recv().is_err());

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "full-access cancelled");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn approval_command_selects_and_applies_mode() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("/approval accept-edits");

        app.submit_input();

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ApprovalModeSelected {
                request_id: 1,
                mode: ApprovalMode::AcceptEdits,
            })
        );
        app.handle_server_event(ServerEvent::ApprovalModeChanged {
            request_id: 1,
            mode: ApprovalMode::AcceptEdits,
        });
        assert_eq!(app.approval_mode, ApprovalMode::AcceptEdits);
        assert_eq!(app.status_message, "approval mode: accept-edits");
    }
    #[test]
    fn connect_command_requests_backend_flow() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(provider_connections_connected_event());
        app.pending_thinking_handoff = Some(super::PendingThinkingHandoff {
            request_id: 1,
            model: model("anthropic", "claude-sonnet-4", "Claude Sonnet 4"),
            activation_succeeded: false,
        });
        app.set_prompt_text("/connect");

        app.submit_input();

        assert!(app.prompt.is_empty());
        assert_eq!(rx.try_recv(), Ok(ClientEvent::ConnectionsRequested));
        assert!(rx.try_recv().is_err());

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "claude-sonnet-4",
            None,
            Some("anthropic"),
            None,
        )));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn connect_command_reports_missing_capability_without_sending() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event_without_capabilities());
        app.set_prompt_text("/connect");

        app.submit_input();

        assert!(app.prompt.is_empty());
        assert_eq!(app.status_message, "provider connections unavailable");
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn connect_command_requires_a_live_connection() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(provider_connections_connected_event());
        app.handle_backend_event(BackendEvent::Disconnected {
            reason: String::from("offline"),
        });
        app.set_prompt_text("/connect");

        app.submit_input();

        assert!(app.prompt.is_empty());
        assert_eq!(app.status_message, "provider connections unavailable");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn list_selectors_support_vim_navigation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.handle_server_event(ServerEvent::AvailableModelsUpdated {
            models: vec![
                model("openai", "gpt-5", "GPT-5"),
                model("google", "gemini", "Gemini"),
            ],
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::AvailableModelsRequested));
        // ModelSelect is a search picker: plain characters (including j/k)
        // feed the query, so navigation is arrows-only there.
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.model_select_index(), 1);
        app.handle_key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.model_select_index(), 0);
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

        app.handle_server_event(ServerEvent::SessionChanged {
            session_id: String::from("sess-2"),
        });
        app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(rx.try_recv(), Ok(ClientEvent::RecentSessionsRequested));
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.session_select_index(), 0);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.status_message, "recent sessions not loaded yet");

        app.handle_server_event(ServerEvent::RecentSessionsUpdated {
            sessions: vec![RecentSession {
                path: String::from("/tmp/session-a.jsonl"),
                id: Some(String::from("session-a")),
                name: Some(String::from("Named session")),
                cwd: Some(String::from("/tmp")),
                modified_unix_ms: Some(1),
                message_count: Some(2),
                first_message: Some(String::from("first prompt")),
            }],
        });
        assert_eq!(app.sessions[1], "/tmp/session-a.jsonl");
        assert_eq!(app.session_labels[1], "Named session (2 messages)");

        app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.thinking_select_index(), 1);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.thinking_select_index(), 0);
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

        app.set_prompt_text("/");
        app.handle_key(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(
            app.slash_completion().map(|(_, selected, _)| selected),
            Some(1)
        );
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(
            app.slash_completion().map(|(_, selected, _)| selected),
            Some(0)
        );

        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-select")),
            title: None,
            prompt: None,
            kind: DialogKind::Select {
                options: vec![
                    yach_proto::DialogOption {
                        label: String::from("Alpha"),
                        value: String::from("alpha"),
                    },
                    yach_proto::DialogOption {
                        label: String::from("Beta"),
                        value: String::from("beta"),
                    },
                ],
            },
        }));
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-select"),
                response: DialogResponse::Selection {
                    value: String::from("beta"),
                },
            })
        );
    }

    #[test]
    fn help_command_opens_readable_overlay() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("/help");

        app.submit_input();
        assert!(matches!(app.mode, AppMode::HelpOverlay));

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn edit_command_requires_backend_capability() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event_without_capabilities());
        app.set_prompt_text("/debug-edit");

        app.submit_input();

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "local edit unavailable");
        assert!(app.prompt.is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn edit_command_opens_composer_when_supported() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(local_edit_connected_event());
        app.set_prompt_text("/debug-edit");

        app.submit_input();

        assert!(matches!(
            app.mode,
            AppMode::LocalEditCompose {
                step: LocalEditComposeStep::Kind,
                draft: LocalEditDraft {
                    kind: None,
                    path: None,
                    ..
                },
            }
        ));
        assert_eq!(app.status_message, "choose edit kind");
        assert!(app.prompt.is_empty());
    }

    #[test]
    fn extension_stop_command_requires_backend_capability() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event_without_capabilities());
        app.set_prompt_text("/extension-stop example.toy-tools");

        app.submit_input();

        assert_eq!(app.status_message, "extension lifecycle unavailable");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn extension_stop_command_emits_lifecycle_request_when_supported() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(extension_lifecycle_connected_event());
        app.set_prompt_text("/extension-stop example.toy-tools");

        app.submit_input();

        assert_eq!(app.status_message, "stopping extension example.toy-tools");
        assert!(app.prompt.is_empty());
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ExtensionLifecycleRequested {
                request_id: String::from("extension-lifecycle-request-0"),
                action: ExtensionLifecycleAction::Stop,
                selector: String::from("example.toy-tools"),
            })
        );
    }

    #[test]
    fn extension_reload_command_emits_lifecycle_request_when_supported() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(extension_lifecycle_connected_event());
        app.set_prompt_text("/extension-reload example.toy-tools");

        app.submit_input();

        assert_eq!(app.status_message, "reloading extension example.toy-tools");
        assert!(app.prompt.is_empty());
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ExtensionLifecycleRequested {
                request_id: String::from("extension-lifecycle-request-0"),
                action: ExtensionLifecycleAction::Reload,
                selector: String::from("example.toy-tools"),
            })
        );
    }

    #[test]
    fn extension_lifecycle_finish_updates_status_for_pending_request() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(extension_lifecycle_connected_event());
        app.pending_extension_lifecycle_request_id =
            Some(String::from("extension-lifecycle-request-0"));

        app.handle_server_event(ServerEvent::ExtensionLifecycleFinished {
            request_id: String::from("extension-lifecycle-request-0"),
            action: ExtensionLifecycleAction::Stop,
            selector: String::from("example.toy-tools"),
            outcome: ExtensionLifecycleOutcome::Completed,
            message: String::from("extension stopped: example.toy-tools"),
        });

        assert_eq!(app.pending_extension_lifecycle_request_id, None);
        assert_eq!(app.status_message, "extension stopped: example.toy-tools");
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ExtensionDiagnosticSnapshotRequested {
                request_id: String::from("extension-diagnostic-request-0"),
                selector: Some(String::from("example.toy-tools")),
            })
        );
        assert_eq!(
            app.pending_extension_diagnostic_request_id,
            Some(String::from("extension-diagnostic-request-0"))
        );
    }

    #[test]
    fn extension_status_command_emits_diagnostic_snapshot_request() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(extension_lifecycle_connected_event());
        app.set_prompt_text("/extension-status example.toy-tools");

        app.submit_input();

        assert_eq!(
            app.status_message,
            "loading extension status example.toy-tools"
        );
        assert!(app.prompt.is_empty());
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ExtensionDiagnosticSnapshotRequested {
                request_id: String::from("extension-diagnostic-request-0"),
                selector: Some(String::from("example.toy-tools")),
            })
        );
    }

    #[test]
    fn extension_diagnostic_snapshot_updates_status_and_transcript() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(extension_lifecycle_connected_event());
        app.pending_extension_diagnostic_request_id =
            Some(String::from("extension-diagnostic-request-0"));

        app.handle_server_event(ServerEvent::ExtensionDiagnosticSnapshotUpdated {
            request_id: String::from("extension-diagnostic-request-0"),
            outcome: ExtensionDiagnosticSnapshotOutcome::Completed,
            records: vec![ExtensionDiagnosticRecord {
                id: Some(String::from("example.toy-tools")),
                version: Some(String::from("0.1.0")),
                scope: String::from("user"),
                package_root: String::from("/tmp/yach-extension"),
                manifest_path: Some(String::from("/tmp/yach-extension/yach.extension.json")),
                source_ref: Some(String::from("test-package-root")),
                install_source: None,
                activation_state: String::from("active"),
                generation: 3,
                last_error_kind: None,
                last_error_summary: None,
                registered_tools: vec![String::from("toy_tool")],
                provider_visible_tools: vec![String::from("toy_tool")],
            }],
            message: None,
        });

        assert_eq!(app.pending_extension_diagnostic_request_id, None);
        assert_eq!(
            app.status_message,
            "extensions: count=1 active=1 stopped=0 failed=0"
        );
        let last_entry = app.transcript.entries().last();
        assert!(last_entry.is_some());
        let Some(last_entry) = last_entry else {
            return;
        };
        assert!(
            last_entry
                .content
                .contains("example.toy-tools state=active")
        );
        assert!(
            last_entry
                .content
                .contains("provider_visible_tools=toy_tool")
        );
    }

    #[test]
    fn local_edit_preview_enters_review_mode() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");

        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        assert!(matches!(
            app.mode,
            AppMode::LocalEditReview {
                preview: LocalEditReview {
                    ref preview_id,
                    ref path,
                    review_state: LocalEditReviewState::NeedsUserApproval,
                    ..
                },
                selected: LocalEditReviewAction::Apply,
            } if preview_id == "preview-1" && path == "src/lib.rs"
        ));
        assert_eq!(app.status_message, "review local edit");
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(
            app.mode,
            AppMode::LocalEditReview {
                selected: LocalEditReviewAction::Reject,
                ..
            }
        ));
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(matches!(
            app.mode,
            AppMode::LocalEditReview {
                selected: LocalEditReviewAction::Apply,
                ..
            }
        ));
    }

    #[test]
    fn tool_review_enters_inline_transcript_row_without_local_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_prompt_text("/m");
        app.mode = AppMode::SlashComplete {
            prefix: String::from("/m"),
            selected: 0,
        };
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });

        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert!(app.transcript.has_pending_review());
        assert_eq!(app.prompt_text(), "/m");
        assert_eq!(
            app.status_message,
            "review pending · ↑/↓ or j/k select · Enter confirm"
        );
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(
            app.transcript
                .entries()
                .last()
                .and_then(|entry| entry.review.as_ref())
                .map(|review| review.selected),
            Some(ToolReviewDecision::Reject)
        );
        app.handle_key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            app.transcript
                .entries()
                .last()
                .and_then(|entry| entry.review.as_ref())
                .map(|review| review.selected),
            Some(ToolReviewDecision::Approve)
        );
        app.handle_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            app.transcript
                .entries()
                .last()
                .and_then(|entry| entry.review.as_ref())
                .map(|review| review.selected),
            Some(ToolReviewDecision::Reject)
        );
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(
            app.transcript
                .entries()
                .last()
                .and_then(|entry| entry.review.as_ref())
                .map(|review| review.selected),
            Some(ToolReviewDecision::Approve)
        );
    }

    #[test]
    fn local_edit_auto_review_unavailable_is_visible() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");

        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::AutoReviewUnavailable),
        });

        assert!(matches!(app.mode, AppMode::LocalEditReview { .. }));
        assert_eq!(
            app.status_message,
            "auto-review unavailable; user approval required"
        );
    }

    #[test]
    fn local_edit_finished_returns_to_normal_mode() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_server_event(ServerEvent::LocalEditFinished {
            preview_id: Some(String::from("preview-1")),
            outcome: LocalEditFinishedOutcome::Applied,
            message: String::from("local edit finished"),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "local edit finished");
    }

    #[test]
    fn local_edit_modify_compose_emits_prepare_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(local_edit_connected_event());
        app.open_local_edit_composer();

        app.handle_key(KeyCode::Char('1'), KeyModifiers::NONE);
        type_chars(&mut app, "src/lib.rs");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        type_chars(&mut app, "abc123");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        type_chars(&mut app, "old");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        type_chars(&mut app, "new");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "preparing local edit");
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::LocalEditPrepareRequested {
                request_id: String::from("local-edit-request-0"),
                operation: LocalEditOperationInput::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: String::from("abc123"),
                    find: String::from("old"),
                    replace: String::from("new"),
                },
            })
        );
    }

    #[test]
    fn local_edit_create_compose_allows_multiline_content() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(local_edit_connected_event());
        app.open_local_edit_composer();

        app.handle_key(KeyCode::Char('2'), KeyModifiers::NONE);
        type_chars(&mut app, "src/new.rs");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        type_chars(&mut app, "one");
        app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        type_chars(&mut app, "two");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::LocalEditPrepareRequested {
                request_id: String::from("local-edit-request-0"),
                operation: LocalEditOperationInput::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("one\ntwo"),
                },
            })
        );
    }

    #[test]
    fn local_edit_preview_ignores_unmatched_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.pending_local_edit_request_id = Some(String::from("local-edit-request-0"));
        app.local_edit_request_counter = 1;

        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("local-edit-request-other"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(
            app.pending_local_edit_request_id.as_deref(),
            Some("local-edit-request-0")
        );
    }

    #[test]
    fn local_edit_preview_ignores_unsolicited_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert!(app.active_local_edit_preview_id.is_none());
    }

    #[test]
    fn local_edit_finished_ignores_unmatched_preview() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_server_event(ServerEvent::LocalEditFinished {
            preview_id: Some(String::from("preview-other")),
            outcome: LocalEditFinishedOutcome::Applied,
            message: String::from("wrong finish"),
        });

        assert!(matches!(app.mode, AppMode::LocalEditReview { .. }));
        assert_eq!(app.status_message, "review local edit");
    }

    #[test]
    fn local_edit_review_emits_decision_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.status_message, "submitting local edit decision");
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::LocalEditDecisionSubmitted {
                preview_id: String::from("preview-1"),
                permission_decision_id: String::from("permission-1"),
                decision: LocalEditDecision::Apply,
            })
        );
    }

    #[test]
    fn local_edit_finish_after_decision_returns_to_normal_mode() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::LocalEditDecisionSubmitted { .. })
        ));

        app.handle_server_event(ServerEvent::LocalEditFinished {
            preview_id: Some(String::from("preview-1")),
            outcome: LocalEditFinishedOutcome::Applied,
            message: String::from("local edit applied"),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "local edit applied");
        assert!(app.active_local_edit_preview_id.is_none());
    }

    #[test]
    fn tool_review_emits_generic_tool_decision_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });

        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.status_message, "review rejection submitted");
        assert!(!app.transcript.has_pending_review());
        assert!(app.transcript.has_unresolved_review());
        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted {
                request_id: String::from("tool-review-request-1"),
                preview_id: String::from("preview-1"),
                permission_decision_id: String::from("permission-1"),
                decision: ToolReviewDecision::Reject,
            })
        );
    }

    #[test]
    fn tool_review_escape_rejects_once() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted {
                request_id: String::from("tool-review-request-1"),
                preview_id: String::from("preview-1"),
                permission_decision_id: String::from("permission-1"),
                decision: ToolReviewDecision::Reject,
            })
        );
        assert_eq!(app.status_message, "review rejection submitted");

        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            app.status_message,
            "review decision submitted; waiting for tool result"
        );
    }

    #[test]
    fn tool_review_submitted_allows_backend_cancel() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(cancellable_native_connected_event());
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted { .. })
        ));

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::PromptCancelled {
                session_id: String::from("default"),
            })
        );
        assert_eq!(app.status_message, "cancelling prompt...");
        app.handle_server_event(ServerEvent::ToolReviewResolved {
            request_id: String::from("tool-review-request-1"),
            resolution: ToolReviewResolution::Interrupted,
        });
        assert_eq!(app.status_message, "review interrupted");
        assert!(matches!(
            app.transcript.entries()[0]
                .review
                .as_ref()
                .map(|review| review.status),
            Some(crate::transcript::ToolReviewRowStatus::Resolved(
                ToolReviewResolution::Interrupted
            ))
        ));
    }

    #[test]
    fn backend_review_resolution_resolves_and_collapses_inline_row() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted { .. })
        ));
        app.handle_server_event(ServerEvent::ToolReviewResolved {
            request_id: String::from("tool-review-request-1"),
            resolution: ToolReviewResolution::Approved,
        });
        assert!(app.transcript.has_unresolved_review());
        app.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.prompt_text().is_empty());

        app.handle_server_event(ServerEvent::ToolCallFinished(ToolResult {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("edit_text_file"),
            output: String::from("updated src/lib.rs"),
            is_error: false,
            outcome_kind: None,
            metadata: None,
        }));

        assert!(!app.transcript.has_unresolved_review());
        let entry = &app.transcript.entries()[0];
        assert!(!entry.expanded);
        assert_eq!(
            entry.review.as_ref().map(|review| review.status),
            Some(crate::transcript::ToolReviewRowStatus::Resolved(
                yach_proto::ToolReviewResolution::Approved
            ))
        );
    }

    #[test]
    fn tool_review_prompt_finish_records_interruption_and_restores_prompt_input() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolCallStarted {
            tool_call_id: Some(String::from("tool-review-request-1")),
            tool_name: String::from("create_text_file"),
            preview: Some(String::from("src/lib.rs")),
        });
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("create_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted { .. })
        ));

        app.handle_server_event(ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome: PromptOutcome::Completed,
            message: Some(String::from("turn_end provider")),
        });

        assert!(!app.transcript.has_unresolved_review());
        app.handle_key(KeyCode::Char('o'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.prompt_text(), "ok");
    }

    #[test]
    fn prompt_paste_inserts_text_as_batch() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_paste("hello\npasted text");

        assert_eq!(app.prompt_text(), "hello\npasted text");
    }

    #[test]
    fn local_edit_review_blocks_duplicate_decisions() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(rx.try_recv().is_ok());

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.status_message, "local edit decision already submitted");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn local_edit_review_modified_accelerators_do_not_submit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        expect_local_edit_preview(&mut app, "request-1");
        app.handle_server_event(ServerEvent::LocalEditPreviewReady {
            request_id: String::from("request-1"),
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        });

        app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn controls_are_blocked_while_backend_busy() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_backend_event(connected_event());
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_start"),
        });

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT);
        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(
            app.status_message,
            "wait for current response before changing model"
        );

        app.handle_key(KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert_eq!(
            app.status_message,
            "wait for current response before forking"
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn dialog_textarea_reuses_cursor_for_unicode_boundaries() {
        let textarea = super::dialog_textarea("🙂é", "🙂".len());

        assert_eq!(textarea.cursor(), (0, 1));
    }

    #[test]
    fn dialog_editor_uses_ctrl_j_newline_and_enter_submit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-editor")),
            title: None,
            prompt: None,
            kind: DialogKind::Editor {
                initial_text: Some(String::from("one")),
            },
        }));

        app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('t'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('w'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('o'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-editor"),
                response: DialogResponse::Text {
                    value: String::from("one\ntwo"),
                },
            })
        );
    }

    #[test]
    fn dialog_input_handles_multibyte_backspace() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-unicode")),
            title: None,
            prompt: None,
            kind: DialogKind::Input { default: None },
        }));

        app.handle_key(KeyCode::Char('🙂'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('é'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-unicode"),
                response: DialogResponse::Text {
                    value: String::from("é")
                },
            }
        );
    }
    #[test]
    fn secret_dialog_masks_unicode_and_never_renders_value() {
        const SECRET: &str = "task3-secret-sentinel-é🙂";

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-render");
        type_chars(&mut app, SECRET);

        let rendered = rendered_active_dialog(&app);
        assert_eq!(
            rendered.chars().filter(|ch| *ch == '•').count(),
            SECRET.chars().count()
        );
        assert!(!rendered.contains(SECRET));
        assert!(
            app.active_dialog.is_some(),
            "secret dialog should be active"
        );
        let Some(dialog) = app.active_dialog.as_ref() else {
            return;
        };
        assert!(!format!("{dialog:?}").contains(SECRET));
    }

    #[test]
    fn secret_dialog_unicode_editing_preserves_character_boundaries() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-unicode");

        type_chars(&mut app, "é🙂a");
        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            3
        );

        app.handle_key(KeyCode::Left, KeyModifiers::NONE);
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            2
        );

        app.handle_key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            1
        );

        app.handle_key(KeyCode::Home, KeyModifiers::NONE);
        app.handle_key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            0
        );

        app.handle_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('界'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Left, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('ß'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Right, KeyModifiers::NONE);
        app.handle_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_key(KeyCode::Char('🙂'), KeyModifiers::NONE);
        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            3
        );

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id,
                response: DialogResponse::Secret { value },
            }) if dialog_id == "dlg-secret-unicode" && !value.is_empty()
        ));
    }
    #[test]
    fn secret_dialog_uses_stable_fixed_backing_and_wipes_vacated_bytes() {
        const MAX_SECRET_BYTES: usize = 8192;

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-backing");
        let allocation = {
            assert!(
                app.active_dialog
                    .as_ref()
                    .and_then(|dialog| dialog.secret_input.as_ref())
                    .is_some(),
                "secret input state should be active"
            );
            let Some(secret) = app
                .active_dialog
                .as_ref()
                .and_then(|dialog| dialog.secret_input.as_ref())
            else {
                return;
            };
            assert_eq!(secret.value.capacity(), MAX_SECRET_BYTES);
            secret.value.as_ptr()
        };

        type_chars(&mut app, "é🙂");
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE);
        app.handle_key(KeyCode::Backspace, KeyModifiers::NONE);

        assert!(
            app.active_dialog
                .as_ref()
                .and_then(|dialog| dialog.secret_input.as_ref())
                .is_some(),
            "secret input state should be active"
        );
        let Some(secret) = app
            .active_dialog
            .as_ref()
            .and_then(|dialog| dialog.secret_input.as_ref())
        else {
            return;
        };
        let backing: &[u8] = secret.value.as_ref();
        assert_eq!(secret.value.as_ptr(), allocation);
        assert_eq!(backing.len(), MAX_SECRET_BYTES);
        assert!(backing.iter().all(|byte| *byte == 0));
    }
    #[test]
    fn secret_input_wipe_zeroizes_the_full_fixed_backing() {
        let mut secret = super::SecretInput::new();
        secret.insert('é');
        secret.insert('🙂');

        secret.wipe();

        assert_eq!(secret.len, 0);
        assert_eq!(secret.cursor_pos, 0);
        assert!(secret.value.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn secret_dialog_rejects_input_that_exceeds_fixed_byte_bound() {
        const MAX_SECRET_BYTES: usize = 8192;

        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-bound");
        for _ in 0..MAX_SECRET_BYTES {
            app.handle_key(KeyCode::Char('a'), KeyModifiers::NONE);
        }
        app.handle_key(KeyCode::Char('é'), KeyModifiers::NONE);

        assert!(
            app.active_dialog
                .as_ref()
                .and_then(|dialog| dialog.secret_input.as_ref())
                .is_some(),
            "secret input state should be active"
        );
        let Some(secret) = app
            .active_dialog
            .as_ref()
            .and_then(|dialog| dialog.secret_input.as_ref())
        else {
            return;
        };
        assert_eq!(secret.masked_value().chars().count(), MAX_SECRET_BYTES);
    }

    #[test]
    fn secret_dialog_submit_and_cancel_clear_state() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        open_secret_dialog(&mut app, "dlg-secret-submit");
        type_chars(&mut app, "task3-secret-sentinel");
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let event = rx.try_recv();
        assert!(!format!("{event:?}").contains("task3-secret-sentinel"));
        assert!(matches!(
            event,
            Ok(ClientEvent::DialogResolved {
                dialog_id,
                response: DialogResponse::Secret { value },
            }) if dialog_id == "dlg-secret-submit" && !value.is_empty()
        ));
        assert!(app.active_dialog.is_none());
        assert!(matches!(app.mode, AppMode::Normal));

        open_secret_dialog(&mut app, "dlg-secret-cancel");
        type_chars(&mut app, "task3-secret-sentinel");
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-secret-cancel"),
                response: DialogResponse::Cancelled,
            })
        );
        assert!(app.active_dialog.is_none());
        assert!(matches!(app.mode, AppMode::Normal));

        open_secret_dialog(&mut app, "dlg-secret-replaced");
        type_chars(&mut app, "task3-secret-sentinel");
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-confirm")),
            title: None,
            prompt: None,
            kind: DialogKind::Confirm,
        }));
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-secret-replaced"),
                response: DialogResponse::Cancelled,
            })
        );
        assert!(matches!(app.mode, AppMode::DialogConfirm));
        assert!(matches!(
            app.active_dialog
                .as_ref()
                .map(|dialog| &dialog.request.kind),
            Some(DialogKind::Confirm)
        ));
        assert!(
            app.active_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.secret_input.is_none())
        );
    }

    #[test]
    fn secret_dialog_paste_inserts_batch_and_never_renders_value() {
        const PASTED: &str = "sk-test-paste-sentinel";

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-paste");

        app.handle_paste(PASTED);

        let rendered = rendered_active_dialog(&app);
        assert!(!rendered.contains(PASTED));
        assert_eq!(
            rendered.chars().filter(|ch| *ch == '•').count(),
            PASTED.chars().count()
        );
        assert!(app.prompt_text().is_empty());

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let event = rx.try_recv();
        assert!(!format!("{event:?}").contains(PASTED));
        assert!(matches!(
            event,
            Ok(ClientEvent::DialogResolved {
                dialog_id,
                response: DialogResponse::Secret { value },
            }) if dialog_id == "dlg-secret-paste" && !value.is_empty()
        ));
    }

    #[test]
    fn secret_dialog_paste_strips_line_breaks() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        open_secret_dialog(&mut app, "dlg-secret-paste-breaks");

        app.handle_paste("sk-abc\r\ndef\nghi\r");

        assert_eq!(
            rendered_active_dialog(&app)
                .chars()
                .filter(|ch| *ch == '•')
                .count(),
            "sk-abcdefghi".chars().count()
        );
    }

    #[test]
    fn dialog_input_paste_inserts_at_cursor_with_normalized_newlines() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-input-paste")),
            title: None,
            prompt: None,
            kind: DialogKind::Input { default: None },
        }));

        type_chars(&mut app, "ab");
        app.handle_key(KeyCode::Left, KeyModifiers::NONE);
        app.handle_paste("X\r\nY");

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-input-paste"),
                response: DialogResponse::Text {
                    value: String::from("aX\nYb"),
                },
            })
        );
    }

    #[test]
    fn dialogs_are_queued_fifo() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-1")),
            title: Some(String::from("First")),
            prompt: None,
            kind: DialogKind::Confirm,
        }));
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-2")),
            title: Some(String::from("Second")),
            prompt: None,
            kind: DialogKind::Confirm,
        }));

        assert_eq!(app.queued_dialogs.len(), 1);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);
        let _ = rx.try_recv();

        assert!(matches!(app.mode, AppMode::DialogConfirm));
        let Some(dialog) = app.active_dialog.as_ref() else {
            return;
        };
        assert_eq!(dialog.request.id.as_deref(), Some("dlg-2"));
    }

    #[test]
    fn dialog_overflow_sends_cancellation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-active")),
            title: Some(String::from("Active")),
            prompt: None,
            kind: DialogKind::Confirm,
        }));
        for idx in 0..super::MAX_QUEUED_DIALOGS {
            app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
                id: Some(format!("dlg-{idx}")),
                title: Some(format!("Queued {idx}")),
                prompt: None,
                kind: DialogKind::Confirm,
            }));
        }

        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: Some(String::from("dlg-overflow")),
            title: Some(String::from("Overflow")),
            prompt: None,
            kind: DialogKind::Confirm,
        }));

        assert_eq!(app.status_message, "dialog queue full");
        assert_eq!(app.queued_dialogs.len(), super::MAX_QUEUED_DIALOGS);
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::from("dlg-overflow"),
                response: DialogResponse::Cancelled,
            })
        );
    }

    #[test]
    fn idless_dialog_preserves_protocol_compatibility() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::DialogRequested(DialogRequest {
            id: None,
            title: Some(String::from("Legacy")),
            prompt: None,
            kind: DialogKind::Confirm,
        }));

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::DialogResolved {
                dialog_id: String::new(),
                response: DialogResponse::Confirmed { accepted: true },
            })
        );
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn status_command_reports_hidden_session_and_runtime_details() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.session_id = String::from("session-owner-dogfood");
        app.model = String::from("gpt-5.6-sol");
        app.model_connection_id = Some(String::from("chatgpt"));
        app.thinking_level = ThinkingLevel::High;
        app.is_connected = true;
        app.handle_server_event(ServerEvent::SessionStatsUpdated(SessionStats {
            message_count: Some(12),
            user_message_count: Some(3),
            assistant_message_count: Some(4),
            tool_message_count: Some(5),
            total_tokens: None,
            context_window: Some(200_000),
            context_used_percent: Some(42),
        }));

        app.set_prompt_text("/status");
        app.submit_input();

        let status = app.transcript.entries().last();
        assert!(matches!(
            status.map(|entry| &entry.kind),
            Some(EntryKind::Status)
        ));
        assert_eq!(
            status.map(|entry| entry.content.as_str()),
            Some(
                "Session status\n\
                 session: session-owner-dogfood\n\
                 model: gpt-5.6-sol\n\
                 thinking: high\n\
                 connection: chatgpt\n\
                 approval: review\n\
                 context: ctx:42%/200k\n\
                 messages: 12 (user 3, assistant 4, tool 5)\n\
                 compactions: 0"
            )
        );
        assert!(app.prompt_text().is_empty());
    }

    #[test]
    fn model_change_never_retains_the_previous_models_context_capacity() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.model = String::from("model-a");
        app.handle_server_event(ServerEvent::SessionStatsUpdated(SessionStats {
            message_count: None,
            user_message_count: None,
            assistant_message_count: None,
            tool_message_count: None,
            total_tokens: None,
            context_window: Some(120_000),
            context_used_percent: Some(42),
        }));

        app.handle_server_event(ServerEvent::ModelChanged(model_change(
            "model-b", None, None, None,
        )));

        assert_eq!(app.model, "model-b");
        assert_eq!(app.context_used_percent, None);
        assert!(matches!(
            app.session_stats.as_ref(),
            Some(SessionStats {
                context_window: None,
                context_used_percent: None,
                ..
            })
        ));

        app.handle_server_event(ServerEvent::SessionStatsUpdated(SessionStats {
            message_count: None,
            user_message_count: None,
            assistant_message_count: None,
            tool_message_count: None,
            total_tokens: None,
            context_window: Some(240_000),
            context_used_percent: Some(21),
        }));
        assert_eq!(app.context_used_percent, Some(21));
        assert_eq!(
            app.session_stats
                .as_ref()
                .and_then(|stats| stats.context_window),
            Some(240_000)
        );
    }

    #[test]
    fn slash_prefixes_do_not_execute_commands() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.transcript.append_user_message("keep me");
        app.set_prompt_text("/clearance");

        app.submit_input();

        assert_eq!(app.transcript.entries().len(), 2);
        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: String::from("/clearance"),
            }
        );
    }

    #[test]
    fn transcript_resize_preserves_bottom_stickiness() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_transcript_viewport(20, 3);
        for idx in 0..10 {
            app.transcript
                .append_user_message(&format!("message {idx}"));
        }
        app.scroll_to_bottom();

        app.set_transcript_viewport(20, 1);

        assert!(app.at_transcript_bottom());
    }

    #[test]
    fn transcript_scroll_keys_adjust_offset() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_transcript_viewport(20, 3);
        for idx in 0..10 {
            app.transcript
                .append_user_message(&format!("message {idx}"));
        }
        app.scroll_to_bottom();
        let bottom = app.scroll_offset;

        app.handle_key(KeyCode::PageUp, KeyModifiers::NONE);
        assert!(app.scroll_offset < bottom);

        app.handle_key(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(app.scroll_offset, bottom);
    }

    #[test]
    fn prompt_editing_preserves_scrolled_transcript_position_until_submit() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_transcript_viewport(20, 3);
        for idx in 0..10 {
            app.transcript
                .append_user_message(&format!("message {idx}"));
        }
        app.scroll_to_bottom();
        app.handle_key(KeyCode::PageUp, KeyModifiers::NONE);
        let reading_offset = app.scroll_offset;

        app.handle_key(KeyCode::Char('x'), KeyModifiers::NONE);

        assert_eq!(app.prompt_text(), "x");
        assert_eq!(app.scroll_offset, reading_offset);

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert!(app.at_transcript_bottom());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::PromptSubmitted { prompt, .. }) if prompt == "x"
        ));
    }

    #[test]
    fn failed_prompt_appends_visible_harness_outcome_entry() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome: PromptOutcome::Failed,
            message: Some(String::from("provider_error kind=invalid_request")),
        });

        assert_eq!(app.transcript.entries().len(), 1);
        assert!(matches!(
            app.transcript.entries()[0].kind,
            EntryKind::HarnessOutcome {
                kind: HarnessOutcomeKind::Failed
            }
        ));
        assert!(
            app.transcript.entries()[0]
                .content
                .contains("invalid_request")
        );
    }

    #[test]
    fn mouse_wheel_scrolls_transcript_by_lines() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.set_transcript_viewport(20, 3);
        for idx in 0..10 {
            app.transcript
                .append_user_message(&format!("message {idx}"));
        }
        app.scroll_to_bottom();
        let bottom = app.scroll_offset;

        app.handle_mouse(crossterm::event::MouseEventKind::ScrollUp);
        assert_eq!(app.scroll_offset, bottom.saturating_sub(3));

        app.handle_mouse(crossterm::event::MouseEventKind::ScrollDown);
        assert_eq!(app.scroll_offset, bottom);

        // Scrolling below the bottom clamps.
        app.handle_mouse(crossterm::event::MouseEventKind::ScrollDown);
        assert_eq!(app.scroll_offset, bottom);
    }

    #[test]
    fn submitting_next_turn_archives_prior_transcript_for_terminal_scrollback() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.transcript.append_user_message("first question");
        app.transcript.append_delta("first answer");
        app.set_prompt_text("second question");

        app.submit_input();

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientEvent::PromptSubmitted { prompt, .. }) if prompt == "second question"
        ));
        let lines = app.take_scrollback_lines(80);
        let archived = lines
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(archived.contains("first question"));
        assert!(archived.contains("first answer"));
        assert_eq!(app.transcript.entries().len(), 1);
        assert!(matches!(
            app.transcript.entries()[0].kind,
            EntryKind::UserMessage
        ));
    }

    #[test]
    fn tool_output_summary_stays_compact() {
        assert_eq!(
            tool_output_summary("", false, None, None),
            "completed with no output"
        );
        assert_eq!(
            tool_output_summary("one\ntwo\n", false, None, None),
            "completed: 2 lines, 8 bytes"
        );
    }

    #[test]
    fn tool_output_summary_preserves_legacy_display_output_without_metadata() {
        let output = "completed:\nsrc/lib.rs:2: needle evidence line";
        assert_eq!(tool_output_summary(output, false, None, None), output);
    }

    #[test]
    fn tool_output_summary_uses_structured_metadata_and_outcome() {
        let metadata = ToolResultMetadata {
            byte_count: 2_048,
            truncated: true,
            reason: Some(String::from("user_rejected")),
        };
        assert_eq!(
            tool_output_summary(
                "permission denied",
                true,
                Some(&metadata),
                Some(HarnessOutcomeKind::Denied),
            ),
            "denied: 1 line, 2048 bytes, truncated; permission denied"
        );
    }

    #[test]
    fn failed_tool_output_summary_includes_bounded_error_excerpt() {
        assert_eq!(
            tool_output_summary("one\ntwo\n", true, None, None),
            "failed: 2 lines, 8 bytes; one"
        );
        let long_error = "a".repeat(MAX_TOOL_ERROR_EXCERPT_CHARS + 1);
        assert_eq!(
            tool_output_summary(&long_error, true, None, None),
            format!(
                "failed: 1 line, {} bytes; {}...",
                MAX_TOOL_ERROR_EXCERPT_CHARS + 1,
                "a".repeat(MAX_TOOL_ERROR_EXCERPT_CHARS)
            )
        );
    }
}
