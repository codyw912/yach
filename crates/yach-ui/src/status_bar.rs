use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

const PRIORITY_CONTEXT: u8 = 100;
const PRIORITY_MODEL: u8 = 99;
const PRIORITY_CONNECTION: u8 = 80;
const PRIORITY_COMPACTION: u8 = 60;
const PRIORITY_STATUS: u8 = 20;
const SEGMENT_SEPARATOR: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentId {
    Connection,
    Model,
    Context,
    Compaction,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    id: SegmentId,
    text: String,
    priority: u8,
}

impl Segment {
    fn new(id: SegmentId, text: impl Into<String>, priority: u8) -> Self {
        Self {
            id,
            text: text.into(),
            priority,
        }
    }
}

/// Deliberately compact status bar segments. Segments are selected as whole
/// units, so a narrow terminal loses low-priority information rather than
/// cutting a label in half.
pub struct StatusBar<'a> {
    pub model: &'a str,
    pub thinking_level: &'a str,
    pub status_message: &'a str,
    pub is_connected: bool,
    pub compaction_count: usize,
    /// Estimated percent of the usable context window in use; colored as
    /// a warning while the auto-compaction threshold approaches.
    pub context_used_percent: Option<u8>,
    /// Configured model context window before output and compaction reserves.
    pub context_window: Option<u64>,
    /// True while the context value is the post-compaction estimate and no
    /// provider-reported usage has refreshed it yet.
    pub context_usage_is_estimate: bool,
    pub theme: &'a Theme,
}

impl StatusBar<'_> {
    fn segments(&self) -> Vec<Segment> {
        let mut segments = vec![
            Segment::new(SegmentId::Connection, "●", PRIORITY_CONNECTION),
            Segment::new(
                SegmentId::Model,
                model_segment_text(self.model, self.thinking_level),
                PRIORITY_MODEL,
            ),
        ];

        if let Some(percent) = self.context_used_percent {
            segments.push(Segment::new(
                SegmentId::Context,
                format_context_meter(percent, self.context_usage_is_estimate, self.context_window),
                PRIORITY_CONTEXT,
            ));
        }
        if self.compaction_count > 0 {
            segments.push(Segment::new(
                SegmentId::Compaction,
                format!("⟲{}", self.compaction_count),
                PRIORITY_COMPACTION,
            ));
        }
        if !self.status_message.is_empty() {
            segments.push(Segment::new(
                SegmentId::Status,
                self.status_message,
                PRIORITY_STATUS,
            ));
        }

        segments
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        let segments = fit_segments(self.segments(), area.width);
        let mut spans = Vec::with_capacity(segments.len().saturating_mul(2));
        for (index, segment) in segments.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(SEGMENT_SEPARATOR));
            }
            let style = segment_style(
                segment.id,
                self.is_connected,
                self.context_used_percent,
                self.theme,
            );
            spans.push(Span::styled(segment.text, style));
        }

        let paragraph = Paragraph::new(Line::from(spans));
        Widget::render(paragraph, area, buf);
    }
}

fn fit_segments(mut segments: Vec<Segment>, width: u16) -> Vec<Segment> {
    while segment_width(&segments) > usize::from(width) {
        let Some(drop_index) = segments
            .iter()
            .enumerate()
            .min_by_key(|(index, segment)| (segment.priority, std::cmp::Reverse(*index)))
            .map(|(index, _)| index)
        else {
            break;
        };
        segments.remove(drop_index);
    }
    segments
}

fn segment_width(segments: &[Segment]) -> usize {
    segments
        .iter()
        .map(|segment| segment.text.width())
        .sum::<usize>()
        + SEGMENT_SEPARATOR.width() * segments.len().saturating_sub(1)
}

fn segment_style(
    id: SegmentId,
    is_connected: bool,
    context_used_percent: Option<u8>,
    theme: &Theme,
) -> Style {
    let colors = theme.colors;
    match id {
        SegmentId::Connection => Style::new().fg(if is_connected {
            colors.success
        } else {
            colors.error
        }),
        SegmentId::Model => Style::new().fg(colors.accent),
        SegmentId::Context => {
            let color = match context_used_percent.unwrap_or_default().min(100) {
                0..=69 => colors.dim,
                70..=89 => colors.warning,
                _ => colors.error,
            };
            Style::new().fg(color)
        }
        SegmentId::Compaction => Style::new().fg(colors.harness),
        SegmentId::Status => Style::new().fg(colors.muted),
    }
}

