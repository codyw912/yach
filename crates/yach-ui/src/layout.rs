use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::Span;

use crate::input::{InputComposer, composer_marker, input_metrics, input_styles};
use crate::status_bar::{StatusBar, StatusFill, StatusMessage};
use crate::theme::{ComposerStyle, Theme};
use crate::transcript;
use crate::transcript::{Transcript, TranscriptAlignment, TranscriptRenderCache};

pub struct RenderParams<'a> {
    pub transcript: &'a Transcript,
    pub transcript_cache: &'a mut TranscriptRenderCache,
    pub scroll_offset: usize,
    pub transcript_alignment: TranscriptAlignment,
    pub is_streaming: bool,
    pub input: &'a mut ratatui_textarea::TextArea<'static>,
    pub model: &'a str,
    pub thinking_level: &'a str,
    pub approval_mode: &'a str,
    pub reviewer_id: Option<&'a str>,
    pub status_message: &'a StatusMessage,
    pub is_connected: bool,
    pub compaction_count: u64,
    pub total_tokens: Option<u64>,
    pub context_used_percent: Option<u8>,
    /// Window and whether it is the catalog floor rather than a listed value.
    pub context_window: Option<(u64, bool)>,
    pub terminal_focused: bool,
    pub theme: &'a Theme,
}

/// Shared geometry keeps the transcript viewport and drawn composer aligned.
struct ComposerAreas {
    transcript: Rect,
    status: Rect,
    input: Rect,
    closing: Option<Rect>,
}

fn composer_areas(area: Rect, style: ComposerStyle, prompt_rows: u16) -> ComposerAreas {
    match style {
        ComposerStyle::Box => {
            let [transcript, _gap, input, status] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows.saturating_add(2)),
                Constraint::Length(1),
            ])
            .areas(area);
            ComposerAreas {
                transcript,
                status,
                input,
                closing: None,
            }
        }
        ComposerStyle::Rule => {
            let [transcript, status, input, closing] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows),
                Constraint::Length(1),
            ])
            .areas(area);
            ComposerAreas {
                transcript,
                status,
                input,
                closing: Some(closing),
            }
        }
        ComposerStyle::Band => {
            let [transcript, status, input] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows),
            ])
            .areas(area);
            ComposerAreas {
                transcript,
                status,
                input,
                closing: None,
            }
        }
    }
}

pub fn transcript_viewport_size(
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
    theme: &Theme,
) -> (u16, u16) {
    let prompt_rows = input_metrics(input, area.width).prompt_rows;
    let areas = composer_areas(area, theme.composer_style(), prompt_rows);
    (area.width, areas.transcript.height.max(1))
}

