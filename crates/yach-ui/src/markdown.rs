//! Assistant Markdown to styled, width-wrapped ratatui lines.
//!
//! The source text is parsed with `pulldown-cmark` and laid out into
//! `Line<'static>` rows *after* parsing, so delimiters never reach the screen
//! and wrapping preserves per-span styles. Widths are Unicode display cells
//! (grapheme clusters measured with `unicode-width`).
//!
//! Layout decisions:
//! - Prose is word-wrapped; words longer than the line, and code lines, are
//!   hard-wrapped by grapheme so no content is ever dropped.
//! - Code blocks keep their whitespace and are padded to the full width with a
//!   background so they read as one block. No syntax highlighting.
//! - Tables are not laid out: each row becomes one wrapped line with cells
//!   separated by a dim bar, header row in bold.
//! - The parser treats an unclosed fence as running to the end of the input and
//!   unmatched delimiters as literal text, so a partially streamed message
//!   renders sensibly without special handling.

use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

const QUOTE_BAR: &str = "│ ";
const BULLET: &str = "- ";
const TABLE_SEPARATOR: &str = " │ ";
const TAB_WIDTH: usize = 4;

/// Every style choice the renderer makes, derived from existing theme colors.
///
/// This is the single place to migrate when the theming engine grows
/// dedicated Markdown tokens: replace the field initialisers, nothing else.
struct MarkdownStyles {
    text: Style,
    quote_text: Style,
    quote_bar: Style,
    heading_major: Style,
    heading_minor: Style,
    strong: Style,
    emphasis: Style,
    strikethrough: Style,
    inline_code: Style,
    code_block: Style,
    link: Style,
    link_url: Style,
    list_marker: Style,
    rule: Style,
    table_separator: Style,
    table_header: Style,
}

impl MarkdownStyles {
    fn from_theme(theme: &Theme) -> Self {
        let colors = theme.colors;
        Self {
            text: Style::new().fg(colors.text),
            quote_text: Style::new().fg(colors.muted),
            quote_bar: Style::new().fg(colors.dim),
            heading_major: Style::new().fg(colors.accent).bold(),
            heading_minor: Style::new().bold(),
            strong: Style::new().bold(),
            emphasis: Style::new().italic(),
            strikethrough: Style::new().crossed_out(),
            inline_code: Style::new().fg(colors.warning),
            code_block: Style::new()
                .fg(colors.text)
                .bg(colors.tool_pending_background),
            link: Style::new().fg(colors.accent).underlined(),
            link_url: Style::new().fg(colors.dim),
            list_marker: Style::new().fg(colors.accent),
            rule: Style::new().fg(colors.dim),
            table_separator: Style::new().fg(colors.dim),
            table_header: Style::new().bold(),
        }
    }

    fn heading(&self, level: HeadingLevel) -> Style {
        match level {
            HeadingLevel::H1 => self.heading_major.underlined(),
            HeadingLevel::H2 => self.heading_major,
            _ => self.heading_minor,
        }
    }
}

/// Render `markdown` into lines no wider than `width` display cells.
///
/// Lines carry no assistant prefix; the caller adds its own gutter and passes
/// the width left over after it.
pub(crate) fn render(markdown: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let styles = MarkdownStyles::from_theme(theme);
    let mut renderer = Renderer::new(&styles, width.max(1));
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(markdown, options) {
        renderer.event(event);
    }
    renderer.finish()
}

enum Piece {
    Text(String, Style),
    Break,
}

enum Container {
    Quote,
    List {
        next: Option<u64>,
        loose: bool,
    },
    Item {
        marker: Option<String>,
        width: usize,
    },
}

struct LinkState {
    dest: String,
    first_piece: usize,
    shows_url: bool,
}

struct Renderer<'s> {
    styles: &'s MarkdownStyles,
    width: usize,
    lines: Vec<Line<'static>>,
    containers: Vec<Container>,
    inline: Vec<Piece>,
    style_stack: Vec<Style>,
    pending_gap: bool,
    block_open: bool,
    code: Option<String>,
    links: Vec<LinkState>,
    images: Vec<String>,
    table_cell: usize,
    first_line_of_block: bool,
}

