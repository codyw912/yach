use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use yach_proto::{
    HarnessOutcomeKind, ToolReviewDecision, ToolReviewHistory, ToolReviewPayload,
    ToolReviewResolution,
};

use crate::theme::Theme;

#[derive(Debug, Clone, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    revision: u64,
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
        outcome_kind: Option<HarnessOutcomeKind>,
    },
    HarnessOutcome {
        kind: HarnessOutcomeKind,
    },
    Error,
}

/// Most lines of live tool output kept visible under a running tool call;
/// older lines scroll away, matching the bounded "active tool card" shape.
const STREAM_TAIL_MAX_LINES: usize = 8;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolReviewRowStatus {
    Pending,
    Submitted(ToolReviewDecision),
    Resolved(ToolReviewResolution),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReviewRow {
    pub request_id: String,
    pub payload: ToolReviewPayload,
    pub status: ToolReviewRowStatus,
    pub selected: ToolReviewDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    pub content: String,
    pub kind: EntryKind,
    /// Bounded call argument preview retained after the final result arrives.
    pub call_preview: String,
    /// Exact bounded tool-result detail. `content` remains the compact summary.
    pub detail: Option<String>,
    /// Per-row state retained even though Wave 2 exposes one global toggle.
    pub expanded: bool,
    /// Optional inline review state for this tool call.
    pub review: Option<ToolReviewRow>,
    /// Bounded tail of live output while a tool call runs; cleared when the
    /// call finishes (the result summary replaces it).
    pub stream_tail: String,
}

impl TranscriptEntry {
    fn new(content: String, kind: EntryKind) -> Self {
        Self {
            content,
            kind,
            stream_tail: String::new(),
            call_preview: String::new(),
            detail: None,
            expanded: false,
            review: None,
        }
    }
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
            self.bump_revision();
            return;
        }

        self.entries.push(TranscriptEntry::new(
            delta.to_owned(),
            EntryKind::AssistantText,
        ));
        self.bump_revision();
    }

    pub fn append_user_message(&mut self, message: &str) {
        self.entries.push(TranscriptEntry::new(
            message.to_owned(),
            EntryKind::UserMessage,
        ));
        self.bump_revision();
    }

    pub fn append_assistant_message(&mut self, message: &str) {
        self.entries.push(TranscriptEntry::new(
            message.to_owned(),
            EntryKind::AssistantText,
        ));
        self.bump_revision();
    }

    pub fn append_tool_call(&mut self, id: Option<&str>, name: &str, preview: Option<&str>) {
        let mut entry = TranscriptEntry::new(
            String::new(),
            EntryKind::ToolCall {
                id: id.map(ToOwned::to_owned),
                name: name.to_owned(),
            },
        );
        preview
            .unwrap_or_default()
            .clone_into(&mut entry.call_preview);
        self.entries.push(entry);
        self.bump_revision();
    }

    /// Append bounded live output under the matching running tool call.
    /// Display-only: older lines beyond the tail cap scroll away, and the
    /// finished result summary replaces the tail entirely.
    pub fn append_tool_call_output(&mut self, id: &str, chunk: &str) {
        let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            matches!(
                &entry.kind,
                EntryKind::ToolCall {
                    id: Some(entry_id),
                    ..
                } if entry_id == id
            )
        }) else {
            return;
        };
        entry.stream_tail.push_str(chunk);
        trim_to_last_lines(&mut entry.stream_tail, STREAM_TAIL_MAX_LINES);
        self.bump_revision();
    }

    /// Append a turn-level error so failures are visible in the scrollback,
    /// not only in the transient status bar.
    pub fn append_error(&mut self, message: &str) {
        self.entries
            .push(TranscriptEntry::new(message.to_owned(), EntryKind::Error));
        self.bump_revision();
    }
    pub fn append_harness_outcome(&mut self, kind: HarnessOutcomeKind, message: &str) {
        self.entries.push(TranscriptEntry::new(
            message.to_owned(),
            EntryKind::HarnessOutcome { kind },
        ));
        self.bump_revision();
    }

    pub fn append_tool_result(
        &mut self,
        id: Option<&str>,
        name: &str,
        result: &str,
        is_error: bool,
    ) {
        self.append_tool_result_with_kind(id, name, result, is_error, None);
    }

    pub fn append_tool_result_with_kind(
        &mut self,
        id: Option<&str>,
        name: &str,
        result: &str,
        is_error: bool,
        outcome_kind: Option<HarnessOutcomeKind>,
    ) {
        self.append_tool_result_record(id, name, result, result, is_error, outcome_kind, None);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a hydrated tool result carries the complete protocol row"
    )]
    pub fn append_tool_result_record(
        &mut self,
        id: Option<&str>,
        name: &str,
        summary: &str,
        detail: &str,
        is_error: bool,
        outcome_kind: Option<HarnessOutcomeKind>,
        review: Option<ToolReviewHistory>,
    ) {
        let mut entry = TranscriptEntry::new(
            summary.to_owned(),
            EntryKind::ToolResult {
                id: id.map(ToOwned::to_owned),
                name: name.to_owned(),
                is_error,
                outcome_kind,
            },
        );
        entry.detail = Some(detail.to_owned());
        entry.review = review.map(review_row_from_history);
        self.entries.push(entry);
        self.bump_revision();
    }

    pub fn finish_tool_call(
        &mut self,
        id: Option<&str>,
        name: &str,
        label: &str,
        result: &str,
        is_error: bool,
    ) -> bool {
        self.finish_tool_call_with_kind(id, name, label, result, is_error, None)
    }

    pub fn finish_tool_call_with_kind(
        &mut self,
        id: Option<&str>,
        name: &str,
        label: &str,
        result: &str,
        is_error: bool,
        outcome_kind: Option<HarnessOutcomeKind>,
    ) -> bool {
        self.finish_tool_call_record(
            id,
            name,
            label,
            result,
            result,
            is_error,
            outcome_kind,
            None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "tool finalization updates the complete transcript row atomically"
    )]
    pub fn finish_tool_call_record(
        &mut self,
        id: Option<&str>,
        name: &str,
        label: &str,
        summary: &str,
        detail: &str,
        is_error: bool,
        outcome_kind: Option<HarnessOutcomeKind>,
        review: Option<ToolReviewHistory>,
    ) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| matches_tool_call(&entry.kind, id, name))
        else {
            return false;
        };

        summary.clone_into(&mut entry.content);
        entry.detail = Some(detail.to_owned());
        entry.expanded = false;
        entry.stream_tail.clear();
        if let Some(review) = review {
            entry.review = Some(review_row_from_history(review));
        }
        entry.kind = EntryKind::ToolResult {
            id: id.map(ToOwned::to_owned),
            name: label.to_owned(),
            is_error,
            outcome_kind,
        };
        self.bump_revision();
        true
    }

    pub fn begin_tool_review(
        &mut self,
        request_id: &str,
        tool_name: &str,
        payload: ToolReviewPayload,
    ) {
        if !self.entries.iter().any(|entry| {
            matches!(
                &entry.kind,
                EntryKind::ToolCall {
                    id: Some(entry_id),
                    ..
                } if entry_id == request_id
            )
        }) {
            self.append_tool_call(Some(request_id), tool_name, None);
        }
        if let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            matches!(
                &entry.kind,
                EntryKind::ToolCall {
                    id: Some(entry_id),
                    ..
                } if entry_id == request_id
            )
        }) {
            entry.review = Some(ToolReviewRow {
                request_id: request_id.to_owned(),
                payload,
                status: ToolReviewRowStatus::Pending,
                selected: ToolReviewDecision::Approve,
            });
            entry.expanded = true;
            self.bump_revision();
        }
    }
    pub fn resolve_tool_review(
        &mut self,
        request_id: &str,
        resolution: ToolReviewResolution,
    ) -> bool {
        let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            entry
                .review
                .as_ref()
                .is_some_and(|review| review.request_id == request_id)
        }) else {
            return false;
        };
        let Some(review) = entry.review.as_mut() else {
            return false;
        };
        review.status = ToolReviewRowStatus::Resolved(resolution);
        entry.expanded = false;
        self.bump_revision();
        true
    }

    pub fn has_pending_review(&self) -> bool {
        self.entries.iter().any(|entry| {
            entry
                .review
                .as_ref()
                .is_some_and(|review| matches!(review.status, ToolReviewRowStatus::Pending))
        })
    }

    pub fn has_unresolved_review(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(entry.kind, EntryKind::ToolCall { .. })
                && entry.review.as_ref().is_some_and(|review| {
                    !matches!(
                        review.status,
                        ToolReviewRowStatus::Resolved(ToolReviewResolution::Interrupted)
                    )
                })
        })
    }

    pub fn select_pending_review(&mut self, decision: ToolReviewDecision) {
        let Some(review) = self.entries.iter_mut().rev().find_map(|entry| {
            entry
                .review
                .as_mut()
                .filter(|review| matches!(review.status, ToolReviewRowStatus::Pending))
        }) else {
            return;
        };
        if review.selected != decision {
            review.selected = decision;
            self.bump_revision();
        }
    }

    pub fn submit_pending_review(
        &mut self,
    ) -> Option<(String, String, String, ToolReviewDecision)> {
        let decision = self.entries.iter().rev().find_map(|entry| {
            entry
                .review
                .as_ref()
                .filter(|review| matches!(review.status, ToolReviewRowStatus::Pending))
                .map(|review| review.selected)
        })?;
        self.submit_pending_review_as(decision)
    }

    pub fn submit_pending_review_as(
        &mut self,
        decision: ToolReviewDecision,
    ) -> Option<(String, String, String, ToolReviewDecision)> {
        let entry = self.entries.iter_mut().rev().find(|entry| {
            entry
                .review
                .as_ref()
                .is_some_and(|review| matches!(review.status, ToolReviewRowStatus::Pending))
        })?;
        let review = entry.review.as_mut()?;
        let (preview_id, permission_decision_id) = review_correlation_ids(&review.payload);
        let submission = (
            review.request_id.clone(),
            preview_id.to_owned(),
            permission_decision_id.to_owned(),
            decision,
        );
        review.selected = decision;
        review.status = ToolReviewRowStatus::Submitted(decision);
        entry.expanded = false;
        self.bump_revision();
        Some(submission)
    }

    pub fn interrupt_pending_reviews(&mut self) {
        let mut changed = false;
        for entry in &mut self.entries {
            if let Some(review) = entry.review.as_mut()
                && matches!(
                    review.status,
                    ToolReviewRowStatus::Pending | ToolReviewRowStatus::Submitted(_)
                )
            {
                review.status = ToolReviewRowStatus::Resolved(ToolReviewResolution::Interrupted);
                entry.expanded = false;
                changed = true;
            }
        }
        if changed {
            self.bump_revision();
        }
    }

    pub fn toggle_tool_details(&mut self) {
        let expand = self.entries.iter().any(|entry| {
            matches!(entry.kind, EntryKind::ToolResult { .. })
                && (entry.detail.is_some() || entry.review.is_some())
                && !entry.expanded
        });
        let mut changed = false;
        for entry in &mut self.entries {
            if matches!(entry.kind, EntryKind::ToolResult { .. })
                && (entry.detail.is_some() || entry.review.is_some())
                && entry.expanded != expand
            {
                entry.expanded = expand;
                changed = true;
            }
        }
        if changed {
            self.bump_revision();
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bump_revision();
    }

    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn compaction_count(&self) -> usize {
        0
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}
fn review_row_from_history(history: ToolReviewHistory) -> ToolReviewRow {
    ToolReviewRow {
        request_id: history.request_id,
        payload: history.payload,
        status: ToolReviewRowStatus::Resolved(history.resolution),
        selected: ToolReviewDecision::Approve,
    }
}

fn review_correlation_ids(payload: &ToolReviewPayload) -> (&str, &str) {
    match payload {
        ToolReviewPayload::LocalEdit { preview } => {
            (&preview.preview_id, &preview.permission_decision_id)
        }
        ToolReviewPayload::Command { command } => {
            (&command.review_id, &command.permission_decision_id)
        }
    }
}

fn review_status_label(status: ToolReviewRowStatus) -> &'static str {
    match status {
        ToolReviewRowStatus::Pending => "pending",
        ToolReviewRowStatus::Submitted(ToolReviewDecision::Approve) => "approve submitted",
        ToolReviewRowStatus::Submitted(ToolReviewDecision::Reject) => "reject submitted",
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Approved) => "approved",
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Rejected) => "rejected",
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Interrupted) => "interrupted",
    }
}

