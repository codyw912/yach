use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use yach_adapter_pi_rpc::TranscriptEntry;

use crate::input::InputComposer;
use crate::status_bar::StatusBar;
use crate::tool_area::ToolArea;
use crate::transcript;

const TOOL_AREA_HEIGHT: u16 = 3;
const INPUT_HEIGHT: u16 = 3;
const STATUS_HEIGHT: u16 = 1;

pub struct RenderParams<'a> {
    pub entries: &'a [TranscriptEntry],
    pub scroll_offset: usize,
    pub focused_entry: usize,
    pub is_streaming: bool,
    pub active_tools: &'a [String],
    pub input_buffer: &'a str,
    pub cursor_pos: usize,
    pub model: &'a str,
    pub session_id: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
}

pub fn render(frame: &mut Frame, params: &RenderParams<'_>) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(STATUS_HEIGHT),
            Constraint::Min(1),
            Constraint::Length(TOOL_AREA_HEIGHT),
            Constraint::Length(INPUT_HEIGHT),
        ])
        .split(area);

    let status_bar = StatusBar {
        model: params.model,
        session_id: params.session_id,
        status_message: params.status_message,
        is_connected: params.is_connected,
    };
    frame.render_widget(status_bar, chunks[0]);

    transcript::render(
        chunks[1],
        frame.buffer_mut(),
        params.entries,
        params.scroll_offset,
        params.is_streaming,
        params.focused_entry,
    );

    let tool_area_widget = ToolArea {
        active_tools: params.active_tools,
    };
    frame.render_widget(tool_area_widget, chunks[2]);

    let input_widget = InputComposer {
        buffer: params.input_buffer,
        cursor_pos: params.cursor_pos,
        is_focused: true,
        is_streaming: params.is_streaming,
    };
    frame.render_widget(input_widget, chunks[3]);
}