impl<'s> Renderer<'s> {
    fn new(styles: &'s MarkdownStyles, width: usize) -> Self {
        Self {
            styles,
            width,
            lines: Vec::new(),
            containers: Vec::new(),
            inline: Vec::new(),
            style_stack: Vec::new(),
            pending_gap: false,
            block_open: false,
            code: None,
            links: Vec::new(),
            images: Vec::new(),
            table_cell: 0,
            first_line_of_block: true,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_inline();
        self.lines
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => {
                self.ensure_block();
                let style = self.current().patch(self.styles.inline_code);
                self.push_text(&code, style);
            }
            Event::InlineHtml(html) | Event::Html(html) => self.text(&html),
            Event::SoftBreak => {
                self.ensure_block();
                let style = self.current();
                self.inline.push(Piece::Text(" ".to_owned(), style));
            }
            Event::HardBreak => {
                self.ensure_block();
                self.inline.push(Piece::Break);
            }
            Event::Rule => {
                self.flush_inline();
                self.gap();
                let line = vec![Span::styled("─".repeat(self.avail()), self.styles.rule)];
                self.emit(line);
                self.pending_gap = true;
            }
            Event::TaskListMarker(done) => self.task_marker(done),
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush_inline();
                if let Some(Container::Item { .. }) = self.containers.last()
                    && let Some(Container::List { loose, .. }) =
                        self.containers.iter_mut().rev().nth(1)
                {
                    *loose = true;
                }
                self.ensure_block();
            }
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.ensure_block();
                self.push_style(self.styles.heading(level));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.gap();
                self.containers.push(Container::Quote);
                self.push_style(self.styles.quote_text);
            }
            Tag::CodeBlock(_) => {
                self.flush_inline();
                self.gap();
                // The info string is intentionally not shown: without
                // highlighting a language label is just noise.
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush_inline();
                self.gap();
                self.containers.push(Container::List {
                    next: start,
                    loose: false,
                });
            }
            Tag::Item => self.start_item(),
            Tag::TableHead => {
                self.table_cell = 0;
                self.push_style(self.styles.table_header);
            }
            Tag::TableRow => self.table_cell = 0,
            Tag::TableCell => {
                if self.table_cell > 0 {
                    self.ensure_block();
                    let style = self.styles.table_separator;
                    self.inline
                        .push(Piece::Text(TABLE_SEPARATOR.to_owned(), style));
                }
                self.table_cell += 1;
            }
            Tag::Emphasis => self.push_style(self.styles.emphasis),
            Tag::Strong => self.push_style(self.styles.strong),
            Tag::Strikethrough => self.push_style(self.styles.strikethrough),
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                self.ensure_block();
                self.push_style(self.styles.link);
                self.links.push(LinkState {
                    dest: dest_url.into_string(),
                    first_piece: self.inline.len(),
                    shows_url: !matches!(link_type, LinkType::Autolink | LinkType::Email),
                });
            }
            Tag::Image { dest_url, .. } => {
                self.ensure_block();
                let style = self.current();
                self.inline.push(Piece::Text("[image: ".to_owned(), style));
                self.images.push(dest_url.into_string());
            }
            Tag::Table(_) | Tag::HtmlBlock => {
                self.flush_inline();
                self.gap();
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::Table => {
                self.flush_inline();
                self.pending_gap = true;
            }
            TagEnd::Heading(_) => {
                self.flush_inline();
                self.style_stack.pop();
                self.pending_gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                self.style_stack.pop();
                self.containers.pop();
                self.pending_gap = true;
            }
            TagEnd::CodeBlock => {
                self.emit_code();
                self.pending_gap = true;
            }
            TagEnd::List(_) => {
                self.flush_inline();
                self.containers.pop();
                self.pending_gap = true;
            }
            TagEnd::Item => {
                self.flush_inline();
                self.containers.pop();
            }
            TagEnd::TableHead => {
                self.flush_inline();
                self.style_stack.pop();
            }
            TagEnd::TableRow => self.flush_inline(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.style_stack.pop();
            }
            TagEnd::Link => self.end_link(),
            TagEnd::Image => self.end_image(),
            _ => {}
        }
    }

    fn start_item(&mut self) {
        self.flush_inline();
        let loose_list = self
            .containers
            .iter()
            .rev()
            .find_map(|container| match container {
                Container::List { loose, .. } => Some(*loose),
                _ => None,
            });
        if loose_list == Some(true) {
            self.gap();
        } else {
            self.pending_gap = false;
        }
        let marker = self
            .containers
            .iter_mut()
            .rev()
            .find_map(|container| match container {
                Container::List { next, .. } => Some(match next {
                    Some(number) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    None => BULLET.to_owned(),
                }),
                _ => None,
            })
            .unwrap_or_else(|| BULLET.to_owned());
        let width = marker.width();
        self.containers.push(Container::Item {
            marker: Some(marker),
            width,
        });
    }

    fn task_marker(&mut self, done: bool) {
        let label = if done { "[x] " } else { "[ ] " };
        if let Some(Container::Item { marker, width }) = self.containers.last_mut() {
            *marker = Some(label.to_owned());
            *width = label.width();
        }
    }

    fn end_link(&mut self) {
        self.style_stack.pop();
        let Some(link) = self.links.pop() else {
            return;
        };
        if !link.shows_url || link.dest.is_empty() {
            return;
        }
        let label: String = self.inline[link.first_piece.min(self.inline.len())..]
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text(text, _) => Some(text.as_str()),
                Piece::Break => None,
            })
            .collect();
        if label == link.dest || label == link.dest.trim_start_matches("mailto:") {
            return;
        }
        let style = self.current().patch(self.styles.link_url);
        self.inline
            .push(Piece::Text(format!(" ({})", link.dest), style));
    }

    fn end_image(&mut self) {
        let dest = self.images.pop().unwrap_or_default();
        let style = self.current();
        self.inline.push(Piece::Text("]".to_owned(), style));
        if !dest.is_empty() {
            let url_style = style.patch(self.styles.link_url);
            self.inline
                .push(Piece::Text(format!(" ({dest})"), url_style));
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(code) = &mut self.code {
            code.push_str(text);
            return;
        }
        self.ensure_block();
        let style = self.current();
        self.push_text(text, style);
    }

    /// Push literal text; embedded newlines (HTML blocks) become line breaks.
    fn push_text(&mut self, text: &str, style: Style) {
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.inline.push(Piece::Break);
            }
            let part = part.replace('\r', "").replace('\t', &" ".repeat(TAB_WIDTH));
            if !part.is_empty() {
                self.inline.push(Piece::Text(part, style));
            }
        }
    }

    fn current(&self) -> Style {
        self.style_stack.last().copied().unwrap_or(self.styles.text)
    }

    fn push_style(&mut self, patch: Style) {
        let style = self.current().patch(patch);
        self.style_stack.push(style);
    }

    /// Start a block of inline content, emitting the separating blank line
    /// first when the previous block asked for one.
    fn ensure_block(&mut self) {
        if !self.block_open {
            self.gap();
            self.block_open = true;
            self.first_line_of_block = true;
        }
    }

    fn gap(&mut self) {
        if self.pending_gap && !self.lines.is_empty() {
            let mut spans = self.prefix(false);
            if let Some(last) = spans.last_mut() {
                last.content = last.content.trim_end().to_owned().into();
            }
            spans.retain(|span| !span.content.is_empty());
            self.lines.push(Line::from(spans));
        }
        self.pending_gap = false;
    }

    /// Columns consumed by quote bars and list indentation.
    fn prefix_width(&self) -> usize {
        self.containers
            .iter()
            .map(|container| match container {
                Container::Quote => QUOTE_BAR.width(),
                Container::Item { width, .. } => *width,
                Container::List { .. } => 0,
            })
            .sum()
    }

    fn avail(&self) -> usize {
        self.width.saturating_sub(self.prefix_width()).max(1)
    }

    /// Gutter spans for one output line. List markers are consumed by the
    /// first line of an item; every later line gets matching indentation.
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for container in &mut self.containers {
            match container {
                Container::Quote => {
                    spans.push(Span::styled(QUOTE_BAR, self.styles.quote_bar));
                }
                Container::Item { marker, width } => match first.then(|| marker.take()).flatten() {
                    Some(marker) => spans.push(Span::styled(marker, self.styles.list_marker)),
                    None => spans.push(Span::raw(" ".repeat(*width))),
                },
                Container::List { .. } => {}
            }
        }
        spans
    }

    fn emit(&mut self, content: Vec<Span<'static>>) {
        let first = self.first_line_of_block;
        self.first_line_of_block = false;
        let mut spans = self.prefix(first);
        spans.extend(content);
        self.lines.push(Line::from(spans));
    }

    fn flush_inline(&mut self) {
        self.block_open = false;
        let mut pieces = std::mem::take(&mut self.inline);
        while matches!(pieces.last(), Some(Piece::Break)) {
            pieces.pop();
        }
        if pieces.is_empty() {
            return;
        }
        let width = self.avail();
        let mut segment: Vec<(&str, Style)> = Vec::new();
        for piece in &pieces {
            match piece {
                Piece::Text(text, style) => segment.push((text, *style)),
                Piece::Break => {
                    let wrapped = wrap_spans(&segment, width);
                    self.emit_wrapped(wrapped);
                    segment.clear();
                }
            }
        }
        let wrapped = wrap_spans(&segment, width);
        self.emit_wrapped(wrapped);
        // The next block starts with a fresh marker decision.
        self.first_line_of_block = true;
    }

    fn emit_wrapped(&mut self, lines: Vec<Vec<Span<'static>>>) {
        for content in lines {
            self.emit(content);
        }
    }

    fn emit_code(&mut self) {
        let Some(code) = self.code.take() else {
            return;
        };
        let code = code.strip_suffix('\n').unwrap_or(&code);
        if code.is_empty() {
            return;
        }
        let width = self.avail();
        let style = self.styles.code_block;
        self.first_line_of_block = true;
        for raw_line in code.split('\n') {
            let raw_line = raw_line
                .trim_end_matches('\r')
                .replace('\t', &" ".repeat(TAB_WIDTH));
            for (chunk, chunk_width) in hard_wrap(&raw_line, width) {
                let padding = " ".repeat(width.saturating_sub(chunk_width));
                self.emit(vec![Span::styled(format!("{chunk}{padding}"), style)]);
            }
        }
        self.first_line_of_block = true;
    }
}

