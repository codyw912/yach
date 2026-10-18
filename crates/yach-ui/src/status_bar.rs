use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

// Keep model identity longest, so even a model-only bar identifies the active
// provider. Approval is the last non-model segment: permission safety matters
// more than context health or transient feedback, especially in full-access.
const PRIORITY_MODEL: u8 = 100;
const PRIORITY_APPROVAL: u8 = 99;
// A warning or error needs attention now: it outranks the context meter and
// is truncated, not dropped, while 12 cells fit. An info status stays below.
const PRIORITY_ALERT: u8 = 98;
const PRIORITY_CONTEXT: u8 = 97;
// A status answers the user's last action. Keep it above ambient details and
// truncate it before sacrificing approval, rather than losing that feedback.
const PRIORITY_STATUS: u8 = 95;
const PRIORITY_REVIEWER: u8 = 85;
const PRIORITY_CONNECTION: u8 = 80;
const PRIORITY_COMPACTION: u8 = 60;
// Below context: a narrow terminal should drop the cumulative total before
// the context-health meter, which is the actionable number.
const PRIORITY_TOKENS: u8 = 50;
const SEGMENT_SEPARATOR: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub text: String,
    pub severity: StatusSeverity,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            severity: StatusSeverity::Info,
        }
    }

    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            severity: StatusSeverity::Warning,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            severity: StatusSeverity::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentId {
    Connection,
    Model,
    Approval,
    Reviewer,
    Context,
    Compaction,
    Tokens,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusFill {
    /// `box`: segments only.
    None,
    /// `rule`: segments, a space, then `─` in this style up to the marker.
    Rule(Style),
    /// `band`: the row painted with this background, one leading space.
    Band(Color),
}

/// Deliberately compact status bar segments. Segments are selected as whole
/// units, so a narrow terminal loses low-priority information rather than
/// cutting a label in half.
pub struct StatusBar<'a> {
    pub model: &'a str,
    pub thinking_level: &'a str,
    pub approval_mode: &'a str,
    /// Active reviewer id when `approval_mode == "auto-review"`.
    pub reviewer_id: Option<&'a str>,
    pub status_message: &'a StatusMessage,
    pub is_connected: bool,
    pub compaction_count: u64,
    /// Provider-reported tokens summed across the session, when any turn
    /// reported usage. `None` stays absent rather than rendering as zero.
    pub total_tokens: Option<u64>,
    /// Estimated percent of the usable context window in use; colored as
    /// a warning while the auto-compaction threshold approaches.
    pub context_used_percent: Option<u8>,
    /// Configured model context window before output and compaction reserves.
    /// The bool is `true` when the number is the catalog floor, not a listed value.
    pub context_window: Option<(u64, bool)>,
    pub theme: &'a Theme,
    pub fill: StatusFill,
    /// `running` / `more ↑`, drawn at the right end. Its width is taken
    /// from the segment budget before fitting.
    pub marker: Option<Span<'a>>,
}

