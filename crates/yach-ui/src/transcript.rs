use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    UserMessage,
    AssistantText,
    ToolCall {
        id: Option<String>,
        name: String,
    },
    ToolResult {
        id: Option<String>,
        name: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub content: String,
    pub kind: EntryKind,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append_delta(&mut self, delta: &str) {
        if let Some(last) = self.entries.last_mut()
            && matches!(last.kind, EntryKind::AssistantText)
        {
            last.content.push_str(delta);
            return;
        }

        self.entries.push(TranscriptEntry {
            content: delta.to_owned(),
            kind: EntryKind::AssistantText,
        });
    }

    pub fn append_user_message(&mut self, message: &str) {
        self.entries.push(TranscriptEntry {
            content: message.to_owned(),
            kind: EntryKind::UserMessage,
        });
    }

    pub fn append_tool_call(&mut self, id: Option<&str>, name: &str, preview: Option<&str>) {
        self.entries.push(TranscriptEntry {
            content: preview.unwrap_or_default().to_owned(),
            kind: EntryKind::ToolCall {
                id: id.map(ToOwned::to_owned),
                name: name.to_owned(),
            },
        });
    }

    pub fn append_tool_result(
        &mut self,
        id: Option<&str>,
        name: &str,
        result: &str,
        is_error: bool,
    ) {
        self.entries.push(TranscriptEntry {
            content: result.to_owned(),
            kind: EntryKind::ToolResult {
                id: id.map(ToOwned::to_owned),
                name: name.to_owned(),
                is_error,
            },
        });
    }

    pub fn finish_tool_call(
        &mut self,
        id: Option<&str>,
        name: &str,
        label: &str,
        result: &str,
        is_error: bool,
    ) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| matches_tool_call(&entry.kind, id, name))
        else {
            return false;
        };

        result.clone_into(&mut entry.content);
        entry.kind = EntryKind::ToolResult {
            id: id.map(ToOwned::to_owned),
            name: label.to_owned(),
            is_error,
        };
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn compaction_count(&self) -> usize {
        0
    }
}

pub fn render(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    entries: &[TranscriptEntry],
    scroll_offset: usize,
    is_streaming: bool,
) {
    let lines = render_lines(entries, area.width);

    let total_lines = lines.len();
    let start = scroll_offset.min(total_lines.saturating_sub(area.height as usize));
    let mut visible: Vec<Line<'_>> = lines
        .into_iter()
        .skip(start)
        .take(area.height as usize)
        .collect();

    let top_padding = bottom_aligned_top_padding(visible.len(), area.height as usize);
    if top_padding > 0 {
        let mut padded = Vec::with_capacity(top_padding + visible.len());
        padded.extend(std::iter::repeat_with(|| Line::raw("")).take(top_padding));
        padded.append(&mut visible);
        visible = padded;
    }

    let mut paragraph = Paragraph::new(visible).style(Style::new().fg(Color::White));
    if is_streaming {
        paragraph = paragraph.style(Style::new().fg(Color::White));
    }

    Widget::render(paragraph, area, buf);
}

pub fn rendered_line_count(entries: &[TranscriptEntry], width: u16) -> usize {
    render_lines(entries, width).len()
}

pub fn max_scroll_start(entries: &[TranscriptEntry], width: u16, height: u16) -> usize {
    rendered_line_count(entries, width).saturating_sub(height as usize)
}

fn render_lines(entries: &[TranscriptEntry], width: u16) -> Vec<Line<'static>> {
    entries
        .iter()
        .flat_map(|entry| {
            let (prefix, content_style) = match &entry.kind {
                EntryKind::UserMessage => (
                    Span::styled("▸ ", Style::new().fg(Color::Cyan)),
                    Style::new().fg(Color::White).bold(),
                ),
                EntryKind::AssistantText => (
                    Span::styled("◂ ", Style::new().fg(Color::Green)),
                    Style::new().fg(Color::Gray),
                ),
                EntryKind::ToolCall { name, .. } => (
                    Span::styled(format!("⚙ {name} "), Style::new().fg(Color::Yellow).bold()),
                    Style::new().fg(Color::Yellow),
                ),
                EntryKind::ToolResult { name, is_error, .. } => {
                    let color = if *is_error { Color::Red } else { Color::Blue };
                    (
                        Span::styled(format!("✓ {name} "), Style::new().fg(color)),
                        Style::new().fg(Color::DarkGray),
                    )
                }
            };

            let wrapped = wrap_text(&entry.content, (width as usize).saturating_sub(2));
            let mut result: Vec<Line<'_>> = Vec::new();
            for (i, line) in wrapped.iter().enumerate() {
                let span = Span::styled(line.clone(), content_style);
                if i == 0 {
                    result.push(Line::from(vec![prefix.clone(), span]));
                } else {
                    result.push(Line::from(vec![Span::styled("  ", Style::new()), span]));
                }
            }
            if !wrapped.is_empty() {
                result.push(Line::raw(""));
            }
            result
        })
        .collect()
}

