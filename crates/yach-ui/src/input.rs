use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub struct InputComposer<'a> {
    pub buffer: &'a str,
    pub cursor_pos: usize,
    pub is_focused: bool,
    pub is_streaming: bool,
}

impl Widget for InputComposer<'_> {
    fn render(self, area: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(if self.is_streaming {
                "input (streaming...)"
            } else {
                "input (enter to send)"
            })
            .title_style(Style::new().fg(Color::Yellow));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let prompt = Span::styled("> ", Style::new().fg(Color::Cyan).bold());

        let line = if self.is_focused {
            let before = &self.buffer[..self.cursor_pos.min(self.buffer.len())];
            let after = &self.buffer[self.cursor_pos.min(self.buffer.len())..];

            Line::from(vec![
                prompt,
                Span::raw(before),
                Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
                Span::raw(after),
            ])
        } else {
            Line::from(vec![prompt, Span::raw(self.buffer)])
        };

        let paragraph = Paragraph::new(line);
        Widget::render(paragraph, inner, buf);
    }
}