/// Split `text` into chunks of at most `width` cells, never inside a grapheme.
/// A grapheme wider than `width` still gets a line of its own.
fn hard_wrap(text: &str, width: usize) -> Vec<(String, usize)> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0;
    for grapheme in text.graphemes(true) {
        let grapheme_width = grapheme.width();
        if chunk_width > 0 && chunk_width + grapheme_width > width {
            chunks.push((std::mem::take(&mut chunk), chunk_width));
            chunk_width = 0;
        }
        chunk.push_str(grapheme);
        chunk_width += grapheme_width;
    }
    chunks.push((chunk, chunk_width));
    chunks
}

#[derive(Clone, Copy)]
struct Cell<'a> {
    text: &'a str,
    style: Style,
    width: usize,
}

impl Cell<'_> {
    fn is_space(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }

    fn is_wide(&self) -> bool {
        self.width >= 2
    }
}

/// Word-wrap styled text to `width` cells. Break opportunities are whitespace
/// runs and the edges of double-width graphemes (CJK, emoji); anything longer
/// than a line is split by grapheme. Whitespace at a wrap point is dropped.
/// Always returns at least one (possibly empty) line.
fn wrap_spans(segment: &[(&str, Style)], width: usize) -> Vec<Vec<Span<'static>>> {
    let cells: Vec<Cell<'_>> = segment
        .iter()
        .flat_map(|&(text, style)| {
            text.graphemes(true).map(move |grapheme| Cell {
                text: grapheme,
                style,
                width: grapheme.width(),
            })
        })
        .collect();

    let mut lines: Vec<Vec<Cell<'_>>> = Vec::new();
    let mut current: Vec<Cell<'_>> = Vec::new();
    let mut current_width = 0;
    let mut pending_space: &[Cell<'_>] = &[];

    for token in tokens(&cells) {
        let token_width: usize = token.iter().map(|cell| cell.width).sum();
        if token.first().is_some_and(Cell::is_space) {
            if current_width > 0 {
                pending_space = token;
            }
            continue;
        }
        let space_width: usize = pending_space.iter().map(|cell| cell.width).sum();
        if current_width > 0 && current_width + space_width + token_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        } else {
            current.extend_from_slice(pending_space);
            current_width += space_width;
        }
        pending_space = &[];
        for cell in token {
            if current_width > 0 && current_width + cell.width > width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(*cell);
            current_width += cell.width;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines.into_iter().map(merge_cells).collect()
}

/// Group cells into runs that must not be split when wrapping.
fn tokens<'a, 'c>(cells: &'c [Cell<'a>]) -> Vec<&'c [Cell<'a>]> {
    let mut tokens = Vec::new();
    let mut start = 0;
    for index in 1..=cells.len() {
        let boundary = index == cells.len() || {
            let (previous, next) = (&cells[index - 1], &cells[index]);
            let (previous_space, next_space) = (previous.is_space(), next.is_space());
            previous_space != next_space
                || (!previous_space && (previous.is_wide() || next.is_wide()))
        };
        if boundary {
            tokens.push(&cells[start..index]);
            start = index;
        }
    }
    tokens
}

