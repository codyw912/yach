use yach_adapter_pi_rpc::{EntryKind, TranscriptEntry};

use ratatui::prelude::Stylize;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub fn render(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    entries: &[TranscriptEntry],
    scroll_offset: usize,
    is_streaming: bool,
    focused_entry: usize,
) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut entry_start_lines: Vec<usize> = Vec::new();

    for (entry_idx, entry) in entries.iter().enumerate() {
        entry_start_lines.push(lines.len());

        let (prefix, content_style, separator) = match &entry.kind {
            EntryKind::UserMessage => (
                Span::styled("▸ ", Style::new().fg(Color::Cyan)),
                Style::new().fg(Color::White).bold(),
                true,
            ),
            EntryKind::AssistantText => (
                Span::styled("◂ ", Style::new().fg(Color::Green)),
                Style::new().fg(Color::Gray),
                true,
            ),
            EntryKind::ToolCall { name } => (
                Span::styled(format!("⚙ {name} "), Style::new().fg(Color::Yellow).bold()),
                Style::new().fg(Color::Yellow),
                false,
            ),
            EntryKind::ToolResult { name } => (
                Span::styled(format!("✓ {name} "), Style::new().fg(Color::Blue)),
                Style::new().fg(Color::DarkGray),
                true,
            ),
            EntryKind::Compaction => (
                Span::styled("⟲ ", Style::new().fg(Color::Magenta)),
                Style::new().fg(Color::Magenta).dim(),
                true,
            ),
        };

        let is_focused = entry_idx == focused_entry;
        let wrapped = wrap_text(&entry.content, (area.width as usize).saturating_sub(2));
        for (i, line) in wrapped.iter().enumerate() {
            let mut span_style = content_style;
            if is_focused {
                span_style = span_style.bg(Color::DarkGray);
            }
            let span = Span::styled(line.clone(), span_style);
            if i == 0 {
                lines.push(Line::from(vec![prefix.clone(), span]));
            } else {
                lines.push(Line::from(vec![Span::styled("  ", Style::new()), span]));
            }
        }
        if separator && !wrapped.is_empty() {
            lines.push(Line::raw(""));
        }
    }

    entry_start_lines.push(lines.len());

    let total_lines = lines.len();
    let start = scroll_offset.min(total_lines.saturating_sub(area.height as usize));
    let visible: Vec<Line<'_>> = lines
        .into_iter()
        .skip(start)
        .take(area.height as usize)
        .collect();

    let mut paragraph = Paragraph::new(visible).style(Style::new().fg(Color::White));
    if is_streaming {
        paragraph = paragraph.style(Style::new().fg(Color::White));
    }

    Widget::render(paragraph, area, buf);
}

pub fn scroll_to_entry(entries: &[TranscriptEntry], entry_idx: usize, area_height: usize) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let mut lines = 0;
    for (i, entry) in entries.iter().enumerate() {
        if i == entry_idx {
            let entry_lines = wrap_text(&entry.content, 80).len().max(1) + 1;
            if lines + entry_lines > area_height + lines {
                return lines.saturating_sub(1);
            }
            return lines;
        }
        lines += wrap_text(&entry.content, 80).len().max(1) + 1;
    }
    entries.len()
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

#[cfg(test)]
mod tests {
    use super::{char_boundary_at_or_before, wrap_text};

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
}
