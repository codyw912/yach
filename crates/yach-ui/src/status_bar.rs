use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// Generated session ids (`session-{pid}-{nanos}`) are far too wide for
/// the one-line status bar and push the context meter and status message
/// off-screen; show a distinguishing tail instead. Short ids (like
/// "default") pass through unchanged.
fn short_session_label(session_id: &str) -> String {
    const MAX_VISIBLE_CHARS: usize = 12;
    let char_count = session_id.chars().count();
    if char_count <= MAX_VISIBLE_CHARS {
        return session_id.to_owned();
    }
    let tail_start = session_id
        .char_indices()
        .nth(char_count - (MAX_VISIBLE_CHARS - 1))
        .map_or(0, |(index, _)| index);
    format!("…{}", &session_id[tail_start..])
}

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session_id: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    pub thinking_level: &'a str,
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

        let model_span = Span::styled(
            format!("model:{}", self.model),
            Style::new().fg(Color::Cyan),
        );
        let session_span = Span::styled(
            format!("session:{}", short_session_label(self.session_id)),
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
        parts.push(Span::raw("  "));
        parts.push(status);

        let line = Line::from(parts);

        let paragraph = Paragraph::new(line);
        Widget::render(paragraph, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::short_session_label;

    #[test]
    fn session_labels_keep_short_ids_and_trim_generated_ids_to_a_tail() {
        assert_eq!(short_session_label("default"), "default");
        assert_eq!(
            short_session_label("session-48647-1784658891096879000"),
            "…91096879000"
        );
        assert_eq!(short_session_label("…91096879000").chars().count(), 12);
    }
}
