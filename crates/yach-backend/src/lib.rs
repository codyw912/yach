//! Backend runner groundwork for yach.
//!
//! This crate owns backend-facing concepts that are not specific to the
//! temporary Pi RPC adapter or to the eventual native provider implementation.
//! The first slice intentionally stays small: runner extraction, session
//! persistence, and provider adapters will exercise these boundaries before
//! they split into larger APIs.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use std::collections::{BTreeSet, VecDeque};

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

/// Native resource root classes owned by yach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourceRootKind {
    /// Project-local resources rooted at the current workspace/project.
    Project,
}

/// Errors produced while resolving native resource paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourcePathError {
    RootUnavailable,
    Missing,
    EscapesRoot,
    ExpectedFile,
    ExpectedDirectory,
}

impl std::fmt::Display for NativeResourcePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RootUnavailable => "native resource root unavailable",
            Self::Missing => "native resource path missing",
            Self::EscapesRoot => "native resource path escapes root",
            Self::ExpectedFile => "native resource path is not a file",
            Self::ExpectedDirectory => "native resource path is not a directory",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NativeResourcePathError {}

/// Provider visibility policy for native resource reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourceProviderVisibility {
    Never,
}

/// Errors produced while reading native resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResourceReadError {
    Path(NativeResourcePathError),
    TooLarge { max_bytes: u64, actual_bytes: u64 },
    NotUtf8,
    Io,
}

impl From<NativeResourcePathError> for NativeResourceReadError {
    fn from(error: NativeResourcePathError) -> Self {
        Self::Path(error)
    }
}

/// Explicit read policy for backend-internal native resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeResourceReadPolicy {
    pub max_bytes: u64,
    pub provider_visibility: NativeResourceProviderVisibility,
}

impl NativeResourceReadPolicy {
    #[must_use]
    pub const fn local_only(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            provider_visibility: NativeResourceProviderVisibility::Never,
        }
    }
}

/// Text resource read through an explicit native resource policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceRead {
    pub path: PathBuf,
    pub text: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}

/// Canonicalized native resource root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceRoot {
    pub kind: NativeResourceRootKind,
    canonical_path: PathBuf,
}

impl NativeResourceRoot {
    /// Canonicalize a project root for backend-internal resource resolution.
    ///
    /// This does not make files provider-visible; it only records the root
    /// needed for later explicit, policy-bound reads.
    pub fn project(path: impl AsRef<Path>) -> Result<Self, NativeResourcePathError> {
        let canonical_path = path
            .as_ref()
            .canonicalize()
            .map_err(|_| NativeResourcePathError::RootUnavailable)?;
        if !canonical_path.is_dir() {
            return Err(NativeResourcePathError::RootUnavailable);
        }

        Ok(Self {
            kind: NativeResourceRootKind::Project,
            canonical_path,
        })
    }

    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn resolve_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let path = self.resolve_existing(relative_path)?;
        if !path.is_file() {
            return Err(NativeResourcePathError::ExpectedFile);
        }
        Ok(path)
    }

    pub fn read_text_file(
        &self,
        relative_path: impl AsRef<Path>,
        policy: NativeResourceReadPolicy,
    ) -> Result<NativeResourceRead, NativeResourceReadError> {
        let path = self.resolve_file(relative_path)?;
        let metadata = fs::metadata(&path).map_err(|_| NativeResourceReadError::Io)?;
        if metadata.len() > policy.max_bytes {
            return Err(NativeResourceReadError::TooLarge {
                max_bytes: policy.max_bytes,
                actual_bytes: metadata.len(),
            });
        }

        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|_| NativeResourceReadError::Io)?;
        let byte_count = bytes.len();
        if u64::try_from(byte_count).map_or(true, |actual| actual > policy.max_bytes) {
            return Err(NativeResourceReadError::TooLarge {
                max_bytes: policy.max_bytes,
                actual_bytes: u64::try_from(byte_count).unwrap_or(u64::MAX),
            });
        }
        let text = String::from_utf8(bytes).map_err(|_| NativeResourceReadError::NotUtf8)?;

        Ok(NativeResourceRead {
            path,
            text,
            byte_count,
            redacted: false,
            truncated: false,
            provider_visibility: policy.provider_visibility,
        })
    }

    pub fn resolve_directory(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let path = self.resolve_existing(relative_path)?;
        if !path.is_dir() {
            return Err(NativeResourcePathError::ExpectedDirectory);
        }
        Ok(path)
    }

    fn resolve_existing(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let requested = relative_path.as_ref();
        if requested.is_absolute() {
            return Err(NativeResourcePathError::EscapesRoot);
        }

        let canonical = self
            .canonical_path
            .join(requested)
            .canonicalize()
            .map_err(|_| NativeResourcePathError::Missing)?;
        if !canonical.starts_with(&self.canonical_path) {
            return Err(NativeResourcePathError::EscapesRoot);
        }
        Ok(canonical)
    }
}

/// Risk class for yach-owned native tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeToolRisk {
    FixtureSafe,
    ReadsLocalMetadata,
    ReadsLocalContent,
    MutatesLocalState,
    UsesNetwork,
    RunsProcess,
}

/// Permission state assigned after validating a native tool request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeToolPermissionState {
    Allowed,
    Denied,
    NeedsApproval,
}

/// Normalized native tool validation/permission errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeToolError {
    UnknownTool,
    MalformedArguments,
    ArgumentsTooLarge,
    MissingRequiredField { field: String },
    InvalidFieldType { field: String },
    UnexpectedField { field: String },
    PermissionDenied,
}

/// Minimal allowlisted object schema for first native tool validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolInputSchema {
    required_string_fields: BTreeSet<String>,
    optional_string_fields: BTreeSet<String>,
    max_serialized_bytes: usize,
}

impl NativeToolInputSchema {
    #[must_use]
    pub fn string_object(
        required: impl IntoIterator<Item = impl Into<String>>,
        optional: impl IntoIterator<Item = impl Into<String>>,
        max_serialized_bytes: usize,
    ) -> Self {
        Self {
            required_string_fields: required.into_iter().map(Into::into).collect(),
            optional_string_fields: optional.into_iter().map(Into::into).collect(),
            max_serialized_bytes,
        }
    }

    pub fn validate(&self, arguments: &serde_json::Value) -> Result<(), NativeToolError> {
        let serialized_len = serde_json::to_vec(arguments)
            .map_err(|_| NativeToolError::MalformedArguments)?
            .len();
        if serialized_len > self.max_serialized_bytes {
            return Err(NativeToolError::ArgumentsTooLarge);
        }

        let Some(object) = arguments.as_object() else {
            return Err(NativeToolError::MalformedArguments);
        };

        for field in &self.required_string_fields {
            let Some(value) = object.get(field) else {
                return Err(NativeToolError::MissingRequiredField {
                    field: field.clone(),
                });
            };
            if !value.is_string() {
                return Err(NativeToolError::InvalidFieldType {
                    field: field.clone(),
                });
            }
        }

        for (field, value) in object {
            if !self.required_string_fields.contains(field)
                && !self.optional_string_fields.contains(field)
            {
                return Err(NativeToolError::UnexpectedField {
                    field: field.clone(),
                });
            }
            if !value.is_string() {
                return Err(NativeToolError::InvalidFieldType {
                    field: field.clone(),
                });
            }
        }

        Ok(())
    }
}

/// Backend-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: NativeToolInputSchema,
    pub risk: NativeToolRisk,
}

impl NativeToolDefinition {
    #[must_use]
    pub fn fixture_echo_metadata() -> Self {
        Self {
            name: String::from("fixture_echo_metadata"),
            description: String::from("Fixture-safe tool that validates metadata arguments only."),
            input_schema: NativeToolInputSchema::string_object(["label"], ["note"], 1024),
            risk: NativeToolRisk::FixtureSafe,
        }
    }
}

/// Yach-owned pending native tool request derived from provider/tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNativeToolRequest {
    pub request_id: String,
    pub turn_id: NativeTurnId,
    pub tool_name: String,
    pub provider_call_id: Option<String>,
    pub arguments: serde_json::Value,
}

/// Result of validating and authorizing a pending native tool request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolValidation {
    pub request_id: String,
    pub tool_name: String,
    pub permission: NativeToolPermissionState,
}

/// Backend-internal native tool execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolExecutionResult {
    pub request_id: String,
    pub summary: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
}

/// Provider-bound yach-owned tool result after validation/execution/redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeProviderToolResult {
    pub tool_request_id: String,
    pub provider_call_id: Option<String>,
    pub status: NativeToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Session/turn context for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolContinuationContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
}

/// Limits for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeToolContinuationPolicy {
    pub max_tool_calls: usize,
    pub max_result_bytes: usize,
}

impl NativeToolContinuationPolicy {
    #[must_use]
    pub const fn fixture_default() -> Self {
        Self {
            max_tool_calls: 4,
            max_result_bytes: 256,
        }
    }
}

