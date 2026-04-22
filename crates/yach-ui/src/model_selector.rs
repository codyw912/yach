use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

pub const KNOWN_MODELS: &[&str] = &[
    "gpt-5",
    "gpt-4o",
    "gpt-4o-mini",
    "claude-sonnet-4-20250514",
    "claude-opus-4-20250514",
    "claude-3-5-sonnet-20241022",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
];

pub struct ModelSelector<'a> {
    pub models: &'a [&'a str],
    pub current_model: &'a str,
    pub selected_index: usize,
}

impl Widget for ModelSelector<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(60, 50, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Select Model")
            .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines: Vec<Line<'_>> = self
            .models
            .iter()
            .enumerate()
            .map(|(i, model)| {
                let is_selected = i == self.selected_index;
                let is_current = *model == self.current_model;
                let prefix = if is_selected { "▸ " } else { "  " };
                let suffix = if is_current { " (current)" } else { "" };
                let style = if is_selected {
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().fg(Color::Gray)
                };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{model}{suffix}"), style),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        Widget::render(paragraph, inner, buf);
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
