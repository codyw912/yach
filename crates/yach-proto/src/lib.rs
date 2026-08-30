use serde::{Deserialize, Serialize};
use std::fmt;

use zeroize::Zeroize;

pub const PROTOCOL_VERSION: &str = "0.3.0";
pub const MAX_PROTOCOL_VERSION_BYTES: usize = 32;

/// Truncates a protocol version for diagnostics and mismatch copy.
#[must_use]
pub fn bounded_protocol_version(version: &str) -> String {
    if version.len() <= MAX_PROTOCOL_VERSION_BYTES {
        return String::from(version);
    }
    let mut end = MAX_PROTOCOL_VERSION_BYTES;
    while end > 0 && !version.is_char_boundary(end) {
        end -= 1;
    }
    String::from(&version[..end])
}

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
    ProviderConnections,
    ToolOutputStreaming,
    StructuredReviewRows,
    ApprovalModes,
    ModelState,
    PromptAttemptReset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

impl<'de> Deserialize<'de> for Handshake {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct HandshakeWire {
            protocol_version: String,
            agent_name: String,
            #[serde(default)]
            capabilities: Option<serde_json::Value>,
        }

        let wire = HandshakeWire::deserialize(deserializer)?;
        if wire.protocol_version != PROTOCOL_VERSION {
            return Ok(Self {
                protocol_version: bounded_protocol_version(&wire.protocol_version),
                agent_name: wire.agent_name,
                capabilities: Vec::new(),
            });
        }
        let Some(capabilities) = wire.capabilities else {
            return Err(serde::de::Error::missing_field("capabilities"));
        };
        Ok(Self {
            protocol_version: wire.protocol_version,
            agent_name: wire.agent_name,
            capabilities: serde_json::from_value(capabilities).map_err(serde::de::Error::custom)?,
        })
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

