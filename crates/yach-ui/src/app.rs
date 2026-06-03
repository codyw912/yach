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
    BackendEvent, BackendState, Capability, ClientEvent, DialogKind, DialogRequest, DialogResponse,
    ExtensionLifecycleAction, ExtensionLifecycleOutcome, ForkMessage, ForkPosition,
    LocalEditDecision, LocalEditOperationInput, LocalEditReviewState, ModelInfo,
    NegotiatedCapabilities, RecentSession, ServerEvent, ToolReviewPayload,
};

use crate::layout;
use crate::lifecycle::{StatusLifecycle, is_lifecycle_status, status_lifecycle};
use crate::perf_metrics::PerfMetrics;
use crate::session_tree::{SessionTree, branch_summary_line, build_session_tree};
use crate::slash_commands::{
    SlashAction, SlashCommand, SlashParseResult, match_slash_commands, parse_slash_command,
};
use crate::thinking_level::ThinkingLevel;
use crate::transcript::{self, Transcript, TranscriptRenderCache};

#[derive(Debug, Clone)]
pub struct StartupTrace {
    path: PathBuf,
    start: Instant,
    marks: Arc<Mutex<Vec<StartupTraceMark>>>,
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

fn tool_output_summary(output: &str, is_error: bool) -> String {
    let status = if is_error { "failed" } else { "completed" };
    if output.is_empty() {
        return format!("{status} with no output");
    }

    let line_count = output.lines().count().max(1);
    let byte_count = output.len();
    let line_label = if line_count == 1 { "line" } else { "lines" };
    format!("{status}: {line_count} {line_label}, {byte_count} bytes")
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

fn state_model_label(state: &BackendState) -> Option<String> {
    state
        .model_name
        .clone()
        .or_else(|| match (&state.model_provider, &state.model_id) {
            (Some(provider), Some(id)) => Some(format!("{provider}/{id}")),
            _ => state.model_id.clone(),
        })
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
    HelpOverlay,
    DialogConfirm,
    DialogInput,
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

#[derive(Debug, Clone)]
struct PendingDialog {
    request: DialogRequest,
    input_buffer: String,
    cursor_pos: usize,
    selected: usize,
    confirm_accepted: bool,
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
const DEFAULT_TRANSCRIPT_VIEW_WIDTH: u16 = 80;
const DEFAULT_TRANSCRIPT_VIEW_HEIGHT: u16 = 20;

pub struct App {
    transcript: Transcript,
    transcript_cache: TranscriptRenderCache,
    scroll_offset: usize,
    prompt: TextArea<'static>,
    active_tools: Vec<ActiveTool>,
    model: String,
    available_models: Vec<ModelInfo>,
    session_id: String,
    status_message: String,
    is_connected: bool,
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
    pending_model: Option<String>,
    pending_session_id: Option<String>,
    pending_thinking_level: Option<ThinkingLevel>,
    perf_metrics: PerfMetrics,
    negotiated: Option<NegotiatedCapabilities>,
    active_dialog: Option<PendingDialog>,
    queued_dialogs: VecDeque<PendingDialog>,
    transcript_view_width: u16,
    transcript_view_height: u16,
    local_edit_request_counter: u64,
    extension_lifecycle_request_counter: u64,
    pending_local_edit_request_id: Option<String>,
    pending_extension_lifecycle_request_id: Option<String>,
    active_local_edit_preview_id: Option<String>,
    pending_tool_review_request_id: Option<String>,
    active_tool_review_preview_id: Option<String>,
    submitted_tool_review_preview_id: Option<String>,
    local_edit_decision_submission: LocalEditDecisionSubmission,
    client_tx: mpsc::UnboundedSender<ClientEvent>,
}

impl App {
    fn new(client_tx: mpsc::UnboundedSender<ClientEvent>) -> Self {
        Self {
            transcript: Transcript::new(),
            transcript_cache: TranscriptRenderCache::new(),
            scroll_offset: 0,
            prompt: TextArea::default(),
            active_tools: Vec::new(),
            model: String::from("default"),
            available_models: Vec::new(),
            session_id: String::from("default"),
            status_message: String::from("connecting..."),
            is_connected: false,
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
            pending_model: None,
            pending_session_id: None,
            pending_thinking_level: None,
            perf_metrics: PerfMetrics::new(),
            negotiated: None,
            active_dialog: None,
            queued_dialogs: VecDeque::new(),
            transcript_view_width: DEFAULT_TRANSCRIPT_VIEW_WIDTH,
            transcript_view_height: DEFAULT_TRANSCRIPT_VIEW_HEIGHT,
            local_edit_request_counter: 0,
            extension_lifecycle_request_counter: 0,
            pending_local_edit_request_id: None,
            pending_extension_lifecycle_request_id: None,
            active_local_edit_preview_id: None,
            pending_tool_review_request_id: None,
            active_tool_review_preview_id: None,
            submitted_tool_review_preview_id: None,
            local_edit_decision_submission: LocalEditDecisionSubmission::Idle,
            client_tx,
        }
    }

    fn set_stream_state(&mut self, stream_state: StreamState) {
        self.is_streaming = stream_state.is_display_streaming();
        self.stream_state = stream_state;
        if matches!(self.stream_state, StreamState::Idle) {
            self.apply_pending_backend_state();
        }
    }

    fn apply_pending_backend_state(&mut self) {
        if let Some(session_id) = self.pending_session_id.take() {
            self.session_id = session_id;
        }
        if let Some(model) = self.pending_model.take() {
            self.model = model;
        }
        if let Some(level) = self.pending_thinking_level.take() {
            self.thinking_level = level;
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

    fn request_available_models(&mut self) {
        self.send_client_event(ClientEvent::AvailableModelsRequested);
    }

    fn send_client_event(&mut self, event: ClientEvent) -> bool {
        if self.client_tx.send(event).is_ok() {
            true
        } else {
            self.is_connected = false;
            self.set_stream_state(StreamState::Idle);
            self.status_message = String::from("backend disconnected");
            false
        }
    }

    fn handle_backend_event(&mut self, event: BackendEvent) {
        match event {
            BackendEvent::Connected { negotiated } => {
                self.is_connected = true;
                self.negotiated = Some(negotiated.clone());
                self.status_message = format!("connected: {}", negotiated.adapter_agent_name);
            }
            BackendEvent::Server(event) => self.handle_server_event(event),
            BackendEvent::Disconnected { reason } => {
                self.is_connected = false;
                self.set_stream_state(StreamState::Idle);
                self.pending_model = None;
                self.pending_session_id = None;
                self.pending_thinking_level = None;
                self.pending_local_edit_request_id = None;
                self.pending_extension_lifecycle_request_id = None;
                self.active_local_edit_preview_id = None;
                self.pending_tool_review_request_id = None;
                self.active_tool_review_preview_id = None;
                self.submitted_tool_review_preview_id = None;
                self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
                self.active_tools.clear();
                self.active_dialog = None;
                self.queued_dialogs.clear();
                self.mode = AppMode::Normal;
                self.status_message = if reason.is_empty() {
                    String::from("disconnected")
                } else {
                    reason
                };
            }
        }
    }

    fn handle_server_event(&mut self, event: ServerEvent) {
        match event {
            ServerEvent::Ready { .. } => {}
            ServerEvent::StateUpdated(state) => self.apply_backend_state(state),
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
                self.set_stream_state(StreamState::Idle);
                self.active_tools.clear();
                self.clear_tool_review_state();
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
            ServerEvent::ToolCallFinished(result) => {
                if matches!(self.stream_state, StreamState::LocallyCancelled { .. }) {
                    return;
                }
                let active_tool =
                    self.take_active_tool(result.tool_call_id.as_deref(), &result.tool_name);
                let label = active_tool
                    .as_ref()
                    .map_or_else(|| result.tool_name.clone(), ActiveTool::label);
                let summary = tool_output_summary(&result.output, result.is_error);
                if !self.transcript.finish_tool_call(
                    result.tool_call_id.as_deref(),
                    &result.tool_name,
                    &label,
                    &summary,
                    result.is_error,
                ) {
                    self.transcript.append_tool_result(
                        result.tool_call_id.as_deref(),
                        &label,
                        &summary,
                        result.is_error,
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
            ServerEvent::ModelChanged { model } => {
                if self.backend_busy() {
                    self.pending_model = Some(model.clone());
                    self.status_message = format!("model pending: {model}");
                } else {
                    self.model = model;
                }
            }
            ServerEvent::AvailableModelsUpdated { models } => {
                self.available_models = models;
                if self.available_models.is_empty() {
                    self.status_message = String::from("no available models reported");
                }
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
                self.status_message = branch_summary_line(&tree);
                self.session_tree = Some(tree);
            }
            ServerEvent::SessionStatsUpdated(stats) => {
                self.status_message = stats.message_count.map_or_else(
                    || String::from("session stats loaded"),
                    |count| format!("session messages: {count}"),
                );
            }
            ServerEvent::RecentSessionsUpdated { sessions } => {
                self.apply_recent_sessions(sessions);
            }
            ServerEvent::DialogRequested(request) => self.open_dialog(request),
            ServerEvent::ToolReviewRequested {
                request_id,
                tool_name: _,
                payload: ToolReviewPayload::LocalEdit { preview },
            } => {
                let status_message = local_edit_review_status_message(preview.review_state);
                self.pending_local_edit_request_id = None;
                self.active_local_edit_preview_id = None;
                self.pending_tool_review_request_id = Some(request_id);
                self.active_tool_review_preview_id = Some(preview.preview_id.clone());
                self.submitted_tool_review_preview_id = None;
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
                self.pending_tool_review_request_id = None;
                self.active_tool_review_preview_id = None;
                self.submitted_tool_review_preview_id = None;
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
                self.pending_tool_review_request_id = None;
                self.active_tool_review_preview_id = None;
                self.submitted_tool_review_preview_id = None;
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
                ..
            } => {
                if self.pending_extension_lifecycle_request_id.as_deref() != Some(&request_id) {
                    return;
                }
                self.pending_extension_lifecycle_request_id = None;
                self.status_message = if message.is_empty() {
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

    fn has_local_edit_in_flight(&self) -> bool {
        self.pending_local_edit_request_id.is_some()
            || self.active_local_edit_preview_id.is_some()
            || self.pending_tool_review_request_id.is_some()
            || self.active_tool_review_preview_id.is_some()
            || self.submitted_tool_review_preview_id.is_some()
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
            Some(preview_id) => {
                self.active_local_edit_preview_id.as_deref() == Some(preview_id)
                    || self.active_tool_review_preview_id.as_deref() == Some(preview_id)
                    || self.submitted_tool_review_preview_id.as_deref() == Some(preview_id)
            }
            None => {
                self.pending_local_edit_request_id.is_some()
                    || self.pending_tool_review_request_id.is_some()
            }
        }
    }

    fn apply_backend_state(&mut self, state: BackendState) {
        let busy = self.backend_busy();
        if let Some(model) = state_model_label(&state) {
            if busy {
                self.pending_model = Some(model);
            } else {
                self.model = model;
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

        if let Some(level) = state.thinking_level
            && let Some(level) = ThinkingLevel::from_str(&level)
        {
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
        if self.active_dialog.is_some() {
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
            DialogKind::Confirm => PendingDialog {
                request,
                input_buffer: String::new(),
                cursor_pos: 0,
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
                    selected: 0,
                    confirm_accepted: false,
                }
            }
            DialogKind::Select { .. } => PendingDialog {
                request,
                input_buffer: String::new(),
                cursor_pos: 0,
                selected: 0,
                confirm_accepted: false,
            },
        }
    }

    fn activate_dialog(&mut self, pending: PendingDialog) {
        self.status_message = dialog_summary(&pending.request);
        self.mode = match &pending.request.kind {
            DialogKind::Confirm => AppMode::DialogConfirm,
            DialogKind::Input { .. } | DialogKind::Editor { .. } => AppMode::DialogInput,
            DialogKind::Select { .. } => AppMode::DialogSelect,
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
        match &self.mode {
            AppMode::Normal => self.handle_normal_key(key, modifiers),
            AppMode::SlashComplete { .. } => self.handle_slash_complete_key(key, modifiers),
            AppMode::ModelSelect { .. } => self.handle_model_select_key(key, modifiers),
            AppMode::SessionSelect { .. } => self.handle_session_select_key(key, modifiers),
            AppMode::ForkSelect { .. } => self.handle_fork_select_key(key, modifiers),
            AppMode::ThinkingSelect { .. } => self.handle_thinking_select_key(key, modifiers),
            AppMode::HelpOverlay => self.handle_help_overlay_key(key, modifiers),
            AppMode::DialogConfirm => self.handle_dialog_confirm_key(key, modifiers),
            AppMode::DialogInput => self.handle_dialog_input_key(key, modifiers),
            AppMode::DialogSelect => self.handle_dialog_select_key(key, modifiers),
            AppMode::PerfOverlay => self.handle_perf_overlay_key(key, modifiers),
            AppMode::LocalEditCompose { .. } => self.handle_local_edit_compose_key(key, modifiers),
            AppMode::LocalEditReview { .. } => self.handle_local_edit_review_key(key, modifiers),
        }
    }

    fn handle_normal_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match (key, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if matches!(self.stream_state, StreamState::Streaming { .. }) {
                    let session_id = self.session_id.clone();
                    self.set_stream_state(StreamState::LocallyCancelled {
                        session_id: session_id.clone(),
                    });
                    self.active_tools.clear();
                    if self.supports_backend_cancel() {
                        let _sent =
                            self.send_client_event(ClientEvent::PromptCancelled { session_id });
                        self.status_message = String::from("cancelling prompt...");
                    } else {
                        self.status_message =
                            String::from("cancelled locally; waiting for backend");
                    }
                } else {
                    self.should_quit = true;
                }
            }
            (KeyCode::PageUp, _) => {
                self.scroll_transcript_up();
            }
            (KeyCode::PageDown, _) => {
                self.scroll_transcript_down();
            }
            (KeyCode::End, modifiers) if modifiers.is_empty() => {
                self.scroll_to_bottom();
            }
            (KeyCode::Char('m'), modifiers)
                if modifiers.contains(KeyModifiers::ALT)
                    || modifiers.contains(KeyModifiers::META)
                    || modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.open_model_selector();
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                self.open_session_selector();
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.request_session_tree();
            }
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                self.open_thinking_selector();
            }
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::PerfOverlay;
            }
            (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.fork_current_session();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                self.clear_input();
            }
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.insert_input_newline();
            }
            (KeyCode::Enter, modifiers)
                if modifiers.contains(KeyModifiers::SHIFT)
                    || (modifiers.contains(KeyModifiers::CONTROL) && self.prompt_has_text()) =>
            {
                self.insert_input_newline();
            }
            (KeyCode::Enter, _) if self.prompt_has_text() && !self.backend_busy() => {
                self.submit_input();
            }
            (KeyCode::Enter, _) if self.prompt_has_text() => {
                self.status_message = String::from("wait for current response before submitting");
            }
            (KeyCode::Enter, _) if !self.prompt.is_empty() => {
                self.clear_input();
            }
            (KeyCode::Backspace, modifiers) if clears_input(modifiers) => {
                self.clear_input();
            }
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

    fn handle_prompt_input_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        if modifiers.contains(KeyModifiers::SUPER) || modifiers.contains(KeyModifiers::HYPER) {
            return;
        }

        self.prompt.input(textarea_input(key, modifiers));
    }

    fn handle_paste(&mut self, text: &str) {
        if !matches!(self.mode, AppMode::Normal | AppMode::SlashComplete { .. }) {
            return;
        }

        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        self.prompt.insert_str(&normalized);
        self.refresh_slash_completion(0);
    }

    fn clear_tool_review_state(&mut self) {
        let had_tool_review = self.pending_tool_review_request_id.is_some()
            || self.active_tool_review_preview_id.is_some()
            || self.submitted_tool_review_preview_id.is_some();
        self.pending_tool_review_request_id = None;
        self.active_tool_review_preview_id = None;
        self.submitted_tool_review_preview_id = None;
        if had_tool_review {
            self.local_edit_decision_submission = LocalEditDecisionSubmission::Idle;
            if matches!(self.mode, AppMode::LocalEditReview { .. })
                && self.active_local_edit_preview_id.is_none()
            {
                self.mode = AppMode::Normal;
            }
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
            if self.available_models.is_empty() {
                self.request_available_models();
                self.status_message = String::from("loading available models");
            }
            self.mode = AppMode::ModelSelect { selected: 0 };
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
            self.mode = AppMode::ThinkingSelect { selected: 0 };
        }
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

    fn submit_extension_stop(&mut self, selector: &str) {
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
            action: ExtensionLifecycleAction::Stop,
            selector: selector.to_string(),
        }) {
            self.pending_extension_lifecycle_request_id = Some(request_id);
            self.clear_input();
            self.status_message = format!("stopping extension {selector}");
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
        let AppMode::ModelSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (key, modifiers) if is_selection_up_key(key, modifiers) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::ModelSelect { selected };
            }
            (key, modifiers) if is_selection_down_key(key, modifiers) => {
                selected = (selected + 1).min(self.available_models.len().saturating_sub(1));
                self.mode = AppMode::ModelSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if self.backend_busy() {
                    self.status_message =
                        String::from("wait for current response before changing model");
                } else if self.available_models.is_empty() {
                    self.status_message = String::from("available models not loaded yet");
                } else if let Some(model) = self.available_models.get(selected).cloned()
                    && self.send_client_event(ClientEvent::ModelSelectedDetailed {
                        provider: model.provider.clone(),
                        model_id: model.id.clone(),
                    })
                {
                    self.status_message = format!("model requested: {}", model.label());
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.mode = AppMode::Normal;
            }
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
                } else if let Some(session_path) = self.sessions.get(selected).cloned()
                    && self.send_client_event(ClientEvent::SessionPathSelected {
                        session_path: session_path.clone(),
                    })
                {
                    self.status_message = format!("switching session: {session_path}");
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
                    && self.send_client_event(ClientEvent::ThinkingLevelSelected {
                        level: level.as_str().to_string(),
                    })
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

    fn handle_dialog_confirm_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        let Some(dialog) = self.active_dialog.as_mut() else {
            self.mode = AppMode::Normal;
            return;
        };

        let mut response = None;
        let mut cancelled = false;

        match key {
            KeyCode::Esc => cancelled = true,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                dialog.confirm_accepted = !dialog.confirm_accepted;
            }
            KeyCode::Char('y' | 'Y') => {
                dialog.confirm_accepted = true;
            }
            KeyCode::Char('n' | 'N') => {
                dialog.confirm_accepted = false;
            }
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
            self.pending_tool_review_request_id = None;
            self.active_tool_review_preview_id = None;
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
            (KeyCode::Left | KeyCode::Right | KeyCode::Tab, KeyModifiers::NONE) => {
                let selected = match selected {
                    LocalEditReviewAction::Apply => LocalEditReviewAction::Reject,
                    LocalEditReviewAction::Reject => LocalEditReviewAction::Apply,
                };
                self.mode = AppMode::LocalEditReview { preview, selected };
            }
            _ => {}
        }
    }

    fn submit_local_edit_review(&mut self, decision: LocalEditDecision) {
        let AppMode::LocalEditReview { preview, .. } = self.mode.clone() else {
            return;
        };

        let preview_id = preview.preview_id.clone();
        let is_tool_review =
            self.active_tool_review_preview_id.as_deref() == Some(preview_id.as_str());
        let submitted = if is_tool_review {
            let Some(request_id) = self.pending_tool_review_request_id.clone() else {
                return;
            };
            self.send_client_event(ClientEvent::ToolReviewDecisionSubmitted {
                request_id,
                preview_id: preview_id.clone(),
                permission_decision_id: preview.permission_decision_id,
                decision,
            })
        } else {
            self.send_client_event(ClientEvent::LocalEditDecisionSubmitted {
                preview_id: preview_id.clone(),
                permission_decision_id: preview.permission_decision_id,
                decision,
            })
        };

        if submitted {
            self.pending_local_edit_request_id = None;
            self.pending_tool_review_request_id = None;
            self.local_edit_decision_submission = LocalEditDecisionSubmission::Submitted;
            if is_tool_review {
                self.submitted_tool_review_preview_id = Some(preview_id);
                self.active_tool_review_preview_id = None;
                if self.active_local_edit_preview_id.is_none() {
                    self.mode = AppMode::Normal;
                }
            }
            self.status_message = String::from("submitting local edit decision");
        }
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
                return;
            }
            SlashParseResult::Command(SlashAction::Model) => {
                self.clear_input();
                self.open_model_selector();
                return;
            }
            SlashParseResult::Command(SlashAction::Session) => {
                self.clear_input();
                self.open_session_selector();
                return;
            }
            SlashParseResult::Command(SlashAction::Thinking) => {
                self.clear_input();
                self.open_thinking_selector();
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
            SlashParseResult::Command(SlashAction::ExtensionStop) => {
                self.clear_input();
                self.status_message = String::from("extension selector required");
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
                self.submit_extension_stop(&args);
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
            self.transcript.append_user_message(&input);
            self.scroll_to_bottom();
            self.status_message = String::from("sending...");
            self.set_stream_state(StreamState::Streaming {
                session_id: self.session_id.clone(),
            });
        }
    }

    fn request_session_tree(&mut self) {
        if self.backend_busy() {
            self.status_message = String::from("wait for current response before loading branches");
        } else if self.send_client_event(ClientEvent::SessionMessagesRequested) {
            self.status_message = String::from("loading session tree");
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

    fn mode(&self) -> &AppMode {
        &self.mode
    }

    fn model_select_index(&self) -> usize {
        if let AppMode::ModelSelect { selected } = &self.mode {
            *selected
        } else {
            0
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
    const ALTERNATE_SCREEN: u8 = 1 << 1;
    const CURSOR_HIDDEN: u8 = 1 << 2;
    const BRACKETED_PASTE: u8 = 1 << 3;
    const RESTORED: u8 = 1 << 4;

    fn new() -> Self {
        Self { flags: 0 }
    }

    fn mark_raw_mode(&mut self) {
        self.flags |= Self::RAW_MODE;
    }

    fn mark_alternate_screen(&mut self) {
        self.flags |= Self::ALTERNATE_SCREEN;
    }

    fn mark_cursor_hidden(&mut self) {
        self.flags |= Self::CURSOR_HIDDEN;
    }

    fn mark_bracketed_paste(&mut self) {
        self.flags |= Self::BRACKETED_PASTE;
    }

    fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    fn restore(&mut self) -> io::Result<()> {
        use crossterm::ExecutableCommand;
        use crossterm::cursor::Show;
        use crossterm::event::DisableBracketedPaste;
        use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

        if self.has_flag(Self::RESTORED) {
            return Ok(());
        }
        self.flags |= Self::RESTORED;

        let mut first_error = None;
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
        if self.has_flag(Self::ALTERNATE_SCREEN)
            && let Err(error) = io::stdout().execute(LeaveAlternateScreen)
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
        let input_snapshot = self.app.prompt.clone();
        let (viewport_width, viewport_height) =
            layout::transcript_viewport_size(area.into(), &input_snapshot);
        self.app
            .set_transcript_viewport(viewport_width, viewport_height);

        let tools: Vec<String> = self
            .app
            .active_tools
            .iter()
            .map(ActiveTool::label)
            .collect();
        let render_params = layout::RenderParams {
            transcript: &self.app.transcript,
            transcript_cache: &mut self.app.transcript_cache,
            scroll_offset: self.app.scroll_offset,
            is_streaming: self.app.is_streaming,
            active_tools: &tools,
            input: &input_snapshot,
            model: &self.app.model,
            session_id: &self.app.session_id,
            status_message: &self.app.status_message,
            is_connected: self.app.is_connected,
            compaction_count: self.app.transcript.compaction_count(),
            thinking_level: self.app.thinking_level.as_str(),
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
    mut rx: mpsc::UnboundedReceiver<BackendEvent>,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    use crossterm::ExecutableCommand;
    use crossterm::cursor::Hide;
    use crossterm::event::EnableBracketedPaste;
    use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use tokio_stream::StreamExt;

    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("run_tui_start");
    }
    let mut app = App::new(client_tx);
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
    io::stdout().execute(EnterAlternateScreen)?;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_alternate_screen_entered");
    }
    terminal_guard.mark_alternate_screen();
    io::stdout().execute(Hide)?;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tui_cursor_hidden");
    }
    terminal_guard.mark_cursor_hidden();
    io::stdout().execute(EnableBracketedPaste)?;
    terminal_guard.mark_bracketed_paste();

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
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
                    match event {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            app.handle_key(key.code, key.modifiers);
                        }
                        Event::Paste(text) => {
                            app.handle_paste(&text);
                        }
                        _ => {}
                    }
                }
            }
            else => break,
        }

        let input_snapshot = app.prompt.clone();
        if let Ok(area) = terminal.size() {
            let (width, height) = layout::transcript_viewport_size(area.into(), &input_snapshot);
            app.set_transcript_viewport(width, height);
        }
        let tools: Vec<String> = app.active_tools.iter().map(ActiveTool::label).collect();
        let mode = app.mode().clone();
        let model_idx = app.model_select_index();
        let session_idx = app.session_select_index();
        let fork_idx = app.fork_select_index();
        let slash_info = app.slash_completion().map(|(prefix, selected, matches)| {
            (prefix, selected, matches.into_iter().copied().collect())
        });
        let dialog = app.active_dialog.clone();
        let available_models = app.available_models.clone();
        let model = app.model.clone();
        let sessions = app.sessions.clone();
        let session_labels = app.session_labels.clone();
        let fork_messages = app.fork_messages.clone();
        let session_id = app.session_id.clone();
        let status_message = app.status_message.clone();
        let thinking_level = app.thinking_level;
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
                active_tools: &tools,
                input: &input_snapshot,
                model: &model,
                session_id: &session_id,
                status_message: &status_message,
                is_connected: app.is_connected,
                compaction_count: app.transcript.compaction_count(),
                thinking_level: thinking_level.as_str(),
            };
            layout::render(frame, &mut render_params);
            match &mode {
                AppMode::ModelSelect { .. } => {
                    let selector = crate::model_selector::ModelSelector {
                        models: &available_models,
                        current_model: &model,
                        selected_index: model_idx,
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
                    };
                    frame.render_widget(picker, frame.area());
                }
                AppMode::ForkSelect { .. } => {
                    let picker = crate::fork_picker::ForkPicker {
                        messages: &fork_messages,
                        selected_index: fork_idx,
                    };
                    frame.render_widget(picker, frame.area());
                }
                AppMode::SlashComplete { .. } => {
                    if let Some((_prefix, selected, matches)) = slash_info {
                        let popup = crate::slash_popup::SlashPopup { selected, matches };
                        frame.render_widget(popup, frame.area());
                    }
                }
                AppMode::Normal
                | AppMode::DialogConfirm
                | AppMode::DialogInput
                | AppMode::DialogSelect => {}
                AppMode::LocalEditCompose { step, draft } => {
                    render_local_edit_compose_overlay(frame, *step, draft);
                }
                AppMode::LocalEditReview { preview, selected } => {
                    render_local_edit_review_overlay(frame, preview, *selected);
                }
                AppMode::HelpOverlay => {
                    frame.render_widget(crate::help_overlay::HelpOverlay, frame.area());
                }
                AppMode::ThinkingSelect { .. } => {
                    let selector = crate::thinking_selector::ThinkingLevelSelector {
                        levels: &ThinkingLevel::ALL,
                        current_level: thinking_level,
                        selected_index: thinking_idx,
                    };
                    frame.render_widget(selector, frame.area());
                }
                AppMode::PerfOverlay => {
                    let overlay = crate::perf_overlay::PerfMetricsOverlay {
                        metrics: &perf_metrics,
                    };
                    frame.render_widget(overlay, frame.area());
                }
            }

            if let Some(dialog) = dialog.as_ref() {
                render_dialog_overlay(frame, dialog);
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

    terminal_guard.restore()?;

    Ok(())
}

fn render_dialog_overlay(frame: &mut ratatui::Frame<'_>, dialog: &PendingDialog) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(70, 50, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .title(dialog_summary(&dialog.request))
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));

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
                Style::new().fg(Color::Black).bg(Color::Green)
            } else {
                Style::new().fg(Color::Green)
            };
            let no_style = if dialog.confirm_accepted {
                Style::new().fg(Color::Red)
            } else {
                Style::new().fg(Color::Black).bg(Color::Red)
            };
            lines.push(Line::from(vec![
                Span::styled(" Yes ", yes_style),
                Span::raw("  "),
                Span::styled(" No ", no_style),
            ]));
            lines.push(Line::raw(""));
            lines.push(Line::from("Enter to confirm, Esc to cancel"));
        }
        DialogKind::Input { .. } => {
            render_dialog_textarea(
                frame,
                inner,
                lines,
                dialog,
                "Enter to submit, Esc to cancel",
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
            );
            return;
        }
        DialogKind::Select { options } => {
            for (idx, option) in options.iter().enumerate() {
                let is_selected = idx == dialog.selected;
                let style = if is_selected {
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray)
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

    let paragraph = Paragraph::new(lines);
    Widget::render(paragraph, inner, frame.buffer_mut());
}

fn render_local_edit_compose_overlay(
    frame: &mut ratatui::Frame<'_>,
    step: LocalEditComposeStep,
    draft: &LocalEditDraft,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(70, 50, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .title("local edit")
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let mut lines = Vec::new();
    if step == LocalEditComposeStep::Kind {
        lines.push(Line::from(vec![
            Span::styled("1", Style::new().fg(Color::Yellow)),
            Span::raw(" Modify existing file"),
        ]));
        lines.push(Line::from(vec![
            Span::styled("2", Style::new().fg(Color::Yellow)),
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

    Widget::render(Paragraph::new(lines), inner, frame.buffer_mut());
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
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

    let popup_area = centered_rect(76, 62, frame.area());
    Clear.render(popup_area, frame.buffer_mut());

    let block = Block::default()
        .borders(Borders::ALL)
        .title("review local edit")
        .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));
    let inner = block.inner(popup_area);
    block.render(popup_area, frame.buffer_mut());

    let apply_style = if selected == LocalEditReviewAction::Apply {
        Style::new().fg(Color::Black).bg(Color::Green)
    } else {
        Style::new().fg(Color::Green)
    };
    let reject_style = if selected == LocalEditReviewAction::Reject {
        Style::new().fg(Color::Black).bg(Color::Red)
    } else {
        Style::new().fg(Color::Red)
    };
    let mut lines = vec![
        Line::from(format!("Path: {}", preview.path)),
        Line::from(format!("Operation: {}", preview.operation)),
        Line::from(format!("Review: {:?}", preview.review_state)),
        Line::raw(""),
    ];
    let action_lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled(" Apply ", apply_style),
            Span::raw("  "),
            Span::styled(" Reject ", reject_style),
        ]),
        Line::from("Enter to submit, Tab to toggle, Esc to reject"),
    ];
    let diff_line_budget =
        usize::from(inner.height).saturating_sub(lines.len() + action_lines.len() + 1);
    let mut rendered_diff_lines = 0;
    for line in preview.diff_summary.lines().take(diff_line_budget) {
        lines.push(Line::from(line.to_string()));
        rendered_diff_lines += 1;
    }
    let diff_was_line_truncated = preview.diff_summary.lines().count() > rendered_diff_lines;
    if preview.diff_summary_truncated || diff_was_line_truncated {
        lines.push(Line::from("[diff summary truncated]"));
    }
    lines.extend(action_lines);

    Widget::render(Paragraph::new(lines), inner, frame.buffer_mut());
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
    dialog: &PendingDialog,
    hint: &'static str,
) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
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
        Widget::render(Paragraph::new(prompt_lines), chunks[0], frame.buffer_mut());
    }

    let mut textarea = dialog_textarea(&dialog.input_buffer, dialog.cursor_pos);
    textarea.set_block(
        Block::default()
            .borders(Borders::ALL)
            .title("input")
            .title_style(Style::new().fg(Color::Yellow)),
    );
    textarea.set_wrap_mode(WrapMode::Word);
    textarea.set_cursor_line_style(Style::default());
    textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

    Widget::render(&textarea, chunks[1], frame.buffer_mut());
    Widget::render(
        Paragraph::new(Line::from(hint)),
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
        App, AppMode, LocalEditComposeStep, LocalEditDecisionSubmission, LocalEditDraft,
        LocalEditReview, LocalEditReviewAction, StartupTrace, tool_output_summary,
    };
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::sync::Arc;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, BackendState, Capability, ClientEvent, DialogKind, DialogRequest,
        DialogResponse, ExtensionLifecycleAction, ExtensionLifecycleOutcome, ForkMessage,
        ForkPosition, Handshake, LocalEditDecision, LocalEditFinishedOutcome,
        LocalEditOperationInput, LocalEditPreviewSummary, LocalEditReviewState, ModelInfo,
        NegotiatedCapabilities, PromptOutcome, RecentSession, ServerEvent, SessionMessage,
        ToolResult, ToolReviewPayload, default_rpc_handshake, default_ui_handshake,
    };

    fn connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &default_rpc_handshake(),
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

    fn native_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native-dogfood", vec![]),
            ),
        }
    }

