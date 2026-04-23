use ratatui::layout::Rect;
use ratatui::prelude::Stylize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::branch_tracker::BranchTracker;

pub struct BranchSummaryOverlay<'a> {
    pub tracker: &'a BranchTracker,
}

impl Widget for BranchSummaryOverlay<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        Clear.render(area, buf);

        let tree = self.tracker.branch_tree();
        let current = self.tracker.current();

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "── Branch Summary ──",
            Style::default().fg(Color::Cyan),
        )));
        lines.push(Line::raw(""));

        if tree.is_empty() {
            lines.push(Line::from(Span::styled(
                "no branches",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for branch in &tree {
                let is_current = branch.session_id == current;
                let prefix = if is_current { "▸ " } else { "  " };
                let id_short = if branch.session_id.len() > 12 {
                    format!(
                        "...{}",
                        &branch.session_id[branch.session_id.len().saturating_sub(8)..]
                    )
                } else {
                    branch.session_id.clone()
                };
                let style = if is_current {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let text = format!("{}{} ({} entries)", prefix, id_short, branch.entry_count);
                lines.push(Line::from(Span::styled(text, style)));
            }
        }

        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Ctrl+B to close",
            Style::default().fg(Color::DarkGray).dim(),
        )));

        let width = lines
            .iter()
            .map(|l| {
                u16::try_from(l.spans.iter().map(|s| s.content.len()).sum::<usize>()).unwrap_or(30)
            })
            .max()
            .unwrap_or(30)
            .min(area.width.saturating_sub(4))
            + 4;
        let height = u16::try_from(lines.len()).unwrap_or(10) + 2;
        let popup_width = width.min(area.width);
        let popup_height = height.min(area.height);
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Branches ")
            .style(Style::default().fg(Color::Cyan));

        let paragraph = Paragraph::new(lines).block(block);
        Widget::render(paragraph, popup_area, buf);
    }
}