fn review_detail(review: &ToolReviewRow) -> String {
    let mut lines = vec![format!("Review: {}", review_status_label(review.status))];
    match &review.payload {
        ToolReviewPayload::LocalEdit { preview } => {
            lines.push(format!("Path: {}", preview.path));
            lines.push(format!("Operation: {}", preview.operation));
            lines.push(String::from("Diff:"));
            lines.push(preview.diff_summary.clone());
            if preview.diff_summary_truncated {
                lines.push(String::from("[diff summary truncated]"));
            }
        }
        ToolReviewPayload::Command { command } => {
            lines.push(format!("Command: {}", command.command));
            lines.push(format!(
                "Workdir: {}",
                command.workdir.as_deref().unwrap_or(".")
            ));
            lines.push(format!("Timeout: {}ms", command.timeout_ms));
        }
    }
    if matches!(review.status, ToolReviewRowStatus::Pending) {
        let approve = if review.selected == ToolReviewDecision::Approve {
            "› Approve"
        } else {
            "  Approve"
        };
        let reject = if review.selected == ToolReviewDecision::Reject {
            "› Reject"
        } else {
            "  Reject"
        };
        lines.push(approve.to_owned());
        lines.push(reject.to_owned());
        lines.push(String::from("↑/↓ or j/k select · Enter confirm"));
    }
    lines.join("\n")
}

