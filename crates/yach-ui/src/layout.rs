use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::input::{InputComposer, input_metrics};
use crate::status_bar::StatusBar;
use crate::theme::Theme;
use crate::transcript;
use crate::transcript::{Transcript, TranscriptRenderCache};

const STATUS_HEIGHT: u16 = 1;
const COMPOSER_GAP_HEIGHT: u16 = 1;
pub struct RenderParams<'a> {
    pub transcript: &'a Transcript,
    pub transcript_cache: &'a mut TranscriptRenderCache,
    pub scroll_offset: usize,
    pub is_streaming: bool,
    pub input: &'a mut ratatui_textarea::TextArea<'static>,
    pub model: &'a str,
    pub thinking_level: &'a str,
    pub approval_mode: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    pub context_used_percent: Option<u8>,
    pub context_window: Option<u64>,
    pub terminal_focused: bool,
    pub theme: &'a Theme,
}

pub fn transcript_viewport_size(
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
) -> (u16, u16) {
    let composer_height = input_metrics(input, area.width).height;
    let reserved = composer_height + COMPOSER_GAP_HEIGHT + STATUS_HEIGHT;
    (area.width, area.height.saturating_sub(reserved).max(1))
}

pub fn render(frame: &mut Frame, params: &mut RenderParams<'_>) {
    let area = frame.area();
    let composer_metrics = input_metrics(params.input, area.width);

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
        theme: params.theme,
    };
    frame.render_widget(input_widget, chunks[2]);

    let status_bar = StatusBar {
        model: params.model,
        thinking_level: params.thinking_level,
        approval_mode: params.approval_mode,
        status_message: params.status_message,
        is_connected: params.is_connected,
        compaction_count: params.compaction_count,
        context_used_percent: params.context_used_percent,
        context_window: params.context_window,
        theme: params.theme,
    };
    frame.render_widget(status_bar, chunks[3]);
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use ratatui_textarea::TextArea;

    use super::{RenderParams, render, transcript_viewport_size};
    use crate::theme::Theme;
    use crate::transcript::{Transcript, TranscriptRenderCache};

    #[test]
    fn composer_spans_the_full_pane_width() {
        let backend = TestBackend::new(160, 24);
        let Ok(mut terminal) = Terminal::new(backend);
        let transcript = Transcript::new();
        let mut transcript_cache = TranscriptRenderCache::default();
        let mut input = TextArea::from(["hello"]);

        let result = terminal.draw(|frame| {
            render(
                frame,
                &mut RenderParams {
                    transcript: &transcript,
                    transcript_cache: &mut transcript_cache,
                    scroll_offset: 0,
                    is_streaming: false,
                    input: &mut input,
                    model: "model",
                    thinking_level: "high",
                    approval_mode: "review",
                    status_message: "ready",
                    is_connected: true,
                    compaction_count: 0,
                    context_used_percent: None,
                    context_window: None,
                    terminal_focused: true,
                    theme: &Theme::default(),
                },
            );
        });

        assert!(result.is_ok());
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 20)].symbol(), "┌");
        assert_eq!(buffer[(159, 20)].symbol(), "┐");
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
    fn transcript_viewport_wraps_against_the_full_pane_width() {
        let long_line = "x".repeat(300);
        let input = TextArea::from([long_line.as_str()]);
        assert_eq!(
            transcript_viewport_size(Rect::new(0, 0, 160, 24), &input),
            (160, 18)
        );
    }
}
