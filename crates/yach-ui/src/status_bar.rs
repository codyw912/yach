use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Deliberately minimal (owner decision, 2026-07-21): the bar holds only
/// what is useful at a glance — connection, model, context pressure,
/// compaction count, and the current status message, which renders last
/// so overflow clips it rather than the meters. Session id and thinking
/// level moved out; the full layout question belongs to the UX-sprint
/// status-bar design pass (docs/project/next.md).
pub struct StatusBar<'a> {
    pub model: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    /// Estimated percent of the usable context window in use; colored as
    /// a warning while the auto-compaction threshold approaches.
    pub context_used_percent: Option<u8>,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let connection_indicator = if self.is_connected {
            Span::styled("●", Style::new().fg(Color::Green))
        } else {
            Span::styled("●", Style::new().fg(Color::Red))
        };

        let mut parts = vec![
            connection_indicator,
            Span::raw(" "),
            Span::styled(self.model.to_owned(), Style::new().fg(Color::Cyan)),
        ];
        if let Some(percent) = self.context_used_percent {
            let color = match percent {
                0..=69 => Color::DarkGray,
                70..=89 => Color::Yellow,
                _ => Color::Red,
            };
            parts.push(Span::raw("  "));
            parts.push(Span::styled(
                format!("ctx:{percent}%"),
                Style::new().fg(color),
            ));
        }
        if self.compaction_count > 0 {
            parts.push(Span::raw("  "));
            parts.push(Span::styled(
                format!("⟲{}", self.compaction_count),
                Style::new().fg(Color::Magenta),
            ));
        }
        parts.push(Span::raw("  "));
        parts.push(Span::styled(
            self.status_message,
            Style::new().fg(Color::Gray),
        ));

        let paragraph = Paragraph::new(Line::from(parts));
        Widget::render(paragraph, area, buf);
    }
}
