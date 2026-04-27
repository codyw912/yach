use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use yach_proto::ForkMessage;

pub struct ForkPicker<'a> {
    pub messages: &'a [ForkMessage],
    pub selected_index: usize,
}

impl Widget for ForkPicker<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(70, 60, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Fork From Message")
            .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines: Vec<Line<'_>> = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, message)| {
                let is_selected = i == self.selected_index;
                let prefix = if is_selected { "▸ " } else { "  " };
                let style = if is_selected {
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(Color::Gray)
                };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(
                        format!("{} — {}", message.entry_id, preview(&message.text)),
                        style,
                    ),
                ])
            })
            .collect();

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  Enter", Style::new().fg(Color::Yellow)),
            Span::styled(
                " fork before selected message · ",
                Style::new().fg(Color::DarkGray),
            ),
            Span::styled("Esc", Style::new().fg(Color::Yellow)),
            Span::styled(" cancel", Style::new().fg(Color::DarkGray)),
        ]));

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: true });
        Widget::render(paragraph, inner, buf);
    }
}

fn preview(text: &str) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = flattened.chars().take(96).collect();
    if flattened.chars().count() > 96 {
        format!("{preview}...")
    } else {
        preview
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