impl StatusBar<'_> {
    fn segments(&self) -> Vec<Segment> {
        let mut segments = Vec::new();
        // A healthy connection needs no indicator; the dot is a warning sign.
        if !self.is_connected || self.status_message.severity == StatusSeverity::Error {
            segments.push(Segment::new(
                SegmentId::Connection,
                "●",
                PRIORITY_CONNECTION,
            ));
        }
        segments.push(Segment::new(
            SegmentId::Model,
            model_segment_text(self.model, self.thinking_level),
            PRIORITY_MODEL,
        ));
        segments.push(Segment::new(
            SegmentId::Approval,
            format!("approval:{}", self.approval_mode),
            PRIORITY_APPROVAL,
        ));
        if let Some(reviewer_id) = self.reviewer_id {
            segments.push(Segment::new(
                SegmentId::Reviewer,
                format!("auto:{reviewer_id}"),
                PRIORITY_REVIEWER,
            ));
        }

        if let Some(percent) = self.context_used_percent {
            segments.push(Segment::new(
                SegmentId::Context,
                format_context_meter(percent, self.context_window),
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
        if let Some(total) = self.total_tokens {
            segments.push(Segment::new(
                SegmentId::Tokens,
                format!("tok:{}", format_token_capacity(total)),
                PRIORITY_TOKENS,
            ));
        }
        if !self.status_message.text.is_empty() {
            let priority = if self.status_message.severity == StatusSeverity::Info {
                PRIORITY_STATUS
            } else {
                PRIORITY_ALERT
            };
            segments.push(Segment::new(
                SegmentId::Status,
                self.status_message.text.as_str(),
                priority,
            ));
        }

        segments
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if area.is_empty() {
            return;
        }
        let band = matches!(self.fill, StatusFill::Band(_));
        if let StatusFill::Band(color) = self.fill {
            buf.set_style(area, Style::new().bg(color));
        }
        let lead = u16::from(band);
        // A rule needs a space and at least one `─` after the segments.
        let tail = if matches!(self.fill, StatusFill::Rule(_)) {
            2
        } else {
            0
        };
        let marker_width = self.marker.as_ref().map_or(0, |marker| {
            u16::try_from(marker.width()).unwrap_or(u16::MAX)
        });
        let budget = area
            .width
            .saturating_sub(lead.saturating_add(tail).saturating_add(marker_width));
        let segments = fit_segments(self.segments(), budget);
        let used = u16::try_from(segment_width(&segments)).unwrap_or(u16::MAX);
        let mut spans = Vec::with_capacity(segments.len().saturating_mul(2));
        // The spec's band rule: separators take `muted`, not the default fg.
        let separator = if band {
            self.theme.muted()
        } else {
            Style::default()
        };
        for (index, segment) in segments.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(SEGMENT_SEPARATOR, separator));
            }
            let style = segment_style(
                segment.id,
                self.is_connected,
                self.context_used_percent,
                self.approval_mode,
                self.status_message.severity,
                band,
                self.theme,
            );
            spans.push(Span::styled(segment.text, style));
        }
        let row = Rect {
            x: area.x.saturating_add(lead),
            width: area.width.saturating_sub(lead),
            ..area
        };
        Widget::render(Paragraph::new(Line::from(spans)), row, buf);
        let marker_x = area.right().saturating_sub(marker_width);
        if let StatusFill::Rule(style) = self.fill {
            for x in row.x.saturating_add(used).saturating_add(1)..marker_x {
                buf[(x, area.y)].set_symbol("─").set_style(style);
            }
        }
        if let Some(marker) = &self.marker {
            buf.set_span(marker_x, area.y, marker, marker_width);
        }
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
        // A long status message would otherwise force out every ambient
        // segment one by one and then itself. Truncating it keeps the
        // answer to the user's last action visible on a narrow bar, which
        // is where losing it hurts most.
        if segments[drop_index].id == SegmentId::Status {
            // Display width throughout: `segment_width` measures terminal
            // cells, so mixing in scalar counts would let wide glyphs
            // overflow the bar after the break below.
            let others = segment_width(&segments).saturating_sub(segments[drop_index].text.width());
            let budget = usize::from(width).saturating_sub(others);
            if budget >= MIN_STATUS_WIDTH {
                truncate_segment_text(&mut segments[drop_index], budget);
                break;
            }
        }
        segments.remove(drop_index);
    }
    segments
}

/// Shortest status worth showing; below this an ellipsis carries no meaning.
const MIN_STATUS_WIDTH: usize = 12;

/// Shorten `segment` to at most `budget` terminal cells, ellipsis included.
fn truncate_segment_text(segment: &mut Segment, budget: usize) {
    const ELLIPSIS: char = '…';
    let keep = budget.saturating_sub(ELLIPSIS.width().unwrap_or(1));
    let mut truncated = String::new();
    let mut used = 0usize;
    for grapheme in segment.text.graphemes(true) {
        let cells = grapheme.width();
        if used + cells > keep {
            break;
        }
        truncated.push_str(grapheme);
        used += cells;
    }
    truncated.push(ELLIPSIS);
    segment.text = truncated;
}

fn segment_width(segments: &[Segment]) -> usize {
    segments
        .iter()
        .map(|segment| segment.text.width())
        .sum::<usize>()
        + SEGMENT_SEPARATOR.width() * segments.len().saturating_sub(1)
}