/// Normalized continuation-loop errors before any real provider continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolContinuationError {
    TooManyToolCalls {
        max: usize,
        actual: usize,
    },
    Validation(NativeToolError),
    Execution(NativeToolExecutionError),
    ResultTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
}

/// Normalized native tool execution errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolExecutionError {
    UnknownTool,
    PermissionDenied,
    UnsupportedTool,
}

/// Backend-internal execution boundary for yach-owned native tools.
pub trait NativeToolExecutor {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError>;
}

/// Fixture-only native tool executor used to prove the execution boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixtureNativeToolExecutor;

impl NativeToolExecutor for FixtureNativeToolExecutor {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnknownTool);
        };
        if validation.permission != NativeToolPermissionState::Allowed {
            return Err(NativeToolExecutionError::PermissionDenied);
        }
        if definition.name != "fixture_echo_metadata"
            || definition.risk != NativeToolRisk::FixtureSafe
        {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }

        let byte_count = serde_json::to_vec(&request.arguments).map_or(0, |bytes| bytes.len());
        Ok(NativeToolExecutionResult {
            request_id: request.request_id.clone(),
            summary: String::from("fixture tool executed with redacted arguments"),
            byte_count,
            redacted: true,
            truncated: false,
        })
    }
}

/// Explicit allowlist policy for first native tool slices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolPermissionPolicy {
    allowed_fixture_tools: BTreeSet<String>,
}

impl NativeToolPermissionPolicy {
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn allow_fixture_tool(name: impl Into<String>) -> Self {
        Self {
            allowed_fixture_tools: BTreeSet::from([name.into()]),
        }
    }

    #[must_use]
    pub fn authorize(&self, definition: &NativeToolDefinition) -> NativeToolPermissionState {
        if definition.risk == NativeToolRisk::FixtureSafe
            && self.allowed_fixture_tools.contains(&definition.name)
        {
            NativeToolPermissionState::Allowed
        } else {
            NativeToolPermissionState::Denied
        }
    }
}

/// Backend-owned native tool registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolRegistry {
    definitions: Vec<NativeToolDefinition>,
}

impl NativeToolRegistry {
    #[must_use]
    pub fn with_fixture_tools() -> Self {
        Self {
            definitions: vec![NativeToolDefinition::fixture_echo_metadata()],
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&NativeToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    pub fn validate_request(
        &self,
        request: &PendingNativeToolRequest,
        policy: &NativeToolPermissionPolicy,
    ) -> Result<NativeToolValidation, NativeToolError> {
        let definition = self
            .get(&request.tool_name)
            .ok_or(NativeToolError::UnknownTool)?;
        definition.input_schema.validate(&request.arguments)?;
        let permission = policy.authorize(definition);
        if permission == NativeToolPermissionState::Denied {
            return Err(NativeToolError::PermissionDenied);
        }

        Ok(NativeToolValidation {
            request_id: request.request_id.clone(),
            tool_name: request.tool_name.clone(),
            permission,
        })
    }
}

/// Build a yach-owned pending tool request from provider-emitted tool-call metadata.
#[must_use]
pub fn pending_tool_request_from_provider_call(
    request_id: impl Into<String>,
    turn_id: NativeTurnId,
    tool_call: ProviderToolCall,
) -> PendingNativeToolRequest {
    PendingNativeToolRequest {
        request_id: request_id.into(),
        turn_id,
        tool_name: tool_call.name,
        provider_call_id: Some(tool_call.call_id),
        arguments: tool_call.arguments_json,
    }
}

/// Validate a pending tool request and append provisional redacted session records.
pub fn record_native_tool_validation(
    log: &mut NativeSessionLog,
    session_id: NativeSessionId,
    request: &PendingNativeToolRequest,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
) -> Result<NativeToolValidation, NativeToolError> {
    let validation = registry.validate_request(request, policy);
    let permission = if validation.is_ok() {
        NativeToolPermissionState::Allowed
    } else {
        NativeToolPermissionState::Denied
    };
    log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: session_id.clone(),
        turn_id: request.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: request.provider_call_id.clone(),
        validation: validation.as_ref().map(|_| ()).map_err(Clone::clone),
        permission,
        argument_summary: summarize_tool_payload(&request.arguments),
    });
    if let Err(error) = &validation {
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id: request.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: match error {
                NativeToolError::PermissionDenied => NativeToolOutcome::Denied,
                _ => NativeToolOutcome::ValidationFailed,
            },
            reason: Some(native_tool_error_label(error)),
            result_summary: None,
        });
    }
    validation
}

fn summarize_tool_payload(value: &serde_json::Value) -> NativeToolPayloadSummary {
    let byte_count = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    NativeToolPayloadSummary {
        summary: String::from("tool payload redacted"),
        byte_count,
        redacted: true,
        truncated: false,
    }
}

/// Execute fixture-safe provider tool calls and return provider-bound redacted results.
pub fn build_fixture_provider_tool_results(
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    executor: &impl NativeToolExecutor,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    if tool_calls.len() > continuation_policy.max_tool_calls {
        return Err(NativeToolContinuationError::TooManyToolCalls {
            max: continuation_policy.max_tool_calls,
            actual: tool_calls.len(),
        });
    }

    let mut results = Vec::new();
    for (index, tool_call) in tool_calls.into_iter().enumerate() {
        let request = pending_tool_request_from_provider_call(
            format!("tool-request-{}", index + 1),
            context.turn_id.clone(),
            tool_call,
        );
        let validation = record_native_tool_validation(
            log,
            context.session_id.clone(),
            &request,
            registry,
            policy,
        )
        .map_err(NativeToolContinuationError::Validation)?;
        let execution = executor
            .execute(registry, &request, &validation)
            .map_err(NativeToolContinuationError::Execution)?;
        if execution.byte_count > continuation_policy.max_result_bytes {
            log.push(NativeSessionEvent::ToolExecutionFinished {
                session_id: context.session_id.clone(),
                turn_id: context.turn_id.clone(),
                tool_request_id: NativeToolRequestId(request.request_id.clone()),
                outcome: NativeToolOutcome::Failed,
                reason: Some(String::from("result_too_large")),
                result_summary: None,
            });
            return Err(NativeToolContinuationError::ResultTooLarge {
                max_bytes: continuation_policy.max_result_bytes,
                actual_bytes: execution.byte_count,
            });
        }

        let result_summary = NativeToolPayloadSummary {
            summary: execution.summary.clone(),
            byte_count: execution.byte_count,
            redacted: execution.redacted,
            truncated: execution.truncated,
        };
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(result_summary),
        });
        results.push(NativeProviderToolResult {
            tool_request_id: request.request_id,
            provider_call_id: request.provider_call_id,
            status: NativeToolOutcome::Completed,
            content: execution.summary,
            byte_count: execution.byte_count,
            redacted: execution.redacted,
            truncated: execution.truncated,
            reason: None,
        });
    }

    Ok(results)
}

fn native_tool_error_label(error: &NativeToolError) -> String {
    match error {
        NativeToolError::UnknownTool => String::from("unknown_tool"),
        NativeToolError::MalformedArguments => String::from("malformed_arguments"),
        NativeToolError::ArgumentsTooLarge => String::from("arguments_too_large"),
        NativeToolError::MissingRequiredField { .. } => String::from("missing_required_field"),
        NativeToolError::InvalidFieldType { .. } => String::from("invalid_field_type"),
        NativeToolError::UnexpectedField { .. } => String::from("unexpected_field"),
        NativeToolError::PermissionDenied => String::from("permission_denied"),
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

/// Provider/model target for a native LLM request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
}

/// Single message sent to a provider adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMessage {
    pub role: NativeRole,
    pub content: String,
}

/// Adapter-owned provider-specific options.
///
/// The common backend seam treats these as validated metadata supplied by the
/// adapter layer, not as core semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderExtension {
    pub key: String,
    pub value: serde_json::Value,
}

/// Dogfood-minimum provider request owned by yach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub messages: Vec<ProviderMessage>,
    pub extensions: Vec<ProviderExtension>,
}

/// Normalized provider error categories surfaced above adapter crates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Authentication,
    RateLimited,
    InvalidRequest,
    ContextLength,
    UnavailableModel,
    Timeout,
    Network,
    ProviderInternal,
    SafetyRefusal,
    MalformedStream,
    Backpressure,
    Cancelled,
    Unknown,
}

/// Redacted provider error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub redacted_debug: Option<String>,
}

impl ProviderError {
    #[must_use]
    pub fn fixture_failure() -> Self {
        Self {
            kind: ProviderErrorKind::ProviderInternal,
            message: String::from("native dogfood fixture provider failure"),
            redacted_debug: Some(String::from("fixture=failure")),
        }
    }

