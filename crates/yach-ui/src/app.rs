use std::io;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};
use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, DialogKind, DialogRequest, DialogResponse,
    NegotiatedCapabilities, ServerEvent,
};

use crate::layout;
use crate::model_selector::KNOWN_MODELS;
use crate::perf_metrics::PerfMetrics;
use crate::slash_commands::{SlashCommand, match_slash_commands};
use crate::thinking_level::ThinkingLevel;
use crate::transcript::{Transcript, TranscriptEntry};

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

fn is_lifecycle_status(message: &str) -> bool {
    matches!(
        message,
        "agent_started"
            | "agent_start"
            | "turn_start"
            | "turn_end"
            | "agent_end"
            | "message_start"
            | "message_end"
    )
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Normal,
    SlashComplete { prefix: String, selected: usize },
    ModelSelect { selected: usize },
    SessionSelect { selected: usize },
    ThinkingSelect { selected: usize },
    DialogConfirm,
    DialogInput,
    DialogSelect,
    PerfOverlay,
}

#[derive(Debug, Clone)]
struct PendingDialog {
    request: DialogRequest,
    input_buffer: String,
    cursor_pos: usize,
    selected: usize,
    confirm_accepted: bool,
}

pub struct App {
    transcript: Transcript,
    scroll_offset: usize,
    prompt: TextArea<'static>,
    active_tools: Vec<ActiveTool>,
    model: String,
    session_id: String,
    status_message: String,
    is_connected: bool,
    is_streaming: bool,
    should_quit: bool,
    mode: AppMode,
    sessions: Vec<String>,
    thinking_level: ThinkingLevel,
    perf_metrics: PerfMetrics,
    negotiated: Option<NegotiatedCapabilities>,
    active_dialog: Option<PendingDialog>,
    client_tx: mpsc::UnboundedSender<ClientEvent>,
}

