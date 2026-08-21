use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::input::{InputComposer, input_height};
use crate::status_bar::StatusBar;
use crate::transcript;
use crate::transcript::{Transcript, TranscriptRenderCache};

const STATUS_HEIGHT: u16 = 1;

// Independent UI facts (connection, focus, streaming, estimate), not
// encodable states of one machine.
#[expect(clippy::struct_excessive_bools)]
pub struct RenderParams<'a> {
    pub transcript: &'a Transcript,
    pub transcript_cache: &'a mut TranscriptRenderCache,
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub input: &'a mut ratatui_textarea::TextArea<'static>,
    pub model: &'a str,
    pub session_id: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    pub context_used_percent: Option<u8>,
    pub context_usage_is_estimate: bool,
    pub terminal_focused: bool,
}

pub fn transcript_viewport_size(
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
) -> (u16, u16) {
    let composer_height = input_height(input, area.width);
    let reserved = composer_height + STATUS_HEIGHT;
    (area.width, area.height.saturating_sub(reserved).max(1))
}

pub fn render(frame: &mut Frame, params: &mut RenderParams<'_>) {
    let area = frame.area();
    let composer_height = input_height(params.input, area.width);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(composer_height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area);

    transcript::render(
        chunks[0],
        frame.buffer_mut(),
        params.transcript,
        params.transcript_cache,
        params.scroll_offset,
        params.is_streaming,
    );

    let input_widget = InputComposer {
        textarea: &mut *params.input,
        is_streaming: params.is_streaming,
        terminal_focused: params.terminal_focused,
    };
    frame.render_widget(input_widget, chunks[1]);

    let status_bar = StatusBar {
        model: params.model,
        session_id: params.session_id,
        status_message: params.status_message,
        is_connected: params.is_connected,
        compaction_count: params.compaction_count,
        context_used_percent: params.context_used_percent,
        context_usage_is_estimate: params.context_usage_is_estimate,
    };
    frame.render_widget(status_bar, chunks[2]);
}
