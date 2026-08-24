use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use yach_proto::{ApprovalMode, ToolReviewDecision, ToolReviewPayload};

use crate::static_context::{StaticContextOmission, StaticContextSummary};
use crate::{
    EditPreviewId, EditTransactionId, PermissionDecisionId, PermissionDecisionSummary, ToolError,
    ToolPermissionState,
};

/// Native session identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

/// Native transcript entry identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(pub String);

/// Native turn/request identifier used to reject stale stream events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub String);

/// Native tool request identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolRequestId(pub String);

/// Native compaction checkpoint identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompactionCheckpointId(pub String);

/// Why a compaction checkpoint was produced.
/// Design: `docs/superpowers/specs/2026-07-20-context-compaction-design.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    Threshold,
    Manual,
    Overflow,
}

/// Why a tool result was masked from future provider context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskReason {
    ThresholdPrePass,
}

/// Redacted summary for tool arguments or results persisted in native logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPayloadSummary {
    pub summary: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
}

/// Provisional persisted native tool outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Completed,
    Failed,
    Denied,
    Cancelled,
    ValidationFailed,
}

/// Role for a native session entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    Tool,
    System,
}

/// Terminal state for an assistant stream in the native session log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// Provider-owned metadata stored as optional native session annotations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMetadata {
    pub provider: String,
    pub model: String,
    pub response_id: Option<String>,
    /// Provider-reported token usage summed across the turn's requests,
    /// when the provider stream carried it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::ProviderUsage>,
}

/// Provider-ready transcript message reconstructed from native entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptMessage {
    pub role: Role,
    pub text: String,
}

/// Low-cardinality metric attribute persisted with a native duration metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricAttribute {
    pub key: String,
    pub value: String,
}

/// Summarized low-frequency native duration metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurationMetric {
    pub name: String,
    pub duration_ms: u64,
    pub attributes: Vec<MetricAttribute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditTraceId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditTracePhase {
    ToolValidation,
    ArgumentNormalization,
    PermissionDecision,
    Preview,
    ReviewWait,
    Apply,
    Reject,
    ResultShaping,
    ProviderContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditTraceSource {
    ProviderTool,
    LocalUi,
    ExtensionTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditTraceOutcome {
    Completed,
    Failed,
    Denied,
    Rejected,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditTraceRecord {
    pub trace_id: EditTraceId,
    pub phase: EditTracePhase,
    pub source: EditTraceSource,
    pub tool_name: Option<String>,
    pub tool_request_id: Option<ToolRequestId>,
    pub provider_call_id: Option<String>,
    pub preview_id: Option<EditPreviewId>,
    pub permission_decision_id: Option<PermissionDecisionId>,
    pub transaction_id: Option<EditTransactionId>,
    pub outcome: EditTraceOutcome,
    pub duration_ms: u64,
    pub reason_label: Option<String>,
    pub attributes: Vec<MetricAttribute>,
}

#[cfg(test)]
impl EditTraceRecord {
    pub(crate) fn test_record(trace_id: EditTraceId, phase: EditTracePhase) -> Self {
        Self {
            trace_id,
            phase,
            source: EditTraceSource::ProviderTool,
            tool_name: Some(String::from("edit_text_file")),
            tool_request_id: Some(ToolRequestId(String::from("tool-request-1"))),
            provider_call_id: Some(String::from("call-edit-1")),
            preview_id: None,
            permission_decision_id: None,
            transaction_id: None,
            outcome: EditTraceOutcome::Completed,
            duration_ms: 1,
            reason_label: None,
            attributes: Vec::new(),
        }
    }
}

const TRACE_TOOL_NAME_MAX_BYTES: usize = 64;
const TRACE_PROVIDER_CALL_ID_MAX_BYTES: usize = 256;
const TRACE_REASON_LABEL_MAX_BYTES: usize = 64;
const TRACE_ATTRIBUTE_LIMIT: usize = 8;
const TRACE_ATTRIBUTE_KEY_MAX_BYTES: usize = 48;
const TRACE_ATTRIBUTE_VALUE_MAX_BYTES: usize = 128;

#[must_use]
pub fn bounded_edit_trace_record(mut record: EditTraceRecord) -> EditTraceRecord {
    record.tool_name = record
        .tool_name
        .map(|value| bounded_trace_string(&value, TRACE_TOOL_NAME_MAX_BYTES));
    record.provider_call_id = record
        .provider_call_id
        .map(|value| bounded_trace_string(&value, TRACE_PROVIDER_CALL_ID_MAX_BYTES));
    record.reason_label = record
        .reason_label
        .map(|value| bounded_trace_reason_label(&value));
    record.attributes = record
        .attributes
        .into_iter()
        .take(TRACE_ATTRIBUTE_LIMIT)
        .map(|attribute| MetricAttribute {
            key: bounded_trace_string(&attribute.key, TRACE_ATTRIBUTE_KEY_MAX_BYTES),
            value: bounded_trace_string(&attribute.value, TRACE_ATTRIBUTE_VALUE_MAX_BYTES),
        })
        .collect();
    record
}

fn bounded_trace_reason_label(value: &str) -> String {
    let bounded = bounded_trace_string(value, TRACE_REASON_LABEL_MAX_BYTES);
    if bounded
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        bounded
    } else {
        String::from("redacted_reason")
    }
}

fn bounded_trace_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }

    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_owned()
}

