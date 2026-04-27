use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use yach_proto::ModelInfo;

pub struct ModelSelector<'a> {
    pub models: &'a [ModelInfo],
    pub current_model: &'a str,
    pub selected_index: usize,
}

impl Widget for ModelSelector<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(70, 60, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .title("Select Model")
            .title_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines = if self.models.is_empty() {
            vec![
                Line::from(Span::styled(
                    "Available models are loading from the backend...",
                    Style::new().fg(Color::Yellow),
                )),
                Line::raw(""),
                Line::from("Close with Esc and try again shortly."),
            ]
        } else {
            let mut lines = vec![
                Line::from(Span::styled(
                    "Backend-provided models. j/k or arrows move; Enter requests a model change; current model updates after confirmation.",
                    Style::new().fg(Color::DarkGray),
                )),
                Line::raw(""),
            ];
            let visible_rows = usize::from(inner.height).saturating_sub(4).max(1);
            let scroll_start = self
                .selected_index
                .min(self.models.len().saturating_sub(1))
                .saturating_sub(visible_rows.saturating_sub(1));
            lines.extend(
                self.models
                    .iter()
                    .enumerate()
                    .skip(scroll_start)
                    .take(visible_rows)
                    .map(|(i, model)| {
                        let is_selected = i == self.selected_index;
                        let model_label = model.label();
                        let is_current = model_label == self.current_model
                            || model.id == self.current_model
                            || model.name == self.current_model;
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
                            Span::styled(
                                format!("{} — {}{suffix}", model.label(), model.name),
                                style,
                            ),
                        ])
                    }),
            );
            if scroll_start > 0 {
                lines.insert(
                    2,
                    Line::from(Span::styled("  ↑ more", Style::new().fg(Color::DarkGray))),
                );
            }
            if scroll_start + visible_rows < self.models.len() {
                lines.push(Line::from(Span::styled(
                    "  ↓ more",
                    Style::new().fg(Color::DarkGray),
                )));
            }
            lines
        };

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