fn merge_cells(cells: Vec<Cell<'_>>) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for cell in cells {
        if let Some(style) = run_style
            && style != cell.style
        {
            spans.push(Span::styled(std::mem::take(&mut run), style));
        }
        run_style = Some(cell.style);
        run.push_str(cell.text);
    }
    if let Some(style) = run_style {
        spans.push(Span::styled(run, style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::theme::Theme;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use unicode_width::UnicodeWidthStr;

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn rendered(markdown: &str, width: usize) -> Vec<String> {
        render(markdown, width, &Theme::default())
            .iter()
            .map(text_of)
            .collect()
    }

    fn style_at(lines: &[Line<'static>], needle: &str) -> Style {
        let found = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains(needle))
            .map(|span| span.style);
        assert!(
            found.is_some(),
            "no span containing {needle:?} in {lines:?}"
        );
        found.unwrap_or_default()
    }

    #[test]
    fn inline_delimiters_are_removed_and_styles_applied() {
        let theme = Theme::default();
        let lines = render("plain **bold** *it* ~~gone~~ `code`", 80, &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(text_of(&lines[0]), "plain bold it gone code");

        assert!(
            style_at(&lines, "bold")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_at(&lines, "it")
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            style_at(&lines, "gone")
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
        assert_eq!(style_at(&lines, "code").fg, Some(theme.colors.warning));
        assert_eq!(style_at(&lines, "plain").fg, Some(theme.colors.text));
        assert!(
            !style_at(&lines, "plain")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn nested_emphasis_combines_modifiers() {
        let lines = render("***both***", 80, &Theme::default());
        let style = style_at(&lines, "both");
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn headings_drop_hashes_and_are_styled() {
        let theme = Theme::default();
        let lines = render("# Title\n\n### Small\n\nbody", 80, &theme);
        assert_eq!(
            lines.iter().map(text_of).collect::<Vec<_>>(),
            ["Title", "", "Small", "", "body"]
        );
        let title = style_at(&lines, "Title");
        assert_eq!(title.fg, Some(theme.colors.accent));
        assert!(title.add_modifier.contains(Modifier::BOLD));
        assert!(
            style_at(&lines, "Small")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn paragraphs_are_separated_by_one_blank_line_and_soft_breaks_join() {
        assert_eq!(
            rendered("one\ntwo\n\nthree  \nfour", 80),
            ["one two", "", "three", "four"]
        );
    }

    #[test]
    fn mixed_style_line_wraps_at_word_boundaries_keeping_styles() {
        let lines = render("aaa **bbb ccc** ddd", 7, &Theme::default());
        assert_eq!(
            lines.iter().map(text_of).collect::<Vec<_>>(),
            ["aaa bbb", "ccc ddd"]
        );
        // The bold run is split across the wrap and both halves stay bold.
        assert!(
            style_at(&lines, "bbb")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            style_at(&lines[1..], "ccc")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !style_at(&lines[1..], "ddd")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        for line in &lines {
            assert!(text_of(line).width() <= 7);
        }
    }

    #[test]
    fn overlong_words_are_hard_wrapped_without_losing_text() {
        let lines = rendered("abcdefghij", 4);
        assert_eq!(lines, ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn fenced_code_preserves_whitespace_and_markup() {
        let theme = Theme::default();
        let lines = render(
            "before\n\n```rust\nfn main() {\n    let **x** = 1;\n}\n```\n\nafter",
            40,
            &theme,
        );
        let text: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(text[0], "before");
        assert_eq!(text[1], "");
        assert_eq!(text[2].trim_end(), "fn main() {");
        assert_eq!(text[3].trim_end(), "    let **x** = 1;");
        assert_eq!(text[4].trim_end(), "}");
        assert_eq!(text[5], "");
        assert_eq!(text[6], "after");
        // Code rows are padded to the full width and carry the block background.
        assert_eq!(text[3].width(), 40);
        assert_eq!(
            lines[3].spans[0].style.bg,
            Some(theme.colors.tool_pending_background)
        );
    }

    #[test]
    fn long_code_lines_wrap_by_width_without_dropping_characters() {
        let lines = rendered("```\n0123456789\n```", 4);
        let joined: String = lines
            .iter()
            .map(|line| line.trim_end().to_owned())
            .collect();
        assert_eq!(joined, "0123456789");
        assert!(lines.iter().all(|line| line.width() == 4));
    }

    #[test]
    fn indented_code_block_is_code() {
        let lines = rendered("text\n\n    indented *code*\n", 30);
        assert_eq!(lines[0], "text");
        assert_eq!(lines[2].trim_end(), "indented *code*");
    }

    #[test]
    fn unclosed_fence_while_streaming_renders_the_partial_code() {
        let lines = rendered("intro\n\n```py\nprint(1)\nprint(", 30);
        assert_eq!(lines[0], "intro");
        assert_eq!(lines[2].trim_end(), "print(1)");
        assert_eq!(lines[3].trim_end(), "print(");
        assert!(!lines.iter().any(|line| line.contains("```")));
    }

    #[test]
    fn unclosed_inline_syntax_stays_literal_while_streaming() {
        assert_eq!(rendered("so **bold", 80), ["so **bold"]);
        assert_eq!(rendered("a `code", 80), ["a `code"]);
        assert_eq!(
            rendered("see [label](http://exa", 80),
            ["see [label](http://exa"]
        );
        assert_eq!(rendered("```", 80), Vec::<String>::new());
        assert_eq!(rendered("|a|b|", 80), ["|a|b|"]);
        assert_eq!(rendered("", 80), Vec::<String>::new());
    }

    #[test]
    fn unordered_lists_use_markers_and_hanging_indent() {
        assert_eq!(
            rendered("- alpha beta gamma delta\n- two", 12),
            ["- alpha beta", "  gamma", "  delta", "- two"]
        );
    }

    #[test]
    fn ordered_lists_keep_numbers_and_start_value() {
        assert_eq!(rendered("3. a\n4. b", 20), ["3. a", "4. b"]);
    }

    #[test]
    fn nested_lists_indent_under_their_parent() {
        assert_eq!(
            rendered("- top\n  - mid\n    1. deep\n- next", 30),
            ["- top", "  - mid", "    1. deep", "- next"]
        );
    }

    #[test]
    fn loose_lists_get_blank_lines_between_items() {
        assert_eq!(rendered("- a\n\n- b", 30), ["- a", "", "- b"]);
    }

    #[test]
    fn list_item_continuation_paragraph_is_indented() {
        assert_eq!(
            rendered("1. first\n\n   more text\n2. second", 30),
            ["1. first", "", "   more text", "", "2. second"]
        );
    }

    #[test]
    fn task_list_items_show_checkboxes() {
        assert_eq!(
            rendered("- [x] done\n- [ ] todo", 30),
            ["[x] done", "[ ] todo"]
        );
    }

    #[test]
    fn code_block_inside_list_item_is_indented() {
        let lines = rendered("- item\n\n  ```\n  code\n  ```", 20);
        assert_eq!(lines[0], "- item");
        assert_eq!(lines[2].trim_end(), "  code");
        assert_eq!(lines[2].width(), 20);
    }

    #[test]
    fn block_quotes_get_a_bar_and_wrap_inside_it() {
        let theme = Theme::default();
        let lines = render("> quoted words here\n>\n> second", 10, &theme);
        assert_eq!(
            lines.iter().map(text_of).collect::<Vec<_>>(),
            ["│ quoted", "│ words", "│ here", "│", "│ second"]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(theme.colors.dim));
        assert_eq!(style_at(&lines, "quoted").fg, Some(theme.colors.muted));
    }

    #[test]
    fn links_show_label_and_url_only_when_they_differ() {
        let theme = Theme::default();
        let lines = render(
            "[docs](https://x.dev) and <https://y.dev> and [https://z.dev](https://z.dev)",
            120,
            &theme,
        );
        assert_eq!(
            text_of(&lines[0]),
            "docs (https://x.dev) and https://y.dev and https://z.dev"
        );
        let label = style_at(&lines, "docs");
        assert_eq!(label.fg, Some(theme.colors.accent));
        assert!(label.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(
            style_at(&lines, " (https://x.dev)").fg,
            Some(theme.colors.dim)
        );
    }

    #[test]
    fn images_render_alt_text_and_url() {
        assert_eq!(
            rendered("![a cat](http://c.at/x.png)", 80),
            ["[image: a cat] (http://c.at/x.png)"]
        );
    }

    #[test]
    fn horizontal_rule_fills_the_width() {
        assert_eq!(rendered("a\n\n---\n\nb", 6), ["a", "", "──────", "", "b"]);
    }

    #[test]
    fn tables_fall_back_to_one_plain_line_per_row() {
        let lines = render(
            "| h1 | h2 |\n|----|----|\n| a | **b** |\n| c | d |",
            40,
            &Theme::default(),
        );
        assert_eq!(
            lines.iter().map(text_of).collect::<Vec<_>>(),
            ["h1 │ h2", "a │ b", "c │ d"]
        );
        assert!(style_at(&lines, "h1").add_modifier.contains(Modifier::BOLD));
        assert!(
            !style_at(&lines[1..], "a")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn wide_characters_wrap_by_display_width() {
        let lines = rendered("漢字漢字漢字", 5);
        assert_eq!(lines, ["漢字", "漢字", "漢字"]);
        assert!(lines.iter().all(|line| line.width() <= 5));

        let emoji = rendered("🦀🦀🦀🦀", 6);
        assert_eq!(emoji, ["🦀🦀🦀", "🦀"]);

        let mixed = rendered("ab 🦀🦀 cd", 6);
        assert!(mixed.iter().all(|line| line.width() <= 6));
        assert_eq!(mixed.concat().replace(' ', ""), "ab🦀🦀cd");
    }

    #[test]
    fn grapheme_clusters_are_never_split() {
        // Family emoji is one grapheme; width 1 forces an overflowing line
        // rather than splitting it.
        let family = "👨‍👩‍👧";
        let lines = rendered(&format!("a{family}b"), 1);
        assert!(lines.iter().any(|line| line == family));
    }

    #[test]
    fn width_of_one_never_panics_or_loops() {
        let lines = rendered(
            "# h\n\n- **a** `b`\n\n> q\n\n```\nxyz\n```\n\n| a | b |\n|---|---|\n| c | d |",
            1,
        );
        assert!(!lines.is_empty());
    }

    #[test]
    fn deeply_nested_quotes_do_not_panic_on_narrow_widths() {
        let lines = rendered(">>>>>>>>>>>> deep", 5);
        assert!(!lines.is_empty());
    }

    #[test]
    fn html_is_shown_literally() {
        assert_eq!(rendered("a <b>x</b> c", 40), ["a <b>x</b> c"]);
        assert_eq!(rendered("<div>\nhi\n</div>", 40), ["<div>", "hi", "</div>"]);
    }

    #[test]
    fn escapes_and_entities_resolve() {
        assert_eq!(rendered(r"\*not\* &amp; &copy;", 40), ["*not* & ©"]);
    }

    #[test]
    fn colors_come_from_the_theme() {
        let mut theme = Theme::default();
        theme.colors.accent = Color::Rgb(1, 2, 3);
        let lines = render("# Head", 20, &theme);
        assert_eq!(style_at(&lines, "Head").fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn rendering_a_20kb_message_is_far_below_a_frame() {
        let section = "## Section\n\nSome **bold** prose with `inline code` and a [link](https://example.com/path) that wraps across the line. 日本語のテキスト 🦀\n\n- item one\n  - nested item\n- item two\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n> quoted text\n\n";
        let message = section.repeat(20_000 / section.len() + 1);
        assert!(message.len() >= 20_000);
        let theme = Theme::default();
        let start = std::time::Instant::now();
        let lines = render(&message, 80, &theme);
        let elapsed = start.elapsed();
        assert!(lines.len() > 100);
        // A 60 fps frame is ~16 ms; debug builds are slow, so allow generous
        // headroom while still catching accidental quadratic behaviour.
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "rendering {} bytes took {elapsed:?}",
            message.len()
        );
    }
}
