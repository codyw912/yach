use std::io;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;
use yach_adapter_pi_rpc::{
    DispatchAction, PiRpcSession, Transcript, TranscriptEntry, dispatch_event, resolve_dialog,
};
use yach_proto::{ClientEvent, MessageMeta, TransportMessage};

use crate::layout;

pub struct App {
    transcript: Transcript,
    scroll_offset: usize,
    input_buffer: String,
    cursor_pos: usize,
    active_tools: Vec<String>,
    model: String,
    session_id: String,
    status_message: String,
    is_connected: bool,
    is_streaming: bool,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
        Self {
            transcript: Transcript::new(),
            scroll_offset: 0,
            input_buffer: String::new(),
            cursor_pos: 0,
            active_tools: Vec::new(),
            model: String::from("default"),
            session_id: String::from("default"),
            status_message: String::from("starting..."),
            is_connected: false,
            is_streaming: false,
            should_quit: false,
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.transcript.entries().len();
    }

    fn handle_adapter_action(&mut self, action: DispatchAction) {
        match action {
            DispatchAction::AppendDelta(delta) => {
                self.transcript.append_delta(&delta);
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
                self.active_tools.push(tool_name);
            }
            DispatchAction::SessionChanged { session_id } => {
                self.session_id = session_id;
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
            (KeyCode::Char(c), _) => {
                self.input_buffer.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
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

        self.transcript.append_user_message(&input);
        self.scroll_to_bottom();
        self.status_message = String::from("sending...");
    }

    fn get_input(&self) -> String {
        self.input_buffer.clone()
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

    let mut app = App::new();
    app.is_connected = true;
    app.status_message = String::from("connecting...");

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(Hide)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut crossterm_stream = crossterm::event::EventStream::new();

    let adapter_handle = tokio::task::spawn_blocking(move || {
        adapter_init_and_read(session, handshake, &tx);
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

        let render_params = layout::RenderParams {
            entries: &entries,
            scroll_offset: app.scroll_offset,
            is_streaming: app.is_streaming,
            active_tools: &tools,
            input_buffer: &input_snapshot,
            cursor_pos: app.cursor_pos,
            model: &app.model,
            session_id: &app.session_id,
            status_message: &app.status_message,
            is_connected: app.is_connected,
        };

        terminal.draw(|frame| {
            layout::render(frame, &render_params);
        })?;
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
) {
    let _ = session.initialize(handshake);
    let _ = tx.send(DispatchAction::StatusMessage(String::from("connected")));
    adapter_read_loop(session, tx);
}

fn adapter_read_loop(mut session: PiRpcSession, tx: &mpsc::UnboundedSender<DispatchAction>) {
    loop {
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