pub(crate) fn format_context_meter(
    percent: u8,
    is_estimate: bool,
    context_window: Option<u64>,
) -> String {
    let overflow_marker = if percent > 100 { "+" } else { "" };
    let estimate_marker = if is_estimate { "~" } else { "" };
    let mut text = format!(
        "ctx:{estimate_marker}{}%{overflow_marker}",
        percent.min(100)
    );
    if let Some(context_window) = context_window {
        text.push_str(" · win:");
        text.push_str(&format_token_capacity(context_window));
    }
    text
}

fn model_segment_text(model: &str, thinking_level: &str) -> String {
    if model.trim().is_empty()
        || model.eq_ignore_ascii_case("fixture echo")
        || model.eq_ignore_ascii_case("provider not configured")
        || model.eq_ignore_ascii_case("provider-unconfigured")
    {
        String::from("no model (run /connect)")
    } else {
        format!("{model} · think:{thinking_level}")
    }
}

pub(crate) fn format_token_capacity(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let tenths = tokens.div_ceil(100_000);
        if tenths.is_multiple_of(10) {
            format!("{}m", tenths / 10)
        } else {
            format!("{}.{}m", tenths / 10, tenths % 10)
        }
    } else if tokens >= 1_000 {
        format!("{}k", tokens.div_ceil(1_000))
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Segment, SegmentId, StatusBar, fit_segments, format_context_meter, format_token_capacity,
        model_segment_text, segment_width,
    };
    use crate::theme::Theme;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    #[test]
    fn narrow_bar_drops_low_priority_segments_as_whole_units() {
        let segments = vec![
            Segment::new(SegmentId::Model, "model", 99),
            Segment::new(SegmentId::Context, "ctx:42%", 100),
            Segment::new(SegmentId::Status, "long status", 20),
            Segment::new(SegmentId::Compaction, "⟲12345678", 0),
        ];

        let selected = fit_segments(segments, 16);

        assert!(segment_width(&selected) <= 16);
        assert_eq!(
            selected
                .iter()
                .map(|segment| segment.id)
                .collect::<Vec<_>>(),
            vec![SegmentId::Model, SegmentId::Context],
        );
        assert!(!selected.iter().any(|segment| segment.text == "long sta"));
    }

    #[test]
    fn rendered_bar_prioritizes_model_thinking_and_context_over_session_id() {
        let area = Rect::new(0, 0, 120, 1);
        let mut buffer = Buffer::empty(area);
        Widget::render(
            StatusBar {
                model: "gpt-5.6-sol",
                thinking_level: "high",
                status_message: "ready",
                is_connected: true,
                compaction_count: 0,
                context_used_percent: Some(42),
                context_window: Some(200_000),
                context_usage_is_estimate: true,
                theme: &Theme::default(),
            },
            area,
            &mut buffer,
        );
        let rendered = (0..area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();

        assert!(rendered.contains("gpt-5.6-sol · think:high"));
        assert!(rendered.contains("ctx:~42% · win:200k"));
        assert!(!rendered.contains("sid:"));
    }

    #[test]
    fn context_meter_shows_usage_estimate_and_configured_window() {
        assert_eq!(format_context_meter(125, false, None), "ctx:100%+");
        assert_eq!(
            format_context_meter(42, true, Some(200_000)),
            "ctx:~42% · win:200k"
        );
        assert_eq!(
            format_context_meter(125, true, Some(1_048_576)),
            "ctx:~100%+ · win:1.1m"
        );
        assert_eq!(format_token_capacity(999), "999");
    }

    #[test]
    fn fixture_model_is_replaced_with_setup_hint() {
        assert_eq!(
            model_segment_text("Fixture Echo", "high"),
            "no model (run /connect)"
        );
        assert_eq!(
            model_segment_text("Provider Not Configured", "high"),
            "no model (run /connect)"
        );
        assert_eq!(
            model_segment_text("gpt-5.6-sol", "high"),
            "gpt-5.6-sol · think:high"
        );
    }
}
