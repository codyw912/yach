use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Widget};
use ratatui_textarea::{TextArea, WrapMode};

const MIN_INPUT_HEIGHT: u16 = 3;
const MAX_INPUT_HEIGHT: u16 = 8;

pub struct InputComposer<'a> {
    pub textarea: &'a TextArea<'static>,
    pub is_streaming: bool,
}

impl Widget for InputComposer<'_> {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let title = if self.is_streaming {
            "input (streaming...)"
        } else {
            "input (enter to send, ctrl+j newline)"
        };

        let mut textarea = self.textarea.clone();
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(Style::new().fg(Color::Yellow)),
        );
        textarea.set_wrap_mode(WrapMode::Word);
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));

        Widget::render(&textarea, area, buf);
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
    use ratatui_textarea::TextArea;

    use super::input_height;

    #[test]
    fn input_height_grows_for_wrapped_text() {
        assert_eq!(input_height(&TextArea::from(["hello"]), 20), 3);
        assert_eq!(input_height(&TextArea::from(["abcdef"]), 4), 5);
    }

    #[test]
    fn input_height_counts_explicit_newlines() {
        assert_eq!(input_height(&TextArea::from(["a", "b"]), 20), 4);
    }
}