    #[must_use]
    pub fn ready_handshake(&self) -> Handshake {
        Handshake {
            protocol_version: self.protocol_version.clone(),
            agent_name: self.adapter_agent_name.clone(),
            capabilities: self.shared_capabilities.clone(),
        }
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

    /// Encodes a message for persistent record/replay storage.
    ///
    /// Submitted secrets are intentionally omitted; use [`Self::to_jsonl`] for
    /// the direct wire transport instead.
    pub fn to_record_jsonl(&self) -> Result<Option<String>, serde_json::Error> {
        match &self.body {
            MessageBody::ClientEvent(event) if !event.is_recordable() => Ok(None),
            _ => self.to_jsonl().map(Some),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Review,
    AcceptEdits,
    FullAccess,
}

impl ApprovalMode {
    pub const ALL: [Self; 3] = [Self::Review, Self::AcceptEdits, Self::FullAccess];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::AcceptEdits => "accept-edits",
            Self::FullAccess => "full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
    Max,
}

impl ThinkingLevel {
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Max];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub fn parse(level: &str) -> Option<Self> {
        match level {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTargetRequest {
    pub provider: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTarget {
    pub provider: String,
    pub model_id: String,
    pub connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_key: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelActivationIntent {
    SessionOnly,
    SessionAndDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTargetResolutionReason {
    InvalidConfig,
    ConnectionMissing,
    ConnectionKeyRequired,
    ConnectionNotReady,
    AuthenticationUnavailable,
    ModelUnavailable,
    AvailabilityUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultUpdateOutcome {
    NotAttempted,
    Saved,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionModelState {
    Resolving {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<ModelTargetRequest>,
    },
    Active {
        target: ModelTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    Unresolved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<ModelTargetRequest>,
        reason: ModelTargetResolutionReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DefaultModelState {
    Absent,
    Resolved {
        target: ModelTarget,
    },
    Unresolved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<ModelTargetRequest>,
        reason: ModelTargetResolutionReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendState {
    pub session_model: SessionModelState,
    pub default_model: DefaultModelState,
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub is_streaming: bool,
    pub is_compacting: bool,
    pub message_count: Option<u64>,
    pub pending_message_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultMetadata {
    pub byte_count: usize,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]

pub struct ToolResult {
    pub tool_call_id: Option<String>,
    pub tool_name: String,
    pub output: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_kind: Option<HarnessOutcomeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ToolResultMetadata>,
}

/// A harness-authored transcript outcome. This is display metadata only; it
/// does not alter provider-visible result content or persisted session events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessOutcomeKind {
    Blocked,
    Failed,
    Denied,
    Cancelled,
    Limit,
}

impl HarnessOutcomeKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Denied => "denied",
            Self::Cancelled => "cancelled",
            Self::Limit => "limit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_display: Option<String>,
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
    /// Configured model context window before output and compaction reserves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_kind: Option<HarnessOutcomeKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result_metadata: Option<ToolResultMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_review: Option<ToolReviewHistory>,
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DialogKind {
    Select {
        options: Vec<DialogOption>,
    },
    Confirm,
    Input {
        default: Option<String>,
    },
    Editor {
        initial_text: Option<String>,
    },
    SecretInput,
    DeviceCode {
        verification_uri: String,
        user_code: String,
    },
}

impl fmt::Debug for DialogKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Select { options } => formatter
                .debug_struct("Select")
                .field("options", options)
                .finish(),
            Self::Confirm => formatter.write_str("Confirm"),
            Self::Input { default } => formatter
                .debug_struct("Input")
                .field("default", default)
                .finish(),
            Self::Editor { initial_text } => formatter
                .debug_struct("Editor")
                .field("initial_text", initial_text)
                .finish(),
            Self::SecretInput => formatter.write_str("SecretInput"),
            Self::DeviceCode {
                verification_uri, ..
            } => formatter
                .debug_struct("DeviceCode")
                .field("verification_uri", verification_uri)
                .field("user_code", &"[REDACTED]")
                .finish(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DialogRequest {
    pub id: Option<String>,
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub kind: DialogKind,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedSecret(String);

impl SubmittedSecret {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn into_inner(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl fmt::Debug for SubmittedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Drop for SubmittedSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
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
    Secret { value: SubmittedSecret },
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
pub enum ToolReviewDecision {
    Approve,
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
pub enum ToolReviewResolution {
    Approved,
    Rejected,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReviewHistory {
    pub request_id: String,
    pub payload: ToolReviewPayload,
    pub resolution: ToolReviewResolution,
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
    ConnectionsRequested,
    ModelActivationRequested {
        target: ModelTarget,
        intent: ModelActivationIntent,
        request_id: u64,
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
        decision: ToolReviewDecision,
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
        level: ThinkingLevel,
    },
    ApprovalModeSelected {
        request_id: u64,
        mode: ApprovalMode,
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

    fn is_recordable(&self) -> bool {
        !matches!(
            self,
            Self::DialogResolved {
                response: DialogResponse::Secret { .. },
                ..
            }
        )
    }

    /// Encodes a client event for persistent record/replay storage.
    ///
    /// Submitted secrets are intentionally omitted; use [`Self::to_jsonl`] for
    /// the direct wire transport instead.
    pub fn to_record_jsonl(&self) -> Result<Option<String>, serde_json::Error> {
        if self.is_recordable() {
            self.to_jsonl().map(Some)
        } else {
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelActivationResult {
    pub request_id: u64,
    pub target: ModelTarget,
    pub session_activated: bool,
    pub default_update: DefaultUpdateOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Ready {
        handshake: Handshake,
    },
    StateUpdated(Box<BackendState>),
    PromptDelta {
        session_id: String,
        delta: String,
    },
    PromptAttemptReset {
        session_id: String,
        /// Nonzero prompt-wide attempt identity; clients reject zero, duplicate, or decreasing values.
        attempt_sequence: u64,
        /// Exact live UTF-8 assistant-text suffix to retract from the failed attempt.
        discarded_utf8_bytes: usize,
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
    DiscoveredModelsUpdated {
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
    ModelActivationFinished(ModelActivationResult),
    ModelSelectionRequired {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        requested: Option<ModelTargetRequest>,
        reason: ModelTargetResolutionReason,
    },
    ThinkingLevelApplied {
        level: ThinkingLevel,
    },
    ApprovalModeChanged {
        request_id: u64,
        mode: ApprovalMode,
    },
    ApprovalModeChangeFailed {
        request_id: u64,
        mode: ApprovalMode,
        message: String,
    },
    DialogRequested(DialogRequest),
    ToolReviewRequested {
        request_id: String,
        tool_name: String,
        payload: ToolReviewPayload,
    },
    ToolReviewResolved {
        request_id: String,
        resolution: ToolReviewResolution,
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

/// Why a live client rejected [`ServerEvent::PromptAttemptReset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAttemptResetError {
    ZeroSequence,
    StaleOrDecreasingSequence,
    ImpossibleByteCount,
    InvalidUtf8Boundary,
}

/// Accepts a prompt-wide attempt sequence: nonzero and strictly increasing.
pub fn accept_prompt_attempt_sequence(
    previous: Option<u64>,
    attempt_sequence: u64,
) -> Result<u64, PromptAttemptResetError> {
    if attempt_sequence == 0 {
        return Err(PromptAttemptResetError::ZeroSequence);
    }
    if previous.is_some_and(|previous| attempt_sequence <= previous) {
        return Err(PromptAttemptResetError::StaleOrDecreasingSequence);
    }
    Ok(attempt_sequence)
}

/// Truncates exactly `discarded_utf8_bytes` from a UTF-8 string suffix.
pub fn truncate_utf8_suffix(
    text: &mut String,
    discarded_utf8_bytes: usize,
) -> Result<(), PromptAttemptResetError> {
    if discarded_utf8_bytes == 0 {
        return Ok(());
    }
    if discarded_utf8_bytes > text.len() {
        return Err(PromptAttemptResetError::ImpossibleByteCount);
    }
    let keep = text.len() - discarded_utf8_bytes;
    if !text.is_char_boundary(keep) {
        return Err(PromptAttemptResetError::InvalidUtf8Boundary);
    }
    text.truncate(keep);
    Ok(())
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
            Capability::ProviderConnections,
            Capability::ToolOutputStreaming,
            Capability::StructuredReviewRows,
            Capability::ApprovalModes,
            Capability::ModelState,
            Capability::PromptAttemptReset,
        ],
    )
}

#[must_use]
pub fn default_backend_handshake() -> Handshake {
    Handshake::new(
        "yach-adapter-pi-rpc",
        vec![
            Capability::PromptStreaming,
            Capability::Dialogs,
            Capability::Notifications,
            Capability::StatusEntries,
            Capability::Widgets,
            Capability::SessionForking,
            Capability::StructuredReviewRows,
            Capability::ApprovalModes,
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
    let resolved = ServerEvent::ToolReviewResolved {
        request_id: String::from("tool-review-request-1"),
        resolution: ToolReviewResolution::Approved,
    };
    let resolution_line = resolved.to_jsonl();
    assert!(resolution_line.is_ok());
    let Ok(resolution_line) = resolution_line else {
        return;
    };
    assert!(resolution_line.contains("\"type\":\"tool_review_resolved\""));
    assert!(resolution_line.contains("\"resolution\":\"approved\""));
    let decoded_resolution = ServerEvent::from_jsonl(&resolution_line);
    assert!(decoded_resolution.is_ok());
    let Ok(decoded_resolution) = decoded_resolution else {
        return;
    };
    assert_eq!(decoded_resolution, resolved);

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
        decision: ToolReviewDecision::Approve,
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
    assert!(line.contains("\"decision\":\"approve\""));

    let command = ServerEvent::ToolReviewRequested {
        request_id: String::from("command-review-request-1"),
        tool_name: String::from("bash"),
        payload: ToolReviewPayload::Command {
            command: CommandReviewSummary {
                review_id: String::from("command-review-1"),
                permission_decision_id: String::from("permission-decision-2"),
                command: String::from("cargo test"),
                workdir: Some(String::from("/workspace")),
                timeout_ms: 30_000,
            },
        },
    };
    let line = command.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    assert_eq!(ServerEvent::from_jsonl(&line).ok(), Some(command));
    assert!(line.contains("\"kind\":\"command\""));
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
    assert!(handshake.supports(Capability::StructuredReviewRows));
    assert!(default_backend_handshake().supports(Capability::StructuredReviewRows));
}

#[cfg(test)]
#[test]
fn finished_tool_result_metadata_round_trips_as_jsonl() {
    let event = ServerEvent::ToolCallFinished(ToolResult {
        tool_call_id: Some(String::from("tool-request-1")),
        tool_name: String::from("bash"),
        output: String::from("line one\nline two\n"),
        is_error: false,
        outcome_kind: None,
        metadata: Some(ToolResultMetadata {
            byte_count: 64_000,
            truncated: true,
            reason: None,
        }),
    });
    let line = event.to_jsonl();
    assert!(line.is_ok());
    let Ok(line) = line else {
        return;
    };
    assert_eq!(ServerEvent::from_jsonl(&line).ok(), Some(event));
    assert!(line.contains("\"byte_count\":64000"));
    assert!(line.contains("\"truncated\":true"));
}

#[cfg(test)]
mod tests {
    use super::{
        ApprovalMode, BackendState, Capability, ClientEvent, DefaultModelState,
        DefaultUpdateOutcome, DialogKind, DialogResponse, Handshake, LocalEditDecision,
        LocalEditFinishedOutcome, LocalEditOperationInput, LocalEditPreviewSummary,
        LocalEditReviewState, MessageBody, MessageDirection, MessageMeta, ModelActivationIntent,
        ModelActivationResult, ModelInfo, ModelTarget, NegotiatedCapabilities, PROTOCOL_VERSION,
        PromptAttemptResetError, ServerEvent, SessionModelState, SubmittedSecret, ThinkingLevel,
        TransportMessage, accept_prompt_attempt_sequence, bounded_protocol_version,
        default_backend_handshake, default_ui_handshake, truncate_utf8_suffix,
    };
    use crate::{
        ExtensionDiagnosticRecord, ExtensionDiagnosticSnapshotOutcome, ExtensionLifecycleAction,
        ExtensionLifecycleOutcome,
    };

    #[test]
    fn protocol_version_tracks_prd_seed() {
        assert_eq!(PROTOCOL_VERSION, "0.3.0");
    }

    #[test]
    fn full_access_mode_round_trips_with_kebab_case_wire_name() {
        let event = ClientEvent::ApprovalModeSelected {
            request_id: 9,
            mode: ApprovalMode::FullAccess,
        };
        let line = event.to_jsonl();
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };
        assert!(line.contains("\"mode\":\"full-access\""));
        assert_eq!(ClientEvent::from_jsonl(&line).ok(), Some(event));
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
        assert!(handshake.supports(Capability::PromptAttemptReset));
        assert!(!default_backend_handshake().supports(Capability::PromptAttemptReset));
        assert!(
            !NegotiatedCapabilities::from_handshakes(&handshake, &default_backend_handshake(),)
                .supports(Capability::PromptAttemptReset)
        );
    }

    #[test]
    fn ui_handshake_exposes_local_edit_capability() {
        let handshake = default_ui_handshake();

        assert!(handshake.supports(Capability::LocalEdit));
    }

    #[test]
    fn backend_handshake_does_not_claim_theme_loading() {
        let handshake = default_backend_handshake();

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
            &default_backend_handshake(),
        );

        assert!(negotiation.supports(Capability::PromptStreaming));
        assert!(!negotiation.supports(Capability::ThemeLoading));
    }

    #[test]
    fn submitted_secret_debug_redacts_complete_client_event() {
        let sentinel = "task-1-secret-sentinel";
        let event = ClientEvent::DialogResolved {
            dialog_id: String::from("provider-api-key"),
            response: DialogResponse::Secret {
                value: SubmittedSecret::new(sentinel),
            },
        };

        let debug = format!("{event:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(sentinel));

        let wire = event.to_jsonl();
        assert!(wire.is_ok());
        let Ok(wire) = wire else {
            return;
        };
        assert!(wire.contains(sentinel));

        let decoded = ClientEvent::from_jsonl(&wire);
        assert!(matches!(
            &decoded,
            Ok(ClientEvent::DialogResolved {
                response: DialogResponse::Secret { .. },
                ..
            })
        ));
        let Ok(ClientEvent::DialogResolved {
            response: DialogResponse::Secret { value },
            ..
        }) = decoded
        else {
            return;
        };
        assert_eq!(value.into_inner(), sentinel);
    }

    #[test]
    fn device_code_debug_redacts_user_code() {
        let sentinel = "chatgpt-device-code-sentinel";
        let kind = DialogKind::DeviceCode {
            verification_uri: String::from("https://auth.openai.com/device"),
            user_code: String::from(sentinel),
        };

        let debug = format!("{kind:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(sentinel));
        assert!(debug.contains("https://auth.openai.com/device"));
    }

    #[test]
    fn secret_response_is_not_recordable() {
        let secret_event = ClientEvent::DialogResolved {
            dialog_id: String::from("provider-api-key"),
            response: DialogResponse::Secret {
                value: SubmittedSecret::new("task-1-secret-sentinel"),
            },
        };
        let secret_record = secret_event.to_record_jsonl();
        assert!(secret_record.is_ok());
        let Ok(secret_record) = secret_record else {
            return;
        };
        assert_eq!(secret_record, None);

        let secret_message = TransportMessage::client(
            MessageMeta::new("secret-message"),
            ClientEvent::DialogResolved {
                dialog_id: String::from("provider-api-key"),
                response: DialogResponse::Secret {
                    value: SubmittedSecret::new("task-1-secret-sentinel"),
                },
            },
        );
        let message_record = secret_message.to_record_jsonl();
        assert!(message_record.is_ok());
        let Ok(message_record) = message_record else {
            return;
        };
        assert_eq!(message_record, None);

        let ordinary_event = ClientEvent::PromptSubmitted {
            session_id: String::from("session-1"),
            prompt: String::from("record this"),
        };
        let ordinary_record = ordinary_event.to_record_jsonl();
        assert!(ordinary_record.is_ok());
        assert!(matches!(&ordinary_record, Ok(Some(_))));
        let Ok(Some(ordinary_record)) = ordinary_record else {
            return;
        };
        assert!(ordinary_record.contains("\"type\":\"prompt_submitted\""));

        let ordinary_message = TransportMessage::client(
            MessageMeta::new("ordinary-message").with_correlation_id("record-1"),
            ordinary_event,
        );
        let ordinary_message_record = ordinary_message.to_record_jsonl();
        assert!(ordinary_message_record.is_ok());
        assert!(matches!(&ordinary_message_record, Ok(Some(_))));
        let Ok(Some(ordinary_message_record)) = ordinary_message_record else {
            return;
        };
        assert!(ordinary_message_record.contains("\"direction\":\"client_to_adapter\""));
        let replayed_message = TransportMessage::from_jsonl(&ordinary_message_record);
        assert_eq!(replayed_message.ok(), Some(ordinary_message));
    }

    #[test]
    fn thinking_level_application_is_a_non_status_terminal() {
        let applied = ServerEvent::ThinkingLevelApplied {
            level: ThinkingLevel::Medium,
        };
        let wire = applied.to_jsonl();
        assert!(wire.is_ok());
        let Ok(wire) = wire else { return };
        assert_eq!(
            wire,
            "{\"type\":\"thinking_level_applied\",\"level\":\"medium\"}\n"
        );

        let decoded = ServerEvent::from_jsonl(&wire);
        assert_eq!(decoded.ok(), Some(applied));
    }

    #[test]
    fn connection_aware_model_state_round_trips() {
        let target = ModelTarget {
            provider: String::from("catalog-provider"),
            model_id: String::from("catalog-model"),
            connection_id: String::from("work-connection"),
            connection_key: Some(String::from("work")),
        };
        let state_event = ServerEvent::StateUpdated(Box::new(BackendState {
            session_model: SessionModelState::Active {
                target: target.clone(),
                display_name: Some(String::from("Catalog Model")),
            },
            default_model: DefaultModelState::Resolved {
                target: target.clone(),
            },
            session_id: Some(String::from("session-1")),
            session_file: Some(String::from("/tmp/session")),
            thinking_level: Some(ThinkingLevel::Medium),
            is_streaming: false,
            is_compacting: false,
            message_count: Some(1),
            pending_message_count: Some(0),
        }));
        let Ok(state_wire) = state_event.to_jsonl() else {
            return;
        };
        assert_eq!(ServerEvent::from_jsonl(&state_wire).ok(), Some(state_event));

        let selection = ClientEvent::ModelActivationRequested {
            target: target.clone(),
            intent: ModelActivationIntent::SessionAndDefault,
            request_id: 73,
        };
        let Ok(selection_wire) = selection.to_jsonl() else {
            return;
        };
        assert_eq!(
            ClientEvent::from_jsonl(&selection_wire).ok(),
            Some(selection)
        );

        let finished = ServerEvent::ModelActivationFinished(ModelActivationResult {
            request_id: 73,
            target,
            session_activated: true,
            default_update: DefaultUpdateOutcome::Saved,
            message: None,
        });
        let Ok(finished_wire) = finished.to_jsonl() else {
            return;
        };
        assert_eq!(ServerEvent::from_jsonl(&finished_wire).ok(), Some(finished));

        assert!(
            ClientEvent::from_jsonl(
                r#"{"type":"model_selected_detailed","provider":"legacy","model_id":"old"}"#
            )
            .is_err()
        );
        assert!(ServerEvent::from_jsonl(r#"{"type":"model_changed","model":"old"}"#).is_err());
    }

    #[test]
    fn provider_connections_capability_negotiates_only_when_both_peers_offer_it() {
        let ui = default_ui_handshake();
        let default_backend = default_backend_handshake();
        assert!(ui.supports(Capability::ProviderConnections));
        assert!(!default_backend.supports(Capability::ProviderConnections));
        assert!(
            !NegotiatedCapabilities::from_handshakes(&ui, &default_backend)
                .supports(Capability::ProviderConnections)
        );

        let backend_only = Handshake::new(
            "provider-enabled-backend",
            vec![Capability::ProviderConnections],
        );
        let ui_without_connections = Handshake::new("legacy-ui", Vec::new());
        assert!(
            !NegotiatedCapabilities::from_handshakes(&ui_without_connections, &backend_only)
                .supports(Capability::ProviderConnections)
        );

        let both = NegotiatedCapabilities::from_handshakes(&ui, &backend_only);
        assert!(both.supports(Capability::ProviderConnections));
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
            handshake: default_backend_handshake(),
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
    fn discovered_models_updated_round_trips_as_jsonl() {
        let event = ServerEvent::DiscoveredModelsUpdated {
            models: vec![ModelInfo {
                id: String::from("complete-only"),
                name: String::from("Complete Only"),
                provider: String::from("openai"),
                connection_id: Some(String::from("connection-a")),
                connection_display: Some(String::from("Connection A")),
            }],
        };

        let wire = event.to_jsonl();
        assert!(wire.is_ok());
        let Ok(wire) = wire else {
            return;
        };
        assert!(wire.contains("\"type\":\"discovered_models_updated\""));
        assert_eq!(ServerEvent::from_jsonl(&wire).ok(), Some(event));
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

    #[test]
    fn prompt_attempt_reset_round_trips_as_jsonl() {
        let event = ServerEvent::PromptAttemptReset {
            session_id: String::from("session-1"),
            attempt_sequence: 2,
            discarded_utf8_bytes: 11,
        };
        let wire = event.to_jsonl();
        assert!(wire.is_ok());
        let Ok(wire) = wire else {
            return;
        };
        assert_eq!(
            wire,
            "{\"type\":\"prompt_attempt_reset\",\"session_id\":\"session-1\",\"attempt_sequence\":2,\"discarded_utf8_bytes\":11}\n"
        );
        assert_eq!(ServerEvent::from_jsonl(&wire).ok(), Some(event));
    }

    #[test]
    fn same_version_handshake_without_reset_capability_does_not_assume_it() {
        let client = Handshake::new("external-v0.3", vec![Capability::PromptStreaming]);
        assert_eq!(client.protocol_version, PROTOCOL_VERSION);
        assert!(!client.supports(Capability::PromptAttemptReset));
        let backend = Handshake::new(
            "yach-backend",
            vec![Capability::PromptStreaming, Capability::PromptAttemptReset],
        );
        let negotiated = NegotiatedCapabilities::from_handshakes(&client, &backend);
        assert_eq!(negotiated.protocol_version, PROTOCOL_VERSION);
        assert!(!negotiated.supports(Capability::PromptAttemptReset));
    }

    #[test]
    fn future_initialize_unknown_capability_skips_closed_enum() {
        let line = r#"{"type":"initialize","protocol_version":"0.4.0","agent_name":"future","capabilities":["prompt_streaming","brand_new_cap"]}"#;
        let decoded = ClientEvent::from_jsonl(line);
        assert!(decoded.is_ok());
        let Ok(decoded) = decoded else {
            return;
        };
        assert!(matches!(
            decoded,
            ClientEvent::Initialize(handshake)
                if handshake.protocol_version == "0.4.0"
                    && handshake.agent_name == "future"
                    && handshake.capabilities.is_empty()
        ));
    }

    #[test]
    fn same_version_unknown_capability_still_fails_to_deserialize() {
        let line = format!(
            r#"{{"type":"initialize","protocol_version":"{PROTOCOL_VERSION}","agent_name":"now","capabilities":["prompt_streaming","brand_new_cap"]}}"#
        );
        assert!(ClientEvent::from_jsonl(&line).is_err());
    }

    #[test]
    fn protocol_version_mismatch_copy_is_bounded() {
        let oversized = "9".repeat(10_000);
        let line = format!(
            r#"{{"protocol_version":"{oversized}","agent_name":"flood","capabilities":["brand_new_cap"]}}"#
        );
        let decoded = serde_json::from_str::<Handshake>(&line);
        assert!(decoded.is_ok());
        let Ok(handshake) = decoded else {
            return;
        };
        assert_eq!(
            handshake.protocol_version,
            bounded_protocol_version(&oversized)
        );
        assert!(handshake.protocol_version.len() <= super::MAX_PROTOCOL_VERSION_BYTES);
        assert_ne!(handshake.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn ready_handshake_exposes_exactly_the_negotiated_set() {
        let ui = Handshake::new("ui", vec![Capability::PromptStreaming, Capability::Dialogs]);
        let backend = Handshake::new(
            "yach-native",
            vec![Capability::PromptStreaming, Capability::LocalEdit],
        );
        let negotiated = NegotiatedCapabilities::from_handshakes(&ui, &backend);
        let ready = negotiated.ready_handshake();
        assert_eq!(ready.protocol_version, PROTOCOL_VERSION);
        assert_eq!(ready.agent_name, "yach-native");
        assert_eq!(ready.capabilities, vec![Capability::PromptStreaming]);
    }

    #[test]
    fn prompt_attempt_sequence_rejects_zero_duplicate_and_decreasing() {
        assert_eq!(accept_prompt_attempt_sequence(None, 1), Ok(1));
        assert_eq!(accept_prompt_attempt_sequence(Some(1), 2), Ok(2));
        assert_eq!(
            accept_prompt_attempt_sequence(None, 0),
            Err(PromptAttemptResetError::ZeroSequence)
        );
        assert_eq!(
            accept_prompt_attempt_sequence(Some(2), 2),
            Err(PromptAttemptResetError::StaleOrDecreasingSequence)
        );
        assert_eq!(
            accept_prompt_attempt_sequence(Some(3), 2),
            Err(PromptAttemptResetError::StaleOrDecreasingSequence)
        );
    }

    #[test]
    fn truncate_utf8_suffix_is_exact_and_fail_closed() {
        let mut text = String::from("hello world");
        assert_eq!(truncate_utf8_suffix(&mut text, 6), Ok(()));
        assert_eq!(text, "hello");

        let mut cafe = String::from("café");
        assert_eq!(
            truncate_utf8_suffix(&mut cafe, 1),
            Err(PromptAttemptResetError::InvalidUtf8Boundary)
        );
        assert_eq!(cafe, "café");

        let mut short = String::from("hi");
        assert_eq!(
            truncate_utf8_suffix(&mut short, 3),
            Err(PromptAttemptResetError::ImpossibleByteCount)
        );
        assert_eq!(short, "hi");
    }
}
