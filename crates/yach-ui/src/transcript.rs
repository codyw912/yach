use yach_adapter_pi_rpc::TranscriptEntry;

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
) {
    let lines: Vec<Line<'_>> = entries
        .iter()
        .flat_map(|entry| {
            let prefix = if entry.is_user {
                Span::styled("▸ ", Style::new().fg(Color::Cyan))
            } else {
                Span::styled("◂ ", Style::new().fg(Color::Green))
            };
            let content_style = if entry.is_user {
                Style::new().fg(Color::White).bold()
            } else {
                Style::new().fg(Color::Gray)
            };
            let wrapped = wrap_text(&entry.content, (area.width as usize).saturating_sub(2));
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
        .collect();

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