    #[must_use]
    pub fn malformed_stream(message: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::MalformedStream,
            message: message.into(),
            redacted_debug: Some(String::from("fixture=malformed_stream")),
        }
    }

    #[must_use]
    pub fn backpressure() -> Self {
        Self {
            kind: ProviderErrorKind::Backpressure,
            message: String::from("Native backend fell behind this stream."),
            redacted_debug: Some(String::from("bounded provider stream buffer full")),
        }
    }

    #[must_use]
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self {
            kind: ProviderErrorKind::Cancelled,
            message: reason.into(),
            redacted_debug: None,
        }
    }
}

/// Streaming tool-call state emitted by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderToolCall {
    /// Provider call id used to pair tool results with requests.
    pub call_id: String,
    /// Tool/function name requested by the model.
    pub name: String,
    /// Raw JSON argument payload emitted by the provider.
    pub arguments_json: serde_json::Value,
}

/// Token usage reported by a provider stream when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// Provider finish reason normalized enough for native dogfood accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFinishReason {
    Stop,
    Length,
    ToolCalls,
    Safety,
    ContentFilter,
    Unknown,
}

/// Dogfood-minimum streaming events produced by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    Started {
        turn_id: NativeTurnId,
        model: ProviderModel,
    },
    TextDelta {
        turn_id: NativeTurnId,
        delta: String,
    },
    ToolCallStarted {
        turn_id: NativeTurnId,
        call_id: String,
        name: String,
    },
    ToolCallDelta {
        turn_id: NativeTurnId,
        call_id: String,
        arguments_delta: String,
    },
    ToolCallCompleted {
        turn_id: NativeTurnId,
        tool_call: ProviderToolCall,
    },
    Completed {
        turn_id: NativeTurnId,
        finish_reason: Option<ProviderFinishReason>,
        usage: Option<ProviderUsage>,
        provider_response_id: Option<String>,
    },
    Failed {
        turn_id: NativeTurnId,
        error: ProviderError,
    },
    Cancelled {
        turn_id: NativeTurnId,
        reason: Option<String>,
    },
}

impl ProviderStreamEvent {
    #[must_use]
    pub const fn turn_id(&self) -> &NativeTurnId {
        match self {
            Self::Started { turn_id, .. }
            | Self::TextDelta { turn_id, .. }
            | Self::ToolCallStarted { turn_id, .. }
            | Self::ToolCallDelta { turn_id, .. }
            | Self::ToolCallCompleted { turn_id, .. }
            | Self::Completed { turn_id, .. }
            | Self::Failed { turn_id, .. }
            | Self::Cancelled { turn_id, .. } => turn_id,
        }
    }

    #[must_use]
    pub const fn is_lifecycle_boundary(&self) -> bool {
        matches!(
            self,
            Self::Started { .. }
                | Self::ToolCallStarted { .. }
                | Self::ToolCallCompleted { .. }
                | Self::Completed { .. }
                | Self::Failed { .. }
                | Self::Cancelled { .. }
        )
    }
}

/// Bounded fixture buffer used to make native provider-stream backpressure explicit.
#[derive(Debug, Clone)]
pub struct BoundedProviderStreamBuffer {
    capacity: usize,
    events: VecDeque<ProviderStreamEvent>,
}

impl BoundedProviderStreamBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn push(&mut self, event: ProviderStreamEvent) -> Result<(), ProviderStreamEvent> {
        if self.capacity == 0 {
            return Err(Self::backpressure_failure(event.turn_id().clone()));
        }
        if self.events.len() < self.capacity {
            self.events.push_back(event);
            return Ok(());
        }
        if self.coalesce_text_delta(&event) {
            return Ok(());
        }
        if event.is_lifecycle_boundary() && self.drop_oldest_text_delta() {
            self.events.push_back(event);
            return Ok(());
        }
        Err(Self::backpressure_failure(event.turn_id().clone()))
    }

    pub fn pop_front(&mut self) -> Option<ProviderStreamEvent> {
        self.events.pop_front()
    }

    fn coalesce_text_delta(&mut self, event: &ProviderStreamEvent) -> bool {
        let ProviderStreamEvent::TextDelta { turn_id, delta } = event else {
            return false;
        };
        let Some(ProviderStreamEvent::TextDelta {
            turn_id: existing_turn_id,
            delta: existing_delta,
        }) = self.events.back_mut()
        else {
            return false;
        };
        if existing_turn_id != turn_id {
            return false;
        }
        existing_delta.push_str(delta);
        true
    }

    fn drop_oldest_text_delta(&mut self) -> bool {
        let Some(index) = self
            .events
            .iter()
            .position(|event| matches!(event, ProviderStreamEvent::TextDelta { .. }))
        else {
            return false;
        };
        self.events.remove(index).is_some()
    }

    fn backpressure_failure(turn_id: NativeTurnId) -> ProviderStreamEvent {
        ProviderStreamEvent::Failed {
            turn_id,
            error: ProviderError::backpressure(),
        }
    }
}

/// Thin Rig mapping helpers for the first provider-library adapter spike.
pub mod rig_adapter {
    use std::error::Error;
    use std::path::PathBuf;
    use std::time::Duration;

    use futures::StreamExt;
    use rig::agent::{MultiTurnStreamItem, StreamingError};
    use rig::client::CompletionClient;
    use rig::providers::{anthropic, chatgpt, openai};
    use rig::streaming::{
        RawStreamingChoice, RawStreamingToolCall, StreamedAssistantContent, StreamingPrompt,
        ToolCallDeltaContent,
    };

    use crate::{
        NativeRole, NativeTurnId, ProviderError, ProviderErrorKind, ProviderFinishReason,
        ProviderRequest, ProviderStreamEvent, ProviderToolCall,
    };

