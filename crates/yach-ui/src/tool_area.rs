use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

pub struct ToolArea<'a> {
    pub active_tools: &'a [String],
}

impl Widget for ToolArea<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if self.active_tools.is_empty() {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .title("tools")
            .title_style(Style::new().fg(Color::Magenta));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let lines: Vec<Line<'_>> = self
            .active_tools
            .iter()
            .map(|tool| {
                Line::from(vec![
                    Span::styled("⟳ ", Style::new().fg(Color::Yellow)),
                    Span::raw(tool.clone()),
                ])
            })
            .take(inner.height as usize)
            .collect();

        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);
    }
}
