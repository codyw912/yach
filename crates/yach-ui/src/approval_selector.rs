use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use yach_proto::ApprovalMode;

use crate::theme::Theme;

pub struct ApprovalModeSelector<'a> {
    pub current_mode: ApprovalMode,
    pub selected_index: usize,
    pub theme: &'a Theme,
}

impl Widget for ApprovalModeSelector<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        Clear.render(area, buf);
        let popup_area = centered_rect(area, 32, 5);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(self.theme.colors.border))
            .title(" Approval Mode ")
            .style(Style::new().fg(self.theme.colors.accent));
        let lines = ApprovalMode::ALL
            .iter()
            .enumerate()
            .map(|(index, mode)| {
                let selected = index == self.selected_index;
                let current = *mode == self.current_mode;
                let style = if selected {
                    Style::new()
                        .fg(self.theme.colors.selected_text)
                        .add_modifier(Modifier::BOLD)
                } else if current {
                    Style::new().fg(self.theme.colors.success)
                } else {
                    Style::new().fg(self.theme.colors.muted)
                };
                Line::from(Span::styled(
                    format!(
                        "{}{}{}",
                        if selected { "▸ " } else { "  " },
                        if current { "● " } else { "  " },
                        mode.as_str()
                    ),
                    style,
                ))
            })
            .collect::<Vec<_>>();
        Widget::render(Paragraph::new(lines).block(block), popup_area, buf);
    }
}

pub struct FullAccessConfirmation<'a> {
    pub enable_selected: bool,
    pub theme: &'a Theme,
}

impl Widget for FullAccessConfirmation<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        Clear.render(area, buf);
        let popup_area = centered_rect(area, 76, 12);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(self.theme.colors.error))
            .title(" Full Access — Host Access ")
            .title_style(
                Style::new()
                    .fg(self.theme.colors.error)
                    .add_modifier(Modifier::BOLD),
            );
        let enable_style = if self.enable_selected {
            Style::new()
                .fg(self.theme.colors.selected_text)
                .bg(self.theme.colors.error)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(self.theme.colors.error)
        };
        let cancel_style = if self.enable_selected {
            Style::new().fg(self.theme.colors.muted)
        } else {
            Style::new()
                .fg(self.theme.colors.selected_text)
                .bg(self.theme.colors.success)
                .add_modifier(Modifier::BOLD)
        };
        let lines = vec![
            Line::from("Commands run directly on this host and may access:"),
            Line::from("• files outside the project"),
            Line::from("• credentials, network services, and other processes"),
            Line::from(""),
            Line::from("This mode lasts for this session only."),
            Line::from(""),
            Line::from(Span::styled(
                if self.enable_selected {
                    "› Enable for this session"
                } else {
                    "  Enable for this session"
                },
                enable_style,
            )),
            Line::from(Span::styled(
                if self.enable_selected {
                    "  Cancel"
                } else {
                    "› Cancel"
                },
                cancel_style,
            )),
            Line::from("↑/↓ or j/k select · Enter confirm · Esc cancel"),
        ];
        Widget::render(Paragraph::new(lines).block(block), popup_area, buf);
    }
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}