/// Redacted edit transaction summary persisted in native session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditEvidenceSummary {
    pub operation_count: usize,
    pub operations: Vec<EditOperationEvidence>,
    pub diff_summary: ToolPayloadSummary,
}

/// Redacted per-operation edit evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditOperationEvidence {
    ModifyTextFile {
        relative_path: String,
        before_sha256: String,
        after_sha256: String,
        before_bytes: usize,
        after_bytes: usize,
        hunk_count: usize,
        bytes_written: Option<usize>,
    },
    CreateTextFile {
        relative_path: String,
        after_sha256: String,
        after_bytes: usize,
        bytes_written: Option<usize>,
    },
}

/// Categorical edit transaction outcome for durable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditEvidenceOutcome {
    ApplyStarted,
    Completed,
    ValidationFailed,
    Failed,
}

/// Append-only native session event record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    EntryAppended {
        session_id: SessionId,
        entry_id: EntryId,
        parent_entry_id: Option<EntryId>,
        turn_id: TurnId,
        role: Role,
        text: String,
        provider: Option<ProviderMetadata>,
    },
    ToolRequestRecorded {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: ToolRequestId,
        tool_name: String,
        provider_call_id: Option<String>,
        validation: Result<(), ToolError>,
        permission: ToolPermissionState,
        argument_summary: ToolPayloadSummary,
        /// Validated tool argument JSON as sent to execution. Absent on
        /// validation failure and in logs written before the session tool
        /// payload persistence design.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        argument_content: Option<String>,
    },
    ToolExecutionFinished {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: ToolRequestId,
        outcome: ToolOutcome,
        reason: Option<String>,
        result_summary: Option<ToolPayloadSummary>,
        /// Exact bounded provider-visible result payload. Absent when no
        /// provider-visible result exists and in logs written before the
        /// session tool payload persistence design.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_content: Option<String>,
    },
    TurnFinished {
        session_id: SessionId,
        turn_id: TurnId,
        outcome: TurnOutcome,
        reason: Option<String>,
    },
    MetricRecorded {
        session_id: SessionId,
        turn_id: Option<TurnId>,
        metric: DurationMetric,
    },
    StaticContextIncluded {
        session_id: SessionId,
        turn_id: TurnId,
        summary: StaticContextSummary,
        omissions: Vec<StaticContextOmission>,
    },
    PermissionDecisionRecorded {
        session_id: SessionId,
        turn_id: TurnId,
        summary: PermissionDecisionSummary,
    },
    ApprovalModeChanged {
        session_id: SessionId,
        mode: ApprovalMode,
    },
    ToolReviewRequested {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: ToolRequestId,
        tool_name: String,
        payload: ToolReviewPayload,
    },
    ToolReviewDecisionRecorded {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: ToolRequestId,
        decision: ToolReviewDecision,
    },
    ToolReviewInterrupted {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: ToolRequestId,
        reason: String,
    },
    EditTraceRecorded {
        session_id: SessionId,
        turn_id: TurnId,
        trace: EditTraceRecord,
    },
    EditTransactionPrepared {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: Option<ToolRequestId>,
        transaction_id: EditTransactionId,
        summary: EditEvidenceSummary,
    },
    EditTransactionFinished {
        session_id: SessionId,
        turn_id: TurnId,
        tool_request_id: Option<ToolRequestId>,
        transaction_id: Option<EditTransactionId>,
        outcome: EditEvidenceOutcome,
        reason: Option<String>,
        summary: Option<EditEvidenceSummary>,
    },
    /// Context compaction checkpoint: the log is never truncated; provider
    /// context rebuilds as this summary plus events from
    /// `first_kept_entry_id` forward.
    /// Design: `docs/superpowers/specs/2026-07-20-context-compaction-design.md`.
    CompactionCheckpoint {
        session_id: SessionId,
        turn_id: TurnId,
        checkpoint_id: CompactionCheckpointId,
        summary: String,
        first_kept_entry_id: EntryId,
        tokens_before: u64,
        tokens_after_estimate: u64,
        reason: CompactionReason,
        compactor: String,
        /// Compactor-specific state carried across checkpoints (e.g. the
        /// summary compactor's cumulative read/modified file lists).
        details: serde_json::Value,
    },
    ToolResultMasked {
        session_id: SessionId,
        turn_id: TurnId,
        masked_turn_id: TurnId,
        tool_request_id: ToolRequestId,
        bytes_freed: u64,
        reason: MaskReason,
    },
}

