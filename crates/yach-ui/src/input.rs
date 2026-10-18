use std::collections::VecDeque;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Widget};
use ratatui_textarea::{TextArea, WrapMode};

use crate::theme::{ComposerStyle, Theme};

/// Prompt rows before the composer scrolls, in every style.
const MAX_PROMPT_ROWS: u16 = 6;
const PROMPT_MARKER: &str = "› ";
const BOX_HINT: &str = " enter send · ctrl+j newline ";
const OPEN_HINT: &str = " enter send · ctrl+j newline";

/// Renders the persistent prompt textarea directly (not a clone): the
/// widget records its rendered area on the textarea, and wrap-aware cursor
/// movement (up/down across soft-wrapped display rows) only works when the
/// same instance that receives key input has seen its area and wrap mode.
pub struct InputComposer<'a> {
    pub textarea: &'a mut TextArea<'static>,
    pub is_streaming: bool,
    pub terminal_focused: bool,
    pub overflowed: bool,
    /// The drawn style, `Theme::composer_style()`, after the band fallback.
    pub style: ComposerStyle,
    pub theme: &'a Theme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputStyles {
    pub(crate) border: Style,
    pub(crate) title: Style,
    pub(crate) hint: Style,
    pub(crate) marker: Style,
    pub(crate) cursor: Style,
}

pub(crate) fn input_styles(terminal_focused: bool, theme: &Theme) -> InputStyles {
    if terminal_focused {
        InputStyles {
            border: theme.border(false),
            title: theme.accent().add_modifier(Modifier::BOLD),
            hint: theme.dim(),
            // The prompt marker is chrome, so it follows the rules' border color.
            marker: theme.border(false).add_modifier(Modifier::BOLD),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
        }
    } else {
        // Unfocused dims the frame, title, hint and marker together.
        let dim = theme.dim().add_modifier(Modifier::DIM);
        InputStyles {
            border: dim,
            title: dim,
            hint: dim,
            marker: dim.add_modifier(Modifier::BOLD),
            cursor: Style::default().add_modifier(Modifier::HIDDEN),
        }
    }
}

/// `box` puts this in its top border; `rule` and `band` at the right end of
/// the status row.
pub(crate) fn composer_marker(is_streaming: bool, overflowed: bool) -> Option<&'static str> {
    match (is_streaming, overflowed) {
        (true, true) => Some(" running · more ↑ "),
        (true, false) => Some(" running "),
        (false, true) => Some(" more ↑ "),
        (false, false) => None,
    }
}

impl Widget for InputComposer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let styles = input_styles(self.terminal_focused, self.theme);
        self.textarea.set_wrap_mode(WrapMode::Glyph);
        self.textarea.set_style(self.theme.text());
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_cursor_style(styles.cursor);
        match self.style {
            ComposerStyle::Box => self.render_box(&styles, area, buf),
            ComposerStyle::Rule | ComposerStyle::Band => self.render_open(&styles, area, buf),
        }
    }
}

