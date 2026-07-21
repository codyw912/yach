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
    LocalEdit,
    ExtensionLifecycle,
    RichUi,
    FirstRenderEvents,
    ToolOutputStreaming,
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
    /// Estimated share of the usable context window currently occupied,
    /// from the same accounting as the auto-compaction trigger.
    #[serde(default)]
    pub context_used_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub text: String,
    pub entry_id: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub is_error: Option<bool>,
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
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalEditOperationInput {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        find: String,
        replace: String,
    },
    CreateTextFile {
        path: String,
        content: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditDecision {
    Apply,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditReviewState {
    Allowed,
    NeedsUserApproval,
    AutoReviewUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalEditPreviewSummary {
    pub preview_id: String,
    pub transaction_id: String,
    pub permission_decision_id: String,
    pub path: String,
    pub operation: String,
    pub review_state: LocalEditReviewState,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
}

/// Command awaiting user review before execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandReviewSummary {
    pub review_id: String,
    pub permission_decision_id: String,
    pub command: String,
    pub workdir: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolReviewPayload {
    LocalEdit { preview: LocalEditPreviewSummary },
    Command { command: CommandReviewSummary },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEditFinishedOutcome {
    Applied,
    Rejected,
    Denied,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionLifecycleAction {
    Stop,
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionLifecycleOutcome {
    Completed,
    NotFound,
    NotActive,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDiagnosticSnapshotOutcome {
    Completed,
    NotFound,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionDiagnosticRecord {
    pub id: Option<String>,
    pub version: Option<String>,
    pub scope: String,
    pub package_root: String,
    pub manifest_path: Option<String>,
    pub source_ref: Option<String>,
    pub install_source: Option<String>,
    pub activation_state: String,
    pub generation: u64,
    pub last_error_kind: Option<String>,
    pub last_error_summary: Option<String>,
    pub registered_tools: Vec<String>,
    pub provider_visible_tools: Vec<String>,
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
    /// Manual context compaction (`/compact [focus instructions]`).
    CompactionRequested {
        session_id: String,
        #[serde(default)]
        instructions: Option<String>,
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
    LocalEditPrepareRequested {
        request_id: String,
        operation: LocalEditOperationInput,
    },
    LocalEditDecisionSubmitted {
        preview_id: String,
        permission_decision_id: String,
        decision: LocalEditDecision,
    },
    ToolReviewDecisionSubmitted {
        request_id: String,
        preview_id: String,
        permission_decision_id: String,
        decision: LocalEditDecision,
    },
    ExtensionLifecycleRequested {
        request_id: String,
        action: ExtensionLifecycleAction,
        selector: String,
    },
    ExtensionDiagnosticSnapshotRequested {
        request_id: String,
        selector: Option<String>,
    },
    FirstRenderCompleted,
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
    /// Bounded live output from a running tool (negotiated by
    /// `Capability::ToolOutputStreaming`); display-only, the model-visible
    /// result still arrives in `ToolCallFinished`.
    ToolCallOutput {
        tool_call_id: String,
        chunk: String,
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
    ToolReviewRequested {
        request_id: String,
        tool_name: String,
        payload: ToolReviewPayload,
    },
    LocalEditPreviewReady {
        request_id: String,
        preview: LocalEditPreviewSummary,
    },
    LocalEditFinished {
        preview_id: Option<String>,
        outcome: LocalEditFinishedOutcome,
        message: String,
    },
    ExtensionLifecycleFinished {
        request_id: String,
        action: ExtensionLifecycleAction,
        selector: String,
        outcome: ExtensionLifecycleOutcome,
        message: String,
    },
    ExtensionDiagnosticSnapshotUpdated {
        request_id: String,
        outcome: ExtensionDiagnosticSnapshotOutcome,
        records: Vec<ExtensionDiagnosticRecord>,
        message: Option<String>,
    },
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
            Capability::LocalEdit,
            Capability::ExtensionLifecycle,
            Capability::FirstRenderEvents,
            Capability::ToolOutputStreaming,
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
#[test]
fn tool_review_events_round_trip_as_jsonl() {
    let requested = ServerEvent::ToolReviewRequested {
        request_id: String::from("tool-review-request-1"),
        tool_name: String::from("edit_text_file"),
        payload: ToolReviewPayload::LocalEdit {
            preview: LocalEditPreviewSummary {
                preview_id: String::from("edit-preview-1"),
                transaction_id: String::from("edit-transaction-1"),
                permission_decision_id: String::from("permission-decision-1"),
                path: String::from("src/lib.rs"),
                operation: String::from("modify_text_file"),
                review_state: LocalEditReviewState::NeedsUserApproval,
                diff_summary: String::from("-old\n+new\n"),
                diff_summary_truncated: false,
            },
        },
    };

    let line = requested.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    let decoded = ServerEvent::from_jsonl(&line);
    assert!(decoded.is_ok());
    let Ok(decoded) = decoded else {
        return;
    };
    assert_eq!(decoded, requested);
    assert!(line.contains("\"type\":\"tool_review_requested\""));
    assert!(line.contains("\"kind\":\"local_edit\""));

    let submitted = ClientEvent::ToolReviewDecisionSubmitted {
        request_id: String::from("tool-review-request-1"),
        preview_id: String::from("edit-preview-1"),
        permission_decision_id: String::from("permission-decision-1"),
        decision: LocalEditDecision::Apply,
    };

    let line = submitted.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    let decoded = ClientEvent::from_jsonl(&line);
    assert!(decoded.is_ok());
    let Ok(decoded) = decoded else {
        return;
    };
    assert_eq!(decoded, submitted);
    assert!(line.contains("\"type\":\"tool_review_decision_submitted\""));
    assert!(line.contains("\"decision\":\"apply\""));
}

#[cfg(test)]
#[test]
fn compaction_requested_round_trips_as_jsonl() {
    let event = ClientEvent::CompactionRequested {
        session_id: String::from("default"),
        instructions: Some(String::from("keep the migration plan")),
    };
    let line = event.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    assert!(line.contains("\"type\":\"compaction_requested\""));
    let decoded = ClientEvent::from_jsonl(&line);
    assert_eq!(decoded.ok(), Some(event));
}

#[cfg(test)]
#[test]
fn tool_call_output_round_trips_as_jsonl() {
    let event = ServerEvent::ToolCallOutput {
        tool_call_id: String::from("tool-request-1-1"),
        chunk: String::from("Compiling yach-proto v0.1.0\n"),
    };

    let line = event.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    let decoded = ServerEvent::from_jsonl(&line);
    assert!(decoded.is_ok());
    let Ok(decoded) = decoded else {
        return;
    };
    assert_eq!(decoded, event);
    assert!(line.contains("\"type\":\"tool_call_output\""));

    let handshake = default_ui_handshake();
    assert!(handshake.supports(Capability::ToolOutputStreaming));
}

#[cfg(test)]
mod tests {
    use super::{
        Capability, ClientEvent, Handshake, LocalEditDecision, LocalEditFinishedOutcome,
        LocalEditOperationInput, LocalEditPreviewSummary, LocalEditReviewState, MessageBody,
        MessageDirection, MessageMeta, NegotiatedCapabilities, PROTOCOL_VERSION, ServerEvent,
        TransportMessage, default_rpc_handshake, default_ui_handshake,
    };
    use crate::{
        ExtensionDiagnosticRecord, ExtensionDiagnosticSnapshotOutcome, ExtensionLifecycleAction,
        ExtensionLifecycleOutcome,
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
        assert!(handshake.supports(Capability::LocalEdit));
        assert!(handshake.supports(Capability::ExtensionLifecycle));
        assert!(!handshake.supports(Capability::RichUi));
    }

    #[test]
    fn ui_handshake_exposes_local_edit_capability() {
        let handshake = default_ui_handshake();

        assert!(handshake.supports(Capability::LocalEdit));
    }

    #[test]
    fn rpc_handshake_does_not_claim_theme_loading() {
        let handshake = default_rpc_handshake();

        assert!(!handshake.supports(Capability::ThemeLoading));
        assert!(!handshake.supports(Capability::LocalEdit));
        assert!(!handshake.supports(Capability::ExtensionLifecycle));
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
    fn local_edit_events_round_trip_as_jsonl() {
        let prepare = ClientEvent::LocalEditPrepareRequested {
            request_id: String::from("local-edit-request-1"),
            operation: LocalEditOperationInput::ModifyTextFile {
                path: String::from("src/lib.rs"),
                expected_sha256: String::from("abc123"),
                find: String::from("old"),
                replace: String::from("new"),
            },
        };

        let line = prepare.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ClientEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, prepare);
        assert!(line.contains("\"type\":\"local_edit_prepare_requested\""));

        let decision = ClientEvent::LocalEditDecisionSubmitted {
            preview_id: String::from("edit-preview-1"),
            permission_decision_id: String::from("permission-decision-1"),
            decision: LocalEditDecision::Apply,
        };

        let line = decision.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ClientEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, decision);
        assert!(line.contains("\"decision\":\"apply\""));

        let preview = ServerEvent::LocalEditPreviewReady {
            request_id: String::from("local-edit-request-1"),
            preview: LocalEditPreviewSummary {
                preview_id: String::from("edit-preview-1"),
                transaction_id: String::from("edit-transaction-1"),
                permission_decision_id: String::from("permission-decision-1"),
                path: String::from("src/lib.rs"),
                operation: String::from("modify_text_file"),
                review_state: LocalEditReviewState::NeedsUserApproval,
                diff_summary: String::from("-old\n+new\n"),
                diff_summary_truncated: false,
            },
        };

        let line = preview.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ServerEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, preview);
        assert!(line.contains("\"type\":\"local_edit_preview_ready\""));

        let finished = ServerEvent::LocalEditFinished {
            preview_id: Some(String::from("edit-preview-1")),
            outcome: LocalEditFinishedOutcome::Rejected,
            message: String::from("rejected"),
        };

        let line = finished.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ServerEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, finished);
        assert!(line.contains("\"outcome\":\"rejected\""));
    }

    #[test]
    fn extension_lifecycle_events_round_trip_as_jsonl() {
        let requested = ClientEvent::ExtensionLifecycleRequested {
            request_id: String::from("extension-lifecycle-request-1"),
            action: ExtensionLifecycleAction::Stop,
            selector: String::from("example.toy-tools"),
        };

        let line = requested.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ClientEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, requested);
        assert!(line.contains("\"type\":\"extension_lifecycle_requested\""));
        assert!(line.contains("\"action\":\"stop\""));

        let finished = ServerEvent::ExtensionLifecycleFinished {
            request_id: String::from("extension-lifecycle-request-1"),
            action: ExtensionLifecycleAction::Stop,
            selector: String::from("example.toy-tools"),
            outcome: ExtensionLifecycleOutcome::Completed,
            message: String::from("extension stopped"),
        };

        let line = finished.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ServerEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, finished);
        assert!(line.contains("\"type\":\"extension_lifecycle_finished\""));
        assert!(line.contains("\"outcome\":\"completed\""));
    }

    #[test]
    fn extension_diagnostic_snapshot_events_round_trip_as_jsonl() {
        let requested = ClientEvent::ExtensionDiagnosticSnapshotRequested {
            request_id: String::from("extension-diagnostic-request-1"),
            selector: Some(String::from("example.toy-tools")),
        };

        let line = requested.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ClientEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, requested);
        assert!(line.contains("\"type\":\"extension_diagnostic_snapshot_requested\""));

        let updated = ServerEvent::ExtensionDiagnosticSnapshotUpdated {
            request_id: String::from("extension-diagnostic-request-1"),
            outcome: ExtensionDiagnosticSnapshotOutcome::Completed,
            records: vec![ExtensionDiagnosticRecord {
                id: Some(String::from("example.toy-tools")),
                version: Some(String::from("0.1.0")),
                scope: String::from("user"),
                package_root: String::from("/tmp/example"),
                manifest_path: Some(String::from("/tmp/example/yach.extension.json")),
                source_ref: Some(String::from("test-package-root")),
                install_source: None,
                activation_state: String::from("active"),
                generation: 1,
                last_error_kind: None,
                last_error_summary: None,
                registered_tools: vec![String::from("toy_tool")],
                provider_visible_tools: vec![String::from("toy_tool")],
            }],
            message: None,
        };

        let line = updated.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        let decoded = ServerEvent::from_jsonl(&line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert_eq!(decoded, updated);
        assert!(line.contains("\"type\":\"extension_diagnostic_snapshot_updated\""));
        assert!(line.contains("\"activation_state\":\"active\""));
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