/// In-memory view reconstructed from a native append-only event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionLog {
    pub events: Vec<SessionEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLoadResult {
    pub log: SessionLog,
    pub warnings: Vec<SessionLoadWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLoadWarning {
    InvalidJson { line_number: usize, reason: String },
}

impl SessionLog {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn push(&mut self, event: SessionEvent) {
        self.events.push(event);
    }

    #[must_use]
    pub fn next_turn_index(&self) -> u64 {
        self.events
            .iter()
            .filter_map(event_turn_id)
            .filter_map(numeric_turn_index)
            .max()
            .map_or(0, |index| index.saturating_add(1))
    }

    #[must_use]
    pub fn last_entry_id(&self) -> Option<EntryId> {
        self.events.iter().rev().find_map(|event| match event {
            SessionEvent::EntryAppended { entry_id, .. } => Some(entry_id.clone()),
            SessionEvent::ToolRequestRecorded { .. }
            | SessionEvent::ToolExecutionFinished { .. }
            | SessionEvent::TurnFinished { .. }
            | SessionEvent::MetricRecorded { .. }
            | SessionEvent::StaticContextIncluded { .. }
            | SessionEvent::PermissionDecisionRecorded { .. }
            | SessionEvent::ApprovalModeChanged { .. }
            | SessionEvent::ToolReviewRequested { .. }
            | SessionEvent::ToolReviewDecisionRecorded { .. }
            | SessionEvent::ToolReviewInterrupted { .. }
            | SessionEvent::EditTraceRecorded { .. }
            | SessionEvent::EditTransactionPrepared { .. }
            | SessionEvent::EditTransactionFinished { .. }
            | SessionEvent::CompactionCheckpoint { .. }
            | SessionEvent::ToolResultMasked { .. } => None,
        })
    }

    #[must_use]
    pub fn transcript_messages(&self) -> Vec<TranscriptMessage> {
        self.events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::EntryAppended { role, text, .. } => Some(TranscriptMessage {
                    role: *role,
                    text: text.clone(),
                }),
                SessionEvent::ToolRequestRecorded { .. }
                | SessionEvent::ToolExecutionFinished { .. }
                | SessionEvent::TurnFinished { .. }
                | SessionEvent::MetricRecorded { .. }
                | SessionEvent::StaticContextIncluded { .. }
                | SessionEvent::PermissionDecisionRecorded { .. }
                | SessionEvent::ApprovalModeChanged { .. }
                | SessionEvent::ToolReviewRequested { .. }
                | SessionEvent::ToolReviewDecisionRecorded { .. }
                | SessionEvent::ToolReviewInterrupted { .. }
                | SessionEvent::EditTraceRecorded { .. }
                | SessionEvent::EditTransactionPrepared { .. }
                | SessionEvent::EditTransactionFinished { .. }
                | SessionEvent::CompactionCheckpoint { .. }
                | SessionEvent::ToolResultMasked { .. } => None,
            })
            .collect()
    }

    pub fn record_static_context_included(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        summary: StaticContextSummary,
        omissions: Vec<StaticContextOmission>,
    ) {
        self.push(SessionEvent::StaticContextIncluded {
            session_id,
            turn_id,
            summary,
            omissions,
        });
    }

    pub fn record_duration_metric(
        &mut self,
        session_id: SessionId,
        turn_id: Option<TurnId>,
        name: impl Into<String>,
        duration: Duration,
        attributes: Vec<MetricAttribute>,
    ) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.push(SessionEvent::MetricRecorded {
            session_id,
            turn_id,
            metric: DurationMetric {
                name: name.into(),
                duration_ms,
                attributes,
            },
        });
    }

    pub fn record_permission_decision(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        summary: PermissionDecisionSummary,
    ) {
        self.push(SessionEvent::PermissionDecisionRecorded {
            session_id,
            turn_id,
            summary,
        });
    }

    pub fn record_edit_trace(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        trace: EditTraceRecord,
    ) {
        self.push(SessionEvent::EditTraceRecorded {
            session_id,
            turn_id,
            trace: bounded_edit_trace_record(trace),
        });
    }

    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut options = OpenOptions::new();
        options.create(true).write(true).truncate(true);
        configure_session_file_create_options(&mut options);
        let mut file = options.open(path)?;
        for event in &self.events {
            let line = serde_json::to_string(event).map_err(io::Error::other)?;
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.flush()?;
        file.sync_data()
    }

    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        Self::load_from_file_with_warnings(path).map(|result| result.log)
    }

    pub fn load_from_file_with_warnings(path: &Path) -> io::Result<SessionLoadResult> {
        let file = OpenOptions::new().read(true).open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        let mut warnings = Vec::new();

        for (line_index, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(event) => events.push(event),
                Err(error) => warnings.push(SessionLoadWarning::InvalidJson {
                    line_number: line_index.saturating_add(1),
                    reason: error.to_string(),
                }),
            }
        }

        Ok(SessionLoadResult {
            log: Self { events },
            warnings,
        })
    }
}

