use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::theme::Theme;

pub struct SessionPicker<'a> {
    pub sessions: &'a [String],
    pub labels: &'a [String],
    pub current_session: &'a str,
    pub selected_index: usize,
    pub show_fork_hint: bool,
    pub theme: &'a Theme,
}

impl Widget for SessionPicker<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let popup_area = centered_rect(60, 50, area);
        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.theme.border(false))
            .title("Select Session")
            .title_style(self.theme.accent().add_modifier(Modifier::BOLD));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines: Vec<Line<'_>> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, session)| {
                let label = self.labels.get(i).unwrap_or(session);
                let is_selected = i == self.selected_index;
                let is_current = session == self.current_session;
                let prefix = if is_selected { "▸ " } else { "  " };
                let suffix = if is_current { " (current)" } else { "" };
                let style = if is_selected {
                    self.theme.selected_label()
                } else if is_current {
                    self.theme.warning()
                } else {
                    self.theme.muted()
                };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("{label}{suffix}"), style),
                ])
            })
            .collect();

        if self.show_fork_hint {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  ", Style::new()),
                Span::styled("Ctrl+F to fork current session", self.theme.dim()),
            ]));
        }

        let paragraph = Paragraph::new(lines).style(self.theme.text());
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
