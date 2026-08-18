use ratatui::style::{Color, Modifier, Style};
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InputStyles {
    border: Style,
    title: Style,
    cursor: Style,
}

fn input_styles(terminal_focused: bool) -> InputStyles {
    if terminal_focused {
        InputStyles {
            border: Style::default(),
            title: Style::new().fg(Color::Yellow),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
        }
    } else {
        let dim = Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM);
        InputStyles {
            border: dim,
            title: dim,
            cursor: Style::default().add_modifier(Modifier::HIDDEN),
        }
    }
}

impl Widget for InputComposer<'_> {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let title = if self.is_streaming {
            "input (streaming...)"
        } else {
            "input (enter to send, ctrl+j newline)"
        };
        let styles = input_styles(self.terminal_focused);

        self.textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(styles.border)
                .title(title)
                .title_style(styles.title),
        );
        self.textarea.set_wrap_mode(WrapMode::Word);
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_cursor_style(styles.cursor);

        Widget::render(&*self.textarea, area, buf);
    }
}

pub fn input_height(textarea: &TextArea<'_>, area_width: u16) -> u16 {
    let inner_width = area_width.saturating_sub(2) as usize;
    let line_count = textarea
        .lines()
        .iter()
        .map(|line| wrapped_line_count(line, inner_width))
        .sum::<usize>()
        .max(1);

    u16::try_from(line_count)
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .clamp(MIN_INPUT_HEIGHT, MAX_INPUT_HEIGHT)
}

fn wrapped_line_count(line: &str, width: usize) -> usize {
    if line.is_empty() || width == 0 {
        return 1;
    }

    let mut count = 1;
    let mut current_width = 0;
    for ch in line.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width > 0 && current_width + ch_width > width {
            count += 1;
            current_width = 0;
        }
        current_width += ch_width;
    }
    count
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};
    use ratatui_textarea::TextArea;

    use super::{input_height, input_styles};

    #[test]
    fn input_height_grows_for_wrapped_text() {
        assert_eq!(input_height(&TextArea::from(["hello"]), 20), 3);
        assert_eq!(input_height(&TextArea::from(["abcdef"]), 4), 5);
    }

    #[test]
    fn input_height_counts_explicit_newlines() {
        assert_eq!(input_height(&TextArea::from(["a", "b"]), 20), 4);
    }

    #[test]
    fn input_styles_dim_and_hide_cursor_when_unfocused() {
        let focused = input_styles(true);
        let unfocused = input_styles(false);

        assert_eq!(
            focused.title,
            Style::new().fg(ratatui::style::Color::Yellow)
        );
        assert_ne!(focused.border, unfocused.border);
        assert_eq!(
            unfocused.cursor,
            Style::default().add_modifier(Modifier::HIDDEN)
        );
    }
}