fn configure_session_file_create_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
}

fn event_turn_id(event: &SessionEvent) -> Option<&TurnId> {
    match event {
        SessionEvent::EntryAppended { turn_id, .. }
        | SessionEvent::ToolRequestRecorded { turn_id, .. }
        | SessionEvent::ToolExecutionFinished { turn_id, .. }
        | SessionEvent::TurnFinished { turn_id, .. }
        | SessionEvent::PermissionDecisionRecorded { turn_id, .. }
        | SessionEvent::ToolReviewRequested { turn_id, .. }
        | SessionEvent::ToolReviewDecisionRecorded { turn_id, .. }
        | SessionEvent::ToolReviewInterrupted { turn_id, .. }
        | SessionEvent::EditTraceRecorded { turn_id, .. }
        | SessionEvent::EditTransactionPrepared { turn_id, .. }
        | SessionEvent::EditTransactionFinished { turn_id, .. }
        | SessionEvent::CompactionCheckpoint { turn_id, .. }
        | SessionEvent::ToolResultMasked { turn_id, .. } => Some(turn_id),
        SessionEvent::MetricRecorded { turn_id, .. } => turn_id.as_ref(),
        SessionEvent::StaticContextIncluded { .. } | SessionEvent::ApprovalModeChanged { .. } => {
            None
        }
    }
}

fn numeric_turn_index(turn_id: &TurnId) -> Option<u64> {
    turn_id.0.strip_prefix("turn-")?.parse().ok()
}

/// Build the minimum persisted event sequence for a completed text exchange.
#[must_use]
pub fn completed_text_exchange(
    session_id: SessionId,
    user_entry_id: EntryId,
    assistant_entry_id: EntryId,
    turn_id: TurnId,
    prompt: String,
    response: String,
) -> SessionLog {
    let mut log = SessionLog::default();
    log.push(SessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: user_entry_id.clone(),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: Role::User,
        text: prompt,
        provider: None,
    });
    log.push(SessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: assistant_entry_id,
        parent_entry_id: Some(user_entry_id),
        turn_id: turn_id.clone(),
        role: Role::Assistant,
        text: response,
        provider: None,
    });
    log.push(SessionEvent::TurnFinished {
        session_id,
        turn_id,
        outcome: TurnOutcome::Completed,
        reason: None,
    });
    log
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_masked_event_round_trips_through_jsonl() {
        let event = SessionEvent::ToolResultMasked {
            session_id: SessionId(String::from("s")),
            turn_id: TurnId(String::from("turn-2")),
            masked_turn_id: TurnId(String::from("turn-1")),
            tool_request_id: ToolRequestId(String::from("req-1")),
            bytes_freed: 12_345,
            reason: MaskReason::ThresholdPrePass,
        };

        let line = serde_json::to_string(&event);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

        assert!(line.contains("\"type\":\"tool_result_masked\""));

        let parsed: Result<SessionEvent, _> = serde_json::from_str(&line);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else {
            return;
        };

        assert_eq!(parsed, event);
        assert_eq!(event_turn_id(&event), Some(&TurnId(String::from("turn-2"))));

        let mut log = SessionLog::default();
        log.push(SessionEvent::EntryAppended {
            session_id: SessionId(String::from("s")),
            entry_id: EntryId(String::from("entry-1")),
            parent_entry_id: None,
            turn_id: TurnId(String::from("turn-1")),
            role: Role::User,
            text: String::from("prompt"),
            provider: None,
        });
        log.push(event);

        assert_eq!(log.last_entry_id(), Some(EntryId(String::from("entry-1"))));
    }
}
