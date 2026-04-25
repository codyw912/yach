use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::input::{InputComposer, input_height};
use crate::status_bar::StatusBar;
use crate::tool_area::ToolArea;
use crate::transcript;
use crate::transcript::TranscriptEntry;

const TOOL_AREA_HEIGHT: u16 = 3;
const STATUS_HEIGHT: u16 = 1;

pub struct RenderParams<'a> {
    pub entries: &'a [TranscriptEntry],
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub active_tools: &'a [String],
    pub input: &'a ratatui_textarea::TextArea<'static>,
    pub model: &'a str,
    pub session_id: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    pub thinking_level: &'a str,
}

pub fn render(frame: &mut Frame, params: &RenderParams<'_>) {
    let area = frame.area();
    let composer_height = input_height(params.input, area.width);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(TOOL_AREA_HEIGHT),
            Constraint::Length(composer_height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area);

    transcript::render(
        chunks[0],
        frame.buffer_mut(),
        params.entries,
        params.scroll_offset,
        params.is_streaming,
    );

    let tool_area_widget = ToolArea {
        active_tools: params.active_tools,
    };
    frame.render_widget(tool_area_widget, chunks[1]);

    let input_widget = InputComposer {
        textarea: params.input,
        is_streaming: params.is_streaming,
    };
    frame.render_widget(input_widget, chunks[2]);

    let status_bar = StatusBar {
        model: params.model,
        session_id: params.session_id,
        status_message: params.status_message,
        is_connected: params.is_connected,
        compaction_count: params.compaction_count,
        thinking_level: params.thinking_level,
    };
    frame.render_widget(status_bar, chunks[3]);
}
