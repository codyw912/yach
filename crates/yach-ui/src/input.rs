use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Widget};
use ratatui_textarea::{TextArea, WrapMode};

const MIN_INPUT_HEIGHT: u16 = 3;
const MAX_INPUT_HEIGHT: u16 = 8;

/// Renders the persistent prompt textarea directly (not a clone): the
/// widget records its rendered area on the textarea, and wrap-aware cursor
/// movement (up/down across soft-wrapped display rows) only works when the
/// same instance that receives key input has seen its area and wrap mode.
pub struct InputComposer<'a> {
    pub textarea: &'a mut TextArea<'static>,
    pub is_streaming: bool,
    pub terminal_focused: bool,
    pub overflowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputStyles {
    border: Style,
    title: Style,
    hint: Style,
    cursor: Style,
}

fn input_styles(terminal_focused: bool) -> InputStyles {
    if terminal_focused {
        InputStyles {
            border: Style::new().fg(Color::DarkGray),
            title: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            hint: Style::new().fg(Color::DarkGray),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
        }
    } else {
        let dim = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        InputStyles {
            border: dim,
            title: dim,
            hint: dim,
            cursor: Style::default().add_modifier(Modifier::HIDDEN),
        }
    }
}

impl Widget for InputComposer<'_> {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let title = match (self.is_streaming, self.overflowed) {
            (true, true) => Some(" running · more ↑ "),
            (true, false) => Some(" running "),
            (false, true) => Some(" more ↑ "),
            (false, false) => None,
        };
        let styles = input_styles(self.terminal_focused);
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border);
        if let Some(title) = title {
            block = block.title(Line::styled(title, styles.title));
        }
        if area.width >= 42 {
            block = block.title_bottom(Line::styled(" enter send · ctrl+j newline ", styles.hint));
        }

        self.textarea.set_block(block);
        self.textarea.set_wrap_mode(WrapMode::Glyph);
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_cursor_style(styles.cursor);

        Widget::render(&*self.textarea, area, buf);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMetrics {
    pub height: u16,
    pub overflowed: bool,
}

pub fn input_metrics(textarea: &TextArea<'_>, area_width: u16) -> InputMetrics {
    let inner_width = area_width.saturating_sub(2) as usize;
    let tab_length = textarea.tab_length();
    let content_height = textarea
        .lines()
        .iter()
        .map(|line| wrapped_line_count(line, inner_width, tab_length))
        .sum::<usize>()
        .max(1);
    let total_height = content_height.saturating_add(2);
    InputMetrics {
        height: u16::try_from(total_height)
            .unwrap_or(u16::MAX)
            .clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT),
        overflowed: total_height > usize::from(MAX_INPUT_HEIGHT),
    }
}

fn wrapped_line_count(line: &str, width: usize, tab_length: u8) -> usize {
    if line.is_empty() {
        return 1;
    }

    let width = width.max(1);
    let mut count = 1;
    let mut current_width = 0;
    for grapheme in line.graphemes(true) {
        let next_width = display_width_to(grapheme, current_width, tab_length);
        let grapheme_width = next_width.saturating_sub(current_width);
        if current_width > 0 && current_width.saturating_add(grapheme_width) > width {
            count += 1;
            current_width = display_width_to(grapheme, 0, tab_length);
        } else {
            current_width = next_width;
        }
    }
    count
}

fn display_width_to(text: &str, mut width: usize, tab_length: u8) -> usize {
    for character in text.chars() {
        if character == '\t' && tab_length > 0 {
            let tab_length = usize::from(tab_length);
            width += tab_length - (width % tab_length);
        } else {
            width += character.width().unwrap_or(0);
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::widgets::Widget;
    use ratatui_textarea::TextArea;

    use super::{InputComposer, input_metrics, input_styles};

    #[test]
    fn input_height_grows_for_wrapped_text() {
        assert_eq!(input_metrics(&TextArea::from(["hello"]), 20).height, 3);
        assert_eq!(input_metrics(&TextArea::from(["abcdef"]), 4).height, 5);
    }

    #[test]
    fn input_height_counts_explicit_newlines() {
        assert_eq!(input_metrics(&TextArea::from(["a", "b"]), 20).height, 4);
    }

    #[test]
    fn input_styles_dim_and_hide_cursor_when_unfocused() {
        let focused = input_styles(true);
        let unfocused = input_styles(false);

        assert_eq!(
            focused.title,
            Style::new()
                .fg(ratatui::style::Color::Cyan)
                .add_modifier(Modifier::BOLD)
        );
        assert_ne!(focused.border, unfocused.border);
        assert_eq!(
            unfocused.cursor,
            Style::default().add_modifier(Modifier::HIDDEN)
        );
    }

    #[test]
    fn capped_input_title_reports_running_and_overflow() {
        let mut input = TextArea::from(["one", "two", "three", "four", "five", "six", "seven"]);
        let metrics = input_metrics(&input, 80);
        assert_eq!(metrics.height, 8);
        assert!(metrics.overflowed);

        let area = Rect::new(0, 0, 80, metrics.height);
        let mut buffer = Buffer::empty(area);
        Widget::render(
            InputComposer {
                textarea: &mut input,
                is_streaming: true,
                terminal_focused: true,
                overflowed: metrics.overflowed,
            },
            area,
            &mut buffer,
        );
        let title = (0..area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        assert!(title.contains("running · more ↑"));
        assert!(!title.contains("message"));
    }

    #[test]
    fn composer_glyph_wrap_matches_metrics_for_a_long_token() {
        let mut input = TextArea::from(["abcdefghij"]);
        let area = Rect::new(0, 0, 6, input_metrics(&input, 6).height);
        assert_eq!(area.height, 5);
        let mut buffer = Buffer::empty(area);

        Widget::render(
            InputComposer {
                textarea: &mut input,
                is_streaming: false,
                terminal_focused: true,
                overflowed: false,
            },
            area,
            &mut buffer,
        );

        let content = (1..area.height - 1)
            .map(|y| {
                (1..area.width - 1)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(content, ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn idle_composer_uses_a_clean_top_border_and_bottom_hint() {
        let mut input = TextArea::from(["hello"]);
        let area = Rect::new(0, 0, 48, 3);
        let mut buffer = Buffer::empty(area);
        Widget::render(
            InputComposer {
                textarea: &mut input,
                is_streaming: false,
                terminal_focused: true,
                overflowed: false,
            },
            area,
            &mut buffer,
        );
        let top = (0..area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        let bottom = (0..area.width)
            .map(|x| buffer[(x, 2)].symbol())
            .collect::<String>();
        assert_eq!(top, format!("┌{}┐", "─".repeat(46)));
        assert!(bottom.contains("enter send · ctrl+j newline"));
    }
}