fn entry_display_text(entry: &TranscriptEntry) -> String {
    match &entry.kind {
        EntryKind::ToolCall { .. } => {
            let mut sections = Vec::new();
            if !entry.call_preview.is_empty() {
                sections.push(entry.call_preview.clone());
            }
            if let Some(review) = &entry.review {
                if matches!(review.status, ToolReviewRowStatus::Pending) {
                    sections.push(review_detail(review));
                } else {
                    sections.push(format!("Review: {}", review_status_label(review.status)));
                }
            }
            sections.join("\n")
        }
        EntryKind::ToolResult { .. } if entry.expanded => {
            let mut sections = Vec::new();
            if !entry.call_preview.is_empty() {
                sections.push(format!("Call: {}", entry.call_preview));
            }
            if let Some(review) = &entry.review {
                sections.push(review_detail(review));
            }
            if let Some(detail) = &entry.detail {
                sections.push(format!("Output:\n{detail}"));
            } else if !entry.content.is_empty() {
                sections.push(entry.content.clone());
            }
            sections.join("\n")
        }
        EntryKind::ToolResult { name, .. } => {
            let mut summary = entry.content.clone();
            if let Some(review) = &entry.review {
                if !summary.is_empty() {
                    summary.push_str("; ");
                }
                summary.push_str("review ");
                summary.push_str(review_status_label(review.status));
            }
            if let Some(preview) = collapsed_tool_output(name, &summary, entry.detail.as_deref()) {
                if !summary.is_empty() {
                    summary.push('\n');
                }
                summary.push_str(&preview);
            }
            summary
        }
        EntryKind::UserMessage
        | EntryKind::AssistantText
        | EntryKind::HarnessOutcome { .. }
        | EntryKind::Error => entry.content.clone(),
    }
}

const DEFAULT_TOOL_OUTPUT_PREVIEW_LINES: usize = 10;
const COMMAND_OUTPUT_PREVIEW_LINES: usize = 5;