/// Color only what asks for attention. On a band, `accent` and `dim` do not
/// reliably reach 3:1 against `statusBackground`, so the model uses bold
/// `text` and a healthy context meter uses `muted`.
fn segment_style(
    id: SegmentId,
    is_connected: bool,
    context_used_percent: Option<u8>,
    approval_mode: &str,
    severity: StatusSeverity,
    band: bool,
    theme: &Theme,
) -> Style {
    match id {
        // Present only when disconnected or showing an error (`segments`).
        SegmentId::Connection => {
            if is_connected {
                theme.warning()
            } else {
                theme.error()
            }
        }
        SegmentId::Model if band => theme.text().add_modifier(Modifier::BOLD),
        SegmentId::Model => theme.accent().add_modifier(Modifier::BOLD),
        SegmentId::Approval if approval_mode == "full-access" => {
            theme.error().add_modifier(Modifier::BOLD)
        }
        SegmentId::Approval | SegmentId::Reviewer | SegmentId::Compaction | SegmentId::Tokens => {
            theme.muted()
        }
        SegmentId::Context => match context_used_percent.unwrap_or_default().min(100) {
            0..=69 if band => theme.muted(),
            0..=69 => theme.dim(),
            70..=89 => theme.warning(),
            _ => theme.error(),
        },
        SegmentId::Status => match severity {
            StatusSeverity::Info => theme.muted(),
            StatusSeverity::Warning => theme.warning(),
            StatusSeverity::Error => theme.error(),
        },
    }
}