    const SMOKE_PROMPT: &str = "Reply with exactly: yach-rig-smoke-ok";
    const EXPECTED_SMOKE_TEXT: &str = "yach-rig-smoke-ok";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigOpenAiCompatibleSmokeConfig {
        pub base_url: String,
        pub api_key: String,
        pub model: String,
        pub provider_label: String,
        pub timeout: Duration,
        pub max_tokens: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigOpenAiCompatibleSmokeReport {
        pub provider_label: String,
        pub model: String,
        pub event_count: usize,
        pub text_delta_count: usize,
        pub completed: bool,
        pub matched_expected_text: bool,
        pub response_chars: usize,
        pub provider_response_id: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct OpenAiCompatibleHttpSmokeReport {
        pub status: u16,
        pub content_type: Option<String>,
        pub matched_expected_text: bool,
        pub response_chars: usize,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigAnthropicSmokeConfig {
        pub api_key: String,
        pub model: String,
        pub timeout: Duration,
        pub max_tokens: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigChatGptSubscriptionSmokeConfig {
        pub model: String,
        pub token_dir: PathBuf,
        pub timeout: Duration,
        pub max_tokens: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum RigProviderConfig {
        Anthropic { api_key: String },
        ChatGptSubscription { token_dir: PathBuf },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigProviderAdapterConfig {
        pub provider: RigProviderConfig,
        pub timeout: Duration,
        pub max_tokens: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct RigStreamMapper {
        turn_id: NativeTurnId,
        provider_response_id: Option<String>,
    }

    impl RigStreamMapper {
        #[must_use]
        pub fn new(turn_id: NativeTurnId) -> Self {
            Self {
                turn_id,
                provider_response_id: None,
            }
        }

        #[must_use]
        pub fn provider_response_id(&self) -> Option<&str> {
            self.provider_response_id.as_deref()
        }

        pub fn map_choice<R: Clone>(
            &mut self,
            choice: RawStreamingChoice<R>,
        ) -> Option<ProviderStreamEvent> {
            match choice {
                RawStreamingChoice::Message(delta) => Some(ProviderStreamEvent::TextDelta {
                    turn_id: self.turn_id.clone(),
                    delta,
                }),
                RawStreamingChoice::ToolCall(tool_call) => {
                    Some(ProviderStreamEvent::ToolCallCompleted {
                        turn_id: self.turn_id.clone(),
                        tool_call: map_raw_tool_call(tool_call),
                    })
                }
                RawStreamingChoice::ToolCallDelta {
                    id,
                    internal_call_id,
                    content,
                } => Some(map_tool_call_delta(
                    &self.turn_id,
                    id,
                    internal_call_id,
                    content,
                )),
                RawStreamingChoice::FinalResponse(_) => Some(ProviderStreamEvent::Completed {
                    turn_id: self.turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: self.provider_response_id.clone(),
                }),
                RawStreamingChoice::MessageId(message_id) => {
                    self.provider_response_id = Some(message_id);
                    None
                }
                RawStreamingChoice::Reasoning { .. }
                | RawStreamingChoice::ReasoningDelta { .. } => None,
            }
        }
    }

    #[must_use]
    pub fn map_raw_streaming_choice<R: Clone>(
        turn_id: &NativeTurnId,
        choice: RawStreamingChoice<R>,
    ) -> Option<ProviderStreamEvent> {
        let mut mapper = RigStreamMapper::new(turn_id.clone());
        mapper.map_choice(choice)
    }

    pub async fn run_provider_request(
        config: RigProviderAdapterConfig,
        request: ProviderRequest,
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
        let prompt = prompt_from_request(&request)?;
        match config.provider {
            RigProviderConfig::Anthropic { api_key } => {
                let client = anthropic::Client::builder()
                    .api_key(&api_key)
                    .build()
                    .map_err(|error| provider_internal_error(&error))?;
                let preamble = preamble_from_request(&request);
                let agent = client
                    .agent(request.model.model.clone())
                    .preamble(&preamble)
                    .max_tokens(config.max_tokens)
                    .build();
                let stream = agent.stream_prompt(prompt).await;
                collect_rig_stream(
                    stream,
                    request.turn_id,
                    request.model.provider,
                    request.model.model,
                    config.timeout,
                )
                .await
            }
            RigProviderConfig::ChatGptSubscription { token_dir } => {
                let client = chatgpt::Client::builder()
                    .oauth()
                    .token_dir(&token_dir)
                    .build()
                    .map_err(|error| provider_internal_error(&error))?;
                let preamble = preamble_from_request(&request);
                let agent = client
                    .agent(request.model.model.clone())
                    .preamble(&preamble)
                    .max_tokens(config.max_tokens)
                    .build();
                let stream = agent.stream_prompt(prompt).await;
                collect_rig_stream(
                    stream,
                    request.turn_id,
                    request.model.provider,
                    request.model.model,
                    config.timeout,
                )
                .await
            }
        }
    }

    fn prompt_from_request(request: &ProviderRequest) -> Result<String, ProviderError> {
        let prompt = request
            .messages
            .iter()
            .filter(|message| matches!(message.role, NativeRole::User))
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if prompt.trim().is_empty() {
            Err(ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                message: String::from("Rig provider request requires at least one user message"),
                redacted_debug: None,
            })
        } else {
            Ok(prompt)
        }
    }

    fn preamble_from_request(request: &ProviderRequest) -> String {
        let preamble = request
            .messages
            .iter()
            .filter(|message| matches!(message.role, NativeRole::System))
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if preamble.trim().is_empty() {
            String::from("Follow the user instruction exactly.")
        } else {
            preamble
        }
    }

    pub async fn run_chatgpt_subscription_smoke(
        config: RigChatGptSubscriptionSmokeConfig,
    ) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
        let client = chatgpt::Client::builder()
            .oauth()
            .token_dir(&config.token_dir)
            .build()
            .map_err(|error| provider_internal_error(&error))?;
        let agent = client
            .agent(config.model.clone())
            .preamble("Follow the user instruction exactly.")
            .max_tokens(config.max_tokens)
            .build();
        let stream = agent.stream_prompt(SMOKE_PROMPT).await;
        collect_rig_smoke_stream(stream, "chatgpt-subscription", config.model, config.timeout).await
    }

    pub async fn run_anthropic_smoke(
        config: RigAnthropicSmokeConfig,
    ) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
        let client = anthropic::Client::builder()
            .api_key(&config.api_key)
            .build()
            .map_err(|error| provider_internal_error(&error))?;
        let agent = client
            .agent(config.model.clone())
            .preamble("Follow the user instruction exactly.")
            .max_tokens(config.max_tokens)
            .build();
        let stream = agent.stream_prompt(SMOKE_PROMPT).await;
        collect_rig_smoke_stream(stream, "anthropic", config.model, config.timeout).await
    }

    pub async fn run_openai_compatible_smoke(
        config: RigOpenAiCompatibleSmokeConfig,
    ) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
        let client = openai::Client::builder()
            .api_key(&config.api_key)
            .base_url(&config.base_url)
            .build()
            .map_err(|error| provider_internal_error(&error))?
            .completions_api();
        let agent = client
            .agent(config.model.clone())
            .preamble("Follow the user instruction exactly.")
            .max_tokens(config.max_tokens)
            .build();
        let stream = agent.stream_prompt(SMOKE_PROMPT).await;
        collect_rig_smoke_stream(stream, config.provider_label, config.model, config.timeout).await
    }

    async fn collect_rig_smoke_stream<R>(
        stream: rig::agent::StreamingResult<R>,
        provider_label: impl Into<String>,
        model: String,
        timeout: Duration,
    ) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError>
    where
        R: Clone,
    {
        let provider_label = provider_label.into();
        let (events, text, provider_response_id) = collect_rig_stream_text(
            stream,
            NativeTurnId(String::from("rig-smoke-turn")),
            provider_label.clone(),
            model.clone(),
            timeout,
        )
        .await?;
        let completed = events
            .iter()
            .any(|event| matches!(event, ProviderStreamEvent::Completed { .. }));
        let text_delta_count = events
            .iter()
            .filter(|event| matches!(event, ProviderStreamEvent::TextDelta { .. }))
            .count();
        Ok(RigOpenAiCompatibleSmokeReport {
            provider_label,
            model,
            event_count: events.len(),
            text_delta_count,
            completed,
            matched_expected_text: text.trim() == EXPECTED_SMOKE_TEXT
                || text.contains(EXPECTED_SMOKE_TEXT),
            response_chars: text.chars().count(),
            provider_response_id,
        })
    }

    async fn collect_rig_stream<R>(
        stream: rig::agent::StreamingResult<R>,
        turn_id: NativeTurnId,
        provider_label: String,
        model: String,
        timeout: Duration,
    ) -> Result<Vec<ProviderStreamEvent>, ProviderError>
    where
        R: Clone,
    {
        collect_rig_stream_text(stream, turn_id, provider_label, model, timeout)
            .await
            .map(|(events, _, _)| events)
    }

    async fn collect_rig_stream_text<R>(
        mut stream: rig::agent::StreamingResult<R>,
        turn_id: NativeTurnId,
        provider_label: String,
        model: String,
        timeout: Duration,
    ) -> Result<(Vec<ProviderStreamEvent>, String, Option<String>), ProviderError>
    where
        R: Clone,
    {
        let mut mapper = RigStreamMapper::new(turn_id.clone());
        let mut events = vec![ProviderStreamEvent::Started {
            turn_id,
            model: crate::ProviderModel {
                provider: provider_label,
                model,
            },
        }];
        let mut text = String::new();

        loop {
            let next = tokio::time::timeout(timeout, stream.next())
                .await
                .map_err(|_| ProviderError {
                    kind: ProviderErrorKind::Timeout,
                    message: String::from("Rig provider stream timed out"),
                    redacted_debug: Some(String::from("timeout while awaiting next stream event")),
                })?;
            let Some(item) = next else {
                break;
            };
            let item = item.map_err(|error| map_streaming_error(&error))?;
            match item {
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(delta)) => {
                    let choice = RawStreamingChoice::<()>::Message(delta.text);
                    if let Some(event) = mapper.map_choice(choice) {
                        if let ProviderStreamEvent::TextDelta { delta, .. } = &event {
                            text.push_str(delta);
                        }
                        events.push(event);
                    }
                }
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                    tool_call,
                    internal_call_id,
                }) => {
                    events.push(ProviderStreamEvent::ToolCallCompleted {
                        turn_id: mapper.turn_id.clone(),
                        tool_call: ProviderToolCall {
                            call_id: tool_call.call_id.unwrap_or(tool_call.id),
                            name: tool_call.function.name,
                            arguments_json: tool_call.function.arguments,
                        },
                    });
                    events.push(ProviderStreamEvent::Failed {
                        turn_id: mapper.turn_id.clone(),
                        error: ProviderError {
                            kind: ProviderErrorKind::InvalidRequest,
                            message: String::from("Rig smoke received an unexpected tool call"),
                            redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
                        },
                    });
                    break;
                }
                MultiTurnStreamItem::StreamAssistantItem(
                    StreamedAssistantContent::ToolCallDelta {
                        id,
                        internal_call_id,
                        content,
                    },
                ) => {
                    events.push(map_tool_call_delta(
                        &mapper.turn_id,
                        id,
                        internal_call_id,
                        content,
                    ));
                }
                MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Final(_)) => {
                    if let Some(event) = mapper.map_choice(RawStreamingChoice::FinalResponse(())) {
                        events.push(event);
                    }
                }
                MultiTurnStreamItem::FinalResponse(response) => {
                    response.response().clone_into(&mut text);
                    if let Some(event) = mapper.map_choice(RawStreamingChoice::FinalResponse(())) {
                        events.push(event);
                    }
                }
                _ => {}
            }
        }