fn collapsed_tool_output(name: &str, summary: &str, detail: Option<&str>) -> Option<String> {
    let detail = detail?.trim_end_matches('\n');
    if detail.is_empty() || detail == summary {
        return None;
    }

    let line_count = detail.lines().count();
    let is_command =
        name.starts_with("bash") || name.starts_with("shell") || name.starts_with("exec");
    let max_lines = if is_command {
        COMMAND_OUTPUT_PREVIEW_LINES
    } else {
        DEFAULT_TOOL_OUTPUT_PREVIEW_LINES
    };
    if line_count <= max_lines {
        return Some(detail.to_owned());
    }

    let omitted = line_count - max_lines;
    if is_command {
        let tail = detail.lines().skip(omitted).collect::<Vec<_>>().join("\n");
        Some(format!("… {omitted} earlier lines\n{tail}"))
    } else {
        let head = detail
            .lines()
            .take(max_lines)
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!("{head}\n… {omitted} more lines"))
    }
}

#[derive(Debug, Default)]
pub struct TranscriptRenderCache {
    width: u16,
    revision: u64,
    entries_len: usize,
    theme: Theme,
    lines: Vec<Line<'static>>,
}

impl TranscriptRenderCache {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_theme(theme: Theme) -> Self {
        Self {
            theme,
            ..Self::default()
        }
    }

    pub fn ensure(&mut self, transcript: &Transcript, width: u16) {
        let width = width.max(1);
        if self.width == width
            && self.revision == transcript.revision()
            && self.entries_len == transcript.entries().len()
        {
            return;
        }

        self.width = width;
        self.revision = transcript.revision();
        self.entries_len = transcript.entries().len();
        self.lines = render_lines_with_theme(transcript.entries(), width, &self.theme);
    }

    pub fn max_scroll_start(&mut self, transcript: &Transcript, width: u16, height: u16) -> usize {
        self.ensure(transcript, width);
        self.lines.len().saturating_sub(height as usize)
    }

    pub fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        transcript: &Transcript,
        scroll_offset: usize,
        is_streaming: bool,
    ) {
        self.ensure(transcript, area.width);
        render_cached_lines(
            area,
            buf,
            &self.lines,
            scroll_offset,
            is_streaming,
            &self.theme,
        );
    }
}

pub fn render(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    transcript: &Transcript,
    cache: &mut TranscriptRenderCache,
    scroll_offset: usize,
    is_streaming: bool,
) {
    cache.render(area, buf, transcript, scroll_offset, is_streaming);
}

#[cfg(test)]
fn render_uncached(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    entries: &[TranscriptEntry],
    scroll_offset: usize,
    is_streaming: bool,
) {
    let lines = render_lines(entries, area.width);
    render_cached_lines(
        area,
        buf,
        &lines,
        scroll_offset,
        is_streaming,
        &Theme::default(),
    );
}
fn render_cached_lines(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    lines: &[Line<'static>],
    scroll_offset: usize,
    is_streaming: bool,
    theme: &Theme,
) {
    let total_lines = lines.len();
    let start = scroll_offset.min(total_lines.saturating_sub(area.height as usize));
    let mut visible: Vec<Line<'_>> = lines
        .iter()
        .skip(start)
        .take(area.height as usize)
        .cloned()
        .collect();

    let top_padding = bottom_aligned_top_padding(visible.len(), area.height as usize);
    if top_padding > 0 {
        let mut padded = Vec::with_capacity(top_padding + visible.len());
        padded.extend(std::iter::repeat_with(|| Line::raw("")).take(top_padding));
        padded.append(&mut visible);
        visible = padded;
    }

    let foreground = theme.colors.text;
    let mut paragraph = Paragraph::new(visible).style(Style::new().fg(foreground));
    if is_streaming {
        paragraph = paragraph.style(Style::new().fg(foreground));
    }

    Widget::render(paragraph, area, buf);
}

#[cfg(test)]
fn render_lines(entries: &[TranscriptEntry], width: u16) -> Vec<Line<'static>> {
    render_lines_with_theme(entries, width, &Theme::default())
}

fn render_lines_with_theme(
    entries: &[TranscriptEntry],
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut previous_kind = None;
    for entry in entries {
        if !lines.is_empty() {
            let gap = if matches!(
                previous_kind,
                Some(previous) if is_tool_entry(previous) && is_tool_entry(&entry.kind)
            ) {
                theme.spacing.tool_gap
            } else {
                1
            };
            lines.extend(std::iter::repeat_with(|| Line::raw("")).take(usize::from(gap)));
        }
        lines.extend(render_entry_lines(entry, width, theme));
        previous_kind = Some(&entry.kind);
    }
    lines
}

