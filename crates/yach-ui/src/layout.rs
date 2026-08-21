use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::input::{InputComposer, input_metrics};
use crate::status_bar::StatusBar;
use crate::transcript;
use crate::transcript::{Transcript, TranscriptRenderCache};

const STATUS_HEIGHT: u16 = 1;
const COMPOSER_GAP_HEIGHT: u16 = 1;
const DOCK_SIDE_GUTTER: u16 = 2;
const DOCK_GUTTER_MIN_WIDTH: u16 = 40;
const DOCK_MAX_WIDTH: u16 = 112;
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
    let composer_height = input_metrics(input, dock_width(area.width)).height;
    let reserved = composer_height + COMPOSER_GAP_HEIGHT + STATUS_HEIGHT;
    (area.width, area.height.saturating_sub(reserved).max(1))
}

fn dock_width(area_width: u16) -> u16 {
    let available = if area_width >= DOCK_GUTTER_MIN_WIDTH {
        area_width.saturating_sub(DOCK_SIDE_GUTTER.saturating_mul(2))
    } else {
        area_width
    };
    available.min(DOCK_MAX_WIDTH)
}

fn dock_area(area: Rect) -> Rect {
    let width = dock_width(area.width);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y,
        width,
        height: area.height,
    }
}

pub fn render(frame: &mut Frame, params: &mut RenderParams<'_>) {
    let area = frame.area();
    let composer_metrics = input_metrics(params.input, dock_width(area.width));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(COMPOSER_GAP_HEIGHT),
            Constraint::Length(composer_metrics.height),
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
        overflowed: composer_metrics.overflowed,
    };
    frame.render_widget(input_widget, dock_area(chunks[2]));

    let status_bar = StatusBar {
        model: params.model,
        session_id: params.session_id,
        status_message: params.status_message,
        is_connected: params.is_connected,
        compaction_count: params.compaction_count,
        context_used_percent: params.context_used_percent,
        context_usage_is_estimate: params.context_usage_is_estimate,
    };
    frame.render_widget(status_bar, dock_area(chunks[3]));
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui_textarea::TextArea;

    use super::{dock_area, dock_width, transcript_viewport_size};

    #[test]
    fn dock_is_inset_and_centered_without_exceeding_max_width() {
        assert_eq!(dock_width(39), 39);
        assert_eq!(dock_width(80), 76);
        assert_eq!(dock_width(160), 112);
        assert_eq!(
            dock_area(Rect::new(0, 10, 160, 3)),
            Rect::new(24, 10, 112, 3)
        );
    }

    #[test]
    fn transcript_viewport_reserves_gap_composer_and_status() {
        let input = TextArea::from(["hello"]);
        assert_eq!(
            transcript_viewport_size(Rect::new(0, 0, 80, 24), &input),
            (80, 19)
        );
    }

    #[test]
    fn transcript_viewport_wraps_against_the_capped_dock_width() {
        let long_line = "x".repeat(300);
        let input = TextArea::from([long_line.as_str()]);
        assert_eq!(
            transcript_viewport_size(Rect::new(0, 0, 160, 24), &input),
            (160, 17)
        );
    }
}