        Ok((events, text, mapper.provider_response_id.clone()))
    }

    fn provider_internal_error(error: &impl ToString) -> ProviderError {
        ProviderError {
            kind: ProviderErrorKind::ProviderInternal,
            message: String::from("Rig smoke setup failed"),
            redacted_debug: Some(redact_secrets(&error.to_string())),
        }
    }

    fn map_streaming_error(error: &StreamingError) -> ProviderError {
        let debug = error_chain(error);
        ProviderError {
            kind: classify_provider_error_debug(&debug),
            message: String::from("Rig smoke provider call failed"),
            redacted_debug: Some(redact_secrets(&debug)),
        }
    }

    #[must_use]
    pub fn classify_provider_error_debug(debug: &str) -> ProviderErrorKind {
        let lower = debug.to_ascii_lowercase();
        if lower.contains("auth")
            || lower.contains("api key")
            || lower.contains("401")
            || lower.contains("unauthorized")
        {
            ProviderErrorKind::Authentication
        } else if lower.contains("rate") || lower.contains("429") {
            ProviderErrorKind::RateLimited
        } else if lower.contains("context") || lower.contains("token limit") {
            ProviderErrorKind::ContextLength
        } else if lower.contains("model")
            && (lower.contains("not found")
                || lower.contains("not_found")
                || lower.contains("unavailable")
                || lower.contains("does not exist")
                || lower.contains("not supported")
                || lower.contains("invalid"))
        {
            ProviderErrorKind::UnavailableModel
        } else if lower.contains("timeout") || lower.contains("timed out") {
            ProviderErrorKind::Timeout
        } else if lower.contains("network") || lower.contains("connect") {
            ProviderErrorKind::Network
        } else {
            ProviderErrorKind::ProviderInternal
        }
    }

    fn error_chain(error: &(dyn Error + 'static)) -> String {
        let mut parts = vec![error.to_string()];
        let mut source = error.source();
        while let Some(error) = source {
            parts.push(error.to_string());
            source = error.source();
        }
        parts.join("; caused_by: ")
    }

    #[must_use]
    pub fn redact_secrets(input: &str) -> String {
        input
            .split_whitespace()
            .map(|part| {
                let lower = part.to_ascii_lowercase();
                if part.starts_with("sk-")
                    || lower.contains("authorization")
                    || lower.contains("api_key")
                    || lower.contains("api-key")
                    || lower.contains("apikey")
                    || lower.contains("bearer")
                {
                    "<redacted>"
                } else {
                    part
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub async fn run_openai_compatible_http_smoke(
        config: RigOpenAiCompatibleSmokeConfig,
    ) -> Result<OpenAiCompatibleHttpSmokeReport, ProviderError> {
        let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
        let response = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| provider_internal_error(&error))?
            .post(url)
            .bearer_auth(&config.api_key)
            .json(&serde_json::json!({
                "model": config.model,
                "messages": [{"role": "user", "content": SMOKE_PROMPT}],
                "max_tokens": config.max_tokens,
                "stream": false,
            }))
            .send()
            .await
            .map_err(|error| ProviderError {
                kind: ProviderErrorKind::Network,
                message: String::from("OpenAI-compatible HTTP smoke request failed"),
                redacted_debug: Some(redact_secrets(&error_chain(&error))),
            })?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await.map_err(|error| ProviderError {
            kind: ProviderErrorKind::Network,
            message: String::from("OpenAI-compatible HTTP smoke response read failed"),
            redacted_debug: Some(redact_secrets(&error_chain(&error))),
        })?;
        if !status.is_success() {
            return Err(ProviderError {
                kind: ProviderErrorKind::ProviderInternal,
                message: format!("OpenAI-compatible HTTP smoke returned status {status}"),
                redacted_debug: Some(redact_secrets(&body)),
            });
        }
        let text = extract_chat_completion_text(&body).unwrap_or_default();
        Ok(OpenAiCompatibleHttpSmokeReport {
            status: status.as_u16(),
            content_type,
            matched_expected_text: text.trim() == EXPECTED_SMOKE_TEXT
                || text.contains(EXPECTED_SMOKE_TEXT),
            response_chars: text.chars().count(),
        })
    }

    fn extract_chat_completion_text(body: &str) -> Option<String> {
        let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
        value
            .get("choices")?
            .as_array()?
            .first()?
            .get("message")?
            .get("content")?
            .as_str()
            .map(str::to_owned)
    }

    #[must_use]
    pub fn map_raw_tool_call(tool_call: RawStreamingToolCall) -> ProviderToolCall {
        ProviderToolCall {
            call_id: tool_call.call_id.unwrap_or(tool_call.id),
            name: tool_call.name,
            arguments_json: tool_call.arguments,
        }
    }

    #[must_use]
    pub fn map_backpressure_error(turn_id: NativeTurnId) -> ProviderStreamEvent {
        ProviderStreamEvent::Failed {
            turn_id,
            error: ProviderError::backpressure(),
        }
    }

    #[must_use]
    pub fn map_cancelled(turn_id: NativeTurnId, reason: impl Into<String>) -> ProviderStreamEvent {
        ProviderStreamEvent::Cancelled {
            turn_id,
            reason: Some(reason.into()),
        }
    }

    fn map_tool_call_delta(
        turn_id: &NativeTurnId,
        id: String,
        internal_call_id: String,
        content: ToolCallDeltaContent,
    ) -> ProviderStreamEvent {
        let call_id = id.if_empty(internal_call_id);
        match content {
            ToolCallDeltaContent::Name(name) => ProviderStreamEvent::ToolCallStarted {
                turn_id: turn_id.clone(),
                call_id,
                name,
            },
            ToolCallDeltaContent::Delta(arguments_delta) => ProviderStreamEvent::ToolCallDelta {
                turn_id: turn_id.clone(),
                call_id,
                arguments_delta,
            },
        }
    }

    trait IfEmpty {
        fn if_empty(self, fallback: String) -> String;
    }

    impl IfEmpty for String {
        fn if_empty(self, fallback: String) -> String {
            if self.is_empty() { fallback } else { self }
        }
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

    use rig::streaming::{RawStreamingChoice, RawStreamingToolCall, ToolCallDeltaContent};

    use super::{
        BackendCapabilities, BackendKind, BackendMetadata, BoundedProviderStreamBuffer,
        FixtureNativeToolExecutor, NativeEntryId, NativeProviderToolResult,
        NativeResourcePathError, NativeResourceProviderVisibility, NativeResourceReadError,
        NativeResourceReadPolicy, NativeResourceRoot, NativeResourceRootKind, NativeRole,
        NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeToolContinuationContext,
        NativeToolContinuationError, NativeToolContinuationPolicy, NativeToolError,
        NativeToolExecutionError, NativeToolExecutionResult, NativeToolExecutor, NativeToolOutcome,
        NativeToolPayloadSummary, NativeToolPermissionPolicy, NativeToolPermissionState,
        NativeToolRegistry, NativeToolRequestId, NativeTurnId, NativeTurnOutcome,
        PendingNativeToolRequest, ProviderError, ProviderErrorKind, ProviderExtension,
        ProviderFinishReason, ProviderMessage, ProviderMetadata, ProviderModel, ProviderRequest,
        ProviderStreamEvent, ProviderToolCall, ProviderUsage, announce_connected, backend_channels,
        build_fixture_provider_tool_results, completed_text_exchange,
        pending_tool_request_from_provider_call, record_native_tool_validation, rig_adapter,
        start_backend_session,
    };
    use yach_proto::{BackendEvent, Capability, ClientEvent, Handshake, NegotiatedCapabilities};

    #[test]
    fn native_project_resource_root_resolves_in_root_file() {
        let root_path = temp_resource_dir("native-resource-in-root");
        let nested = root_path.join("docs");
        assert!(std::fs::create_dir_all(&nested).is_ok());
        let file = nested.join("plan.md");
        assert!(std::fs::write(&file, "plan").is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let resolved = root
            .as_ref()
            .and_then(|root| root.resolve_file("docs/plan.md").ok());
        let canonical_file = file.canonicalize().ok();

        assert_eq!(
            root.as_ref().map(|root| root.kind),
            Some(NativeResourceRootKind::Project)
        );
        assert_eq!(resolved, canonical_file);
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_rejects_parent_traversal() {
        let base_path = temp_resource_dir("native-resource-traversal");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let error = root
            .as_ref()
            .map(|root| root.resolve_file("../outside/secret.txt"));

        assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn native_project_resource_root_rejects_symlink_to_outside() {
        let root_path = temp_resource_dir("native-resource-symlink-root");
        let outside_path = temp_resource_dir("native-resource-symlink-outside");
        let outside_file = outside_path.join("secret.txt");
        assert!(std::fs::write(&outside_file, "secret").is_ok());
        assert!(std::os::unix::fs::symlink(&outside_file, root_path.join("secret-link")).is_ok());

        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let error = root.as_ref().map(|root| root.resolve_file("secret-link"));

        assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
        assert!(std::fs::remove_dir_all(outside_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_reports_missing_paths() {
        let root_path = temp_resource_dir("native-resource-missing");
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| root.resolve_file("missing.txt"));

        assert_eq!(error, Some(Err(NativeResourcePathError::Missing)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_returns_local_only_text_with_metadata() {
        let root_path = temp_resource_dir("native-resource-read");
        let file = root_path.join("note.txt");
        assert!(std::fs::write(&file, "hello").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let read = root.as_ref().and_then(|root| {
            root.read_text_file("note.txt", NativeResourceReadPolicy::local_only(16))
                .ok()
        });

        assert_eq!(read.as_ref().map(|read| read.text.as_str()), Some("hello"));
        assert_eq!(read.as_ref().map(|read| read.byte_count), Some(5));
        assert_eq!(
            read.as_ref().map(|read| read.provider_visibility),
            Some(NativeResourceProviderVisibility::Never)
        );
        assert_eq!(read.as_ref().map(|read| read.redacted), Some(false));
        assert_eq!(read.as_ref().map(|read| read.truncated), Some(false));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_enforces_size_limit() {
        let root_path = temp_resource_dir("native-resource-read-large");
        assert!(std::fs::write(root_path.join("large.txt"), "123456789").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root
            .as_ref()
            .map(|root| root.read_text_file("large.txt", NativeResourceReadPolicy::local_only(4)));

        assert_eq!(
            error,
            Some(Err(NativeResourceReadError::TooLarge {
                max_bytes: 4,
                actual_bytes: 9,
            }))
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_rejects_non_utf8() {
        let root_path = temp_resource_dir("native-resource-read-non-utf8");
        assert!(std::fs::write(root_path.join("binary.bin"), [0xff, 0xfe]).is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| {
            root.read_text_file("binary.bin", NativeResourceReadPolicy::local_only(16))
        });

        assert_eq!(error, Some(Err(NativeResourceReadError::NotUtf8)));
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn native_project_resource_read_reuses_path_policy() {
        let base_path = temp_resource_dir("native-resource-read-policy");
        let root_path = base_path.join("project");
        let outside_path = base_path.join("outside");
        assert!(std::fs::create_dir_all(&root_path).is_ok());
        assert!(std::fs::create_dir_all(&outside_path).is_ok());
        assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());

        let error = root.as_ref().map(|root| {
            root.read_text_file(
                "../outside/secret.txt",
                NativeResourceReadPolicy::local_only(16),
            )
        });

        assert_eq!(
            error,
            Some(Err(NativeResourceReadError::Path(
                NativeResourcePathError::EscapesRoot
            )))
        );
        assert!(std::fs::remove_dir_all(base_path).is_ok());
    }

    #[test]
    fn native_project_resource_root_distinguishes_files_and_directories() {
        let root_path = temp_resource_dir("native-resource-kind");
        let directory = root_path.join("directory");
        assert!(std::fs::create_dir_all(&directory).is_ok());
        let file = root_path.join("file.txt");
        assert!(std::fs::write(&file, "file").is_ok());
        let root = NativeResourceRoot::project(&root_path).ok();
        assert!(root.is_some());
        let canonical_directory = directory.canonicalize().ok();

        assert_eq!(
            root.as_ref().map(|root| root.resolve_file("directory")),
            Some(Err(NativeResourcePathError::ExpectedFile))
        );
        assert_eq!(
            root.as_ref().map(|root| root.resolve_directory("file.txt")),
            Some(Err(NativeResourcePathError::ExpectedDirectory))
        );
        assert_eq!(
            root.as_ref()
                .and_then(|root| root.resolve_directory("directory").ok()),
            canonical_directory
        );
        assert!(std::fs::remove_dir_all(root_path).is_ok());
    }

    #[test]
    fn fixture_provider_tool_results_execute_and_record_success() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
        }];

        let results = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            results,
            Ok(vec![NativeProviderToolResult {
                tool_request_id: String::from("tool-request-1"),
                provider_call_id: Some(String::from("provider-call-1")),
                status: NativeToolOutcome::Completed,
                content: String::from("fixture tool executed with redacted arguments"),
                byte_count: 24,
                redacted: true,
                truncated: false,
                reason: None,
            }])
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Completed,
                result_summary: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn fixture_provider_tool_results_stop_on_validation_failure() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"note":"missing label"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::MissingRequiredField {
                    field: String::from("label")
                }
            ))
        );
        assert_eq!(log.events.len(), 2);
    }

    #[test]
    fn fixture_provider_tool_results_stop_on_permission_denial() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"ok"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::deny_all(),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy::fixture_default(),
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::Validation(
                NativeToolError::PermissionDenied
            ))
        );
        assert_eq!(log.events.len(), 2);
    }

    #[test]
    fn fixture_provider_tool_results_enforce_result_size_limit() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
        }];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 1,
            },
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::ResultTooLarge {
                max_bytes: 1,
                actual_bytes: 24,
            })
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::Failed,
                reason: Some(reason),
                ..
            }) if reason == "result_too_large"
        ));
    }

    #[test]
    fn fixture_provider_tool_results_enforce_tool_call_limit() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let mut log = NativeSessionLog::default();
        let calls = vec![
            ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"label":"one"}),
            },
            ProviderToolCall {
                call_id: String::from("provider-call-2"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"label":"two"}),
            },
        ];

        let result = build_fixture_provider_tool_results(
            &mut log,
            &fixture_continuation_context(),
            calls,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
            &FixtureNativeToolExecutor,
            NativeToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 256,
            },
        );

        assert_eq!(
            result,
            Err(NativeToolContinuationError::TooManyToolCalls { max: 1, actual: 2 })
        );
        assert!(log.events.is_empty());
    }

    #[test]
    fn provider_tool_call_maps_to_pending_native_tool_request() {
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"ok"}),
        };

        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            tool_call,
        );

        assert_eq!(
            request,
            PendingNativeToolRequest {
                request_id: String::from("tool-request-1"),
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_name: String::from("fixture_echo_metadata"),
                provider_call_id: Some(String::from("provider-call-1")),
                arguments: serde_json::json!({"label":"ok"}),
            }
        );
    }

    #[test]
    fn provider_tool_call_validation_records_redacted_session_events() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");
        let tool_call = ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("fixture_echo_metadata"),
            arguments_json: serde_json::json!({"label":"secret-label"}),
        };
        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            tool_call,
        );
        let mut log = NativeSessionLog::default();

        let validation = record_native_tool_validation(
            &mut log,
            NativeSessionId(String::from("session-1")),
            &request,
            &registry,
            &policy,
        );

        assert!(validation.is_ok());
        assert_eq!(log.events.len(), 1);
        let path = temp_log_path("native-provider-tool-validation");
        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());
        assert!(raw.is_some_and(|raw| {
            raw.contains("tool_payload_redacted") || !raw.contains("secret-label")
        }));
    }

    #[test]
    fn provider_tool_call_validation_records_rejection_without_execution() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = pending_tool_request_from_provider_call(
            "tool-request-1",
            NativeTurnId(String::from("turn-1")),
            ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("fixture_echo_metadata"),
                arguments_json: serde_json::json!({"note":"missing label"}),
            },
        );
        let mut log = NativeSessionLog::default();

        let validation = record_native_tool_validation(
            &mut log,
            NativeSessionId(String::from("session-1")),
            &request,
            &registry,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(
            validation,
            Err(NativeToolError::MissingRequiredField {
                field: String::from("label")
            })
        );
        assert_eq!(log.events.len(), 2);
        assert!(matches!(
            log.events.last(),
            Some(NativeSessionEvent::ToolExecutionFinished {
                outcome: NativeToolOutcome::ValidationFailed,
                result_summary: None,
                ..
            })
        ));
    }

    #[test]
    fn fixture_native_tool_executor_runs_only_validated_fixture_tool() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"secret-label"}),
        );
        let validation = registry.validate_request(&request, &policy).ok();
        assert!(validation.is_some());

        let result = validation
            .as_ref()
            .map(|validation| FixtureNativeToolExecutor.execute(&registry, &request, validation));

        assert_eq!(
            result,
            Some(Ok(NativeToolExecutionResult {
                request_id: String::from("tool-request-1"),
                summary: String::from("fixture tool executed with redacted arguments"),
                byte_count: 24,
                redacted: true,
                truncated: false,
            }))
        );
    }

    #[test]
    fn fixture_native_tool_executor_rejects_unvalidated_permission() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label":"ok"}));
        let validation = super::NativeToolValidation {
            request_id: String::from("tool-request-1"),
            tool_name: String::from("fixture_echo_metadata"),
            permission: NativeToolPermissionState::Denied,
        };

        let result = FixtureNativeToolExecutor.execute(&registry, &request, &validation);

        assert_eq!(result, Err(NativeToolExecutionError::PermissionDenied));
    }

    #[test]
    fn native_tool_registry_rejects_unknown_tool() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request("missing_tool", serde_json::json!({"label":"ok"}));

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("missing_tool"),
        );

        assert_eq!(result, Err(NativeToolError::UnknownTool));
    }

    #[test]
    fn native_tool_registry_rejects_malformed_args() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!("not-object"));

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(result, Err(NativeToolError::MalformedArguments));
    }

    #[test]
    fn native_tool_registry_rejects_schema_mismatch() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let missing =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"note":"only"}));
        let wrong_type =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label": 42}));
        let unexpected = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"ok","extra":"nope"}),
        );
        let policy = NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata");

        assert_eq!(
            registry.validate_request(&missing, &policy),
            Err(NativeToolError::MissingRequiredField {
                field: String::from("label")
            })
        );
        assert_eq!(
            registry.validate_request(&wrong_type, &policy),
            Err(NativeToolError::InvalidFieldType {
                field: String::from("label")
            })
        );
        assert_eq!(
            registry.validate_request(&unexpected, &policy),
            Err(NativeToolError::UnexpectedField {
                field: String::from("extra")
            })
        );
    }

    #[test]
    fn native_tool_registry_rejects_oversized_args() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"x".repeat(2048)}),
        );

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(result, Err(NativeToolError::ArgumentsTooLarge));
    }

    #[test]
    fn native_tool_registry_denies_by_default() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request =
            fixture_tool_request("fixture_echo_metadata", serde_json::json!({"label":"ok"}));

        let result = registry.validate_request(&request, &NativeToolPermissionPolicy::deny_all());

        assert_eq!(result, Err(NativeToolError::PermissionDenied));
    }

    #[test]
    fn native_tool_registry_allows_explicit_fixture_policy() {
        let registry = NativeToolRegistry::with_fixture_tools();
        let request = fixture_tool_request(
            "fixture_echo_metadata",
            serde_json::json!({"label":"ok","note":"fixture only"}),
        );

        let result = registry.validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_fixture_tool("fixture_echo_metadata"),
        );

        assert_eq!(
            result,
            Ok(super::NativeToolValidation {
                request_id: String::from("tool-request-1"),
                tool_name: String::from("fixture_echo_metadata"),
                permission: NativeToolPermissionState::Allowed,
            })
        );
    }

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
    fn native_session_log_preserves_tool_records_jsonl() {
        let session_id = NativeSessionId(String::from("session-tools"));
        let turn_id = NativeTurnId(String::from("turn-tools"));
        let tool_request_id = NativeToolRequestId(String::from("tool-request-1"));
        let argument_summary = NativeToolPayloadSummary {
            summary: String::from("label=<redacted>"),
            byte_count: 21,
            redacted: true,
            truncated: false,
        };
        let result_summary = NativeToolPayloadSummary {
            summary: String::from("fixture metadata ok"),
            byte_count: 19,
            redacted: false,
            truncated: false,
        };
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("fixture_echo_metadata"),
            provider_call_id: Some(String::from("provider-call-1")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary,
        });
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id,
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(result_summary),
        });
        let path = temp_log_path("native-session-tool-records");

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_tool_validation_failures_without_raw_args() {
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::ToolRequestRecorded {
            session_id: NativeSessionId(String::from("session-tools")),
            turn_id: NativeTurnId(String::from("turn-tools")),
            tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
            tool_name: String::from("fixture_echo_metadata"),
            provider_call_id: Some(String::from("provider-call-1")),
            validation: Err(NativeToolError::MissingRequiredField {
                field: String::from("label"),
            }),
            permission: NativeToolPermissionState::Denied,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("validation failed before persistence"),
                byte_count: 15,
                redacted: true,
                truncated: false,
            },
        });
        let path = temp_log_path("native-session-tool-validation");

        assert!(log.write_to_file(&path).is_ok());
        let raw = std::fs::read_to_string(&path).ok();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
        assert!(raw.is_some_and(|raw| !raw.contains("raw_secret_argument")));
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
    fn provider_request_keeps_common_shape_provider_free() {
        let request = ProviderRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("openai"),
                model: String::from("gpt-test"),
            },
            messages: vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("hello"),
            }],
            extensions: vec![ProviderExtension {
                key: String::from("temperature"),
                value: serde_json::json!(0.2),
            }],
        };

        assert_eq!(request.messages.len(), 1);
        assert_eq!(
            request
                .extensions
                .first()
                .map(|extension| extension.key.as_str()),
            Some("temperature")
        );
    }

    #[test]
    fn provider_stream_events_preserve_turn_identity() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let event = ProviderStreamEvent::TextDelta {
            turn_id: turn_id.clone(),
            delta: String::from("hello"),
        };

        assert_eq!(event.turn_id(), &turn_id);
    }

    #[test]
    fn plain_streaming_text_fixture_has_ordered_lifecycle_events() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let events = [
            ProviderStreamEvent::Started {
                turn_id: turn_id.clone(),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("text-stream"),
                },
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("hel"),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn_id.clone(),
                delta: String::from("lo"),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn_id.clone(),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: Some(ProviderUsage {
                    input_tokens: Some(3),
                    output_tokens: Some(2),
                    total_tokens: Some(5),
                }),
                provider_response_id: Some(String::from("resp_fixture_1")),
            },
        ];

        assert!(events.iter().all(|event| event.turn_id() == &turn_id));
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    ProviderStreamEvent::TextDelta { delta, .. } => Some(delta.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "hello"
        );
        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::Completed { .. })
        ));
    }

    #[test]
    fn streamed_tool_call_fixture_preserves_call_id_and_json_arguments() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_call = ProviderToolCall {
            call_id: String::from("call_1"),
            name: String::from("read_file"),
            arguments_json: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let events = [
            ProviderStreamEvent::ToolCallStarted {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                name: String::from("read_file"),
            },
            ProviderStreamEvent::ToolCallDelta {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                arguments_delta: String::from("{\"path\":"),
            },
            ProviderStreamEvent::ToolCallDelta {
                turn_id: turn_id.clone(),
                call_id: String::from("call_1"),
                arguments_delta: String::from("\"Cargo.toml\"}"),
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id,
                tool_call: tool_call.clone(),
            },
        ];

        assert!(matches!(
            events.last(),
            Some(ProviderStreamEvent::ToolCallCompleted { tool_call: completed, .. })
                if completed == &tool_call
        ));
    }

    #[test]
    fn provider_stream_error_fixtures_cover_normalized_categories() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let fixtures = [
            (ProviderErrorKind::Authentication, "auth failed"),
            (ProviderErrorKind::RateLimited, "rate limited"),
            (ProviderErrorKind::InvalidRequest, "invalid request"),
            (ProviderErrorKind::ContextLength, "context length"),
            (ProviderErrorKind::UnavailableModel, "model unavailable"),
            (ProviderErrorKind::SafetyRefusal, "safety refusal"),
            (ProviderErrorKind::MalformedStream, "malformed stream"),
            (ProviderErrorKind::Backpressure, "backpressure"),
        ];

        let events = fixtures.map(|(kind, message)| ProviderStreamEvent::Failed {
            turn_id: turn_id.clone(),
            error: ProviderError {
                kind,
                message: String::from(message),
                redacted_debug: Some(String::from("authorization=<redacted>")),
            },
        });

        assert!(events.iter().all(|event| event.turn_id() == &turn_id));
        assert!(events.iter().all(|event| matches!(event, ProviderStreamEvent::Failed { error, .. } if error.redacted_debug.as_deref() == Some("authorization=<redacted>"))));
    }

    #[test]
    fn cancellation_fixture_does_not_mark_turn_completed() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let event = ProviderStreamEvent::Cancelled {
            turn_id: turn_id.clone(),
            reason: Some(String::from("ui dropped receiver")),
        };

        assert_eq!(event.turn_id(), &turn_id);
        assert!(!matches!(event, ProviderStreamEvent::Completed { .. }));
    }

    #[test]
    fn bounded_provider_stream_buffer_coalesces_text_when_full() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(1);

        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("hel"),
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id,
                    delta: String::from("lo"),
                })
                .is_ok()
        );

        assert_eq!(buffer.len(), 1);
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::TextDelta { delta, .. }) if delta == "hello"
        ));
    }

    #[test]
    fn bounded_provider_stream_buffer_preserves_lifecycle_by_dropping_text() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(2);

        assert!(
            buffer
                .push(ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("text-stream"),
                    },
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::TextDelta {
                    turn_id: turn_id.clone(),
                    delta: String::from("drop me if needed"),
                })
                .is_ok()
        );
        assert!(
            buffer
                .push(ProviderStreamEvent::Completed {
                    turn_id,
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                })
                .is_ok()
        );

        assert_eq!(buffer.len(), 2);
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::Started { .. })
        ));
        assert!(matches!(
            buffer.pop_front(),
            Some(ProviderStreamEvent::Completed { .. })
        ));
    }

    #[test]
    fn bounded_provider_stream_buffer_returns_backpressure_error_when_full() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut buffer = BoundedProviderStreamBuffer::new(1);

        assert!(
            buffer
                .push(ProviderStreamEvent::Started {
                    turn_id: turn_id.clone(),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("text-stream"),
                    },
                })
                .is_ok()
        );
        let result = buffer.push(ProviderStreamEvent::ToolCallStarted {
            turn_id,
            call_id: String::from("call-1"),
            name: String::from("read_file"),
        });

        assert!(matches!(
            result,
            Err(ProviderStreamEvent::Failed { error, .. })
                if error.message == "Native backend fell behind this stream."
        ));
    }

    #[test]
    fn rig_adapter_maps_text_and_final_stream_choices() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let text = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::Message(String::from("hello")),
        );
        let final_event =
            rig_adapter::map_raw_streaming_choice(&turn_id, RawStreamingChoice::FinalResponse(()));

        assert!(matches!(
            text,
            Some(ProviderStreamEvent::TextDelta { delta, .. }) if delta == "hello"
        ));
        assert!(matches!(
            final_event,
            Some(ProviderStreamEvent::Completed {
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
                ..
            })
        ));
    }

    #[test]
    fn rig_adapter_preserves_tool_call_identity_and_arguments() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let tool_call = RawStreamingToolCall::new(
            String::from("provider-call-1"),
            String::from("read_file"),
            serde_json::json!({ "path": "Cargo.toml" }),
        )
        .with_call_id(String::from("call-1"));

        let event = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCall(tool_call),
        );

        assert!(matches!(
            event,
            Some(ProviderStreamEvent::ToolCallCompleted { tool_call, .. })
                if tool_call.call_id == "call-1"
                    && tool_call.name == "read_file"
                    && tool_call.arguments_json == serde_json::json!({ "path": "Cargo.toml" })
        ));
    }

    #[test]
    fn rig_adapter_maps_tool_call_deltas_without_tool_execution() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let started = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Name(String::from("read_file")),
            },
        );
        let delta = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{\"path\":")),
            },
        );

        assert!(matches!(
            started,
            Some(ProviderStreamEvent::ToolCallStarted { call_id, name, .. })
                if call_id == "call-1" && name == "read_file"
        ));
        assert!(matches!(
            delta,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, arguments_delta, .. })
                if call_id == "call-1" && arguments_delta == "{\"path\":"
        ));
    }

    #[test]
    fn rig_adapter_accumulates_message_id_into_completion_metadata() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let mut mapper = rig_adapter::RigStreamMapper::new(turn_id);

        let message_id =
            mapper.map_choice::<()>(RawStreamingChoice::MessageId(String::from("msg_1")));
        let completed = mapper.map_choice(RawStreamingChoice::FinalResponse(()));

        assert!(message_id.is_none());
        assert_eq!(mapper.provider_response_id(), Some("msg_1"));
        assert!(matches!(
            completed,
            Some(ProviderStreamEvent::Completed {
                provider_response_id: Some(id),
                usage: None,
                ..
            }) if id == "msg_1"
        ));
    }

    #[test]
    fn rig_adapter_preserves_parallel_tool_call_ids() {
        let turn_id = NativeTurnId(String::from("turn-1"));
        let first = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{\"path\":")),
            },
        );
        let second = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::from("call-2"),
                internal_call_id: String::from("rig-internal-2"),
                content: ToolCallDeltaContent::Delta(String::from("{\"cmd\":")),
            },
        );

        assert!(matches!(
            first,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "call-1"
        ));
        assert!(matches!(
            second,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "call-2"
        ));
    }

    #[test]
    fn rig_adapter_uses_internal_tool_call_id_when_provider_id_is_missing() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let event = rig_adapter::map_raw_streaming_choice::<()>(
            &turn_id,
            RawStreamingChoice::ToolCallDelta {
                id: String::new(),
                internal_call_id: String::from("rig-internal-1"),
                content: ToolCallDeltaContent::Delta(String::from("{}")),
            },
        );

        assert!(matches!(
            event,
            Some(ProviderStreamEvent::ToolCallDelta { call_id, .. }) if call_id == "rig-internal-1"
        ));
    }

    #[test]
    fn rig_adapter_maps_cancellation_without_completion() {
        let turn_id = NativeTurnId(String::from("turn-1"));

        let event = rig_adapter::map_cancelled(turn_id, "stream aborted");

        assert!(matches!(
            event,
            ProviderStreamEvent::Cancelled { reason: Some(ref reason), .. } if reason == "stream aborted"
        ));
        assert!(!matches!(event, ProviderStreamEvent::Completed { .. }));
    }

    #[test]
    fn provider_errors_carry_normalized_redacted_debug_details() {
        let error = ProviderError {
            kind: ProviderErrorKind::RateLimited,
            message: String::from("Provider limit reached. Try later or switch model."),
            redacted_debug: Some(String::from("status=429 authorization=<redacted>")),
        };

        assert_eq!(error.kind, ProviderErrorKind::RateLimited);
        assert!(!error.redacted_debug.unwrap_or_default().contains("sk-"));
    }

    #[test]
    fn rig_provider_error_classification_covers_dogfood_failures() {
        assert_eq!(
            rig_adapter::classify_provider_error_debug("401 unauthorized invalid api key"),
            ProviderErrorKind::Authentication
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("not_found_error model: yach-bad-model"),
            ProviderErrorKind::UnavailableModel
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("request timed out while streaming"),
            ProviderErrorKind::Timeout
        );
        assert_eq!(
            rig_adapter::classify_provider_error_debug("network connect error"),
            ProviderErrorKind::Network
        );
    }

    #[test]
    fn rig_secret_redaction_handles_common_key_shapes() {
        let redacted = rig_adapter::redact_secrets(
            "authorization=Bearer sk-test api-key=sk-other apikey=sk-third harmless",
        );

        assert!(!redacted.contains("sk-test"));
        assert!(!redacted.contains("sk-other"));
        assert!(!redacted.contains("sk-third"));
        assert!(redacted.contains("harmless"));
    }

    #[test]
    fn fixture_error_constructors_cover_native_dogfood_failures() {
        let fixture_failure = ProviderError::fixture_failure();
        let malformed = ProviderError::malformed_stream("fixture stream ended mid-event");
        let backpressure = ProviderError::backpressure();
        let cancelled = ProviderError::cancelled("native dogfood fixture cancellation");

        assert_eq!(fixture_failure.kind, ProviderErrorKind::ProviderInternal);
        assert_eq!(malformed.kind, ProviderErrorKind::MalformedStream);
        assert_eq!(backpressure.kind, ProviderErrorKind::Backpressure);
        assert_eq!(cancelled.kind, ProviderErrorKind::Cancelled);
        assert!(cancelled.redacted_debug.is_none());
    }

    #[test]
    fn native_session_log_writes_and_reloads_jsonl() {
        let path = temp_log_path("native-session-log");
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );

        assert!(log.write_to_file(&path).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_preserves_provider_metadata_jsonl() {
        let path = temp_log_path("native-session-log-provider");
        let mut log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );
        if let Some(NativeSessionEvent::EntryAppended { provider, .. }) = log.events.get_mut(1) {
            *provider = Some(ProviderMetadata {
                provider: String::from("chatgpt-subscription"),
                model: String::from("gpt-5.3-codex-spark"),
                response_id: None,
            });
        }

        assert!(log.write_to_file(&path).is_ok());
        let persisted = std::fs::read_to_string(&path).unwrap_or_default();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert!(persisted.contains("chatgpt-subscription"));
        assert!(persisted.contains("gpt-5.3-codex-spark"));
        assert_eq!(loaded, Some(log));
    }

    #[test]
    fn native_session_log_ignores_blank_jsonl_lines() {
        let path = temp_log_path("native-session-log-blanks");
        let log = completed_text_exchange(
            NativeSessionId(String::from("session-1")),
            NativeEntryId(String::from("entry-user")),
            NativeEntryId(String::from("entry-assistant")),
            NativeTurnId(String::from("turn-1")),
            String::from("hello"),
            String::from("hi"),
        );
        let lines = log
            .events
            .iter()
            .filter_map(|event| serde_json::to_string(event).ok())
            .collect::<Vec<_>>()
            .join("\n\n");

        assert!(std::fs::write(&path, format!("\n{lines}\n\n")).is_ok());
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        assert!(std::fs::remove_file(path).is_ok());

        assert_eq!(loaded, Some(log));
    }

    fn fixture_continuation_context() -> NativeToolContinuationContext {
        NativeToolContinuationContext {
            session_id: NativeSessionId(String::from("session-1")),
            turn_id: NativeTurnId(String::from("turn-1")),
        }
    }

    fn fixture_tool_request(
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> PendingNativeToolRequest {
        PendingNativeToolRequest {
            request_id: String::from("tool-request-1"),
            turn_id: NativeTurnId(String::from("turn-1")),
            tool_name: String::from(tool_name),
            provider_call_id: Some(String::from("provider-call-1")),
            arguments,
        }
    }

    fn negotiated_prompt_streaming() -> NegotiatedCapabilities {
        let ui = Handshake::new("ui", vec![Capability::PromptStreaming]);
        let backend = Handshake::new("backend", vec![Capability::PromptStreaming]);
        NegotiatedCapabilities::from_handshakes(&ui, &backend)
    }

    fn temp_resource_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!("{name}-{unique}"));
        assert!(std::fs::create_dir_all(&path).is_ok());
        path
    }

    fn temp_log_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("{name}-{unique}.jsonl"))
    }
}
