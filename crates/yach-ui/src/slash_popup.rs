use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::slash_commands::SlashCommand;
use crate::theme::Theme;

pub struct SlashPopup<'a> {
    pub selected: usize,
    pub matches: Vec<SlashCommand>,
    pub theme: &'a Theme,
}

impl Widget for SlashPopup<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if self.matches.is_empty() {
            return;
        }

        let height = u16::try_from(self.matches.len() + 2).unwrap_or(10).min(10);
        let width = 40;
        let popup_area = bottom_left_rect(width, height, area);

        Clear.render(popup_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(self.theme.colors.border))
            .title("Commands · Tab accepts")
            .title_style(
                Style::new()
                    .fg(self.theme.colors.accent)
                    .add_modifier(Modifier::BOLD),
            );

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let lines: Vec<Line<'_>> = self
            .matches
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                let is_selected = i == self.selected;
                let style = if is_selected {
                    Style::new()
                        .fg(self.theme.colors.selected_text)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(self.theme.colors.muted)
                };
                let prefix = if is_selected { "▸ " } else { "  " };
                Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(cmd.name, Style::new().fg(self.theme.colors.accent)),
                    Span::styled(" - ", Style::new().fg(self.theme.colors.dim)),
                    Span::styled(cmd.description, style),
                ])
            })
            .collect();

        Widget::render(
            Paragraph::new(lines).style(Style::new().fg(self.theme.colors.text)),
            inner,
            buf,
        );
    }
}

fn bottom_left_rect(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x,
        y: area.height.saturating_sub(height + 4),
        width: width.min(area.width),
        height,
    }
}
