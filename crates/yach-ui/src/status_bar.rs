use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session_id: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    pub thinking_level: &'a str,
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let connection_indicator = if self.is_connected {
            Span::styled("●", Style::new().fg(Color::Green))
        } else {
            Span::styled("●", Style::new().fg(Color::Red))
        };

        let model_span = Span::styled(
            format!("model:{}", self.model),
            Style::new().fg(Color::Cyan),
        );
        let session_span = Span::styled(
            format!("session:{}", self.session_id),
            Style::new().fg(Color::Yellow),
        );
        let thinking_span = Span::styled(
            format!("think:{}", self.thinking_level),
            Style::new().fg(Color::Magenta),
        );
        let compaction_span = if self.compaction_count > 0 {
            Span::styled(
                format!("⟲{}", self.compaction_count),
                Style::new().fg(Color::Magenta),
            )
        } else {
            Span::raw("")
        };
        let status = Span::styled(self.status_message, Style::new().fg(Color::Gray));

        let mut parts = vec![
            connection_indicator,
            Span::raw("  "),
            model_span,
            Span::raw("  "),
            session_span,
            Span::raw("  "),
            thinking_span,
        ];
        if self.compaction_count > 0 {
            parts.push(Span::raw("  "));
            parts.push(compaction_span);
        }
        parts.push(Span::raw("  "));
        parts.push(status);

        let line = Line::from(parts);

        let paragraph = Paragraph::new(line);
        Widget::render(paragraph, area, buf);
    }
}
