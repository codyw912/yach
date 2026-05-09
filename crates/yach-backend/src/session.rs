use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{NativeToolError, NativeToolPermissionState};

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
