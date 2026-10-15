use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use yach_proto::{
    HarnessOutcomeKind, PromptAttemptResetError, ToolReviewDecision, ToolReviewHistory,
    ToolReviewPayload, ToolReviewResolution,
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
    Status,
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
    /// A context-compaction checkpoint. The backend owns compaction; this
    /// records that one occurred so `/status` and the status bar can report
    /// observed compactions instead of a placeholder.
    Compaction,
    Error,
}

/// Most lines of live tool output kept visible under a running tool call;
/// older lines scroll away, matching the bounded "active tool card" shape.
///
/// 24 is enough to show a typical cargo/rustc failure (error, snippet, and
/// notes) instead of a spinner tail, while still leaving room in a normal
/// terminal for the prompt, status bar, and prior history. This stays a
/// deliberate retention cap, not unbounded scrollback: completed output is
/// replaced by the result summary and remains expandable separately.
const STREAM_TAIL_MAX_LINES: usize = 24;
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
    /// Render identity. Every entry gets a process-unique stamp when created
    /// and a fresh one on every in-place change, so two entries with equal
    /// stamps render identically. `TranscriptRenderCache` reuses lines on it.
    stamp: u64,
}

static NEXT_ENTRY_STAMP: AtomicU64 = AtomicU64::new(1);

