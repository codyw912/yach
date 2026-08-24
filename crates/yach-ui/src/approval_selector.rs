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
        let popup_area = centered_rect(area, 32, 4);
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