pub(crate) fn format_context_meter(percent: u8, context_window: Option<(u64, bool)>) -> String {
    let overflow_marker = if percent > 100 { "+" } else { "" };
    let mut text = format!("ctx:{}%{overflow_marker}", percent.min(100));
    if let Some((context_window, assumed)) = context_window {
        text.push('/');
        // A leading tilde is the smallest marker that fits the meter's
        // `ctx:N%/Nk` shape and still reads as "not a measured window".
        if assumed {
            text.push('~');
        }
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
        Segment, SegmentId, StatusBar, StatusFill, StatusMessage, StatusSeverity, fit_segments,
        format_context_meter, format_token_capacity, model_segment_text, segment_style,
        segment_width,
    };
    use crate::theme::Theme;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Span;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    const ACCEPTANCE_ERROR: &str =
        "invalid_config; set the provider environment and relaunch yach tui";

    fn acceptance_bar<'a>(
        theme: &'a Theme,
        status: &'a StatusMessage,
        fill: StatusFill,
    ) -> StatusBar<'a> {
        StatusBar {
            model: "gpt-5.1-codex-max",
            thinking_level: "xhigh",
            approval_mode: "full-access",
            reviewer_id: None,
            status_message: status,
            is_connected: true,
            compaction_count: 0,
            total_tokens: None,
            context_used_percent: Some(78),
            context_window: Some((200_000, false)),
            theme,
            fill,
            marker: None,
        }
    }

    fn render_row(bar: StatusBar<'_>, width: u16) -> (Buffer, String) {
        let area = Rect::new(0, 0, width, 1);
        let mut buffer = Buffer::empty(area);
        Widget::render(bar, area, &mut buffer);
        let row = (0..width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        (buffer, row)
    }

    fn cell_index(buffer: &Buffer, needle: &str) -> Option<u16> {
        let symbols: Vec<String> = needle.chars().map(String::from).collect();
        let width = buffer.area.width;
        (0..width).find(|&x| {
            (x..width)
                .zip(&symbols)
                .filter(|(cell, symbol)| buffer[(*cell, 0)].symbol() == symbol.as_str())
                .count()
                == symbols.len()
        })
    }

    #[test]
    fn alerts_outrank_context_at_80_columns_in_every_fill() {
        let theme = Theme::dark();
        let error = StatusMessage::error(ACCEPTANCE_ERROR);
        for fill in [
            StatusFill::None,
            StatusFill::Rule(theme.border(false)),
            StatusFill::Band(Color::Rgb(40, 40, 50)),
        ] {
            let (_, row) = render_row(acceptance_bar(&theme, &error, fill), 80);
            assert!(
                row.contains("gpt-5.1-codex-max · think:xhigh"),
                "{fill:?}: {row}"
            );
            assert!(row.contains("approval:full-access"), "{fill:?}: {row}");
            // At least 12 cells: 11 characters and the ellipsis.
            assert!(row.contains("invalid_con"), "{fill:?}: {row}");
        }
    }

    #[test]
    fn info_status_still_drops_before_context() {
        let theme = Theme::dark();
        let info = StatusMessage::info(ACCEPTANCE_ERROR);
        let (_, row) = render_row(acceptance_bar(&theme, &info, StatusFill::None), 80);
        assert!(row.contains("ctx:78%/200k"), "{row}");
        assert!(!row.contains("invalid"), "{row}");
    }

    #[test]
    fn segment_colors_stay_quiet_while_healthy() {
        let theme = Theme::dark();
        let style = |id, connected, percent, mode, severity| {
            segment_style(id, connected, percent, mode, severity, false, &theme)
        };
        let info = StatusSeverity::Info;
        assert_eq!(
            style(SegmentId::Model, true, None, "review", info),
            theme.accent().add_modifier(Modifier::BOLD)
        );
        for mode in ["review", "accept-edits", "auto-review"] {
            assert_eq!(
                style(SegmentId::Approval, true, None, mode, info),
                theme.muted(),
                "{mode}"
            );
        }
        assert_eq!(
            style(SegmentId::Approval, true, None, "full-access", info),
            theme.error().add_modifier(Modifier::BOLD)
        );
        for id in [
            SegmentId::Reviewer,
            SegmentId::Compaction,
            SegmentId::Tokens,
        ] {
            assert_eq!(
                style(id, true, None, "review", info),
                theme.muted(),
                "{id:?}"
            );
        }
        for (percent, expected) in [
            (69, theme.dim()),
            (70, theme.warning()),
            (89, theme.warning()),
            (90, theme.error()),
        ] {
            assert_eq!(
                style(SegmentId::Context, true, Some(percent), "review", info),
                expected,
                "{percent}"
            );
        }
        for (severity, expected) in [
            (StatusSeverity::Info, theme.muted()),
            (StatusSeverity::Warning, theme.warning()),
            (StatusSeverity::Error, theme.error()),
        ] {
            assert_eq!(
                style(SegmentId::Status, true, None, "review", severity),
                expected
            );
        }
        assert_eq!(
            style(
                SegmentId::Connection,
                true,
                None,
                "review",
                StatusSeverity::Error
            ),
            theme.warning()
        );
        assert_eq!(
            style(SegmentId::Connection, false, None, "review", info),
            theme.error()
        );
    }

    #[test]
    fn connection_dot_shows_only_when_something_is_wrong() {
        let theme = Theme::dark();
        for (connected, status, shown) in [
            (true, StatusMessage::info("ready"), false),
            (true, StatusMessage::warning("slow"), false),
            (true, StatusMessage::error("failed"), true),
            (false, StatusMessage::info("ready"), true),
        ] {
            let mut bar = acceptance_bar(&theme, &status, StatusFill::None);
            bar.is_connected = connected;
            let dot = bar
                .segments()
                .iter()
                .any(|segment| segment.id == SegmentId::Connection);
            assert_eq!(dot, shown, "connected={connected} {:?}", status.severity);
        }
    }

    #[test]
    fn band_paints_the_row_and_uses_band_safe_roles() {
        let theme = Theme::dark();
        let status = StatusMessage::info("ready");
        let mut bar = acceptance_bar(&theme, &status, StatusFill::Band(Color::Rgb(1, 2, 3)));
        bar.approval_mode = "review";
        bar.context_used_percent = Some(42);
        let (buffer, row) = render_row(bar, 120);
        assert!(row.starts_with(" gpt-5.1-codex-max"), "{row}");
        for x in 0..120 {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(1, 2, 3), "cell {x}");
        }
        assert_eq!(Some(buffer[(1, 0)].fg), theme.text().fg);
        assert!(buffer[(1, 0)].modifier.contains(Modifier::BOLD));
        let context = cell_index(&buffer, "ctx:");
        assert!(context.is_some(), "{row}");
        let Some(context) = context else { return };
        assert_eq!(Some(buffer[(context, 0)].fg), theme.muted().fg);
    }

    #[test]
    fn rule_fills_between_the_segments_and_the_marker() {
        let theme = Theme::dark();
        let status = StatusMessage::info("ready");
        let mut bar = acceptance_bar(&theme, &status, StatusFill::Rule(theme.border(false)));
        bar.model = "model";
        bar.thinking_level = "high";
        bar.approval_mode = "review";
        bar.context_used_percent = None;
        bar.marker = Some(Span::styled(" running ", theme.accent()));
        let (buffer, row) = render_row(bar, 60);
        assert!(row.ends_with("─ running "), "{row}");
        assert!(row.contains("ready ─"), "{row}");
        let rule = cell_index(&buffer, "─");
        assert!(rule.is_some(), "{row}");
        let Some(rule) = rule else { return };
        assert_eq!(Some(buffer[(rule, 0)].fg), theme.border(false).fg);
    }

    #[test]
    fn status_severity_colors_status_text() {
        let theme = Theme::default();
        for (severity, expected) in [
            (StatusSeverity::Info, theme.muted()),
            (StatusSeverity::Warning, theme.warning()),
            (StatusSeverity::Error, theme.error()),
        ] {
            let status = StatusMessage {
                text: String::from("notice"),
                severity,
            };
            let area = Rect::new(0, 0, 120, 1);
            let mut buffer = Buffer::empty(area);
            Widget::render(
                StatusBar {
                    model: "default",
                    thinking_level: "off",
                    approval_mode: "review",
                    reviewer_id: None,
                    status_message: &status,
                    is_connected: true,
                    compaction_count: 0,
                    total_tokens: None,
                    context_used_percent: None,
                    context_window: None,
                    theme: &theme,
                    fill: StatusFill::None,
                    marker: None,
                },
                area,
                &mut buffer,
            );
            let rendered = (0..area.width)
                .map(|x| buffer[(x, 0)].symbol())
                .collect::<String>();
            assert!(rendered.contains("notice"));
            for x in 0..area.width.saturating_sub(5) {
                // Match cells rather than treating UTF-8 bytes as columns.
                if (x..x + 6)
                    .zip(["n", "o", "t", "i", "c", "e"])
                    .all(|(cell, symbol)| buffer[(cell, 0)].symbol() == symbol)
                {
                    for cell in x..x + 6 {
                        assert_eq!(Some(buffer[(cell, 0)].fg), expected.fg);
                    }
                }
            }
        }
    }

    #[test]
    fn approval_survives_a_long_status_at_fixture_and_narrow_widths() {
        let theme = Theme::default();
        let bar = StatusBar {
            model: "default",
            thinking_level: "off",
            approval_mode: "review",
            reviewer_id: None,
            status_message: &StatusMessage::info(
                "invalid_config; set the provider environment and relaunch yach tui",
            ),
            is_connected: true,
            compaction_count: 0,
            total_tokens: Some(554),
            context_used_percent: None,
            context_window: None,
            theme: &theme,
            fill: StatusFill::None,
            marker: None,
        };
        for width in [100, 60] {
            let fitted = fit_segments(bar.segments(), width);
            assert!(
                fitted
                    .iter()
                    .any(|segment| segment.text == "approval:review")
            );
            assert!(fitted.iter().any(|segment| segment.id == SegmentId::Status));
            assert!(segment_width(&fitted) <= usize::from(width));
        }
    }

    #[test]
    fn full_access_outlasts_every_non_model_segment() {
        let theme = Theme::default();
        let bar = StatusBar {
            model: "default",
            thinking_level: "off",
            approval_mode: "full-access",
            reviewer_id: Some("reviewer"),
            status_message: &StatusMessage::info("an actionable status that needs truncation"),
            is_connected: true,
            compaction_count: 3,
            total_tokens: Some(554),
            context_used_percent: Some(92),
            context_window: Some((200_000, false)),
            theme: &theme,
            fill: StatusFill::None,
            marker: None,
        };
        for width in 0..=180 {
            let fitted = fit_segments(bar.segments(), width);
            if fitted.iter().any(|segment| segment.id != SegmentId::Model) {
                assert!(
                    fitted
                        .iter()
                        .any(|segment| segment.text == "approval:full-access"),
                    "full-access was lost at width {width}: {fitted:?}"
                );
            }
        }
    }

    #[test]
    fn truncated_status_respects_terminal_cells_for_wide_glyphs() {
        // `segment_width` counts display cells, so truncating by scalar
        // count would let CJK or emoji text overrun the bar.
        let theme = Theme::default();
        // All-wide text is the case that distinguishes cells from scalars:
        // truncating N scalars of double-width text yields 2N cells.
        let cjk: String = "\u{672a}\u{77e5}\u{306e}\u{30b3}\u{30de}\u{30f3}\u{30c9}".repeat(8);
        let emoji: String = "\u{1f680}".repeat(40);
        for message in [
            cjk.as_str(),
            emoji.as_str(),
            "unknown command /aproval \u{2014} did you mean /approval?",
        ] {
            for width in [20u16, 40, 60, 80] {
                let bar = StatusBar {
                    model: "default",
                    thinking_level: "off",
                    approval_mode: "review",
                    reviewer_id: None,
                    status_message: &StatusMessage::info(message),
                    is_connected: true,
                    compaction_count: 0,
                    total_tokens: Some(554),
                    context_used_percent: Some(42),
                    context_window: Some((200_000, false)),
                    theme: &theme,
                    fill: StatusFill::None,
                    marker: None,
                };
                if width == 80 {
                    assert!(bar.segments().iter().any(
                        |segment| segment.id == SegmentId::Tokens && segment.text == "tok:554"
                    ));
                }
                let fitted = fit_segments(bar.segments(), width);
                assert!(
                    segment_width(&fitted) <= usize::from(width),
                    "width {width} exceeded for {message:?}: got {} cells",
                    segment_width(&fitted)
                );
            }
        }
    }

    #[test]
    fn a_narrow_bar_keeps_the_status_message_over_ambient_segments() {
        // The status message answers the user's last action and is the only
        // transient segment. Dropping it first meant a mistyped-command
        // suggestion vanished on exactly the narrow terminals where the
        // user most needs the feedback.
        let theme = Theme::default();
        let bar = StatusBar {
            model: "default",
            thinking_level: "off",
            approval_mode: "review",
            reviewer_id: None,
            status_message: &StatusMessage::info(
                "unknown command /aproval \u{2014} did you mean /approval?",
            ),
            is_connected: true,
            compaction_count: 3,
            total_tokens: Some(554),
            context_used_percent: Some(42),
            context_window: Some((200_000, false)),
            theme: &theme,
            fill: StatusFill::None,
            marker: None,
        };

        let squeezed = fit_segments(bar.segments(), 70);
        let ids: Vec<SegmentId> = squeezed.iter().map(|segment| segment.id).collect();
        assert!(
            ids.contains(&SegmentId::Status),
            "status must survive a squeeze, got {ids:?}"
        );
        assert!(
            !ids.contains(&SegmentId::Tokens),
            "the cumulative token total is ambient and should yield first, got {ids:?}"
        );
    }

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
                approval_mode: "review",
                reviewer_id: None,
                status_message: &StatusMessage::info("ready"),
                is_connected: true,
                compaction_count: 0,
                total_tokens: None,
                context_used_percent: Some(42),
                context_window: Some((200_000, false)),
                theme: &Theme::default(),
                fill: StatusFill::None,
                marker: None,
            },
            area,
            &mut buffer,
        );
        let rendered = (0..area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();

        assert!(rendered.contains("gpt-5.6-sol · think:high"));
        assert!(rendered.contains("ctx:42%/200k"));
        assert!(!rendered.contains("sid:"));
    }

    #[test]
    fn context_meter_shows_usage_and_configured_window() {
        assert_eq!(format_context_meter(125, None), "ctx:100%+");
        assert_eq!(
            format_context_meter(42, Some((200_000, false))),
            "ctx:42%/200k"
        );
        assert_eq!(
            format_context_meter(125, Some((1_048_576, false))),
            "ctx:100%+/1.1m"
        );
        assert_eq!(format_token_capacity(999), "999");
    }

    #[test]
    fn context_meter_marks_an_assumed_window_and_leaves_a_known_window_plain() {
        assert_eq!(
            format_context_meter(42, Some((200_000, false))),
            "ctx:42%/200k"
        );
        assert_eq!(
            format_context_meter(42, Some((200_000, true))),
            "ctx:42%/~200k"
        );
        assert_eq!(format_context_meter(125, None), "ctx:100%+");
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