fn matches_tool_call(kind: &EntryKind, id: Option<&str>, name: &str) -> bool {
    match kind {
        EntryKind::ToolCall {
            id: entry_id,
            name: entry_name,
        } => match (entry_id.as_deref(), id) {
            (Some(entry_id), Some(id)) => entry_id == id,
            _ => entry_name == name,
        },
        _ => false,
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut remaining = raw_line;
        while !remaining.is_empty() {
            let byte_len = char_boundary_at_or_before(remaining, width);
            lines.push(remaining[..byte_len].to_owned());
            remaining = &remaining[byte_len..];
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn char_boundary_at_or_before(s: &str, max_width: usize) -> usize {
    let mut byte_count = 0;
    let mut char_count = 0;
    for (idx, ch) in s.char_indices() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if char_count > 0 && char_count + ch_width > max_width {
            return idx;
        }
        char_count += ch_width;
        byte_count = idx + ch.len_utf8();
    }
    byte_count
}

fn bottom_aligned_top_padding(visible_lines: usize, viewport_height: usize) -> usize {
    viewport_height.saturating_sub(visible_lines)
}

#[cfg(test)]
mod tests {
    use super::{
        EntryKind, Transcript, bottom_aligned_top_padding, char_boundary_at_or_before, wrap_text,
    };

    #[test]
    fn transcript_accumulates_deltas_into_single_entry() {
        let mut transcript = Transcript::new();
        transcript.append_delta("hello");
        transcript.append_delta(" world");

        assert_eq!(transcript.entries().len(), 1);
        assert_eq!(transcript.entries()[0].content, "hello world");
    }

    #[test]
    fn transcript_tracks_tool_entries() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("run a tool");
        transcript.append_tool_call(Some("call-1"), "Read", Some("src/lib.rs"));
        assert!(transcript.finish_tool_call(
            Some("call-1"),
            "Read",
            "Read src/lib.rs",
            "file contents",
            false,
        ));

        assert!(matches!(
            transcript.entries()[1].kind,
            EntryKind::ToolResult { .. }
        ));
        assert_eq!(transcript.entries().len(), 2);
        assert_eq!(transcript.compaction_count(), 0);
    }

    #[test]
    fn unmatched_tool_results_can_still_be_appended() {
        let mut transcript = Transcript::new();
        assert!(!transcript.finish_tool_call(None, "Read", "Read", "missing", false));

        transcript.append_tool_result(None, "Read", "missing", false);

        assert!(matches!(
            transcript.entries()[0].kind,
            EntryKind::ToolResult { .. }
        ));
    }

    #[test]
    fn wrap_text_splits_long_lines() {
        let result = wrap_text("hello world this is a long line", 10);
        assert!(result.len() > 1);
        assert_eq!(result[0], "hello worl");
    }

    #[test]
    fn wrap_text_preserves_empty_lines() {
        let result = wrap_text("line1\n\nline2", 20);
        assert_eq!(result.len(), 3);
        assert!(result[1].is_empty());
    }

    #[test]
    fn char_boundary_respects_char_width() {
        assert_eq!(char_boundary_at_or_before("hello", 3), 3);
        assert_eq!(char_boundary_at_or_before("hello", 10), 5);
    }

    #[test]
    fn transcript_content_bottom_aligns_when_shorter_than_viewport() {
        assert_eq!(bottom_aligned_top_padding(2, 10), 8);
        assert_eq!(bottom_aligned_top_padding(10, 10), 0);
        assert_eq!(bottom_aligned_top_padding(12, 10), 0);
    }
}