fn render_entry_lines(entry: &TranscriptEntry, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let display_text = entry_display_text(entry);
    let colors = theme.colors;
    let (prefix, continuation, content, content_style, prefix_width) = match &entry.kind {
        EntryKind::UserMessage => return render_user_message_lines(&display_text, width, theme),
        EntryKind::AssistantText => (
            Span::styled("• ", Style::new().fg(colors.accent)),
            Span::raw("  "),
            display_text,
            Style::new().fg(colors.text),
            2,
        ),
        EntryKind::ToolCall { name, .. } => (
            Span::styled("⚙ ", Style::new().fg(colors.warning).bold()),
            Span::styled("│ ", Style::new().fg(colors.dim)),
            name_and_detail(name, &display_text),
            Style::new().fg(colors.tool_title),
            2,
        ),
        EntryKind::ToolResult {
            name,
            outcome_kind: Some(kind),
            ..
        } => {
            let (label, style) = harness_outcome_style(*kind, theme);
            (
                Span::styled("! ", style),
                Span::styled("│ ", Style::new().fg(colors.dim)),
                name_and_detail(&format!("{label} {name}"), &display_text),
                style,
                2,
            )
        }
        EntryKind::ToolResult { name, is_error, .. } => {
            let (marker, marker_style, content_style) = if *is_error {
                (
                    "✗ ",
                    Style::new().fg(colors.error).bold(),
                    Style::new().fg(colors.error),
                )
            } else {
                (
                    "✓ ",
                    Style::new().fg(colors.success),
                    Style::new().fg(colors.tool_output),
                )
            };
            (
                Span::styled(marker, marker_style),
                Span::styled("│ ", Style::new().fg(colors.dim)),
                name_and_detail(name, &display_text),
                content_style,
                2,
            )
        }
        EntryKind::HarnessOutcome { kind } => {
            let (label, style) = harness_outcome_style(*kind, theme);
            (
                Span::styled("! ", style),
                Span::raw("  "),
                name_and_detail(label, &display_text),
                style,
                2,
            )
        }
        EntryKind::Error => (
            Span::styled("✗ ", Style::new().fg(colors.error).bold()),
            Span::raw("  "),
            display_text,
            Style::new().fg(colors.error),
            2,
        ),
    };

    let is_tool = is_tool_entry(&entry.kind);
    let horizontal_padding = if is_tool {
        usize::from(theme.spacing.tool_horizontal_padding)
    } else {
        0
    };
    let available_width = usize::from(width)
        .saturating_sub(horizontal_padding.saturating_mul(2))
        .saturating_sub(prefix_width)
        .max(1);
    let wrapped = wrap_text(&content, available_width);
    let mut result = Vec::with_capacity(wrapped.len() + entry.stream_tail.lines().count());
    for (index, line) in wrapped.into_iter().enumerate() {
        result.push(Line::from(vec![
            if index == 0 {
                prefix.clone()
            } else {
                continuation.clone()
            },
            Span::styled(
                line.clone(),
                transcript_line_style(entry, &line, content_style, theme),
            ),
        ]));
    }
    if !entry.stream_tail.is_empty() {
        let tail_style = Style::new().fg(colors.dim);
        for line in wrap_text(&entry.stream_tail, available_width) {
            result.push(Line::from(vec![
                continuation.clone(),
                Span::styled(line, tail_style),
            ]));
        }
    }
    if is_tool {
        render_tool_surface(result, width, tool_background(entry, theme), theme)
    } else {
        result
    }
}

fn render_user_message_lines(display_text: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let horizontal_padding =
        usize::from(theme.spacing.user_message_horizontal_padding).min(width.saturating_sub(1) / 2);
    let inner_width = width
        .saturating_sub(horizontal_padding.saturating_mul(2))
        .max(1);
    let prefix = if inner_width == 1 { "›" } else { "› " };
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);
    let content_width = inner_width.saturating_sub(prefix_width).max(1);
    let background = theme.colors.user_message_background;
    let background_style = Style::new().bg(background);
    let prefix_style = Style::new().fg(theme.colors.accent).bg(background).bold();
    let content_style = Style::new()
        .fg(theme.colors.user_message_text)
        .bg(background);
    let vertical_padding = usize::from(theme.spacing.user_message_vertical_padding);
    let mut lines = Vec::new();
    lines.extend(
        std::iter::repeat_with(|| Line::from(Span::styled(" ".repeat(width), background_style)))
            .take(vertical_padding),
    );
    lines.extend(
        wrap_text(display_text, content_width)
            .into_iter()
            .enumerate()
            .map(|(index, content)| {
                let line_prefix = if index == 0 {
                    prefix.to_owned()
                } else {
                    " ".repeat(prefix_width)
                };
                let used_width =
                    prefix_width + unicode_width::UnicodeWidthStr::width(content.as_str());
                Line::from(vec![
                    Span::styled(" ".repeat(horizontal_padding), background_style),
                    Span::styled(line_prefix, prefix_style),
                    Span::styled(content, content_style),
                    Span::styled(
                        " ".repeat(
                            inner_width
                                .saturating_sub(used_width)
                                .saturating_add(horizontal_padding),
                        ),
                        background_style,
                    ),
                ])
            }),
    );
    lines.extend(
        std::iter::repeat_with(|| Line::from(Span::styled(" ".repeat(width), background_style)))
            .take(vertical_padding),
    );
    lines
}

fn render_tool_surface(
    lines: Vec<Line<'static>>,
    width: u16,
    background: Color,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let horizontal_padding =
        usize::from(theme.spacing.tool_horizontal_padding).min(width.saturating_sub(1) / 2);
    let inner_width = width.saturating_sub(horizontal_padding.saturating_mul(2));
    let background_style = Style::new().bg(background);
    let vertical_padding = usize::from(theme.spacing.tool_vertical_padding);
    let mut rendered = Vec::with_capacity(lines.len() + vertical_padding.saturating_mul(2));
    rendered.extend(
        std::iter::repeat_with(|| Line::from(Span::styled(" ".repeat(width), background_style)))
            .take(vertical_padding),
    );
    for mut line in lines {
        let line_width = line.width().min(inner_width);
        for span in &mut line.spans {
            span.style = span.style.bg(background);
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 2);
        spans.push(Span::styled(
            " ".repeat(horizontal_padding),
            background_style,
        ));
        spans.extend(line.spans);
        spans.push(Span::styled(
            " ".repeat(
                inner_width
                    .saturating_sub(line_width)
                    .saturating_add(horizontal_padding),
            ),
            background_style,
        ));
        rendered.push(Line::from(spans));
    }
    rendered.extend(
        std::iter::repeat_with(|| Line::from(Span::styled(" ".repeat(width), background_style)))
            .take(vertical_padding),
    );
    rendered
}

