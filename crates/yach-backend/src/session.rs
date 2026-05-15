use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::static_context::{NativeStaticContextOmission, NativeStaticContextSummary};
use crate::{
    NativeEditTransactionId, NativePermissionDecisionSummary, NativeToolError,
    NativeToolPermissionState,
};

/// Native session identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeSessionId(pub String);

/// Native transcript entry identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeEntryId(pub String);

/// Native turn/request identifier used to reject stale stream events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeTurnId(pub String);

/// Native tool request identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeToolRequestId(pub String);

/// Redacted summary for tool arguments or results persisted in native logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolPayloadSummary {
    pub summary: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
}

/// Provisional persisted native tool outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolOutcome {
    Completed,
    Failed,
    Denied,
    Cancelled,
    ValidationFailed,
}

/// Role for a native session entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRole {
    User,
    Assistant,
    Tool,
    System,
}

/// Terminal state for an assistant stream in the native session log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTurnOutcome {
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
}

/// Provider-ready transcript message reconstructed from native entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeTranscriptMessage {
    pub role: NativeRole,
    pub text: String,
}

/// Low-cardinality metric attribute persisted with a native duration metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeMetricAttribute {
    pub key: String,
    pub value: String,
}

/// Summarized low-frequency native duration metric.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDurationMetric {
    pub name: String,
    pub duration_ms: u64,
    pub attributes: Vec<NativeMetricAttribute>,
}

/// Redacted edit transaction summary persisted in native session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditEvidenceSummary {
    pub operation_count: usize,
    pub operations: Vec<NativeEditOperationEvidence>,
    pub diff_summary: NativeToolPayloadSummary,
}

/// Redacted per-operation edit evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeEditOperationEvidence {
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
pub enum NativeEditEvidenceOutcome {
    Completed,
    ValidationFailed,
    Failed,
}

/// Append-only native session event record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeSessionEvent {
    EntryAppended {
        session_id: NativeSessionId,
        entry_id: NativeEntryId,
        parent_entry_id: Option<NativeEntryId>,
        turn_id: NativeTurnId,
        role: NativeRole,
        text: String,
        provider: Option<ProviderMetadata>,
    },
    ToolRequestRecorded {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: NativeToolRequestId,
        tool_name: String,
        provider_call_id: Option<String>,
        validation: Result<(), NativeToolError>,
        permission: NativeToolPermissionState,
        argument_summary: NativeToolPayloadSummary,
    },
    ToolExecutionFinished {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: NativeToolRequestId,
        outcome: NativeToolOutcome,
        reason: Option<String>,
        result_summary: Option<NativeToolPayloadSummary>,
    },
    TurnFinished {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        outcome: NativeTurnOutcome,
        reason: Option<String>,
    },
    MetricRecorded {
        session_id: NativeSessionId,
        turn_id: Option<NativeTurnId>,
        metric: NativeDurationMetric,
    },
    StaticContextIncluded {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativeStaticContextSummary,
        omissions: Vec<NativeStaticContextOmission>,
    },
    PermissionDecisionRecorded {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativePermissionDecisionSummary,
    },
    EditTransactionPrepared {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: Option<NativeToolRequestId>,
        transaction_id: NativeEditTransactionId,
        summary: NativeEditEvidenceSummary,
    },
    EditTransactionFinished {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: Option<NativeToolRequestId>,
        transaction_id: Option<NativeEditTransactionId>,
        outcome: NativeEditEvidenceOutcome,
        reason: Option<String>,
        summary: Option<NativeEditEvidenceSummary>,
    },
}

/// In-memory view reconstructed from a native append-only event log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeSessionLog {
    pub events: Vec<NativeSessionEvent>,
}

impl NativeSessionLog {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn push(&mut self, event: NativeSessionEvent) {
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
    pub fn last_entry_id(&self) -> Option<NativeEntryId> {
        self.events.iter().rev().find_map(|event| match event {
            NativeSessionEvent::EntryAppended { entry_id, .. } => Some(entry_id.clone()),
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::PermissionDecisionRecorded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
        })
    }

