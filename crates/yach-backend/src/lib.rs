//! Backend runner groundwork for yach.
//!
//! This crate owns backend-facing concepts that are not specific to the
//! temporary Pi RPC adapter or to the eventual native provider implementation.
//! The first slice intentionally stays small: runner extraction, session
//! persistence, and provider adapters will exercise these boundaries before
//! they split into larger APIs.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yach_proto::{BackendEvent, ClientEvent, NegotiatedCapabilities};

/// UI-facing channels exposed by any backend runner.
#[derive(Debug)]
pub struct BackendChannels {
    /// Sender cloned into the TUI so user actions can reach the backend.
    pub client_tx: mpsc::UnboundedSender<ClientEvent>,
    /// Receiver consumed by the TUI for backend/server events.
    pub backend_rx: mpsc::UnboundedReceiver<BackendEvent>,
}

/// Backend-side channel endpoints used by runner implementations.
#[derive(Debug)]
pub struct BackendEndpoints {
    /// Receives client events submitted by the TUI.
    pub client_rx: mpsc::UnboundedReceiver<ClientEvent>,
    /// Sends backend events consumed by the TUI.
    pub backend_tx: mpsc::UnboundedSender<BackendEvent>,
}

/// Started backend session state shared by CLI launchers.
#[derive(Debug)]
pub struct BackendSession {
    /// User-visible backend metadata.
    pub metadata: BackendMetadata,
    /// UI-facing channels consumed by the TUI.
    pub channels: BackendChannels,
    /// Runner-facing endpoints consumed by backend implementations.
    pub endpoints: BackendEndpoints,
}

/// Create the standard channel pair used between the TUI and a backend runner.
#[must_use]
pub fn backend_channels() -> (BackendChannels, BackendEndpoints) {
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let (backend_tx, backend_rx) = mpsc::unbounded_channel();

    (
        BackendChannels {
            client_tx,
            backend_rx,
        },
        BackendEndpoints {
            client_rx,
            backend_tx,
        },
    )
}

/// Send the initial connected event for a started runner.
#[must_use]
pub fn announce_connected(
    backend_tx: &mpsc::UnboundedSender<BackendEvent>,
    negotiated: NegotiatedCapabilities,
) -> bool {
    backend_tx
        .send(BackendEvent::Connected { negotiated })
        .is_ok()
}

/// Start a backend session by creating channels and announcing connection.
#[must_use]
pub fn start_backend_session(
    metadata: BackendMetadata,
    negotiated: NegotiatedCapabilities,
) -> BackendSession {
    let (channels, endpoints) = backend_channels();
    let _connected = announce_connected(&endpoints.backend_tx, negotiated);

    BackendSession {
        metadata,
        channels,
        endpoints,
    }
}

/// Stable backend families that a future runner selector can expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// The current stock Pi RPC compatibility adapter.
    PiRpc,
    /// The planned yach-owned native backend runtime.
    Native,
}

/// Coarse capability flags for a backend runner.
///
/// These describe behavior that the CLI/TUI may need to surface before a full
/// runner handle exists. They are deliberately backend-owned and provider-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    /// Backend can accept prompt submissions and stream assistant text.
    pub prompt_streaming: bool,
    /// Backend owns an inspectable local session persistence path.
    pub file_first_sessions: bool,
    /// Backend can expose native tool execution through yach-owned policy.
    pub tool_execution: bool,
}

impl BackendCapabilities {
    /// Capabilities expected from the current Pi RPC compatibility path.
    #[must_use]
    pub const fn pi_rpc_compatibility() -> Self {
        Self {
            prompt_streaming: true,
            file_first_sessions: false,
            tool_execution: false,
        }
    }

    /// Capabilities for the first native dogfood runner before tools/resources land.
    #[must_use]
    pub const fn native_dogfood() -> Self {
        Self {
            prompt_streaming: true,
            file_first_sessions: true,
            tool_execution: false,
        }
    }
}

/// Human-readable runner metadata for status and selection surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendMetadata {
    /// Stable backend family.
    pub kind: BackendKind,
    /// Short display label suitable for status/help text.
    pub label: &'static str,
    /// Current coarse capabilities for this backend.
    pub capabilities: BackendCapabilities,
}

impl BackendMetadata {
    /// Metadata for the default Pi-backed runner.
    #[must_use]
    pub const fn pi_rpc() -> Self {
        Self {
            kind: BackendKind::PiRpc,
            label: "pi rpc",
            capabilities: BackendCapabilities::pi_rpc_compatibility(),
        }
    }

    /// Metadata for the constrained native dogfood runner.
    #[must_use]
    pub const fn native_dogfood() -> Self {
        Self {
            kind: BackendKind::Native,
            label: "native dogfood",
            capabilities: BackendCapabilities::native_dogfood(),
        }
    }
}

/// Native session identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeSessionId(pub String);

/// Native transcript entry identifier owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeEntryId(pub String);

