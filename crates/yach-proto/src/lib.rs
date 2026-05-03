use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    PromptStreaming,
    Dialogs,
    Notifications,
    StatusEntries,
    Widgets,
    PromptCancellation,
    SessionForking,
    ThemeLoading,
    RichUi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    pub protocol_version: String,
    pub agent_name: String,
    pub capabilities: Vec<Capability>,
}

impl Handshake {
    #[must_use]
    pub fn new(agent_name: impl Into<String>, capabilities: Vec<Capability>) -> Self {
        Self {
            protocol_version: String::from(PROTOCOL_VERSION),
            agent_name: agent_name.into(),
            capabilities,
        }
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedCapabilities {
    pub protocol_version: String,
    pub ui_agent_name: String,
    pub adapter_agent_name: String,
    pub shared_capabilities: Vec<Capability>,
}

impl NegotiatedCapabilities {
    #[must_use]
    pub fn from_handshakes(ui: &Handshake, adapter: &Handshake) -> Self {
        let shared_capabilities = ui
            .capabilities
            .iter()
            .copied()
            .filter(|capability| adapter.supports(*capability))
            .collect();

        Self {
            protocol_version: ui.protocol_version.clone(),
            ui_agent_name: ui.agent_name.clone(),
            adapter_agent_name: adapter.agent_name.clone(),
            shared_capabilities,
        }
    }

    #[must_use]
    pub fn supports(&self, capability: Capability) -> bool {
        self.shared_capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    Connected { negotiated: NegotiatedCapabilities },
    Server(ServerEvent),
    Disconnected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageMeta {
    pub message_id: String,
    pub correlation_id: Option<String>,
    pub stream_id: Option<String>,
}

impl MessageMeta {
    #[must_use]
    pub fn new(message_id: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            correlation_id: None,
            stream_id: None,
        }
    }

    #[must_use]
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    #[must_use]
    pub fn with_stream_id(mut self, stream_id: impl Into<String>) -> Self {
        self.stream_id = Some(stream_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    ClientToAdapter,
    AdapterToClient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum MessageBody {
    ClientEvent(ClientEvent),
    ServerEvent(ServerEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportMessage {
    pub protocol_version: String,
    pub direction: MessageDirection,
    pub meta: MessageMeta,
    pub body: MessageBody,
}

impl TransportMessage {
    #[must_use]
    pub fn client(meta: MessageMeta, event: ClientEvent) -> Self {
        Self {
            protocol_version: String::from(PROTOCOL_VERSION),
            direction: MessageDirection::ClientToAdapter,
            meta,
            body: MessageBody::ClientEvent(event),
        }
    }

    #[must_use]
    pub fn server(meta: MessageMeta, event: ServerEvent) -> Self {
        Self {
            protocol_version: String::from(PROTOCOL_VERSION),
            direction: MessageDirection::AdapterToClient,
            meta,
            body: MessageBody::ServerEvent(event),
        }
    }

    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut json_line = serde_json::to_string(self)?;
        json_line.push('\n');
        Ok(json_line)
    }

    pub fn from_jsonl(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim_end())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetState {
    pub id: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub level: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendState {
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub model_provider: Option<String>,
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub thinking_level: Option<String>,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub message_count: Option<u64>,
    pub pending_message_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkMessage {
    pub entry_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStats {
    pub message_count: Option<u64>,
    pub user_message_count: Option<u64>,
    pub assistant_message_count: Option<u64>,
    pub tool_message_count: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub text: String,
    pub entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentSession {
    pub path: String,
    pub id: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub modified_unix_ms: Option<u64>,
    pub message_count: Option<u64>,
    pub first_message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptOutcome {
    Completed,
    Failed,
    Cancelled,
}

impl ModelInfo {
    #[must_use]
    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DialogKind {
    Select { options: Vec<DialogOption> },
    Confirm,
    Input { default: Option<String> },
    Editor { initial_text: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogRequest {
    pub id: Option<String>,
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub kind: DialogKind,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

impl ForkPosition {
    #[must_use]
    pub fn as_rpc_value(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::At => "at",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DialogResponse {
    Confirmed { accepted: bool },
    Text { value: String },
    Selection { value: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Initialize(Handshake),
    PromptSubmitted {
        session_id: String,
        prompt: String,
    },
    PromptCancelled {
        session_id: String,
    },
    SessionSelected {
        session_id: String,
    },
    SessionPathSelected {
        session_path: String,
    },
    AvailableModelsRequested,
    ForkMessagesRequested,
    SessionMessagesRequested,
    SessionStatsRequested,
    RecentSessionsRequested,
    ModelSelected {
        model: String,
    },
    ModelSelectedDetailed {
        provider: String,
        model_id: String,
    },
    SessionForkRequested {
        session_id: String,
        #[serde(default)]
        entry_id: Option<String>,
        #[serde(default)]
        position: ForkPosition,
    },
    DialogResolved {
        dialog_id: String,
        response: DialogResponse,
    },
    WidgetCleared {
        widget_id: String,
    },
    ThinkingLevelSelected {
        level: String,
    },
}

impl ClientEvent {
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut json_line = serde_json::to_string(self)?;
        json_line.push('\n');
        Ok(json_line)
    }

    pub fn from_jsonl(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim_end())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Ready {
        handshake: Handshake,
    },
    StateUpdated(BackendState),
    PromptDelta {
        session_id: String,
        delta: String,
    },
    PromptFinished {
        session_id: String,
        outcome: PromptOutcome,
        message: Option<String>,
    },
    ToolCallStarted {
        tool_call_id: Option<String>,
        tool_name: String,
        preview: Option<String>,
    },
    ToolCallFinished(ToolResult),
    StatusUpdated {
        message: String,
    },
    SessionChanged {
        session_id: String,
    },
    AvailableModelsUpdated {
        models: Vec<ModelInfo>,
    },
    ForkMessagesUpdated {
        messages: Vec<ForkMessage>,
    },
    SessionMessagesUpdated {
        messages: Vec<SessionMessage>,
    },
    SessionStatsUpdated(SessionStats),
    RecentSessionsUpdated {
        sessions: Vec<RecentSession>,
    },
    ModelChanged {
        model: String,
    },
    DialogRequested(DialogRequest),
    NotificationRaised(Notification),
    WidgetUpdated(WidgetState),
    TitleChanged {
        title: String,
    },
}

impl ServerEvent {
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut json_line = serde_json::to_string(self)?;
        json_line.push('\n');
        Ok(json_line)
    }

    pub fn from_jsonl(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line.trim_end())
    }
}

#[must_use]
pub fn default_ui_handshake() -> Handshake {
    Handshake::new(
        "yach-ui",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
            Capability::PromptCancellation,
            Capability::SessionForking,
            Capability::ThemeLoading,
        ],
    )
}

#[must_use]
pub fn default_rpc_handshake() -> Handshake {
    Handshake::new(
        "yach-adapter-pi-rpc",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
            Capability::SessionForking,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::{
        Capability, ClientEvent, Handshake, MessageBody, MessageDirection, MessageMeta,
        NegotiatedCapabilities, PROTOCOL_VERSION, ServerEvent, TransportMessage,
        default_rpc_handshake, default_ui_handshake,
    };

    #[test]
    fn protocol_version_tracks_prd_seed() {
        assert_eq!(PROTOCOL_VERSION, "0.1.0");
    }

    #[test]
    fn ui_handshake_exposes_phase_one_capabilities() {
        let handshake = default_ui_handshake();

        assert!(handshake.supports(Capability::PromptStreaming));
        assert!(handshake.supports(Capability::PromptCancellation));
        assert!(handshake.supports(Capability::ThemeLoading));
        assert!(!handshake.supports(Capability::RichUi));
    }

    #[test]
    fn rpc_handshake_does_not_claim_theme_loading() {
        let handshake = default_rpc_handshake();

        assert!(!handshake.supports(Capability::ThemeLoading));
        assert!(handshake.supports(Capability::Widgets));
    }

    #[test]
    fn events_are_equatable_for_record_replay_tests() {
        let client_event = ClientEvent::SessionSelected {
            session_id: String::from("session-1"),
        };
        let server_event = ServerEvent::StatusUpdated {
            message: String::from("ready"),
        };

        assert_eq!(
            client_event,
            ClientEvent::SessionSelected {
                session_id: String::from("session-1"),
            }
        );
        assert_eq!(
            server_event,
            ServerEvent::StatusUpdated {
                message: String::from("ready"),
            }
        );
    }

    #[test]
    fn handshakes_capture_agent_identity() {
        let handshake = Handshake::new("test-agent", vec![Capability::Dialogs]);

        assert_eq!(handshake.protocol_version, PROTOCOL_VERSION);
        assert_eq!(handshake.agent_name, "test-agent");
    }

    #[test]
    fn negotiated_capabilities_capture_the_intersection() {
        let negotiation = NegotiatedCapabilities::from_handshakes(
            &default_ui_handshake(),
            &default_rpc_handshake(),
        );

        assert!(negotiation.supports(Capability::PromptStreaming));
        assert!(!negotiation.supports(Capability::ThemeLoading));
    }

    #[test]
    fn client_events_round_trip_as_jsonl() {
        let event = ClientEvent::PromptSubmitted {
            session_id: String::from("session-1"),
            prompt: String::from("ship it"),
        };

        let json_line = event.to_jsonl();
        assert!(json_line.is_ok());
        let Ok(json_line) = json_line else {
            return;
        };
        let decoded = ClientEvent::from_jsonl(&json_line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };

        assert_eq!(decoded, event);
        assert!(json_line.ends_with('\n'));
        assert!(json_line.contains("\"type\":\"prompt_submitted\""));
    }

    #[test]
    fn prompt_lifecycle_events_round_trip_as_jsonl() {
        let cancel = ClientEvent::PromptCancelled {
            session_id: String::from("session-1"),
        };
        let cancel_line = cancel.to_jsonl();
        assert!(cancel_line.is_ok());
        let Ok(cancel_line) = cancel_line else {
            return;
        };
        let decoded_cancel = ClientEvent::from_jsonl(&cancel_line);
        assert!(decoded_cancel.is_ok());
        let Ok(decoded_cancel) = decoded_cancel else {
            return;
        };
        assert_eq!(decoded_cancel, cancel);
        assert!(cancel_line.contains("\"type\":\"prompt_cancelled\""));

        let finished = ServerEvent::PromptFinished {
            session_id: String::from("session-1"),
            outcome: crate::PromptOutcome::Cancelled,
            message: Some(String::from("cancelled")),
        };
        let finished_line = finished.to_jsonl();
        assert!(finished_line.is_ok());
        let Ok(finished_line) = finished_line else {
            return;
        };
        let decoded_finished = ServerEvent::from_jsonl(&finished_line);
        assert!(decoded_finished.is_ok());
        let Ok(decoded_finished) = decoded_finished else {
            return;
        };
        assert_eq!(decoded_finished, finished);
        assert!(finished_line.contains("\"type\":\"prompt_finished\""));
    }

    #[test]
    fn server_events_round_trip_as_jsonl() {
        let event = ServerEvent::Ready {
            handshake: default_rpc_handshake(),
        };

        let json_line = event.to_jsonl();
        assert!(json_line.is_ok());
        let Ok(json_line) = json_line else {
            return;
        };
        let decoded = ServerEvent::from_jsonl(&json_line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };

        assert_eq!(decoded, event);
        assert!(json_line.contains("\"type\":\"ready\""));
    }

    #[test]
    fn transport_messages_round_trip_with_correlation() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-1")
                .with_correlation_id("req-7")
                .with_stream_id("stream-2"),
            ClientEvent::PromptSubmitted {
                session_id: String::from("session-1"),
                prompt: String::from("hello"),
            },
        );

        let json_line = message.to_jsonl();
        assert!(json_line.is_ok());
        let Ok(json_line) = json_line else {
            return;
        };
        let decoded = TransportMessage::from_jsonl(&json_line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };

        assert_eq!(decoded, message);
        assert!(json_line.contains("\"direction\":\"client_to_adapter\""));
        assert!(json_line.contains("\"correlation_id\":\"req-7\""));
        assert!(json_line.contains("\"stream_id\":\"stream-2\""));
    }

    #[test]
    fn transport_messages_keep_server_payloads_typed() {
        let message = TransportMessage::server(
            MessageMeta::new("msg-2"),
            ServerEvent::StatusUpdated {
                message: String::from("connected"),
            },
        );

        assert_eq!(message.direction, MessageDirection::AdapterToClient);
        assert_eq!(message.protocol_version, PROTOCOL_VERSION);
        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("connected"),
            })
        );
    }
}