    #[must_use]
    pub fn transcript_messages(&self) -> Vec<NativeTranscriptMessage> {
        self.events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EntryAppended { role, text, .. } => {
                    Some(NativeTranscriptMessage {
                        role: *role,
                        text: text.clone(),
                    })
                }
                NativeSessionEvent::ToolRequestRecorded { .. }
                | NativeSessionEvent::ToolExecutionFinished { .. }
                | NativeSessionEvent::TurnFinished { .. }
                | NativeSessionEvent::MetricRecorded { .. }
                | NativeSessionEvent::StaticContextIncluded { .. }
                | NativeSessionEvent::PermissionDecisionRecorded { .. }
                | NativeSessionEvent::EditTransactionPrepared { .. }
                | NativeSessionEvent::EditTransactionFinished { .. } => None,
            })
            .collect()
    }

    pub fn record_static_context_included(
        &mut self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativeStaticContextSummary,
        omissions: Vec<NativeStaticContextOmission>,
    ) {
        self.push(NativeSessionEvent::StaticContextIncluded {
            session_id,
            turn_id,
            summary,
            omissions,
        });
    }

    pub fn record_duration_metric(
        &mut self,
        session_id: NativeSessionId,
        turn_id: Option<NativeTurnId>,
        name: impl Into<String>,
        duration: Duration,
        attributes: Vec<NativeMetricAttribute>,
    ) {
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.push(NativeSessionEvent::MetricRecorded {
            session_id,
            turn_id,
            metric: NativeDurationMetric {
                name: name.into(),
                duration_ms,
                attributes,
            },
        });
    }

    pub fn record_permission_decision(
        &mut self,
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        summary: NativePermissionDecisionSummary,
    ) {
        self.push(NativeSessionEvent::PermissionDecisionRecorded {
            session_id,
            turn_id,
            summary,
        });
    }

    pub fn write_to_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        for event in &self.events {
            let line = serde_json::to_string(event).map_err(io::Error::other)?;
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
        }
        file.flush()
    }

    pub fn load_from_file(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event = serde_json::from_str(&line).map_err(io::Error::other)?;
            events.push(event);
        }

        Ok(Self { events })
    }
}

fn event_turn_id(event: &NativeSessionEvent) -> Option<&NativeTurnId> {
    match event {
        NativeSessionEvent::EntryAppended { turn_id, .. }
        | NativeSessionEvent::ToolRequestRecorded { turn_id, .. }
        | NativeSessionEvent::ToolExecutionFinished { turn_id, .. }
        | NativeSessionEvent::TurnFinished { turn_id, .. }
        | NativeSessionEvent::PermissionDecisionRecorded { turn_id, .. }
        | NativeSessionEvent::EditTransactionPrepared { turn_id, .. }
        | NativeSessionEvent::EditTransactionFinished { turn_id, .. } => Some(turn_id),
        NativeSessionEvent::MetricRecorded { turn_id, .. } => turn_id.as_ref(),
        NativeSessionEvent::StaticContextIncluded { .. } => None,
    }
}

fn numeric_turn_index(turn_id: &NativeTurnId) -> Option<u64> {
    turn_id.0.strip_prefix("turn-")?.parse().ok()
}

/// Build the minimum persisted event sequence for a completed text exchange.
#[must_use]
pub fn completed_text_exchange(
    session_id: NativeSessionId,
    user_entry_id: NativeEntryId,
    assistant_entry_id: NativeEntryId,
    turn_id: NativeTurnId,
    prompt: String,
    response: String,
) -> NativeSessionLog {
    let mut log = NativeSessionLog::default();
    log.push(NativeSessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: user_entry_id.clone(),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: NativeRole::User,
        text: prompt,
        provider: None,
    });
    log.push(NativeSessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: assistant_entry_id,
        parent_entry_id: Some(user_entry_id),
        turn_id: turn_id.clone(),
        role: NativeRole::Assistant,
        text: response,
        provider: None,
    });
    log.push(NativeSessionEvent::TurnFinished {
        session_id,
        turn_id,
        outcome: NativeTurnOutcome::Completed,
        reason: None,
    });
    log
}