    fn cancellable_native_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native-dogfood", vec![Capability::PromptCancellation]),
            ),
        }
    }

    fn local_edit_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native-dogfood", vec![Capability::LocalEdit]),
            ),
        }
    }

    fn extension_lifecycle_connected_event() -> BackendEvent {
        BackendEvent::Connected {
            negotiated: NegotiatedCapabilities::from_handshakes(
                &default_ui_handshake(),
                &Handshake::new("yach-native-dogfood", vec![Capability::ExtensionLifecycle]),
            ),
        }
    }

    fn model(provider: &str, id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            provider: provider.to_string(),
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    fn session_message(role: &str, entry_id: &str, text: &str) -> SessionMessage {
        SessionMessage {
            role: role.to_string(),
            text: text.to_string(),
            entry_id: Some(entry_id.to_string()),
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
            session_id: Some(String::from("sess-1")),
            session_file: Some(String::from("/tmp/session.jsonl")),
            thinking_level: Some(String::from("high")),
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
        app.handle_backend_event(native_connected_event());
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

        app.handle_server_event(ServerEvent::SessionChanged {
            session_id: String::from("sess-2"),
        });
        app.handle_server_event(ServerEvent::ModelChanged {
            model: String::from("model-2"),
        });
        app.handle_server_event(ServerEvent::StateUpdated(BackendState {
            model_id: None,
            model_name: None,
            model_provider: None,
            session_id: None,
            session_file: None,
            thinking_level: Some(String::from("high")),
            is_streaming: true,
            is_compacting: false,
            message_count: None,
            pending_message_count: None,
        }));

        assert_eq!(app.session_id, "default");
        assert_eq!(app.model, "default");
        app.handle_server_event(ServerEvent::StatusUpdated {
            message: String::from("turn_end"),
        });

        assert_eq!(app.session_id, "sess-2");
        assert_eq!(app.model, "model-2");
        assert_eq!(app.thinking_level.as_str(), "high");
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
            })
        );

        app.handle_server_event(ServerEvent::ModelChanged {
            model: String::from("anthropic/claude-sonnet-4-20250514"),
        });
        assert_eq!(app.model, "anthropic/claude-sonnet-4-20250514");
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
        app.handle_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.model_select_index(), 1);
        app.handle_key(KeyCode::Char('k'), KeyModifiers::NONE);
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
        app.handle_backend_event(native_connected_event());
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
        app.handle_backend_event(native_connected_event());
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
    fn extension_lifecycle_finish_updates_status_for_pending_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
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
    }

    #[test]
    fn tool_review_enters_review_mode_without_local_request() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);

        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
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
        assert_eq!(
            app.pending_tool_review_request_id.as_deref(),
            Some("tool-review-request-1")
        );
        assert_eq!(
            app.active_tool_review_preview_id.as_deref(),
            Some("preview-1")
        );
        assert_eq!(app.status_message, "review local edit");
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
        assert!(app.active_tool_review_preview_id.is_none());
    }

    #[test]
    fn tool_review_emits_tool_decision_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
        app.handle_server_event(ServerEvent::ToolReviewRequested {
            request_id: String::from("tool-review-request-1"),
            tool_name: String::from("edit_text_file"),
            payload: ToolReviewPayload::LocalEdit {
                preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
            },
        });

        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(app.status_message, "submitting local edit decision");
        assert_eq!(app.pending_tool_review_request_id, None);
        assert!(app.active_tool_review_preview_id.is_none());
        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(
            rx.try_recv(),
            Ok(ClientEvent::ToolReviewDecisionSubmitted {
                request_id: String::from("tool-review-request-1"),
                preview_id: String::from("preview-1"),
                permission_decision_id: String::from("permission-1"),
                decision: LocalEditDecision::Apply,
            })
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
    }

    #[test]
    fn tool_review_finish_after_decision_returns_to_normal_mode() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
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

        app.handle_server_event(ServerEvent::LocalEditFinished {
            preview_id: Some(String::from("preview-1")),
            outcome: LocalEditFinishedOutcome::Applied,
            message: String::from("tool edit applied"),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.status_message, "tool edit applied");
        assert!(app.active_tool_review_preview_id.is_none());
        assert!(app.active_local_edit_preview_id.is_none());
    }

    #[test]
    fn tool_review_prompt_finish_after_decision_returns_to_normal_mode() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut app = App::new(tx);
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
            message: Some(String::from("turn_end native provider")),
        });

        assert!(matches!(app.mode, AppMode::Normal));
        assert_eq!(app.pending_tool_review_request_id, None);
        assert!(app.active_tool_review_preview_id.is_none());
        assert_eq!(
            app.local_edit_decision_submission,
            LocalEditDecisionSubmission::Idle
        );
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
    fn tool_output_summary_stays_compact() {
        assert_eq!(tool_output_summary("", false), "completed with no output");
        assert_eq!(
            tool_output_summary("one\ntwo\n", true),
            "failed: 2 lines, 8 bytes"
        );
    }
}