pub fn render(frame: &mut Frame, params: &mut RenderParams<'_>) {
    let area = frame.area();
    let style = params.theme.composer_style();
    let metrics = input_metrics(params.input, area.width);
    let areas = composer_areas(area, style, metrics.prompt_rows);

    transcript::render(
        areas.transcript,
        frame.buffer_mut(),
        params.transcript,
        params.transcript_cache,
        params.scroll_offset,
        params.is_streaming,
        params.transcript_alignment,
    );

    let input_widget = InputComposer {
        textarea: &mut *params.input,
        is_streaming: params.is_streaming,
        terminal_focused: params.terminal_focused,
        overflowed: metrics.overflowed,
        style,
        theme: params.theme,
    };
    frame.render_widget(input_widget, areas.input);

    let styles = input_styles(params.terminal_focused, params.theme);
    let fill = match (style, params.theme.status_band()) {
        (ComposerStyle::Box, _) => StatusFill::None,
        (ComposerStyle::Band, Some(color)) => StatusFill::Band(color),
        (ComposerStyle::Rule | ComposerStyle::Band, _) => StatusFill::Rule(styles.border),
    };
    // Band markers use the model's contrast-safe roles, not accent or DIM.
    let marker_style = match (fill, params.terminal_focused) {
        (StatusFill::Band(_), true) => params.theme.text().add_modifier(Modifier::BOLD),
        (StatusFill::Band(_), false) => params.theme.muted(),
        _ => styles.title,
    };
    let marker = (style != ComposerStyle::Box)
        .then(|| composer_marker(params.is_streaming, metrics.overflowed))
        .flatten()
        .map(|marker| Span::styled(marker, marker_style));
    let status_bar = StatusBar {
        model: params.model,
        thinking_level: params.thinking_level,
        approval_mode: params.approval_mode,
        reviewer_id: params.reviewer_id,
        status_message: params.status_message,
        is_connected: params.is_connected,
        compaction_count: params.compaction_count,
        total_tokens: params.total_tokens,
        context_used_percent: params.context_used_percent,
        context_window: params.context_window,
        theme: params.theme,
        fill,
        marker,
    };
    frame.render_widget(status_bar, areas.status);
    if let Some(closing) = areas.closing.filter(|rect| !rect.is_empty()) {
        let rule = "─".repeat(usize::from(closing.width));
        frame
            .buffer_mut()
            .set_string(closing.x, closing.y, rule, styles.border);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use ratatui_textarea::TextArea;

    use super::{RenderParams, render, transcript_viewport_size};
    use crate::status_bar::StatusMessage;
    use crate::theme::{ComposerStyle, Theme, ThemeDocument};
    use crate::transcript::{Transcript, TranscriptAlignment, TranscriptRenderCache};

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
                    transcript_alignment: TranscriptAlignment::Bottom,
                    is_streaming: false,
                    input: &mut input,
                    model: "model",
                    thinking_level: "high",
                    approval_mode: "review",
                    reviewer_id: None,
                    status_message: &StatusMessage::info("ready"),
                    is_connected: true,
                    compaction_count: 0,
                    total_tokens: None,
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
            transcript_viewport_size(Rect::new(0, 0, 80, 24), &input, &Theme::default()),
            (80, 19)
        );
    }

    #[test]
    fn transcript_viewport_wraps_against_the_full_pane_width() {
        let long_line = "x".repeat(300);
        let input = TextArea::from([long_line.as_str()]);
        assert_eq!(
            transcript_viewport_size(Rect::new(0, 0, 160, 24), &input, &Theme::default()),
            (160, 18)
        );
    }

    fn draw(theme: &Theme, input: &mut TextArea<'static>) -> Buffer {
        let backend = TestBackend::new(80, 24);
        let Ok(mut terminal) = Terminal::new(backend);
        let transcript = Transcript::new();
        let mut transcript_cache = TranscriptRenderCache::default();
        let status = StatusMessage::info("ready");
        let result = terminal.draw(|frame| {
            render(
                frame,
                &mut RenderParams {
                    transcript: &transcript,
                    transcript_cache: &mut transcript_cache,
                    scroll_offset: 0,
                    transcript_alignment: TranscriptAlignment::Bottom,
                    is_streaming: false,
                    input,
                    model: "model",
                    thinking_level: "high",
                    approval_mode: "review",
                    reviewer_id: None,
                    status_message: &status,
                    is_connected: true,
                    compaction_count: 0,
                    total_tokens: None,
                    context_used_percent: None,
                    context_window: None,
                    terminal_focused: true,
                    theme,
                },
            );
        });
        assert!(result.is_ok());
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    #[test]
    fn viewport_ends_where_each_style_starts_drawing() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let mut theme = Theme::dark();
            theme.composer = style;
            for lines in [1_u16, 3, 7] {
                let mut input = TextArea::new(vec![String::from("line"); usize::from(lines)]);
                let (_, height) = transcript_viewport_size(Rect::new(0, 0, 80, 24), &input, &theme);
                let buffer = draw(&theme, &mut input);
                let prompt_rows = lines.min(6);
                let label = format!("{style:?}, {lines} lines");
                let first = row(&buffer, height);
                let status_end = if lines > 6 { "more ↑ " } else { "─" };
                match style {
                    ComposerStyle::Box => {
                        assert_eq!(height, 24 - prompt_rows - 4, "{label}");
                        assert!(first.trim().is_empty(), "{label}: gap row {first:?}");
                        assert!(row(&buffer, height + 1).starts_with('┌'), "{label}");
                        assert!(row(&buffer, 23).contains("approval:review"), "{label}");
                    }
                    ComposerStyle::Rule => {
                        assert_eq!(height, 24 - prompt_rows - 2, "{label}");
                        assert!(first.contains("approval:review"), "{label}: {first}");
                        assert!(first.ends_with(status_end), "{label}: {first}");
                        assert!(row(&buffer, height + 1).starts_with('›'), "{label}");
                        assert_eq!(row(&buffer, 23), "─".repeat(80), "{label}");
                    }
                    ComposerStyle::Band => {
                        assert_eq!(height, 24 - prompt_rows - 1, "{label}");
                        assert!(first.contains("approval:review"), "{label}: {first}");
                        assert_eq!(buffer[(0, height)].bg, Color::Rgb(40, 40, 50), "{label}");
                        assert!(row(&buffer, height + 1).starts_with('›'), "{label}");
                        if lines > 6 {
                            assert!(first.ends_with("more ↑ "), "{label}: {first}");
                            let marker_x = (0..buffer.area.width).find(|&x| {
                                (x..buffer.area.width)
                                    .map(|x| buffer[(x, height)].symbol())
                                    .collect::<String>()
                                    .starts_with("more")
                            });
                            assert!(marker_x.is_some(), "{label}: {first}");
                            let Some(marker_x) = marker_x else { return };
                            let marker = &buffer[(marker_x, height)];
                            assert_eq!(Some(marker.fg), theme.text().fg, "{label}: band marker");
                            assert!(marker.modifier.contains(Modifier::BOLD), "{label}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn composer_renders_in_tiny_terminals_while_streaming() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let mut theme = Theme::dark();
            theme.composer = style;
            for height in 1..=3 {
                for width in [1, 2, 40] {
                    for lines in [1, 7] {
                        let backend = TestBackend::new(width, height);
                        let Ok(mut terminal) = Terminal::new(backend);
                        let transcript = Transcript::new();
                        let mut transcript_cache = TranscriptRenderCache::default();
                        let mut input = TextArea::new(vec![String::from("line"); lines]);
                        let status = StatusMessage::info("ready");
                        let result = terminal.draw(|frame| {
                            render(
                                frame,
                                &mut RenderParams {
                                    transcript: &transcript,
                                    transcript_cache: &mut transcript_cache,
                                    scroll_offset: 0,
                                    transcript_alignment: TranscriptAlignment::Bottom,
                                    is_streaming: true,
                                    input: &mut input,
                                    model: "model",
                                    thinking_level: "high",
                                    approval_mode: "review",
                                    reviewer_id: None,
                                    status_message: &status,
                                    is_connected: true,
                                    compaction_count: 0,
                                    total_tokens: None,
                                    context_used_percent: None,
                                    context_window: None,
                                    terminal_focused: true,
                                    theme: &theme,
                                },
                            );
                        });
                        assert!(result.is_ok(), "{style:?}, {width}x{height}, {lines} lines");
                    }
                }
            }
        }
    }

    #[test]
    fn band_without_a_band_background_draws_rule() {
        let reset = ThemeDocument::parse(r#"{"colors":{"statusBackground":"default"}}"#)
            .and_then(|document| document.apply(Theme::dark()));
        assert!(reset.is_ok());
        let Ok(reset) = reset else { return };
        for base in [Theme::system(), Theme::monochrome(), reset] {
            let (mut band, mut rule) = (base, base);
            band.composer = ComposerStyle::Band;
            rule.composer = ComposerStyle::Rule;
            assert_eq!(
                draw(&band, &mut TextArea::from(["hello"])),
                draw(&rule, &mut TextArea::from(["hello"])),
                "{:?}",
                base.mode()
            );
        }
    }
}