fn tool_background(entry: &TranscriptEntry, theme: &Theme) -> Color {
    match entry.kind {
        EntryKind::ToolResult { is_error: true, .. }
        | EntryKind::ToolResult {
            outcome_kind: Some(_),
            ..
        } => theme.colors.tool_error_background,
        EntryKind::ToolResult { .. } => theme.colors.tool_success_background,
        _ => theme.colors.tool_pending_background,
    }
}

fn transcript_line_style(
    entry: &TranscriptEntry,
    line: &str,
    default: Style,
    theme: &Theme,
) -> Style {
    if entry.review.is_none() {
        return default;
    }
    if line.starts_with("+++ ") || line.starts_with("--- ") {
        return Style::new().fg(theme.colors.diff_context).bold();
    }
    if line.starts_with('+') {
        return Style::new().fg(theme.colors.diff_added);
    }
    if line.starts_with('-') {
        return Style::new().fg(theme.colors.diff_removed);
    }
    if line.starts_with("@@") || line.starts_with('›') {
        return Style::new().fg(theme.colors.diff_hunk).bold();
    }
    if line.starts_with("  Approve") || line.starts_with("  Reject") || line.starts_with("↑/↓")
    {
        return Style::new().fg(theme.colors.dim);
    }
    default
}

fn is_tool_entry(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::ToolCall { .. } | EntryKind::ToolResult { .. }
    )
}

fn name_and_detail(name: &str, detail: &str) -> String {
    if detail.is_empty() {
        name.to_owned()
    } else {
        format!("{name} {detail}")
    }
}
fn harness_outcome_style(kind: HarnessOutcomeKind, theme: &Theme) -> (&'static str, Style) {
    (kind.label(), Style::new().fg(theme.colors.harness).bold())
}

