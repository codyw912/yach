use std::io;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;
use yach_adapter_pi_rpc::{
    DispatchAction, PiRpcSession, Transcript, TranscriptEntry, dispatch_event, resolve_dialog,
};
use yach_proto::{ClientEvent, MessageMeta, TransportMessage};

use crate::branch_tracker::BranchTracker;
use crate::layout;
use crate::model_selector::KNOWN_MODELS;
use crate::perf_metrics::PerfMetrics;
use crate::slash_commands::{SlashCommand, match_slash_commands};
use crate::thinking_level::ThinkingLevel;
use crate::transcript;

#[derive(Debug, Clone)]
pub enum AppCommand {
    SetModel { model: String },
    SwitchSession { session_id: String },
    ForkSession { session_id: String },
    SetThinkingLevel { level: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    Normal,
    SlashComplete { prefix: String, selected: usize },
    ModelSelect { selected: usize },
    SessionSelect { selected: usize },
    ThinkingSelect { selected: usize },
    PerfOverlay,
    BranchOverlay,
}

pub struct App {
    transcript: Transcript,
    scroll_offset: usize,
    focused_entry: usize,
    input_buffer: String,
    cursor_pos: usize,
    active_tools: Vec<String>,
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
    branch_tracker: BranchTracker,
    command_tx: mpsc::UnboundedSender<AppCommand>,
}

impl App {
    fn new(command_tx: mpsc::UnboundedSender<AppCommand>) -> Self {
        Self {
            transcript: Transcript::new(),
            scroll_offset: 0,
            focused_entry: 0,
            input_buffer: String::new(),
            cursor_pos: 0,
            active_tools: Vec::new(),
            model: String::from("default"),
            session_id: String::from("default"),
            status_message: String::from("starting..."),
            is_connected: false,
            is_streaming: false,
            should_quit: false,
            mode: AppMode::Normal,
            sessions: vec![String::from("default")],
            thinking_level: ThinkingLevel::Off,
            perf_metrics: PerfMetrics::new(),
            branch_tracker: BranchTracker::new("default"),
            command_tx,
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.transcript.entries().len();
        self.focused_entry = self.transcript.entries().len().saturating_sub(1);
    }

    fn focus_entry(&mut self, idx: usize) {
        let max_idx = self.transcript.entries().len().saturating_sub(1);
        self.focused_entry = idx.min(max_idx);
        self.scroll_offset =
            transcript::scroll_to_entry(self.transcript.entries(), self.focused_entry, 20);
    }

    fn jump_to_next_turn(&mut self) {
        let boundaries = self.transcript.turn_boundaries();
        let current = self.focused_entry;
        let next = boundaries.iter().find(|&&b| b > current).copied();
        if let Some(idx) = next {
            self.focus_entry(idx);
        } else {
            self.scroll_to_bottom();
        }
    }

    fn jump_to_prev_turn(&mut self) {
        let boundaries = self.transcript.turn_boundaries();
        let current = self.focused_entry;
        let prev = boundaries.iter().rev().find(|&&b| b < current).copied();
        if let Some(idx) = prev {
            self.focus_entry(idx);
        } else if !boundaries.is_empty() {
            self.focus_entry(boundaries[0]);
        }
    }

    fn jump_to_next_tool_block(&mut self) {
        let boundaries = self.transcript.tool_call_boundaries();
        let current = self.focused_entry;
        let next = boundaries.iter().find(|&&b| b > current).copied();
        if let Some(idx) = next {
            self.focus_entry(idx);
        }
    }

    fn jump_to_prev_tool_block(&mut self) {
        let boundaries = self.transcript.tool_call_boundaries();
        let current = self.focused_entry;
        let prev = boundaries.iter().rev().find(|&&b| b < current).copied();
        if let Some(idx) = prev {
            self.focus_entry(idx);
        }
    }

    fn handle_adapter_action(&mut self, action: DispatchAction) {
        match action {
            DispatchAction::AppendDelta(delta) => {
                self.transcript.append_delta(&delta);
                self.branch_tracker
                    .update_entry_count(&self.session_id, self.transcript.entries().len());
                if self.scroll_offset >= self.transcript.entries().len().saturating_sub(1) {
                    self.scroll_to_bottom();
                }
            }
            DispatchAction::DialogRequested(_) => {
                self.status_message = String::from("dialog pending...");
            }
            DispatchAction::StatusMessage(msg) => {
                self.status_message.clone_from(&msg);
                if msg.starts_with("agent_end") || msg.starts_with("turn_end") {
                    self.is_streaming = false;
                    self.active_tools.clear();
                }
                if msg.starts_with("agent_start") {
                    self.is_streaming = true;
                }
            }
            DispatchAction::ToolCallStarted { tool_name } => {
                self.transcript.append_tool_call(&tool_name);
                self.active_tools.push(tool_name);
            }
            DispatchAction::SessionChanged { session_id } => {
                let old_session = self.session_id.clone();
                self.session_id.clone_from(&session_id);
                if !self.sessions.contains(&session_id) {
                    self.sessions.push(session_id.clone());
                    self.branch_tracker.record_fork(
                        &old_session,
                        &session_id,
                        self.transcript.entries().len(),
                    );
                }
                self.branch_tracker.set_current(&session_id);
            }
            DispatchAction::ModelChanged { model } => {
                self.model = model;
            }
            DispatchAction::TitleChanged { title } => {
                self.status_message = title;
            }
            DispatchAction::Notification { level, message } => {
                self.status_message = format!("[{level}] {message}");
            }
            DispatchAction::StreamComplete => {
                self.is_streaming = false;
                self.active_tools.clear();
            }
        }
    }

    fn handle_key(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        match &self.mode {
            AppMode::Normal => self.handle_normal_key(key, modifiers),
            AppMode::SlashComplete { .. } => self.handle_slash_complete_key(key, modifiers),
            AppMode::ModelSelect { .. } => self.handle_model_select_key(key, modifiers),
            AppMode::SessionSelect { .. } => self.handle_session_select_key(key, modifiers),
            AppMode::ThinkingSelect { .. } => self.handle_thinking_select_key(key, modifiers),
            AppMode::PerfOverlay => self.handle_perf_overlay_key(key, modifiers),
            AppMode::BranchOverlay => self.handle_branch_overlay_key(key, modifiers),
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
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.jump_to_next_turn();
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.jump_to_prev_turn();
            }
            (KeyCode::Char(']'), _) => {
                self.jump_to_next_tool_block();
            }
            (KeyCode::Char('['), _) => {
                self.jump_to_prev_tool_block();
            }
            (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.mode = AppMode::BranchOverlay;
            }
            (KeyCode::Enter, _) if !self.input_buffer.is_empty() && !self.is_streaming => {
                self.submit_input();
            }
            (KeyCode::Enter, _) => {
                self.input_buffer.insert(self.cursor_pos, '\n');
                self.cursor_pos += 1;
            }
            (KeyCode::Backspace, _) => {
                if self.cursor_pos > 0 {
                    self.input_buffer.remove(self.cursor_pos - 1);
                    self.cursor_pos -= 1;
                }
            }
            (KeyCode::Delete, _) => {
                if self.cursor_pos < self.input_buffer.len() {
                    self.input_buffer.remove(self.cursor_pos);
                }
            }
            (KeyCode::Left, KeyModifiers::CONTROL) => {
                self.cursor_pos = prev_word_boundary(&self.input_buffer, self.cursor_pos);
            }
            (KeyCode::Right, KeyModifiers::CONTROL) => {
                self.cursor_pos = next_word_boundary(&self.input_buffer, self.cursor_pos);
            }
            (KeyCode::Left, _) => {
                self.cursor_pos = self.cursor_pos.saturating_sub(1);
            }
            (KeyCode::Right, _) => {
                self.cursor_pos = (self.cursor_pos + 1).min(self.input_buffer.len());
            }
            (KeyCode::Home, _) => {
                self.cursor_pos = 0;
            }
            (KeyCode::End, _) => {
                self.cursor_pos = self.input_buffer.len();
            }
            (KeyCode::Up, _) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            (KeyCode::Down, _) => {
                self.scroll_offset = (self.scroll_offset + 1).min(self.transcript.entries().len());
            }
            (KeyCode::PageUp, _) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            (KeyCode::PageDown, _) => {
                self.scroll_offset = (self.scroll_offset + 10).min(self.transcript.entries().len());
            }
            (KeyCode::Esc, _) => {
                self.input_buffer.clear();
                self.cursor_pos = 0;
            }
            (KeyCode::Tab, _) => {
                if self.input_buffer.starts_with('/') {
                    self.enter_slash_complete();
                }
            }
            (KeyCode::Char(c), _) => {
                self.input_buffer.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            _ => {}
        }
    }

    fn enter_slash_complete(&mut self) {
        let prefix = if self.input_buffer.starts_with('/') {
            self.input_buffer.clone()
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
                    self.input_buffer = String::from(cmd.name);
                    self.cursor_pos = self.input_buffer.len();
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
                if let Some(model) = KNOWN_MODELS.get(selected) {
                    let _ = self.command_tx.send(AppCommand::SetModel {
                        model: model.to_string(),
                    });
                    self.model = model.to_string();
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
                if let Some(session) = self.sessions.get(selected) {
                    let _ = self.command_tx.send(AppCommand::SwitchSession {
                        session_id: session.clone(),
                    });
                    self.session_id.clone_from(session);
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
                if let Some(level) = ThinkingLevel::ALL.get(selected) {
                    let _ = self.command_tx.send(AppCommand::SetThinkingLevel {
                        level: level.as_str().to_string(),
                    });
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

    fn handle_perf_overlay_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Char('p') => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn handle_branch_overlay_key(&mut self, key: KeyCode, _modifiers: KeyModifiers) {
        match key {
            KeyCode::Esc | KeyCode::Char('b') => {
                self.mode = AppMode::Normal;
            }
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let input = std::mem::take(&mut self.input_buffer);
        self.cursor_pos = 0;

        if input.starts_with("/quit") || input.starts_with("/exit") {
            self.should_quit = true;
            return;
        }

        if input.starts_with("/clear") {
            self.transcript = Transcript::new();
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

        self.transcript.append_user_message(&input);
        self.scroll_to_bottom();
        self.status_message = String::from("sending...");
    }

    fn fork_current_session(&mut self) {
        let session_id = self.session_id.clone();
        self.fork_session(&session_id);
    }

    fn fork_session(&mut self, session_id: &str) {
        let message = TransportMessage::client(
            MessageMeta::new("fork-1"),
            ClientEvent::SessionForkRequested {
                session_id: session_id.to_string(),
            },
        );

        let _ = self.command_tx.send(AppCommand::ForkSession {
            session_id: session_id.to_string(),
        });

        let json = yach_adapter_pi_rpc::serialize_client_message(&message);
        if json.is_ok() {
            self.status_message = format!("forking: {session_id}");
        }
    }

    fn get_input(&self) -> String {
        self.input_buffer.clone()
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

fn prev_word_boundary(s: &str, pos: usize) -> usize {
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
    session: PiRpcSession,
    handshake: yach_proto::Handshake,
    tx: mpsc::UnboundedSender<DispatchAction>,
    rx: mpsc::UnboundedReceiver<DispatchAction>,
) -> std::io::Result<()> {
    use crossterm::ExecutableCommand;
    use crossterm::cursor::Hide;
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use tokio_stream::StreamExt;

    let (command_tx, command_rx) = mpsc::unbounded_channel::<AppCommand>();
    let mut app = App::new(command_tx);
    app.is_connected = true;
    app.status_message = String::from("connecting...");

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(Hide)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut crossterm_stream = crossterm::event::EventStream::new();

    let adapter_handle = tokio::task::spawn_blocking(move || {
        adapter_init_and_read(session, handshake, &tx, command_rx);
    });

    let mut rx = rx;

    loop {
        if app.should_quit {
            break;
        }

        tokio::select! {
            Some(event) = rx.recv() => {
                app.handle_adapter_action(event);
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

        let input_snapshot = app.get_input();
        let entries: Vec<TranscriptEntry> = app.transcript.entries().to_vec();
        let tools: Vec<String> = app.active_tools.clone();
        let mode = app.mode().clone();
        let model_idx = app.model_select_index();
        let session_idx = app.session_select_index();
        let slash_info = app.slash_completion();

        let render_params = layout::RenderParams {
            entries: &entries,
            scroll_offset: app.scroll_offset,
            focused_entry: app.focused_entry,
            is_streaming: app.is_streaming,
            active_tools: &tools,
            input_buffer: &input_snapshot,
            cursor_pos: app.cursor_pos,
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
                        show_fork_hint: true,
                    };
                    frame.render_widget(picker, frame.area());
                }
                AppMode::SlashComplete { .. } => {
                    if let Some((_prefix, selected, matches)) = slash_info {
                        let popup = crate::slash_popup::SlashPopup { selected, matches };
                        frame.render_widget(popup, frame.area());
                    }
                }
                AppMode::Normal => {}
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
                AppMode::BranchOverlay => {
                    let overlay = crate::branch_summary::BranchSummaryOverlay {
                        tracker: &app.branch_tracker,
                    };
                    frame.render_widget(overlay, frame.area());
                }
            }
        })?;

        app.perf_metrics.record_render(render_start.elapsed());
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    let _ = adapter_handle.await;

    Ok(())
}

fn adapter_init_and_read(
    mut session: PiRpcSession,
    handshake: yach_proto::Handshake,
    tx: &mpsc::UnboundedSender<DispatchAction>,
    mut cmd_rx: mpsc::UnboundedReceiver<AppCommand>,
) {
    let _ = session.initialize(handshake);
    let _ = tx.send(DispatchAction::StatusMessage(String::from("connected")));
    adapter_read_loop(session, tx, &mut cmd_rx);
}

fn adapter_read_loop(
    mut session: PiRpcSession,
    tx: &mpsc::UnboundedSender<DispatchAction>,
    cmd_rx: &mut mpsc::UnboundedReceiver<AppCommand>,
) {
    loop {
        if let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                AppCommand::SetModel { model } => {
                    let message = TransportMessage::client(
                        MessageMeta::new("set-model-1"),
                        ClientEvent::ModelSelected { model },
                    );
                    let _ = session.send(&message);
                }
                AppCommand::SwitchSession { session_id } => {
                    let message = TransportMessage::client(
                        MessageMeta::new("switch-session-1"),
                        ClientEvent::SessionSelected { session_id },
                    );
                    let _ = session.send(&message);
                }
                AppCommand::ForkSession { session_id } => {
                    let message = TransportMessage::client(
                        MessageMeta::new("fork-1"),
                        ClientEvent::SessionForkRequested { session_id },
                    );
                    let _ = session.send(&message);
                }
                AppCommand::SetThinkingLevel { level } => {
                    let message = TransportMessage::client(
                        MessageMeta::new("set-thinking-1"),
                        ClientEvent::ThinkingLevelSelected { level },
                    );
                    let _ = session.send(&message);
                }
            }
        }

        let Ok(message) = session.read_next() else {
            let _ = tx.send(DispatchAction::StatusMessage(String::from("disconnected")));
            break;
        };

        let yach_proto::MessageBody::ServerEvent(event) = message.body else {
            continue;
        };

        if let Some(action) = dispatch_event(event) {
            if let DispatchAction::DialogRequested(ref req) = action {
                let dialog_id = req.id.clone().unwrap_or_default();
                let request = req.clone();
                let _ = tx.send(action);
                let mut response_line = String::new();
                let _ = std::io::stdin().read_line(&mut response_line);
                let response = resolve_dialog(&request, response_line.trim());
                let dialog_message = TransportMessage::client(
                    MessageMeta::new("dialog-response-1"),
                    ClientEvent::DialogResolved {
                        dialog_id,
                        response,
                    },
                );
                let _ = session.send(&dialog_message);
                continue;
            }
            let _ = tx.send(action);
        }
    }
}