fn next_entry_stamp() -> u64 {
    NEXT_ENTRY_STAMP.fetch_add(1, Ordering::Relaxed)
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
            stamp: next_entry_stamp(),
        }
    }

    /// Mark this entry changed. Every in-place mutation must call it, or the
    /// render cache keeps showing the old lines.
    fn touch(&mut self) {
        self.stamp = next_entry_stamp();
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
            last.touch();
            self.bump_revision();
            return;
        }

        self.entries.push(TranscriptEntry::new(
            delta.to_owned(),
            EntryKind::AssistantText,
        ));
        self.bump_revision();
    }

    /// Retract exactly `discarded_utf8_bytes` from the contiguous trailing
    /// assistant-text suffix. Stops at any non-assistant row; fail-closed
    /// leaves entries unchanged.
    pub fn truncate_assistant_suffix_bytes(
        &mut self,
        discarded_utf8_bytes: usize,
    ) -> Result<(), PromptAttemptResetError> {
        if discarded_utf8_bytes == 0 {
            return Ok(());
        }

        let mut remaining = discarded_utf8_bytes;
        let mut first_touched = self.entries.len();
        let mut keep_in_first_touched = None;

        for idx in (0..self.entries.len()).rev() {
            if !matches!(self.entries[idx].kind, EntryKind::AssistantText) {
                break;
            }
            let len = self.entries[idx].content.len();
            if remaining > len {
                remaining -= len;
                first_touched = idx;
                keep_in_first_touched = Some(0);
                continue;
            }
            let keep = len - remaining;
            if !self.entries[idx].content.is_char_boundary(keep) {
                return Err(PromptAttemptResetError::InvalidUtf8Boundary);
            }
            first_touched = idx;
            keep_in_first_touched = Some(keep);
            remaining = 0;
            break;
        }

        if remaining != 0 {
            return Err(PromptAttemptResetError::ImpossibleByteCount);
        }
        let Some(keep) = keep_in_first_touched else {
            return Err(PromptAttemptResetError::ImpossibleByteCount);
        };

        if keep == 0 {
            self.entries.truncate(first_touched);
        } else {
            self.entries[first_touched].content.truncate(keep);
            self.entries[first_touched].touch();
            self.entries.truncate(first_touched + 1);
        }
        self.bump_revision();
        Ok(())
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

    pub fn append_status(&mut self, message: &str) {
        self.entries
            .push(TranscriptEntry::new(message.to_owned(), EntryKind::Status));
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
        entry.touch();
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

    pub fn append_compaction(&mut self, message: &str) {
        self.entries.push(TranscriptEntry::new(
            message.to_owned(),
            EntryKind::Compaction,
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
        entry.touch();
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
            entry.expanded = false;
            entry.touch();
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
        entry.touch();
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

    /// Whether a review row is still unfinished, from request until the tool's
    /// result arrives.
    ///
    /// Deliberately includes `Resolved`: between the user's decision and the
    /// tool's result the call is still in flight. This does not hold the
    /// keyboard (only `has_pending_review` does); it only keeps UI handoffs
    /// such as the thinking selector from opening mid-call. Only an
    /// interrupted review releases it early.
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

    /// Currently highlighted option on the pending review row, if any.
    #[must_use]
    pub fn pending_review_selection(&self) -> Option<ToolReviewDecision> {
        self.entries.iter().rev().find_map(|entry| {
            entry
                .review
                .as_ref()
                .filter(|review| matches!(review.status, ToolReviewRowStatus::Pending))
                .map(|review| review.selected)
        })
    }

    /// Whether the pending review is a command, which is the only payload
    /// offering a session grant.
    #[must_use]
    pub fn pending_review_is_command(&self) -> bool {
        self.entries.iter().rev().any(|entry| {
            entry.review.as_ref().is_some_and(|review| {
                matches!(review.status, ToolReviewRowStatus::Pending)
                    && matches!(review.payload, ToolReviewPayload::Command { .. })
            })
        })
    }

    pub fn select_pending_review(&mut self, decision: ToolReviewDecision) {
        let Some(entry) = self.entries.iter_mut().rev().find(|entry| {
            entry
                .review
                .as_ref()
                .is_some_and(|review| matches!(review.status, ToolReviewRowStatus::Pending))
        }) else {
            return;
        };
        let Some(review) = entry.review.as_mut() else {
            return;
        };
        if review.selected != decision {
            review.selected = decision;
            entry.touch();
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
        entry.touch();
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
                entry.touch();
                changed = true;
            }
        }
        if changed {
            self.bump_revision();
        }
    }

    pub fn toggle_tool_details(&mut self) {
        let expand = self
            .entries
            .iter()
            .any(|entry| has_expandable_details(entry) && !entry.expanded);
        let mut changed = false;
        for entry in &mut self.entries {
            if has_expandable_details(entry) && entry.expanded != expand {
                entry.expanded = expand;
                entry.touch();
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

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn drain_prefix_lines(
        &mut self,
        count: usize,
        width: u16,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let count = count.min(self.entries.len());
        if count == 0 {
            return Vec::new();
        }
        let lines = render_lines_with_theme(&self.entries[..count], width, theme);
        self.entries.drain(..count);
        self.bump_revision();
        lines
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

/// Rows whose Ctrl+O state changes what is rendered: finished tool results
/// with captured detail or review history, and pending reviews (whose full
/// diff is collapsed by default).
fn has_expandable_details(entry: &TranscriptEntry) -> bool {
    match &entry.kind {
        EntryKind::ToolResult { .. } => entry.detail.is_some() || entry.review.is_some(),
        EntryKind::ToolCall { .. } => entry
            .review
            .as_ref()
            .is_some_and(|review| matches!(review.status, ToolReviewRowStatus::Pending)),
        _ => false,
    }
}

fn review_status_label(status: ToolReviewRowStatus) -> &'static str {
    match status {
        ToolReviewRowStatus::Pending => "pending",
        ToolReviewRowStatus::Submitted(ToolReviewDecision::Approve) => "approve submitted",
        ToolReviewRowStatus::Submitted(ToolReviewDecision::Reject) => "reject submitted",
        ToolReviewRowStatus::Submitted(ToolReviewDecision::ApproveForSession) => {
            "session approve submitted"
        }
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Approved) => "approved",
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Rejected) => "rejected",
        ToolReviewRowStatus::Resolved(ToolReviewResolution::Interrupted) => "interrupted",
    }
}

/// Diff lines shown in a pending review row before the rest is collapsed
/// behind Ctrl+O. Large enough to judge a typical edit, small enough that the
/// decision controls and the latest transcript context stay on screen.
const REVIEW_DIFF_PREVIEW_LINES: usize = 12;
/// Longest collapsed diff line; a single minified line would otherwise wrap
/// into hundreds of rows and push the controls out of the viewport.
const REVIEW_DIFF_LINE_MAX_CHARS: usize = 160;

fn review_diff_lines(diff: &str, show_all: bool) -> Vec<String> {
    let total = diff.lines().count();
    if show_all || total == 0 {
        return vec![diff.to_owned()];
    }
    let mut lines: Vec<String> = diff
        .lines()
        .take(REVIEW_DIFF_PREVIEW_LINES)
        .map(
            |line| match line.char_indices().nth(REVIEW_DIFF_LINE_MAX_CHARS) {
                Some((end, _)) => format!("{}…", &line[..end]),
                None => line.to_owned(),
            },
        )
        .collect();
    if total > REVIEW_DIFF_PREVIEW_LINES {
        lines.push(format!(
            "… {} more diff lines · Ctrl+O to show all",
            total - REVIEW_DIFF_PREVIEW_LINES
        ));
    }
    lines
}

fn review_detail(review: &ToolReviewRow, show_full_diff: bool) -> String {
    let mut lines = vec![format!("Review: {}", review_status_label(review.status))];
    match &review.payload {
        ToolReviewPayload::LocalEdit { preview } => {
            lines.push(format!("Path: {}", preview.path));
            lines.push(format!("Operation: {}", preview.operation));
            lines.push(String::from("Diff:"));
            lines.extend(review_diff_lines(&preview.diff_summary, show_full_diff));
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
        // Session approval only makes sense for commands: edits already
        // have a session-wide posture in the `accept-edits` approval mode.
        let offers_session_grant = matches!(review.payload, ToolReviewPayload::Command { .. });
        let options: &[(ToolReviewDecision, &str)] = if offers_session_grant {
            &[
                (ToolReviewDecision::Approve, "Approve once"),
                (
                    ToolReviewDecision::ApproveForSession,
                    "Approve for this session",
                ),
                (ToolReviewDecision::Reject, "Reject"),
            ]
        } else {
            &[
                (ToolReviewDecision::Approve, "Approve"),
                (ToolReviewDecision::Reject, "Reject"),
            ]
        };
        for (decision, label) in options {
            let marker = if review.selected == *decision {
                "›"
            } else {
                " "
            };
            lines.push(format!("{marker} {label}"));
        }
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
                    sections.push(review_detail(review, entry.expanded));
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
                sections.push(review_detail(review, true));
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
        | EntryKind::Status
        | EntryKind::HarnessOutcome { .. }
        | EntryKind::Compaction
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
    let detail = match detail.split_once('\n') {
        Some((first_line, remaining)) if summary.ends_with(first_line) => remaining,
        None if summary.ends_with(detail) => return None,
        _ => detail,
    };
    if detail.is_empty() {
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

/// Rendered transcript, kept per entry so a change re-renders only the
/// entries whose stamp changed. Streaming one assistant reply therefore
/// re-parses that reply's Markdown alone, not the whole history.
#[derive(Debug, Default)]
pub struct TranscriptRenderCache {
    width: u16,
    revision: u64,
    entries_len: usize,
    theme: Theme,
    entries: Vec<CachedEntry>,
    total_lines: usize,
}

#[derive(Debug)]
struct CachedEntry {
    stamp: u64,
    /// First row of this entry's leading gap in the flattened transcript.
    start: usize,
    gap: usize,
    lines: Vec<Line<'static>>,
}

impl CachedEntry {
    fn end(&self) -> usize {
        self.start + self.gap + self.lines.len()
    }
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
        let entries = transcript.entries();
        if self.width == width
            && self.revision == transcript.revision()
            && self.entries_len == entries.len()
        {
            return;
        }
        if self.width != width {
            self.entries.clear();
        }
        self.width = width;
        self.revision = transcript.revision();
        self.entries_len = entries.len();

        // Transcripts only grow at the end, lose a prefix (`drain_prefix_lines`)
        // or a suffix, and change entries in place. Aligning on the first
        // entry's stamp covers all of them; a stamp mismatch re-renders.
        let dropped = entries
            .first()
            .and_then(|first| {
                self.entries
                    .iter()
                    .position(|cached| cached.stamp == first.stamp)
            })
            .unwrap_or(0);
        self.entries.drain(..dropped);
        self.entries.truncate(entries.len());

        let mut previous_kind: Option<&EntryKind> = None;
        let mut total = 0;
        for (index, entry) in entries.iter().enumerate() {
            let gap = match previous_kind {
                _ if total == 0 => 0,
                Some(previous) if is_tool_entry(previous) && is_tool_entry(&entry.kind) => {
                    usize::from(self.theme.spacing.tool_gap)
                }
                _ => 1,
            };
            match self.entries.get_mut(index) {
                Some(cached) if cached.stamp == entry.stamp => {}
                Some(cached) => {
                    cached.stamp = entry.stamp;
                    cached.lines = render_entry_lines(entry, width, &self.theme);
                }
                None => self.entries.push(CachedEntry {
                    stamp: entry.stamp,
                    start: 0,
                    gap: 0,
                    lines: render_entry_lines(entry, width, &self.theme),
                }),
            }
            if let Some(cached) = self.entries.get_mut(index) {
                cached.start = total;
                cached.gap = gap;
                total = cached.end();
            }
            previous_kind = Some(&entry.kind);
        }
        self.total_lines = total;
    }

    pub fn max_scroll_start(&mut self, transcript: &Transcript, width: u16, height: u16) -> usize {
        self.ensure(transcript, width);
        self.total_lines.saturating_sub(height as usize)
    }

    /// Clone rows `start..start + count` of the flattened transcript.
    fn visible_lines(&self, start: usize, count: usize) -> Vec<Line<'static>> {
        let end = start.saturating_add(count).min(self.total_lines);
        let mut visible = Vec::with_capacity(end.saturating_sub(start));
        let first = self.entries.partition_point(|cached| cached.end() <= start);
        for cached in self.entries.iter().skip(first) {
            if cached.start >= end {
                break;
            }
            for row in cached.start.max(start)..cached.end().min(end) {
                let offset = row - cached.start;
                if offset < cached.gap {
                    visible.push(Line::raw(""));
                } else if let Some(line) = cached.lines.get(offset - cached.gap) {
                    visible.push(line.clone());
                }
            }
        }
        visible
    }

    pub fn render(
        &mut self,
        area: ratatui::layout::Rect,
        buf: &mut ratatui::buffer::Buffer,
        transcript: &Transcript,
        scroll_offset: usize,
        is_streaming: bool,
        alignment: TranscriptAlignment,
    ) {
        self.ensure(transcript, area.width);
        let height = area.height as usize;
        let start = scroll_offset.min(self.total_lines.saturating_sub(height));
        let visible = self.visible_lines(start, height);
        paint_lines(area, buf, visible, is_streaming, alignment, &self.theme);
    }
}

/// Where a transcript shorter than its viewport sits inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptAlignment {
    /// Pin the content to the composer. Right for a fresh or resumed
    /// session, where nothing of ours sits above the viewport and the first
    /// exchange reads best next to the input.
    Bottom,
    /// Start at the viewport's first row. Required once earlier turns were
    /// moved into terminal scrollback with `insert_before`: that history ends
    /// right above the viewport, so bottom padding would open a blank gap
    /// between the previous reply and the new turn.
    Top,
}

pub fn render(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    transcript: &Transcript,
    cache: &mut TranscriptRenderCache,
    scroll_offset: usize,
    is_streaming: bool,
    alignment: TranscriptAlignment,
) {
    cache.render(
        area,
        buf,
        transcript,
        scroll_offset,
        is_streaming,
        alignment,
    );
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
    let height = area.height as usize;
    let start = scroll_offset.min(lines.len().saturating_sub(height));
    let visible = lines.into_iter().skip(start).take(height).collect();
    paint_lines(
        area,
        buf,
        visible,
        is_streaming,
        TranscriptAlignment::Bottom,
        &Theme::default(),
    );
}

fn paint_lines(
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
    mut visible: Vec<Line<'static>>,
    is_streaming: bool,
    alignment: TranscriptAlignment,
    theme: &Theme,
) {
    let top_padding = match alignment {
        TranscriptAlignment::Bottom => {
            bottom_aligned_top_padding(visible.len(), area.height as usize)
        }
        TranscriptAlignment::Top => 0,
    };
    if top_padding > 0 {
        let mut padded = Vec::with_capacity(top_padding + visible.len());
        padded.extend(std::iter::repeat_with(|| Line::raw("")).take(top_padding));
        padded.append(&mut visible);
        visible = padded;
    }

    let foreground = theme.text().fg.unwrap_or(ratatui::style::Color::Reset);
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

/// Assistant prose: Markdown rendered after parsing, behind the `• ` gutter
/// (hanging two-column indent on every later row; blank rows stay empty).
fn render_assistant_markdown_lines(content: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    const GUTTER_WIDTH: usize = 2;
    let body_width = usize::from(width).saturating_sub(GUTTER_WIDTH).max(1);
    let mut body = crate::markdown::render(content, body_width, theme);
    if body.is_empty() {
        body.push(Line::raw(""));
    }
    body.into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index > 0 && line.spans.is_empty() {
                return line;
            }
            let gutter = if index == 0 {
                Span::styled("• ", theme.accent())
            } else {
                Span::raw("  ")
            };
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(gutter);
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

fn render_entry_lines(entry: &TranscriptEntry, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    if is_tool_entry(&entry.kind) {
        return render_tool_entry_lines(entry, width, theme);
    }
    let display_text = entry_display_text(entry);
    let (prefix, continuation, content, content_style, prefix_width) = match &entry.kind {
        EntryKind::UserMessage => return render_user_message_lines(&display_text, width, theme),
        EntryKind::AssistantText => {
            return render_assistant_markdown_lines(&display_text, width, theme);
        }
        EntryKind::Status => (
            Span::styled("i ", theme.accent().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            display_text,
            theme.muted(),
            2,
        ),
        EntryKind::ToolCall { .. } | EntryKind::ToolResult { .. } => unreachable!(),
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
        EntryKind::Compaction => (
            Span::styled("⟲ ", theme.accent().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            display_text,
            theme.muted(),
            2,
        ),
        EntryKind::Error => (
            Span::styled("✗ ", theme.error().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            display_text,
            theme.error(),
            2,
        ),
    };

    let available_width = usize::from(width).saturating_sub(prefix_width).max(1);
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
    result
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
    let surface = theme.user_message();
    let background = surface.bg.unwrap_or(Color::Reset);
    let background_style = Style::new().bg(background);
    // The prefix is accent on the user surface, not user_message() itself.
    let prefix_style = theme.accent().bg(background).add_modifier(Modifier::BOLD);
    let content_style = surface;
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

const TOOL_RULE_WIDTH: usize = 1;

fn tool_horizontal_padding(width: usize, theme: &Theme) -> usize {
    usize::from(theme.spacing.tool_horizontal_padding)
        .min(width.saturating_sub(TOOL_RULE_WIDTH) / 2)
}

fn render_tool_entry_lines(
    entry: &TranscriptEntry,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let state = tool_state(entry);
    let (marker, title) = match &entry.kind {
        EntryKind::ToolCall { name, .. } => (
            Span::styled("⚙ ", theme.warning().add_modifier(Modifier::BOLD)),
            std::borrow::Cow::Borrowed(name.as_str()),
        ),
        EntryKind::ToolResult {
            name,
            outcome_kind: Some(kind),
            ..
        } => {
            let (label, style) = harness_outcome_style(*kind, theme);
            (
                Span::styled("! ", style),
                std::borrow::Cow::Owned(format!("{label} {name}")),
            )
        }
        EntryKind::ToolResult { name, is_error, .. } => {
            let marker = if *is_error {
                Span::styled("✗ ", theme.error().add_modifier(Modifier::BOLD))
            } else {
                Span::styled("✓ ", theme.success())
            };
            (marker, std::borrow::Cow::Borrowed(name.as_str()))
        }
        _ => unreachable!(),
    };
    let display_text = entry_display_text(entry);
    let content = name_and_detail(&title, &display_text);
    let width_usize = usize::from(width.max(1));
    let available_width = width_usize
        .saturating_sub(TOOL_RULE_WIDTH)
        .saturating_sub(tool_horizontal_padding(width_usize, theme).saturating_mul(2))
        .saturating_sub(marker.width())
        .max(1);
    let wrapped: Vec<_> = content
        .lines()
        .chain(content.is_empty().then_some(""))
        .enumerate()
        .flat_map(|(source_index, source_line)| {
            wrap_text(source_line, available_width)
                .into_iter()
                .map(move |line| (source_index, line))
        })
        .collect();
    let mut result = Vec::with_capacity(wrapped.len() + entry.stream_tail.lines().count());
    let mut title_remaining = title.len();
    for (index, (source_index, mut line)) in wrapped.into_iter().enumerate() {
        let mut spans = vec![if index == 0 {
            marker.clone()
        } else {
            Span::raw("  ")
        }];
        let title_len = title_remaining.min(line.len());
        title_remaining -= title_len;
        let default = if source_index == 0 && !entry.expanded {
            theme.muted()
        } else {
            theme.tool_output(state)
        };
        let style = transcript_line_style(entry, &line, default, theme);
        if title_len == 0 {
            spans.push(Span::styled(line, style));
        } else if title_len == line.len() {
            spans.push(Span::styled(line, theme.tool_title()));
        } else {
            let detail = line.split_off(title_len);
            spans.push(Span::styled(line, theme.tool_title()));
            spans.push(Span::styled(detail, style));
        }
        result.push(Line::from(spans));
    }
    if !entry.stream_tail.is_empty() {
        for line in wrap_text(&entry.stream_tail, available_width) {
            result.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line, theme.tool_output(state)),
            ]));
        }
    }
    render_tool_surface(result, width, state, theme)
}

/// Only tiny viewports need clipping: normal rows already wrap inside the body.
fn clip_tool_line(line: &mut Line<'static>, width: usize) {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    if line.width() <= width {
        return;
    }
    let mut remaining = width;
    line.spans.retain_mut(|span| {
        let mut end = 0;
        for (offset, grapheme) in span.content.grapheme_indices(true) {
            let cells = grapheme.width();
            if cells > remaining {
                break;
            }
            remaining -= cells;
            end = offset + grapheme.len();
        }
        if end < span.content.len() {
            span.content.to_mut().truncate(end);
            remaining = 0;
        }
        !span.content.is_empty()
    });
}

fn render_tool_surface(
    lines: Vec<Line<'static>>,
    width: u16,
    state: crate::theme::ToolState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let surface = theme.tool_surface(state);
    let rule = Span::styled("┃", theme.tool_rule(state));
    let horizontal_padding = tool_horizontal_padding(width, theme);
    let inner_width = width
        .saturating_sub(TOOL_RULE_WIDTH)
        .saturating_sub(horizontal_padding.saturating_mul(2));
    let background_style = Style::new().bg(surface.bg.unwrap_or(Color::Reset));
    let vertical_padding = usize::from(theme.spacing.tool_vertical_padding);
    let padding_row = || {
        Line::from(vec![
            rule.clone(),
            Span::styled(
                " ".repeat(width.saturating_sub(TOOL_RULE_WIDTH)),
                background_style,
            ),
        ])
    };
    let mut rendered = Vec::with_capacity(lines.len() + vertical_padding.saturating_mul(2));
    rendered.extend(std::iter::repeat_with(padding_row).take(vertical_padding));
    for mut line in lines {
        clip_tool_line(&mut line, inner_width);
        let line_width = line.width().min(inner_width);
        for span in &mut line.spans {
            span.style.bg = surface.bg;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 3);
        spans.push(rule.clone());
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
    rendered.extend(std::iter::repeat_with(padding_row).take(vertical_padding));
    rendered
}

fn tool_state(entry: &TranscriptEntry) -> crate::theme::ToolState {
    match entry.kind {
        EntryKind::ToolResult {
            outcome_kind: Some(_),
            ..
        } => crate::theme::ToolState::Harness,
        EntryKind::ToolResult { is_error: true, .. } => crate::theme::ToolState::Error,
        EntryKind::ToolResult { .. } => crate::theme::ToolState::Success,
        _ => crate::theme::ToolState::Pending,
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
        return theme
            .diff(crate::theme::DiffKind::Context)
            .add_modifier(Modifier::BOLD);
    }
    if line.starts_with('+') {
        return theme.diff(crate::theme::DiffKind::Added);
    }
    if line.starts_with('-') {
        return theme.diff(crate::theme::DiffKind::Removed);
    }
    if line.starts_with("@@") || line.starts_with('›') {
        return theme
            .diff(crate::theme::DiffKind::Hunk)
            .add_modifier(Modifier::BOLD);
    }
    // Unselected option rows are dimmed; the selected one is caught by the
    // `›` marker above.
    if line.starts_with("  Approve") || line.starts_with("  Reject") || line.starts_with("↑/↓")
    {
        return theme.dim();
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
    (kind.label(), theme.harness().add_modifier(Modifier::BOLD))
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
        EntryKind, HarnessOutcomeKind, STREAM_TAIL_MAX_LINES, ToolReviewRowStatus, Transcript,
        TranscriptAlignment, TranscriptEntry, TranscriptRenderCache, bottom_aligned_top_padding,
        char_boundary_at_or_before, entry_display_text, harness_outcome_style, render_lines,
        render_lines_with_theme, render_uncached, wrap_text,
    };
    use crate::theme::{DiffKind, Theme, ThemeDocument, ToolState};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use yach_proto::{
        CommandReviewSummary, LocalEditPreviewSummary, LocalEditReviewState,
        PromptAttemptResetError, ToolReviewDecision, ToolReviewHistory, ToolReviewPayload,
        ToolReviewResolution,
    };
    /// Column zero belongs to the rule, not the painted body.
    fn panel_rows<'a>(
        lines: &'a [ratatui::text::Line<'static>],
    ) -> Vec<&'a ratatui::text::Line<'static>> {
        lines
            .iter()
            .filter(|line| line.spans.first().is_some_and(|span| span.content == "┃"))
            .collect()
    }

    #[test]
    fn tool_panel_draws_one_rule_on_every_row_including_padding() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(Some("call-1"), "bash", Some("cargo test"));
        transcript.append_tool_call_output("call-1", "line one\nline two\n");
        let lines = render_lines_with_theme(transcript.entries(), 40, &Theme::dark());
        assert_eq!(lines.len(), 5);
        assert_eq!(
            panel_rows(&lines).len(),
            5,
            "every panel row must carry a rule, including padding"
        );
        let text: Vec<_> = lines
            .iter()
            .map(|line| {
                assert_eq!(line.width(), 40);
                assert_eq!(line.spans[0].content, "┃");
                let text = line.to_string();
                assert!(!text.contains("│ "));
                text.trim_end().to_owned()
            })
            .collect();
        assert_eq!(
            text,
            [
                "┃",
                "┃ ⚙ bash cargo test",
                "┃   line one",
                "┃   line two",
                "┃"
            ]
        );
    }

    #[test]
    fn tool_rule_color_and_marker_follow_every_state() {
        let theme = Theme::dark();
        for (is_error, outcome_kind, marker, state) in [
            (false, None, "✓ read completed", ToolState::Success),
            (true, None, "✗ read completed", ToolState::Error),
            (
                false,
                Some(HarnessOutcomeKind::Denied),
                "! denied read completed",
                ToolState::Harness,
            ),
            (
                true,
                Some(HarnessOutcomeKind::Denied),
                "! denied read completed",
                ToolState::Harness,
            ),
        ] {
            let mut transcript = Transcript::new();
            transcript.append_tool_result_with_kind(
                None,
                "read",
                "completed",
                is_error,
                outcome_kind,
            );
            let lines = render_lines_with_theme(transcript.entries(), 40, &theme);
            assert_eq!(lines.len(), 3);
            assert_eq!(panel_rows(&lines).len(), 3);
            assert_eq!(lines[1].to_string().trim_end(), format!("┃ {marker}"));
            for line in &lines {
                assert_eq!(line.width(), 40);
                assert_eq!(line.spans[0].style, theme.tool_rule(state));
                assert!(
                    line.spans[1..]
                        .iter()
                        .all(|span| span.style.bg == theme.tool_surface(state).bg)
                );
            }
        }
        let mut pending = Transcript::new();
        pending.append_tool_call(None, "read", Some("src/lib.rs"));
        let lines = render_lines_with_theme(pending.entries(), 40, &theme);
        assert_eq!(lines[1].to_string().trim_end(), "┃ ⚙ read src/lib.rs");
        assert!(
            lines
                .iter()
                .all(|line| line.spans[0].style == theme.tool_rule(ToolState::Pending))
        );
    }

    #[test]
    fn tool_header_summary_and_body_have_distinct_roles() {
        let theme = Theme::dark();
        let mut transcript = Transcript::new();
        transcript.append_tool_result_record(None, "bash", "summary", "body", false, None, None);
        let lines = render_lines_with_theme(transcript.entries(), 40, &theme);
        let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
        for (text, style) in [
            ("bash", theme.tool_title()),
            (" summary", theme.muted()),
            ("body", theme.tool_output(ToolState::Success)),
        ] {
            let span = spans.iter().find(|span| span.content == text);
            assert!(span.is_some(), "missing role span {text:?}: {spans:?}");
            let Some(span) = span else { return };
            assert_eq!(span.style.fg, style.fg, "{text}");
            assert_eq!(span.style.add_modifier, style.add_modifier, "{text}");
        }
    }

    #[test]
    fn tool_body_role_follows_crlf_summary_and_wrapped_lines() {
        let theme = Theme::dark();
        for newline in ["\n", "\r\n"] {
            for width in [12, 40] {
                let mut transcript = Transcript::new();
                transcript.append_tool_result(
                    None,
                    "bash",
                    &format!("summary{newline}body{newline}{newline}tail"),
                    false,
                );
                let lines = render_lines_with_theme(transcript.entries(), width, &theme);
                for text in ["body", "tail"] {
                    let span = lines
                        .iter()
                        .flat_map(|line| &line.spans)
                        .find(|span| span.content == text);
                    assert!(
                        span.is_some(),
                        "missing {text:?} at width {width} with {newline:?}"
                    );
                    let Some(span) = span else { return };
                    assert_eq!(
                        span.style,
                        theme.tool_output(ToolState::Success),
                        "{text:?} at width {width} with {newline:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn tool_wrap_reserves_exactly_one_rule_column() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(
            Some("call-1"),
            "read_text_file",
            Some("abcdefghijklmnopqrstuv"),
        );
        transcript.append_tool_call_output("call-1", "0123456789abcdefghijkl");
        let lines = render_lines_with_theme(transcript.entries(), 20, &Theme::dark());
        assert_eq!(lines.len(), 7);
        assert_eq!(panel_rows(&lines).len(), 7);
        for line in &lines {
            assert_eq!(line.width(), 20);
        }
        let text: Vec<_> = lines
            .iter()
            .map(|line| line.to_string().trim_end().to_owned())
            .collect();
        assert_eq!(
            text,
            [
                "┃",
                "┃ ⚙ read_text_file ",
                "┃   abcdefghijklmno",
                "┃   pqrstuv",
                "┃   0123456789abcde",
                "┃   fghijkl",
                "┃"
            ]
            .map(|line| line.trim_end().to_owned())
        );
    }

    #[test]
    fn tool_wrap_keeps_full_unicode_text_and_display_width() {
        let mut transcript = Transcript::new();
        transcript.append_tool_call(None, "read", Some("界界界abc界界"));
        let lines = render_lines_with_theme(transcript.entries(), 12, &Theme::dark());
        assert_eq!(lines.len(), 5);
        for line in &lines {
            assert_eq!(line.width(), 12);
            assert_eq!(line.spans[0].content, "┃");
            assert!(!line.to_string().contains("│ "));
        }
        assert_eq!(
            lines
                .iter()
                .map(|line| line.to_string().trim_end().to_owned())
                .collect::<Vec<_>>(),
            ["┃", "┃ ⚙ read 界", "┃   界界abc", "┃   界界", "┃"]
        );
    }

    #[test]
    fn narrow_tool_rows_fit_and_never_disappear() {
        use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};
        for width in 0..10 {
            let mut transcript = Transcript::new();
            transcript.append_tool_result(
                None,
                "read_text_file",
                "completed: 界 wide output",
                false,
            );
            let lines = render_lines_with_theme(transcript.entries(), width, &Theme::dark());
            let width = width.max(1);
            assert!(!lines.is_empty());
            let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
            for line in &lines {
                assert_eq!(line.spans[0].content, "┃");
                assert_eq!(line.width(), usize::from(width), "width {width}: {line:?}");
            }
            let mut terminal = match Terminal::new(TestBackend::new(width, height)) {
                Ok(terminal) => terminal,
                Err(infallible) => match infallible {},
            };
            assert!(
                terminal
                    .draw(|frame| frame.render_widget(Paragraph::new(lines), frame.area()))
                    .is_ok()
            );
            for row in 0..height {
                assert_eq!(terminal.backend().buffer()[(0, row)].symbol(), "┃");
            }
        }
    }

    #[test]
    fn monochrome_user_tool_and_review_cells_have_no_colors() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut transcript = theme_migration_fixture();
        transcript.append_tool_result_with_kind(
            None,
            "read",
            "blocked",
            true,
            Some(HarnessOutcomeKind::Denied),
        );
        let lines = render_lines_with_theme(transcript.entries(), 80, &Theme::monochrome());
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let mut terminal = match Terminal::new(TestBackend::new(80, height)) {
            Ok(terminal) => terminal,
            Err(infallible) => match infallible {},
        };
        let mut cache = TranscriptRenderCache::with_theme(Theme::monochrome());
        assert!(
            terminal
                .draw(|frame| cache.render(
                    frame.area(),
                    frame.buffer_mut(),
                    &transcript,
                    0,
                    false,
                    TranscriptAlignment::Top
                ))
                .is_ok()
        );
        let buffer = terminal.backend().buffer();
        let mut symbols = String::new();
        for y in 0..height {
            for x in 0..80 {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.fg, ratatui::style::Color::Reset, "({x},{y})");
                assert_eq!(cell.bg, ratatui::style::Color::Reset, "({x},{y})");
                symbols.push_str(cell.symbol());
            }
        }
        for marker in ["›", "⚙", "✓", "✗", "!", "+new line", "-old line", "┃"] {
            assert!(symbols.contains(marker), "missing {marker}");
        }
    }

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
    }

    #[test]
    fn compaction_checkpoints_render_their_summary() {
        // The backend projects a checkpoint as a `system` message; before
        // this entry kind existed the UI dropped it, so a resumed session
        // showed no sign that compaction had happened.
        let mut transcript = Transcript::new();
        transcript.append_compaction("— compacted: 120K → ~40K tokens —\nkept the migration plan");
        let entry = &transcript.entries()[0];
        assert!(matches!(entry.kind, EntryKind::Compaction));

        let rendered = render_lines(transcript.entries(), 80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("kept the migration plan"),
            "compaction summary must be visible, got: {rendered}"
        );
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

        for index in 0..200 {
            transcript.append_tool_call_output("call-1", &format!("line-{index}\n"));
        }
        let expected_start = 200 - STREAM_TAIL_MAX_LINES;
        let expected: String = (expected_start..200).fold(String::new(), |mut acc, index| {
            use std::fmt::Write as _;
            let _ = writeln!(acc, "line-{index}");
            acc
        });
        assert_eq!(transcript.entries()[0].stream_tail, expected);
        assert_eq!(
            transcript.entries()[0].stream_tail.lines().count(),
            STREAM_TAIL_MAX_LINES
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
        assert_eq!(tail.lines().count(), STREAM_TAIL_MAX_LINES);
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
                    review_origin: None,
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
                    review_origin: None,
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
        for line in &lines {
            assert_eq!(
                line.spans[0].style,
                Theme::dark().tool_rule(ToolState::Pending)
            );
        }
        assert!(lines.iter().all(|line| {
            line.spans[1..]
                .iter()
                .all(|span| span.style.bg == Theme::dark().tool_surface(ToolState::Pending).bg)
        }));
    }

    #[test]
    fn command_reviews_offer_a_session_grant_and_edit_reviews_do_not() {
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
                    workdir: None,
                    timeout_ms: 30_000,
                    review_origin: None,
                },
            },
        );
        let rendered = render_lines(transcript.entries(), 100)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            rendered.contains("Approve for this session"),
            "a command review offers a session grant, got: {rendered}"
        );
        assert!(transcript.pending_review_is_command());

        // Edits already have `accept-edits` as their session-wide posture,
        // so offering a per-edit grant would duplicate that authority.
        let mut edits = Transcript::new();
        edits.append_tool_call(Some("request-2"), "edit_text_file", Some("src/lib.rs"));
        edits.begin_tool_review(
            "request-2",
            "edit_text_file",
            ToolReviewPayload::LocalEdit {
                preview: LocalEditPreviewSummary {
                    preview_id: String::from("preview-1"),
                    transaction_id: String::from("transaction-1"),
                    permission_decision_id: String::from("permission-2"),
                    path: String::from("src/lib.rs"),
                    operation: String::from("replace"),
                    review_state: LocalEditReviewState::NeedsUserApproval,
                    diff_summary: String::from("+ added"),
                    diff_summary_truncated: false,
                    review_origin: None,
                },
            },
        );
        let edit_rendered = render_lines(edits.entries(), 100)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            !edit_rendered.contains("for this session"),
            "an edit review must not offer a session grant, got: {edit_rendered}"
        );
        assert!(!edits.pending_review_is_command());
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
                    review_origin: None,
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
        cache.render(
            area,
            &mut cached,
            &transcript,
            1,
            false,
            TranscriptAlignment::Bottom,
        );

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

    /// One persistent cache must paint exactly what a full render paints
    /// after every kind of transcript change: in-place edits, suffix
    /// truncation, prefix drain, width change, and clear. A mutation that
    /// forgets `touch()` leaves stale lines and fails here.
    #[test]
    fn persistent_render_cache_matches_full_render_after_every_mutation() {
        type Step = fn(&mut Transcript);
        fn begin_review(t: &mut Transcript, id: &str) {
            t.begin_tool_review(
                id,
                "bash",
                ToolReviewPayload::Command {
                    command: CommandReviewSummary {
                        review_id: format!("review-{id}"),
                        permission_decision_id: format!("permission-{id}"),
                        command: String::from("cargo test"),
                        workdir: None,
                        timeout_ms: 1_000,
                        review_origin: None,
                    },
                },
            );
        }
        let steps: [(&str, Step); 21] = [
            ("user", |t| {
                t.append_user_message("question one wraps across the narrow width");
            }),
            ("delta", |t| t.append_delta("# Heading\n\nsome **bold**")),
            ("delta grows", |t| {
                t.append_delta(" text and\n\n- item\n- item two");
            }),
            ("tool call", |t| {
                t.append_tool_call(Some("c1"), "bash", Some("cargo test"));
            }),
            ("tool output", |t| {
                t.append_tool_call_output("c1", "line one\nline two\n");
            }),
            ("review", |t| begin_review(t, "c1")),
            ("toggle pending review", Transcript::toggle_tool_details),
            ("select", |t| {
                t.select_pending_review(ToolReviewDecision::Reject);
            }),
            ("submit", |t| {
                let _ = t.submit_pending_review();
            }),
            ("resolve", |t| {
                let _ = t.resolve_tool_review("c1", ToolReviewResolution::Rejected);
            }),
            ("finish", |t| {
                let _ = t.finish_tool_call(Some("c1"), "bash", "bash", "exit 0", false);
            }),
            ("toggle open", Transcript::toggle_tool_details),
            ("toggle closed", Transcript::toggle_tool_details),
            ("tool result", |t| {
                t.append_tool_result(Some("c2"), "Read", "ok", false);
            }),
            ("status", |t| t.append_status("status line")),
            ("delta again", |t| {
                t.append_delta("tail reply that is long enough to wrap");
            }),
            ("truncate suffix", |t| {
                let _ = t.truncate_assistant_suffix_bytes(5);
            }),
            ("drain prefix", |t| {
                let _ = t.drain_prefix_lines(2, 30, &Theme::default());
            }),
            ("error", |t| t.append_error("boom")),
            ("second review", |t| begin_review(t, "c3")),
            ("interrupt review", Transcript::interrupt_pending_reviews),
        ];

        let mut transcript = Transcript::new();
        // One cache per width, each kept across every step, so entries are
        // actually reused. A shared cache would be cleared by each width
        // switch and never exercise reuse. `resized` alternates widths to
        // cover the width-change path.
        let mut caches = [
            (30, TranscriptRenderCache::new()),
            (18, TranscriptRenderCache::new()),
        ];
        let mut resized = TranscriptRenderCache::new();
        let mut assert_matches = |transcript: &Transcript, label: &str| {
            for (width, cache) in &mut caches {
                let width = *width;
                let total = render_lines(transcript.entries(), width).len();
                for scroll in [0, 1, total.saturating_sub(6), usize::MAX] {
                    let area = Rect::new(0, 0, width, 6);
                    let mut expected = Buffer::empty(area);
                    render_uncached(area, &mut expected, transcript.entries(), scroll, false);
                    let mut actual = Buffer::empty(area);
                    let bottom = TranscriptAlignment::Bottom;
                    cache.render(area, &mut actual, transcript, scroll, false, bottom);
                    assert_eq!(actual, expected, "{label}: width {width}, scroll {scroll}");
                    let mut after_resize = Buffer::empty(area);
                    resized.render(area, &mut after_resize, transcript, scroll, false, bottom);
                    assert_eq!(after_resize, expected, "{label}: resized to {width}");
                }
            }
        };
        for (label, step) in steps {
            step(&mut transcript);
            assert_matches(&transcript, label);
        }
        transcript.clear();
        assert_matches(&transcript, "clear");
    }

    #[test]
    fn assistant_markdown_renders_without_delimiters_and_keeps_raw_content() {
        let raw = "# Title\n\nsome **bold** and `code`\n\n- one\n- two";
        let mut transcript = Transcript::new();
        transcript.append_delta(raw);

        let lines = render_lines(transcript.entries(), 40);
        let text: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(
            text,
            [
                "• Title",
                "",
                "  some bold and code",
                "",
                "  - one",
                "  - two"
            ]
        );
        let bold = lines[2].spans.iter().find(|span| span.content == "bold");
        assert!(bold.is_some_and(|span| {
            span.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        }));
        assert_eq!(transcript.entries()[0].content, raw);
    }

    #[test]
    fn assistant_markdown_wrapped_lines_keep_hanging_gutter() {
        let mut transcript = Transcript::new();
        transcript.append_delta("alpha beta gamma delta");
        let text: Vec<String> = render_lines(transcript.entries(), 12)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(text, ["• alpha beta", "  gamma", "  delta"]);
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
    fn collapsed_edit_result_shows_snapshot_and_changed_lines() {
        let mut transcript = Transcript::new();
        let output = concat!(
            "[applied]\n",
            "[src/lib.rs#ABCDEF0123456789]\n",
            "@@ -1 +1 @@\n",
            "-old\n",
            "+new"
        );
        transcript.append_tool_result_record(
            Some("edit-1"),
            "edit_text_file src/lib.rs",
            "completed: 5 lines, 64 bytes",
            output,
            false,
            None,
            None,
        );

        let rendered = entry_display_text(&transcript.entries()[0]);
        assert!(rendered.contains("[src/lib.rs#ABCDEF0123456789]"));
        assert!(rendered.contains("-old"));
        assert!(rendered.contains("+new"));
    }

    #[test]
    fn collapsed_failed_tool_rows_do_not_repeat_the_summary_excerpt() {
        let mut transcript = Transcript::new();
        let error = "[hashline error: The path does not exist.]";
        transcript.append_tool_result_record(
            Some("call-1"),
            "read_text_file package.json",
            &format!("failed: 1 line, 43 bytes; {error}"),
            error,
            true,
            None,
            None,
        );
        assert_eq!(
            entry_display_text(&transcript.entries()[0]),
            format!("failed: 1 line, 43 bytes; {error}")
        );

        transcript.append_tool_result_record(
            Some("call-2"),
            "read_text_file pyproject.toml",
            "failed: 2 lines, 50 bytes; first failure",
            "first failure\nrecovery guidance",
            true,
            None,
            None,
        );
        let multi_line = entry_display_text(&transcript.entries()[1]);
        assert_eq!(multi_line.matches("first failure").count(), 1);
        assert!(multi_line.contains("recovery guidance"));
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
                .all(|span| span.style.bg == theme.user_message().bg)
        );
        assert_eq!(lines[1].spans[1].content, "› ");
        assert_eq!(lines[1].spans[2].style.fg, theme.user_message().fg);
        assert!(lines[3].spans.is_empty());
        assert_eq!(lines[4].spans[0].content, "• ");
        assert_eq!(lines[4].spans[1].style.fg, theme.text().fg);
        assert!(lines[5].spans.is_empty());
        assert_eq!(lines[6].spans[0].style, theme.tool_rule(ToolState::Pending));
        assert!(
            lines[6].spans[1..]
                .iter()
                .all(|span| span.style.bg == theme.tool_surface(ToolState::Pending).bg)
        );
    }

    #[test]
    fn custom_theme_colors_user_and_tool_surfaces() {
        let parsed = ThemeDocument::parse(
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
        let Ok(document) = parsed else {
            return;
        };
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else {
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
                .all(|span| span.style.bg == theme.user_message().bg)
        );
        assert!(
            user_line
                .spans
                .iter()
                .any(|span| span.style.fg == theme.user_message().fg)
        );
        assert_eq!(
            tool_line.spans[0].style,
            theme.tool_rule(ToolState::Pending)
        );
        assert!(
            tool_line.spans[1..]
                .iter()
                .all(|span| span.style.bg == theme.tool_surface(ToolState::Pending).bg)
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
        let background = Theme::dark().tool_surface(ToolState::Success).bg;
        assert_eq!(lines.len(), 11);
        assert!(lines[3].spans.is_empty());
        assert!(lines[7].spans.is_empty());
        for line in lines[4..=6].iter().chain(&lines[8..=10]) {
            assert_eq!(
                line.spans[0].style,
                Theme::dark().tool_rule(ToolState::Success)
            );
        }
        assert!(lines[4..=6].iter().all(|line| {
            line.spans[1..]
                .iter()
                .all(|span| span.style.bg == background)
        }));
        assert!(lines[8..=10].iter().all(|line| {
            line.spans[1..]
                .iter()
                .all(|span| span.style.bg == background)
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
        transcript.entries.push(TranscriptEntry::new(
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

    #[test]
    fn truncate_assistant_suffix_retracts_contiguous_text_only() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ask");
        transcript.append_delta("keep");
        transcript.append_tool_call(Some("call-1"), "Read", Some("src/lib.rs"));
        transcript.append_delta("fail");

        assert_eq!(transcript.truncate_assistant_suffix_bytes(4), Ok(()));
        assert_eq!(transcript.entries().len(), 3);
        assert_eq!(transcript.entries()[0].content, "ask");
        assert_eq!(transcript.entries()[1].content, "keep");
        assert!(matches!(
            transcript.entries()[2].kind,
            EntryKind::ToolCall { .. }
        ));
    }

    #[test]
    fn truncate_assistant_suffix_walks_contiguous_assistant_entries() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ask");
        transcript.append_delta("hello");
        transcript.append_delta(" world");
        assert_eq!(transcript.truncate_assistant_suffix_bytes(6), Ok(()));
        assert_eq!(transcript.entries().len(), 2);
        assert_eq!(transcript.entries()[1].content, "hello");
    }

    #[test]
    fn truncate_assistant_suffix_fails_closed_on_impossible_and_utf8() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ask");
        transcript.append_delta("café");
        let before = transcript.entries().to_vec();

        assert_eq!(
            transcript.truncate_assistant_suffix_bytes(1),
            Err(PromptAttemptResetError::InvalidUtf8Boundary)
        );
        assert_eq!(transcript.entries(), before.as_slice());

        assert_eq!(
            transcript.truncate_assistant_suffix_bytes(40),
            Err(PromptAttemptResetError::ImpossibleByteCount)
        );
        assert_eq!(transcript.entries(), before.as_slice());
    }

    #[test]
    fn truncate_assistant_suffix_stops_at_non_assistant_row() {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ask");
        transcript.append_delta("keep");
        transcript.append_tool_call(Some("call-1"), "Read", Some("src/lib.rs"));
        let before_len = transcript.entries().len();

        assert_eq!(
            transcript.truncate_assistant_suffix_bytes(4),
            Err(PromptAttemptResetError::ImpossibleByteCount)
        );
        assert_eq!(transcript.entries().len(), before_len);
        assert_eq!(transcript.entries()[1].content, "keep");
    }

    fn local_edit_review_payload() -> ToolReviewPayload {
        ToolReviewPayload::LocalEdit {
            preview: LocalEditPreviewSummary {
                preview_id: String::from("preview-1"),
                transaction_id: String::from("transaction-1"),
                permission_decision_id: String::from("permission-1"),
                path: String::from("src/lib.rs"),
                operation: String::from("replace"),
                review_state: LocalEditReviewState::NeedsUserApproval,
                diff_summary: String::from("@@ -1 +1 @@\n-old line\n+new line\n context"),
                diff_summary_truncated: false,
                review_origin: None,
            },
        }
    }

    fn theme_migration_fixture() -> Transcript {
        let mut transcript = Transcript::new();
        transcript.append_user_message("ship the theme");
        transcript.append_tool_call(Some("call-1"), "read_text_file", Some("src/lib.rs"));
        transcript.append_tool_result(Some("call-2"), "read_text_file", "completed: 1 line", false);
        transcript.append_tool_result(Some("call-3"), "bash", "failed: exit 1", true);
        transcript.append_tool_call(Some("call-4"), "edit_text_file", Some("src/lib.rs"));
        transcript.begin_tool_review("call-4", "edit_text_file", local_edit_review_payload());
        transcript
    }

    fn draw_theme_migration_fixture(transcript: &Transcript) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;

        // Bottom alignment pads the viewport, so the height is the rendered
        // row count: a shorter window would clip the `+` diff row (F4).
        let lines = render_lines(transcript.entries(), 80);
        let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let area = Rect::new(0, 0, 80, height);
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(infallible) => match infallible {},
        };
        let drawn = terminal.draw(|frame| {
            render_uncached(area, frame.buffer_mut(), transcript.entries(), 0, false);
        });
        assert!(drawn.is_ok());
        terminal.backend().buffer().clone()
    }

    fn assert_dark_role_cells(buffer: &ratatui::buffer::Buffer, theme: &Theme) {
        use ratatui::style::Color;

        let pending = theme
            .tool_surface(ToolState::Pending)
            .bg
            .unwrap_or(Color::Reset);
        let success = theme
            .tool_surface(ToolState::Success)
            .bg
            .unwrap_or(Color::Reset);
        let error = theme
            .tool_surface(ToolState::Error)
            .bg
            .unwrap_or(Color::Reset);
        let user_bg = theme.user_message().bg.unwrap_or(Color::Reset);
        let mut saw_user = false;
        let mut saw_pending = false;
        let mut saw_success = false;
        let mut saw_error = false;
        let mut saw_diff_added = false;
        let mut saw_diff_removed = false;
        let added = theme.diff(DiffKind::Added).fg.unwrap_or(Color::Reset);
        let removed = theme.diff(DiffKind::Removed).fg.unwrap_or(Color::Reset);
        let area = buffer.area;
        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buffer[(x, y)];
                // Review option rows also use `›`. The user marker is the one
                // sitting on the user-message surface (F4).
                if cell.symbol() == "›" && cell.bg == user_bg {
                    saw_user = true;
                }
                if cell.symbol() == "⚙" {
                    assert_eq!(cell.fg, theme.warning().fg.unwrap_or(Color::Reset));
                    assert_eq!(cell.bg, pending);
                    saw_pending = true;
                }
                if cell.symbol() == "✓" {
                    assert_eq!(cell.fg, theme.success().fg.unwrap_or(Color::Reset));
                    assert_eq!(cell.bg, success, "success row must use the neutral panel");
                    saw_success = true;
                }
                if cell.symbol() == "✗" {
                    assert_eq!(cell.fg, theme.error().fg.unwrap_or(Color::Reset));
                    assert_eq!(cell.bg, error);
                    saw_error = true;
                }
                if cell.symbol() == "+" && cell.fg == added {
                    saw_diff_added = true;
                }
                if cell.symbol() == "-" && cell.fg == removed {
                    saw_diff_removed = true;
                }
            }
        }
        assert!(saw_user && saw_pending && saw_success && saw_error);
        assert!(saw_diff_added && saw_diff_removed);
    }

    fn dump_buffer(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| {
                        let cell = &buffer[(x, y)];
                        format!(
                            "{:?}|{:?}|{:?}|{:?}",
                            cell.symbol(),
                            cell.fg,
                            cell.bg,
                            cell.modifier
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn dark_transcript_buffer_pins_tool_treatment() {
        let transcript = theme_migration_fixture();
        let buffer = draw_theme_migration_fixture(&transcript);
        let theme = Theme::dark();
        assert_dark_role_cells(&buffer, &theme);

        let dumped = dump_buffer(&buffer);
        let pin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/dark-transcript-buffer.txt");
        if std::env::var_os("UPDATE_THEME_PIN").is_some() {
            let _ = std::fs::write(&pin_path, &dumped);
        }
        let pinned = std::fs::read_to_string(&pin_path).unwrap_or_default();
        assert_eq!(
            dumped, pinned,
            "dark transcript buffer changed; review visible deltas before updating the pin"
        );
    }
}