impl InputComposer<'_> {
    fn render_box(self, styles: &InputStyles, area: Rect, buf: &mut Buffer) {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border);
        if let Some(title) = composer_marker(self.is_streaming, self.overflowed) {
            block = block.title(Line::styled(title, styles.title));
        }
        // The hint teaches the keys; once the user is typing it is noise.
        if area.width >= 42 && self.textarea.is_empty() {
            block = block.title_bottom(Line::styled(BOX_HINT, styles.hint));
        }
        self.textarea.set_block(block);
        Widget::render(&*self.textarea, area, buf);
    }

    /// Open styles offset the unblocked textarea two columns, so wrap-aware
    /// cursor movement sees its rendered area and continuation rows stay indented.
    fn render_open(self, styles: &InputStyles, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        buf.set_stringn(
            area.x,
            area.y,
            PROMPT_MARKER,
            usize::from(area.width),
            styles.marker,
        );
        let prompt = Rect {
            x: area.x.saturating_add(2),
            width: area.width.saturating_sub(2),
            ..area
        };
        self.textarea.remove_block();
        // The textarea shows its placeholder after the cursor, only while empty.
        if self.textarea.placeholder_text() != OPEN_HINT {
            self.textarea.set_placeholder_text(OPEN_HINT);
        }
        self.textarea.set_placeholder_style(styles.hint);
        Widget::render(&*self.textarea, prompt, buf);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMetrics {
    pub prompt_rows: u16,
    pub overflowed: bool,
}

pub fn input_metrics(textarea: &TextArea<'_>, area_width: u16) -> InputMetrics {
    let inner_width = area_width.saturating_sub(2) as usize;
    let tab_length = textarea.tab_length();
    let content_rows = textarea
        .lines()
        .iter()
        .map(|line| wrapped_line_count(line, inner_width, tab_length))
        .sum::<usize>()
        .max(1);
    InputMetrics {
        prompt_rows: u16::try_from(content_rows)
            .unwrap_or(u16::MAX)
            .min(MAX_PROMPT_ROWS),
        overflowed: content_rows > usize::from(MAX_PROMPT_ROWS),
    }
}

fn wrapped_line_count(line: &str, width: usize, tab_length: u8) -> usize {
    if line.is_empty() {
        return 1;
    }

    let width = width.max(1);
    let mut count = 1;
    let mut current_width = 0;
    for grapheme in line.graphemes(true) {
        let next_width = display_width_to(grapheme, current_width, tab_length);
        let grapheme_width = next_width.saturating_sub(current_width);
        if current_width > 0 && current_width.saturating_add(grapheme_width) > width {
            count += 1;
            current_width = display_width_to(grapheme, 0, tab_length);
        } else {
            current_width = next_width;
        }
    }
    count
}

fn display_width_to(text: &str, mut width: usize, tab_length: u8) -> usize {
    for character in text.chars() {
        if character == '\t' && tab_length > 0 {
            let tab_length = usize::from(tab_length);
            width += tab_length - (width % tab_length);
        } else {
            width += character.width().unwrap_or(0);
        }
    }
    width
}

const PROMPT_HISTORY_LIMIT: usize = 100;

/// Session-scoped submitted prompts. Newest is at the back.
#[derive(Debug, Default)]
pub struct PromptHistory {
    entries: VecDeque<String>,
    /// 0 = live draft; 1 = newest entry, increasing toward oldest.
    offset: usize,
    draft: Option<String>,
}

impl PromptHistory {
    pub fn record(&mut self, prompt: String) {
        if self.entries.back() != Some(&prompt) {
            if self.entries.len() == PROMPT_HISTORY_LIMIT {
                self.entries.pop_front();
            }
            self.entries.push_back(prompt);
        }
        self.offset = 0;
        self.draft = None;
    }

    pub fn older(&mut self, current: &str) -> Option<String> {
        if self.entries.is_empty() || self.offset >= self.entries.len() {
            return None;
        }
        if self.offset == 0 {
            self.draft = Some(current.to_owned());
        }
        self.offset += 1;
        let idx = self.entries.len() - self.offset;
        self.entries.get(idx).cloned()
    }

    pub fn newer(&mut self) -> Option<String> {
        if self.offset == 0 {
            return None;
        }
        self.offset -= 1;
        if self.offset == 0 {
            Some(self.draft.take().unwrap_or_default())
        } else {
            let idx = self.entries.len() - self.offset;
            self.entries.get(idx).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::widgets::Widget;
    use ratatui_textarea::TextArea;

    use super::{InputComposer, input_metrics, input_styles};
    use crate::theme::{ComposerStyle, Theme};

    #[test]
    fn prompt_rows_grow_for_wrapped_text() {
        assert_eq!(input_metrics(&TextArea::from(["hello"]), 20).prompt_rows, 1);
        assert_eq!(input_metrics(&TextArea::from(["abcdef"]), 4).prompt_rows, 3);
    }

    #[test]
    fn prompt_rows_count_explicit_newlines() {
        assert_eq!(
            input_metrics(&TextArea::from(["a", "b"]), 20).prompt_rows,
            2
        );
    }

    #[test]
    fn input_styles_dim_and_hide_cursor_when_unfocused() {
        let theme = Theme::default();
        let focused = input_styles(true, &theme);
        let unfocused = input_styles(false, &theme);

        assert_eq!(focused.title, theme.accent().add_modifier(Modifier::BOLD));
        assert_ne!(focused.border, unfocused.border);
        assert_eq!(
            unfocused.cursor,
            Style::default().add_modifier(Modifier::HIDDEN)
        );
    }

    #[test]
    fn capped_input_title_reports_running_and_overflow() {
        let mut input = TextArea::from(["one", "two", "three", "four", "five", "six", "seven"]);
        let metrics = input_metrics(&input, 80);
        assert_eq!(metrics.prompt_rows, 6);
        assert!(metrics.overflowed);

        let area = Rect::new(0, 0, 80, metrics.prompt_rows + 2);
        let mut buffer = Buffer::empty(area);
        Widget::render(
            InputComposer {
                textarea: &mut input,
                is_streaming: true,
                terminal_focused: true,
                overflowed: metrics.overflowed,
                style: ComposerStyle::Box,
                theme: &Theme::default(),
            },
            area,
            &mut buffer,
        );
        let title = (0..area.width)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        assert!(title.contains("running · more ↑"));
        assert!(!title.contains("message"));
    }

    fn text_of(buffer: &Buffer) -> String {
        buffer.content.iter().map(Cell::symbol).collect()
    }

    fn render_composer(
        input: &mut TextArea<'static>,
        style: ComposerStyle,
        area: Rect,
        focused: bool,
    ) -> Buffer {
        let mut buffer = Buffer::empty(area);
        Widget::render(
            InputComposer {
                textarea: input,
                is_streaming: false,
                terminal_focused: focused,
                overflowed: false,
                style,
                theme: &Theme::default(),
            },
            area,
            &mut buffer,
        );
        buffer
    }

    #[test]
    fn composer_glyph_wrap_matches_metrics_in_every_style() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let mut input = TextArea::from(["abcdefghij"]);
            let rows = input_metrics(&input, 6).prompt_rows;
            assert_eq!(rows, 3);
            let (area, x0, y0) = if style == ComposerStyle::Box {
                (Rect::new(0, 0, 6, rows + 2), 1, 1)
            } else {
                (Rect::new(0, 0, 6, rows), 2, 0)
            };
            let buffer = render_composer(&mut input, style, area, true);
            let content = (y0..y0 + rows)
                .map(|y| {
                    (x0..x0 + 4)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(content, ["abcd", "efgh", "ij"], "{style:?}");
        }
    }

    #[test]
    fn idle_box_keeps_a_clean_top_border() {
        let mut input = TextArea::default();
        let buffer = render_composer(&mut input, ComposerStyle::Box, Rect::new(0, 0, 48, 3), true);
        let top = (0..48).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(top, format!("┌{}┐", "─".repeat(46)));
    }

    #[test]
    fn key_hint_shows_only_while_the_draft_is_empty() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let height = if style == ComposerStyle::Box { 3 } else { 1 };
            for (draft, shown) in [("", true), ("hello", false)] {
                let mut input = TextArea::from([draft]);
                let buffer = render_composer(&mut input, style, Rect::new(0, 0, 48, height), true);
                assert_eq!(
                    text_of(&buffer).contains("enter send · ctrl+j newline"),
                    shown,
                    "{style:?} draft={draft:?}"
                );
            }
        }
    }

    #[test]
    fn open_prompt_marker_stays_inside_a_one_column_area() {
        for style in [ComposerStyle::Rule, ComposerStyle::Band] {
            let mut input = TextArea::from(["hi"]);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
            buffer[(2, 0)].set_symbol("x");
            Widget::render(
                InputComposer {
                    textarea: &mut input,
                    is_streaming: false,
                    terminal_focused: true,
                    overflowed: false,
                    style,
                    theme: &Theme::default(),
                },
                Rect::new(1, 0, 1, 1),
                &mut buffer,
            );
            assert_eq!(buffer[(1, 0)].symbol(), "›", "{style:?}");
            assert_eq!(buffer[(2, 0)].symbol(), "x", "{style:?}: outside area");
        }
    }

    #[test]
    fn open_styles_mark_the_prompt_and_dim_it_when_unfocused() {
        for style in [ComposerStyle::Rule, ComposerStyle::Band] {
            for focused in [true, false] {
                let mut input = TextArea::from(["hi"]);
                let buffer = render_composer(&mut input, style, Rect::new(0, 0, 20, 1), focused);
                let marker = &buffer[(0, 0)];
                assert_eq!(marker.symbol(), "›", "{style:?}");
                assert!(marker.modifier.contains(Modifier::BOLD), "{style:?}");
                assert_eq!(
                    marker.modifier.contains(Modifier::DIM),
                    !focused,
                    "{style:?} {focused}"
                );
                assert_eq!(buffer[(2, 0)].symbol(), "h", "{style:?}");
                assert_eq!(
                    buffer[(2, 0)].modifier.contains(Modifier::HIDDEN),
                    !focused,
                    "{style:?} {focused}"
                );
            }
        }
    }
}