/// Native turn/request identifier used to reject stale stream events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeTurnId(pub String);

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

    pub fn append_to_file(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        BackendCapabilities, BackendKind, BackendMetadata, NativeEntryId, NativeRole,
        NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeTurnId, NativeTurnOutcome,
        announce_connected, backend_channels, completed_text_exchange, start_backend_session,
    };
    use yach_proto::{BackendEvent, Capability, ClientEvent, Handshake, NegotiatedCapabilities};

    #[test]
    fn pi_rpc_metadata_identifies_compatibility_runner() {
        let metadata = BackendMetadata::pi_rpc();

        assert_eq!(metadata.kind, BackendKind::PiRpc);
        assert_eq!(metadata.label, "pi rpc");
        assert_eq!(
            metadata.capabilities,
            BackendCapabilities::pi_rpc_compatibility()
        );
        assert!(metadata.capabilities.prompt_streaming);
        assert!(!metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn native_dogfood_metadata_identifies_file_first_runner() {
        let metadata = BackendMetadata::native_dogfood();

        assert_eq!(metadata.kind, BackendKind::Native);
        assert_eq!(metadata.label, "native dogfood");
        assert_eq!(metadata.capabilities, BackendCapabilities::native_dogfood());
        assert!(metadata.capabilities.prompt_streaming);
        assert!(metadata.capabilities.file_first_sessions);
        assert!(!metadata.capabilities.tool_execution);
    }

    #[test]
    fn metadata_has_debug_and_equality_behavior() {
        let left = BackendMetadata::native_dogfood();
        let right = BackendMetadata::native_dogfood();

        assert_eq!(left, right);
        assert_eq!(format!("{left:?}"), format!("{right:?}"));
    }

    #[test]
    fn backend_channels_connect_ui_sender_to_runner_receiver() {
        let (channels, mut endpoints) = backend_channels();

        assert!(
            channels
                .client_tx
                .send(ClientEvent::RecentSessionsRequested)
                .is_ok()
        );

        assert_eq!(
            endpoints.client_rx.blocking_recv(),
            Some(ClientEvent::RecentSessionsRequested)
        );
    }

    #[test]
    fn connected_announcement_reaches_ui_receiver() {
        let (mut channels, endpoints) = backend_channels();
        let negotiated = negotiated_prompt_streaming();

        assert!(announce_connected(
            &endpoints.backend_tx,
            negotiated.clone()
        ));

        assert_eq!(
            channels.backend_rx.blocking_recv(),
            Some(BackendEvent::Connected { negotiated })
        );
    }

    #[test]
    fn backend_session_carries_metadata_and_announces_connection() {
        let negotiated = negotiated_prompt_streaming();
        let mut session = start_backend_session(BackendMetadata::pi_rpc(), negotiated.clone());

        assert_eq!(session.metadata, BackendMetadata::pi_rpc());
        assert_eq!(
            session.channels.backend_rx.blocking_recv(),
            Some(BackendEvent::Connected { negotiated })
        );
    }

    #[test]
    fn native_session_log_starts_empty() {
        let log = NativeSessionLog::default();

        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn completed_exchange_has_stable_parent_links() {
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );

        assert_eq!(log.len(), 3);
        assert_eq!(
            log.events.first(),
            Some(&NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("session-1")),
                entry_id: NativeEntryId(String::from("entry-user")),
                parent_entry_id: None,
                turn_id: NativeTurnId(String::from("turn-1")),
                role: NativeRole::User,
                text: String::from("hello"),
                provider: None,
            })
        );
        assert_eq!(
            log.events.get(1),
            Some(&NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("session-1")),
                entry_id: NativeEntryId(String::from("entry-assistant")),
                parent_entry_id: Some(NativeEntryId(String::from("entry-user"))),
                turn_id: NativeTurnId(String::from("turn-1")),
                role: NativeRole::Assistant,
                text: String::from("hi"),
                provider: None,
            })
        );
        assert_eq!(
            log.events.get(2),
            Some(&NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("session-1")),
                turn_id: NativeTurnId(String::from("turn-1")),
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            })
        );
    }

    #[test]
    fn cancelled_or_failed_turns_are_distinct_from_completed_turns() {
        let cancelled = NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
            outcome: NativeTurnOutcome::Cancelled,
            reason: Some(String::from("user cancelled")),
        };
        let failed = NativeSessionEvent::TurnFinished {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
            outcome: NativeTurnOutcome::Failed,
            reason: Some(String::from("provider error")),
        };

        assert_ne!(cancelled, failed);
    }

    #[test]
    fn native_session_log_appends_and_reloads_jsonl() {
        let path = temp_log_path("native-session-log");
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );

        assert!(log.append_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    fn negotiated_prompt_streaming() -> NegotiatedCapabilities {
        let ui = Handshake::new("ui", vec![Capability::PromptStreaming]);
        let backend = Handshake::new("backend", vec![Capability::PromptStreaming]);
        NegotiatedCapabilities::from_handshakes(&ui, &backend)
    }

    fn temp_log_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("{name}-{unique}.jsonl"))
    }
}