impl App {
    fn new(client_tx: mpsc::UnboundedSender<ClientEvent>) -> Self {
        Self {
            transcript: Transcript::new(),
            scroll_offset: 0,
            prompt: TextArea::default(),
            active_tools: Vec::new(),
            model: String::from("default"),
            session_id: String::from("default"),
            status_message: String::from("connecting..."),
            is_connected: false,
            is_streaming: false,
            should_quit: false,
            mode: AppMode::Normal,
            sessions: vec![String::from("default")],
            thinking_level: ThinkingLevel::Off,
            perf_metrics: PerfMetrics::new(),
            negotiated: None,
            active_dialog: None,
            client_tx,
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.transcript.entries().len();
    }

    fn supports(&self, capability: Capability) -> bool {
        self.negotiated
            .as_ref()
            .is_some_and(|negotiated| negotiated.supports(capability))
    }

    fn send_client_event(&mut self, event: ClientEvent) -> bool {
        if self.client_tx.send(event).is_ok() {
            true
        } else {
            self.is_connected = false;
            self.is_streaming = false;
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
                self.is_streaming = false;
                self.active_tools.clear();
                self.active_dialog = None;
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
            ServerEvent::PromptDelta { delta, .. } => {
                self.is_streaming = true;
                self.transcript.append_delta(&delta);
                if self.scroll_offset >= self.transcript.entries().len().saturating_sub(1) {
                    self.scroll_to_bottom();
                }
            }
            ServerEvent::ToolCallStarted {
                tool_call_id,
                tool_name,
                preview,
            } => {
                self.is_streaming = true;
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
                if message.starts_with("agent_end") || message.starts_with("turn_end") {
                    self.is_streaming = false;
                    self.active_tools.clear();
                }
                if message.starts_with("agent_start") || message.starts_with("turn_start") {
                    self.is_streaming = true;
                }
                if !is_lifecycle_status(&message) {
                    self.status_message.clone_from(&message);
                }
            }
            ServerEvent::SessionChanged { session_id } => {
                self.session_id.clone_from(&session_id);
                if !self.sessions.contains(&session_id) {
                    self.sessions.push(session_id);
                }
            }
            ServerEvent::ModelChanged { model } => {
                self.model = model;
            }
            ServerEvent::DialogRequested(request) => self.open_dialog(request),
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

    fn apply_backend_state(&mut self, state: BackendState) {
        if let Some(model) = state.model_name.or(state.model_id) {
            self.model = model;
        }

        if let Some(session_id) = state.session_id {
            self.session_id.clone_from(&session_id);
            if !self.sessions.contains(&session_id) {
                self.sessions.push(session_id);
            }
        }

        if let Some(level) = state.thinking_level
            && let Some(level) = ThinkingLevel::from_str(&level)
        {
            self.thinking_level = level;
        }

        self.is_streaming = state.is_streaming;
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
        let pending = match &request.kind {
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
        };

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
        self.mode = AppMode::Normal;
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
            AppMode::ThinkingSelect { .. } => self.handle_thinking_select_key(key, modifiers),
            AppMode::DialogConfirm => self.handle_dialog_confirm_key(key, modifiers),
            AppMode::DialogInput => self.handle_dialog_input_key(key, modifiers),
            AppMode::DialogSelect => self.handle_dialog_select_key(key, modifiers),
            AppMode::PerfOverlay => self.handle_perf_overlay_key(key, modifiers),
        }
    }

    fn handle_normal_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match (key, modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.is_streaming {
                    self.is_streaming = false;
                    self.active_tools.clear();
                    self.status_message = String::from("cancelled");
                } else {
                    self.should_quit = true;
                }
            }
            (KeyCode::Char('m'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::ModelSelect { selected: 0 };
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::SessionSelect { selected: 0 };
            }
            (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::ThinkingSelect { selected: 0 };
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
                    || modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.insert_input_newline();
            }
            (KeyCode::Enter, _) if !self.prompt.is_empty() && !self.is_streaming => {
                self.submit_input();
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
            }
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

    fn set_prompt_text(&mut self, text: &str) {
        self.prompt = textarea_from_text(text);
    }

    fn prompt_text(&self) -> String {
        self.prompt.lines().join("\n")
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

    fn handle_slash_complete_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::SlashComplete { prefix, selected } = &self.mode else {
            return;
        };
        let prefix = prefix.clone();
        let mut selected = *selected;
        let matches = match_slash_commands(&prefix);

        match (key, modifiers) {
            (KeyCode::Esc | KeyCode::Tab, _) => {
                self.mode = AppMode::Normal;
            }
            (KeyCode::Up, _) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::SlashComplete { prefix, selected };
            }
            (KeyCode::Down, _) => {
                selected = (selected + 1).min(matches.len().saturating_sub(1));
                self.mode = AppMode::SlashComplete { prefix, selected };
            }
            (KeyCode::Enter, _) => {
                if let Some(cmd) = matches.get(selected) {
                    self.set_prompt_text(cmd.name);
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.mode = AppMode::Normal;
                self.handle_normal_key(key, modifiers);
            }
        }
    }

    fn handle_model_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::ModelSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (KeyCode::Up, _) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::ModelSelect { selected };
            }
            (KeyCode::Down, _) => {
                selected = (selected + 1).min(KNOWN_MODELS.len().saturating_sub(1));
                self.mode = AppMode::ModelSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if let Some(model) = KNOWN_MODELS.get(selected)
                    && self.send_client_event(ClientEvent::ModelSelected {
                        model: (*model).to_string(),
                    })
                {
                    self.model = (*model).to_string();
                    self.status_message = format!("model: {model}");
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
            (KeyCode::Up, _) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::SessionSelect { selected };
            }
            (KeyCode::Down, _) => {
                selected = (selected + 1).min(self.sessions.len().saturating_sub(1));
                self.mode = AppMode::SessionSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if let Some(session) = self.sessions.get(selected).cloned()
                    && self.send_client_event(ClientEvent::SessionSelected {
                        session_id: session.clone(),
                    })
                {
                    self.session_id.clone_from(&session);
                    self.status_message = format!("session: {session}");
                }
                self.mode = AppMode::Normal;
            }
            _ => {
                self.mode = AppMode::Normal;
            }
        }
    }

    fn handle_thinking_select_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        let AppMode::ThinkingSelect { selected } = &self.mode else {
            return;
        };
        let mut selected = *selected;

        match (key, modifiers) {
            (KeyCode::Up, _) => {
                selected = selected.saturating_sub(1);
                self.mode = AppMode::ThinkingSelect { selected };
            }
            (KeyCode::Down, _) => {
                selected = (selected + 1).min(ThinkingLevel::ALL.len().saturating_sub(1));
                self.mode = AppMode::ThinkingSelect { selected };
            }
            (KeyCode::Enter, _) => {
                if let Some(level) = ThinkingLevel::ALL.get(selected)
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
            (KeyCode::Enter, KeyModifiers::CONTROL) if is_editor => {
                response = Some(DialogResponse::Text {
                    value: dialog.input_buffer.clone(),
                });
            }
            (KeyCode::Enter, _) if is_editor => {
                dialog.input_buffer.insert(dialog.cursor_pos, '\n');
                dialog.cursor_pos += 1;
            }
            (KeyCode::Enter, _) => {
                response = Some(DialogResponse::Text {
                    value: dialog.input_buffer.clone(),
                });
            }
            (KeyCode::Backspace, _) => {
                if dialog.cursor_pos > 0 {
                    dialog.input_buffer.remove(dialog.cursor_pos - 1);
                    dialog.cursor_pos -= 1;
                }
            }
            (KeyCode::Delete, _) => {
                if dialog.cursor_pos < dialog.input_buffer.len() {
                    dialog.input_buffer.remove(dialog.cursor_pos);
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
                dialog.input_buffer.insert(dialog.cursor_pos, c);
                dialog.cursor_pos += 1;
            }
            _ => {}
        }

        if cancelled {
            self.cancel_dialog();
        } else if let Some(response) = response {
            self.submit_dialog_response(response);
        }
    }

    fn handle_dialog_select_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
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
            KeyCode::Up => {
                dialog.selected = dialog.selected.saturating_sub(1);
            }
            KeyCode::Down => {
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

    fn handle_perf_overlay_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Char('p') => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let input = self.prompt_text();
        self.clear_input();

        if input.starts_with("/quit") || input.starts_with("/exit") {
            self.should_quit = true;
            return;
        }

        if input.starts_with("/clear") {
            self.transcript.clear();
            self.scroll_offset = 0;
            return;
        }

        if input.starts_with("/model") {
            self.mode = AppMode::ModelSelect { selected: 0 };
            return;
        }

        if input.starts_with("/session") {
            self.mode = AppMode::SessionSelect { selected: 0 };
            return;
        }

        if input.starts_with("/thinking") {
            self.mode = AppMode::ThinkingSelect { selected: 0 };
            return;
        }

        if input.starts_with("/perf") {
            self.mode = AppMode::PerfOverlay;
            return;
        }

        if input.starts_with("/fork") {
            self.fork_current_session();
            return;
        }

        if input.starts_with("/help") {
            self.status_message = String::from(
                "Commands: /quit /clear /model /session /fork /thinking /perf /help | Ctrl+M: models | Ctrl+S: sessions | Ctrl+F: fork | Ctrl+T: thinking | Ctrl+P: perf",
            );
            return;
        }

        let session_id = self.session_id.clone();
        if self.send_client_event(ClientEvent::PromptSubmitted {
            session_id,
            prompt: input.clone(),
        }) {
            self.transcript.append_user_message(&input);
            self.scroll_to_bottom();
            self.status_message = String::from("sending...");
            self.is_streaming = true;
        }
    }

    fn fork_current_session(&mut self) {
        let session_id = self.session_id.clone();
        self.fork_session(&session_id);
    }

    fn fork_session(&mut self, session_id: &str) {
        if !self.supports(Capability::SessionForking) {
            self.status_message = String::from("session forking unavailable");
            return;
        }

        if self.send_client_event(ClientEvent::SessionForkRequested {
            session_id: session_id.to_string(),
        }) {
            self.status_message = format!("forking: {session_id}");
        }
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

pub async fn run_tui(
    client_tx: mpsc::UnboundedSender<ClientEvent>,
    mut rx: mpsc::UnboundedReceiver<BackendEvent>,
) -> io::Result<()> {
    use crossterm::ExecutableCommand;
    use crossterm::cursor::Hide;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use tokio_stream::StreamExt;

    let mut app = App::new(client_tx);
    let mut backend_open = true;

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(Hide)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut crossterm_stream = crossterm::event::EventStream::new();

    loop {
        if app.should_quit {
            break;
        }

        tokio::select! {
            maybe_event = rx.recv(), if backend_open => {
                if let Some(event) = maybe_event {
                    app.handle_backend_event(event);
                } else {
                    backend_open = false;
                    app.handle_backend_event(BackendEvent::Disconnected {
                        reason: String::from("backend disconnected"),
                    });
                }
            }
            Some(event) = crossterm_stream.next() => {
                if let Ok(Event::Key(key)) = event
                    && key.kind == KeyEventKind::Press
                {
                    app.handle_key(key.code, key.modifiers);
                }
            }
            else => break,
        }

        let input_snapshot = app.prompt.clone();
        let entries: Vec<TranscriptEntry> = app.transcript.entries().to_vec();
        let tools: Vec<String> = app.active_tools.iter().map(ActiveTool::label).collect();
        let mode = app.mode().clone();
        let model_idx = app.model_select_index();
        let session_idx = app.session_select_index();
        let slash_info = app.slash_completion();
        let dialog = app.active_dialog.clone();

        let render_params = layout::RenderParams {
            entries: &entries,
            scroll_offset: app.scroll_offset,
            is_streaming: app.is_streaming,
            active_tools: &tools,
            input: &input_snapshot,
            model: &app.model,
            session_id: &app.session_id,
            status_message: &app.status_message,
            is_connected: app.is_connected,
            compaction_count: app.transcript.compaction_count(),
            thinking_level: app.thinking_level.as_str(),
        };

        let render_start = std::time::Instant::now();

        terminal.draw(|frame| {
            layout::render(frame, &render_params);
            match &mode {
                AppMode::ModelSelect { .. } => {
                    let selector = crate::model_selector::ModelSelector {
                        models: KNOWN_MODELS,
                        current_model: &app.model,
                        selected_index: model_idx,
                    };
                    frame.render_widget(selector, frame.area());
                }
                AppMode::SessionSelect { .. } => {
                    let picker = crate::session_picker::SessionPicker {
                        sessions: &app.sessions,
                        current_session: &app.session_id,
                        selected_index: session_idx,
                        show_fork_hint: app.supports(Capability::SessionForking),
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
                AppMode::ThinkingSelect { .. } => {
                    let selector = crate::thinking_selector::ThinkingLevelSelector {
                        levels: &ThinkingLevel::ALL,
                        current_level: app.thinking_level,
                        selected_index: app.thinking_select_index(),
                    };
                    frame.render_widget(selector, frame.area());
                }
                AppMode::PerfOverlay => {
                    let overlay = crate::perf_overlay::PerfMetricsOverlay {
                        metrics: &app.perf_metrics,
                    };
                    frame.render_widget(overlay, frame.area());
                }
            }

            if let Some(dialog) = dialog.as_ref() {
                render_dialog_overlay(frame, dialog);
            }
        })?;

        app.perf_metrics.record_render(render_start.elapsed());
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

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
            append_dialog_input_lines(&mut lines, &dialog.input_buffer);
            lines.push(Line::raw(""));
            lines.push(Line::from("Enter to submit, Esc to cancel"));
        }
        DialogKind::Editor { .. } => {
            append_dialog_input_lines(&mut lines, &dialog.input_buffer);
            lines.push(Line::raw(""));
            lines.push(Line::from("Ctrl+Enter to submit, Esc to cancel"));
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

fn append_dialog_input_lines(lines: &mut Vec<ratatui::text::Line<'_>>, input: &str) {
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};

    if input.is_empty() {
        lines.push(Line::from(Span::styled(
            "<empty>",
            Style::new().fg(Color::DarkGray),
        )));
        return;
    }

    for line in input.lines() {
        lines.push(Line::from(line.to_string()));
    }
    if input.ends_with('\n') {
        lines.push(Line::raw(""));
    }
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
    use super::{App, AppMode, tool_output_summary};
    use crossterm::event::{KeyCode, KeyModifiers};
    use tokio::sync::mpsc;
    use yach_proto::{
        BackendEvent, BackendState, ClientEvent, DialogKind, DialogRequest, DialogResponse,
        NegotiatedCapabilities, ServerEvent, ToolResult, default_rpc_handshake,
        default_ui_handshake,
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

        let event = rx.try_recv();
        assert!(event.is_ok());
        let Ok(event) = event else {
            return;
        };
        assert_eq!(
            event,
            ClientEvent::SessionForkRequested {
                session_id: String::from("default"),
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
    fn tool_output_summary_stays_compact() {
        assert_eq!(tool_output_summary("", false), "completed with no output");
        assert_eq!(
            tool_output_summary("one\ntwo\n", true),
            "failed: 2 lines, 8 bytes"
        );
    }
}