/// Drop leading lines so at most `max_lines` newline-terminated lines (plus
/// any trailing partial line) remain.
fn trim_to_last_lines(text: &mut String, max_lines: usize) {
    let mut newlines_from_end = 0;
    for index in (0..text.len()).rev() {
        if text.as_bytes()[index] == b'\n' {
            newlines_from_end += 1;
            if newlines_from_end > max_lines {
                text.drain(..=index);
                return;
            }
        }
    }
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
        EntryKind, HarnessOutcomeKind, ToolReviewRowStatus, Transcript, TranscriptRenderCache,
        bottom_aligned_top_padding, char_boundary_at_or_before, entry_display_text,
        harness_outcome_style, render_lines, render_lines_with_theme, render_uncached, wrap_text,
    };
    use crate::theme::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use yach_proto::{
        CommandReviewSummary, LocalEditPreviewSummary, LocalEditReviewState, ToolReviewDecision,
        ToolReviewHistory, ToolReviewPayload, ToolReviewResolution,
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
    fn tool_call_output_streams_into_a_bounded_tail_and_clears_on_finish() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(Some("call-1"), "bash", Some("cargo test"));

        transcript.append_tool_call_output("call-1", "Compiling yach-proto\n");
        transcript.append_tool_call_output("call-1", "Compiling yach-backend\n");
        assert_eq!(
            transcript.entries()[0].stream_tail,
            "Compiling yach-proto\nCompiling yach-backend\n"
        );

        // Output for an unknown id is ignored, not appended anywhere.
        transcript.append_tool_call_output("call-unknown", "noise\n");
        assert_eq!(
            transcript.entries()[0].stream_tail,
            "Compiling yach-proto\nCompiling yach-backend\n"
        );

        assert!(transcript.finish_tool_call(
            Some("call-1"),
            "bash",
            "bash cargo test",
            "completed: exit 0",
            false,
        ));
        assert!(transcript.entries()[0].stream_tail.is_empty());
    }

    #[test]
    fn tool_call_output_tail_keeps_only_the_last_lines() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(Some("call-1"), "bash", None);
        for index in 0..30 {
            transcript.append_tool_call_output("call-1", &format!("line-{index}\n"));
        }

        let tail = &transcript.entries()[0].stream_tail;
        assert!(!tail.contains("line-0\n"));
        assert!(tail.contains("line-29\n"));
        assert!(tail.lines().count() <= 8);
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
    fn inline_review_row_moves_submits_and_resolves_in_place() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(Some("request-1"), "edit_text_file", Some("src/lib.rs"));
        transcript.begin_tool_review(
            "request-1",
            "edit_text_file",
            ToolReviewPayload::LocalEdit {
                preview: LocalEditPreviewSummary {
                    preview_id: String::from("preview-1"),
                    transaction_id: String::from("transaction-1"),
                    permission_decision_id: String::from("permission-1"),
                    path: String::from("src/lib.rs"),
                    operation: String::from("modify_text_file"),
                    review_state: LocalEditReviewState::NeedsUserApproval,
                    diff_summary: String::from("-old\n+new"),
                    diff_summary_truncated: false,
                },
            },
        );

        assert!(transcript.has_pending_review());
        transcript.select_pending_review(ToolReviewDecision::Reject);
        assert_eq!(
            transcript.submit_pending_review(),
            Some((
                String::from("request-1"),
                String::from("preview-1"),
                String::from("permission-1"),
                ToolReviewDecision::Reject,
            ))
        );
        assert!(transcript.has_unresolved_review());
        let submitted = entry_display_text(&transcript.entries()[0]);
        assert!(submitted.contains("Review: reject submitted"));
        assert!(!submitted.contains("-old"));
        assert!(transcript.resolve_tool_review("request-1", ToolReviewResolution::Rejected));

        assert!(transcript.finish_tool_call_record(
            Some("request-1"),
            "edit_text_file",
            "edit_text_file src/lib.rs",
            "denied: 1 line, 20 bytes",
            "[rejected by review]",
            true,
            Some(HarnessOutcomeKind::Denied),
            None,
        ));
        let entry = &transcript.entries()[0];
        assert_eq!(
            entry.review.as_ref().map(|review| review.status),
            Some(ToolReviewRowStatus::Resolved(
                ToolReviewResolution::Rejected
            ))
        );
        assert!(!entry.expanded);
        assert_eq!(entry.detail.as_deref(), Some("[rejected by review]"));
    }

    #[test]
    fn command_review_row_renders_command_and_inline_selector() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(Some("request-1"), "bash", Some("cargo test"));
        transcript.begin_tool_review(
            "request-1",
            "bash",
            ToolReviewPayload::Command {
                command: CommandReviewSummary {
                    review_id: String::from("command-review-1"),
                    permission_decision_id: String::from("permission-1"),
                    command: String::from("cargo test"),
                    workdir: Some(String::from("/workspace")),
                    timeout_ms: 30_000,
                },
            },
        );

        let lines = render_lines(transcript.entries(), 100);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(rendered.contains("Command: cargo test"));
        assert!(rendered.contains("Workdir: /workspace"));
        assert!(rendered.contains("› Approve"));
        assert!(rendered.contains("  Reject"));
        assert!(rendered.contains("↑/↓ or j/k select · Enter confirm"));
        assert!(lines.iter().all(|line| line.spans.iter().all(|span| {
            span.style.bg == Some(Theme::default().colors.tool_pending_background)
        })));
    }

    #[test]
    fn resumed_tool_result_keeps_review_history_and_exact_output_detail() {
        let mut transcript = Transcript::new();
        let history = ToolReviewHistory {
            request_id: String::from("request-1"),
            payload: ToolReviewPayload::Command {
                command: CommandReviewSummary {
                    review_id: String::from("command-review-1"),
                    permission_decision_id: String::from("permission-1"),
                    command: String::from("cargo test"),
                    workdir: None,
                    timeout_ms: 30_000,
                },
            },
            resolution: ToolReviewResolution::Approved,
        };
        transcript.append_tool_result_record(
            Some("request-1"),
            "bash",
            "completed: 2 lines, 12 bytes",
            "line 1\nline 2",
            false,
            None,
            Some(history),
        );
        transcript.toggle_tool_details();

        let entry = &transcript.entries()[0];
        assert!(entry.expanded);
        assert_eq!(entry.detail.as_deref(), Some("line 1\nline 2"));
        assert_eq!(
            entry.review.as_ref().map(|review| review.status),
            Some(ToolReviewRowStatus::Resolved(
                ToolReviewResolution::Approved
            ))
        );
    }

    #[test]
    fn transcript_revision_changes_on_mutation() {
        let mut transcript = Transcript::new();
        let initial = transcript.revision();
        transcript.append_user_message("hello");
        assert!(transcript.revision() > initial);
        let after_append = transcript.revision();
        transcript.append_delta("world");
        assert!(transcript.revision() > after_append);
        let after_delta = transcript.revision();
        transcript.clear();
        assert!(transcript.revision() > after_delta);
    }

    #[test]
    fn render_cache_matches_uncached_render() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("hello world");
        transcript.append_delta("assistant response with enough words to wrap");
        transcript.append_tool_call(Some("call-1"), "Read", Some("src/lib.rs"));
        transcript.append_tool_result(Some("call-1"), "Read", "Unicode 🦀 測試", false);

        let area = Rect::new(0, 0, 24, 8);
        let mut uncached = Buffer::empty(area);
        render_uncached(area, &mut uncached, transcript.entries(), 1, false);

        let mut cached = Buffer::empty(area);
        let mut cache = TranscriptRenderCache::new();
        cache.render(area, &mut cached, &transcript, 1, false);

        assert_eq!(cached, uncached);
    }

    #[test]
    fn render_cache_rebuilds_on_revision_and_width_change() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("hello world this line wraps");
        let mut cache = TranscriptRenderCache::new();
        let first = cache.max_scroll_start(&transcript, 10, 1);
        let wider = cache.max_scroll_start(&transcript, 80, 1);
        assert!(first >= wider);

        transcript.append_user_message("another line");
        let after_mutation = cache.max_scroll_start(&transcript, 80, 1);
        assert!(after_mutation > wider);
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
    fn collapsed_tool_rows_show_bounded_output_previews() {
        let mut transcript = Transcript::new();
        let command_output = (1..=8)
            .map(|line| format!("command-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        transcript.append_tool_result_record(
            Some("call-1"),
            "bash cargo test",
            "completed: 8 lines, 80 bytes",
            &command_output,
            false,
            None,
            None,
        );
        let command = entry_display_text(&transcript.entries()[0]);
        assert!(command.contains("… 3 earlier lines"));
        assert!(!command.contains("command-1"));
        assert!(command.contains("command-4"));
        assert!(command.contains("command-8"));

        let read_output = (1..=12)
            .map(|line| format!("read-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        transcript.append_tool_result_record(
            Some("call-2"),
            "read_text_file src/lib.rs",
            "completed: 12 lines, 120 bytes",
            &read_output,
            false,
            None,
            None,
        );
        let read = entry_display_text(&transcript.entries()[1]);
        assert!(read.contains("read-1"));
        assert!(read.contains("read-10"));
        assert!(!read.contains("read-11"));
        assert!(read.contains("… 2 more lines"));
    }

    #[test]
    fn render_lines_preserves_representative_entries() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("user");
        transcript.append_delta("assistant");
        transcript.append_tool_call(None, "Read", Some("preview"));

        let lines = render_lines(transcript.entries(), 80);
        let theme = Theme::default();
        assert_eq!(lines.len(), 9);
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme.colors.user_message_background))
        );
        assert_eq!(lines[1].spans[1].content, "› ");
        assert_eq!(
            lines[1].spans[2].style.fg,
            Some(theme.colors.user_message_text)
        );
        assert!(lines[3].spans.is_empty());
        assert_eq!(lines[4].spans[0].content, "• ");
        assert_eq!(lines[4].spans[1].style.fg, Some(theme.colors.text));
        assert!(lines[5].spans.is_empty());
        assert!(
            lines[6]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme.colors.tool_pending_background))
        );
    }

    #[test]
    fn custom_theme_colors_user_and_tool_surfaces() {
        let parsed = Theme::from_json(
            r##"{
                "colors": {
                    "userMessageBackground": "#010203",
                    "userMessageText": "#040506",
                    "toolPendingBackground": "#070809"
                },
                "spacing": {
                    "userMessageVerticalPadding": 0,
                    "toolVerticalPadding": 0
                }
            }"##,
        );
        assert!(parsed.is_ok());
        let Ok(theme) = parsed else {
            return;
        };
        let mut transcript = Transcript::new();
        transcript.append_user_message("hello");
        transcript.append_tool_call(Some("tool-1"), "read", Some("sample.rs"));

        let lines = render_lines_with_theme(transcript.entries(), 40, &theme);
        let user_line = lines.iter().find(|line| line.to_string().contains("hello"));
        assert!(user_line.is_some());
        let Some(user_line) = user_line else {
            return;
        };
        let tool_line = lines
            .iter()
            .find(|line| line.to_string().contains("sample.rs"));
        assert!(tool_line.is_some());
        let Some(tool_line) = tool_line else {
            return;
        };

        assert!(
            user_line
                .spans
                .iter()
                .all(|span| { span.style.bg == Some(theme.colors.user_message_background) })
        );
        assert!(
            user_line
                .spans
                .iter()
                .any(|span| { span.style.fg == Some(theme.colors.user_message_text) })
        );
        assert!(
            tool_line
                .spans
                .iter()
                .all(|span| { span.style.bg == Some(theme.colors.tool_pending_background) })
        );
    }

    #[test]
    fn failed_tool_results_render_failure_marker() {
        let mut transcript = Transcript::new();
        transcript.append_tool_result(None, "Read", "failed: missing file", true);

        let lines = render_lines(transcript.entries(), 80);
        let first_line = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(first_line.contains("✗ Read"));
    }

    #[test]
    fn adjacent_tool_rows_use_separate_success_surfaces() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("inspect");
        transcript.append_tool_result(None, "Read", "completed: 1 line", false);
        transcript.append_tool_result(None, "search", "completed: 2 matches", false);

        let lines = render_lines(transcript.entries(), 80);
        let background = Theme::default().colors.tool_success_background;
        assert_eq!(lines.len(), 11);
        assert!(lines[3].spans.is_empty());
        assert!(lines[7].spans.is_empty());
        assert!(lines[4..=6].iter().all(|line| {
            line.spans
                .iter()
                .all(|span| span.style.bg == Some(background))
        }));
        assert!(lines[8..=10].iter().all(|line| {
            line.spans
                .iter()
                .all(|span| span.style.bg == Some(background))
        }));
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
    #[test]
    fn harness_outcome_styles_are_distinct_and_labeled() {
        let theme = Theme::default();
        let (failed_label, failed_style) =
            harness_outcome_style(HarnessOutcomeKind::Failed, &theme);
        let (denied_label, denied_style) =
            harness_outcome_style(HarnessOutcomeKind::Denied, &theme);
        assert_eq!(failed_label, "failed");
        assert_eq!(denied_label, "denied");
        assert_eq!(failed_style, denied_style);
        assert!(
            failed_style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );

        let mut transcript = Transcript::new();
        transcript.entries.push(super::TranscriptEntry::new(
            String::from("turn stopped"),
            EntryKind::HarnessOutcome {
                kind: HarnessOutcomeKind::Limit,
            },
        ));
        let lines = render_lines(transcript.entries(), 80);
        let text = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.starts_with("! limit "));
    }
}
