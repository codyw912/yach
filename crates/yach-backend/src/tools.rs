use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    NativeResourcePathError, NativeResourceRoot, NativeSessionEvent, NativeSessionId,
    NativeSessionLog, NativeToolOutcome, NativeToolPayloadSummary, NativeToolRequestId,
    NativeTurnId, ProviderExtension, ProviderMessage, ProviderModel, ProviderToolCall,
};

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

/// Ownership boundary for a yach-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolOwner {
    BuiltIn,
    Extension { extension_id: String },
}

/// Whether a native tool may be advertised to model providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderToolVisibility {
    Hidden,
    Visible,
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
    pub owner: NativeToolOwner,
    pub provider_visibility: ProviderToolVisibility,
}

impl NativeToolDefinition {
    #[must_use]
    pub fn fixture_echo_metadata() -> Self {
        Self {
            name: String::from("fixture_echo_metadata"),
            description: String::from("Fixture-safe tool that validates metadata arguments only."),
            input_schema: NativeToolInputSchema::string_object(["label"], ["note"], 1024),
            risk: NativeToolRisk::FixtureSafe,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Hidden,
        }
    }

    #[must_use]
    pub fn project_path_info() -> Self {
        Self {
            name: String::from("project_path_info"),
            description: String::from(
                "Return local-only project path metadata without reading file contents.",
            ),
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn extension_metadata_tool(
        extension_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: NativeToolInputSchema,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::Extension {
                extension_id: extension_id.into(),
            },
            provider_visibility,
        }
    }
}

pub const PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY: &str = "yach.provider_tool_advertising.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAdvertisedToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolAdvertising {
    pub tools: Vec<ProviderAdvertisedToolSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderToolAdvertisingError {
    Malformed,
    EmptyTools,
    DuplicateExtension,
    DuplicateToolName { name: String },
    UnsupportedTool { name: String },
    UnsupportedRisk { name: String, risk: NativeToolRisk },
    UnsupportedSchema { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolRegistrationError {
    DuplicateToolName { name: String },
    UnsupportedOwner { name: String },
    UnsupportedRisk { name: String, risk: NativeToolRisk },
}

pub fn build_provider_tool_advertising_extension(
    tools: &[NativeToolDefinition],
) -> Result<ProviderExtension, ProviderToolAdvertisingError> {
    if tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }

    let mut names = BTreeSet::new();
    let mut advertised_tools = Vec::with_capacity(tools.len());
    for tool in tools {
        validate_unique_tool_name(&mut names, &tool.name)?;
        advertised_tools.push(project_provider_advertised_tool(tool)?);
    }

    let advertising = ProviderToolAdvertising {
        tools: advertised_tools,
    };
    let value =
        serde_json::to_value(advertising).map_err(|_| ProviderToolAdvertisingError::Malformed)?;
    Ok(ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value,
    })
}

pub fn build_project_path_info_provider_tool_advertising_extension()
-> Result<ProviderExtension, ProviderToolAdvertisingError> {
    build_provider_tool_advertising_extension(&[NativeToolDefinition::project_path_info()])
}

pub fn parse_provider_tool_advertising_extensions(
    extensions: &[ProviderExtension],
) -> Result<Option<ProviderToolAdvertising>, ProviderToolAdvertisingError> {
    let mut parsed = None;
    for extension in extensions {
        if extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY {
            continue;
        }
        if parsed.is_some() {
            return Err(ProviderToolAdvertisingError::DuplicateExtension);
        }
        let advertising =
            serde_json::from_value::<ProviderToolAdvertising>(extension.value.clone())
                .map_err(|_| ProviderToolAdvertisingError::Malformed)?;
        validate_provider_tool_advertising(&advertising)?;
        parsed = Some(advertising);
    }

    Ok(parsed)
}

#[must_use]
pub fn strip_provider_tool_advertising_extensions(
    extensions: Vec<ProviderExtension>,
) -> Vec<ProviderExtension> {
    extensions
        .into_iter()
        .filter(|extension| extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        .collect()
}

fn validate_provider_tool_advertising(
    advertising: &ProviderToolAdvertising,
) -> Result<(), ProviderToolAdvertisingError> {
    if advertising.tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }

    let mut names = BTreeSet::new();
    for tool in &advertising.tools {
        validate_unique_tool_name(&mut names, &tool.name)?;
        validate_provider_advertised_tool_schema(tool)?;
    }

    Ok(())
}

fn validate_unique_tool_name(
    names: &mut BTreeSet<String>,
    name: &str,
) -> Result<(), ProviderToolAdvertisingError> {
    if !names.insert(String::from(name)) {
        return Err(ProviderToolAdvertisingError::DuplicateToolName {
            name: String::from(name),
        });
    }
    Ok(())
}

fn project_provider_advertised_tool(
    tool: &NativeToolDefinition,
) -> Result<ProviderAdvertisedToolSchema, ProviderToolAdvertisingError> {
    if let NativeToolOwner::Extension { .. } = &tool.owner {
        if tool.risk != NativeToolRisk::ReadsLocalMetadata {
            return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                name: tool.name.clone(),
                risk: tool.risk,
            });
        }

        let canonical = NativeToolDefinition::extension_metadata_tool(
            "",
            tool.name.clone(),
            tool.description.clone(),
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            tool.provider_visibility,
        );
        if tool.input_schema != canonical.input_schema {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        }

        return Ok(ProviderAdvertisedToolSchema {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: extension_metadata_tool_parameters(),
        });
    }

    if tool.name != "project_path_info" {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }
    if tool.risk != NativeToolRisk::ReadsLocalMetadata {
        return Err(ProviderToolAdvertisingError::UnsupportedRisk {
            name: tool.name.clone(),
            risk: tool.risk,
        });
    }

    let canonical = NativeToolDefinition::project_path_info();
    if tool.input_schema != canonical.input_schema || tool.description != canonical.description {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    Ok(canonical_project_path_info_advertised_tool())
}

fn is_provider_advertising_routable(tool: &NativeToolDefinition) -> bool {
    project_provider_advertised_tool(tool).is_ok()
}

fn validate_provider_advertised_tool_schema(
    tool: &ProviderAdvertisedToolSchema,
) -> Result<(), ProviderToolAdvertisingError> {
    if tool.name != "project_path_info" {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }
    if tool != &canonical_project_path_info_advertised_tool() {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    Ok(())
}

fn canonical_project_path_info_advertised_tool() -> ProviderAdvertisedToolSchema {
    let definition = NativeToolDefinition::project_path_info();
    ProviderAdvertisedToolSchema {
        name: definition.name,
        description: definition.description,
        parameters: canonical_project_path_info_parameters(),
    }
}

fn canonical_project_path_info_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "description": "Project-relative path to inspect."
            }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

fn extension_metadata_tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "label": {
                "type": "string"
            }
        },
        "required": ["label"],
        "additionalProperties": false
    })
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

/// Backend-owned request for a provider continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationRequest {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<NativeProviderToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-independent adapter submission for a validated continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationSubmission {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<ProviderContinuationToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-bound tool result normalized for adapter continuation mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationToolResult {
    pub tool_request_id: String,
    pub provider_call_id: String,
    pub status: NativeToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Adapter-independent provider continuation validation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContinuationValidationPolicy {
    pub require_provider_call_id: bool,
    pub max_result_content_bytes: usize,
    pub allow_redacted_results: bool,
    pub allow_truncated_results: bool,
}

impl ProviderContinuationValidationPolicy {
    #[must_use]
    pub const fn strict_tool_results(max_result_content_bytes: usize) -> Self {
        Self {
            require_provider_call_id: true,
            max_result_content_bytes,
            allow_redacted_results: true,
            allow_truncated_results: false,
        }
    }
}

/// Adapter-independent provider continuation validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderContinuationValidationError {
    MissingProviderCallId {
        tool_request_id: String,
    },
    ResultContentTooLarge {
        tool_request_id: String,
        max_bytes: usize,
        actual_bytes: usize,
    },
    RedactedResultRejected {
        tool_request_id: String,
    },
    TruncatedResultRejected {
        tool_request_id: String,
    },
}

/// Fail-closed errors while preparing adapter continuation input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderContinuationMappingError {
    Validation(ProviderContinuationValidationError),
    EmptyToolResults,
    UnsupportedToolResultStatus {
        tool_request_id: String,
        status: NativeToolOutcome,
    },
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
        tool_call_id: String,
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
    MalformedResult,
    ResourcePath { error: NativeResourcePathError },
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

/// Deep workflow for provider tool-call validation, execution, recording, and result building.
pub struct NativeToolContinuationWorkflow<'a, Executor>
where
    Executor: NativeToolExecutor,
{
    pub registry: &'a NativeToolRegistry,
    pub permission_policy: &'a NativeToolPermissionPolicy,
    pub executor: &'a Executor,
    pub continuation_policy: NativeToolContinuationPolicy,
}

impl<Executor> NativeToolContinuationWorkflow<'_, Executor>
where
    Executor: NativeToolExecutor,
{
    pub fn build_provider_tool_results(
        &self,
        log: &mut NativeSessionLog,
        context: &NativeToolContinuationContext,
        tool_calls: Vec<ProviderToolCall>,
    ) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
        if tool_calls.len() > self.continuation_policy.max_tool_calls {
            return Err(NativeToolContinuationError::TooManyToolCalls {
                max: self.continuation_policy.max_tool_calls,
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
                self.registry,
                self.permission_policy,
            )
            .map_err(NativeToolContinuationError::Validation)?;
            let execution = match self.executor.execute(self.registry, &request, &validation) {
                Ok(execution) => execution,
                Err(error) => {
                    log.push(NativeSessionEvent::ToolExecutionFinished {
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        tool_request_id: NativeToolRequestId(request.request_id.clone()),
                        outcome: NativeToolOutcome::Failed,
                        reason: Some(native_tool_execution_error_label(&error).to_string()),
                        result_summary: None,
                    });
                    return Err(NativeToolContinuationError::Execution(error));
                }
            };
            if execution.byte_count > self.continuation_policy.max_result_bytes {
                log.push(NativeSessionEvent::ToolExecutionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: NativeToolRequestId(request.request_id.clone()),
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(String::from("result_too_large")),
                    result_summary: None,
                });
                return Err(NativeToolContinuationError::ResultTooLarge {
                    tool_call_id: request
                        .provider_call_id
                        .clone()
                        .unwrap_or_else(|| request.request_id.clone()),
                    max_bytes: self.continuation_policy.max_result_bytes,
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

/// Read-only project tool executor for local metadata-only tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReadOnlyToolExecutor {
    root: NativeResourceRoot,
}

impl ProjectReadOnlyToolExecutor {
    #[must_use]
    pub fn new(root: NativeResourceRoot) -> Self {
        Self { root }
    }
}

impl NativeToolExecutor for ProjectReadOnlyToolExecutor {
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
        if definition.name != "project_path_info"
            || definition.risk != NativeToolRisk::ReadsLocalMetadata
        {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }

        let Some(path) = request
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
        else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        let metadata = self
            .root
            .path_metadata(path)
            .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
        let summary = serde_json::json!({
            "relative_path": metadata.relative_path,
            "kind": match metadata.kind {
                crate::NativeResourceEntryKind::File => "file",
                crate::NativeResourceEntryKind::Directory => "directory",
                crate::NativeResourceEntryKind::Other => "other",
            },
            "byte_size": metadata.byte_size,
            "provider_visibility": "never",
        })
        .to_string();
        Ok(NativeToolExecutionResult {
            request_id: request.request_id.clone(),
            byte_count: summary.len(),
            summary,
            redacted: false,
            truncated: false,
        })
    }
}

/// In-memory extension tool handler used by the first routing slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolHandler {
    response: String,
    malformed: bool,
}

impl ExtensionToolHandler {
    #[must_use]
    pub fn static_metadata(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            malformed: false,
        }
    }

    #[must_use]
    pub fn malformed_result() -> Self {
        Self {
            response: String::new(),
            malformed: true,
        }
    }
}

/// In-memory extension-owned native tool executor router.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionToolExecutorRouter {
    handlers: BTreeMap<String, ExtensionToolHandler>,
}

impl ExtensionToolExecutorRouter {
    #[must_use]
    pub fn from_handlers(
        handlers: impl IntoIterator<Item = (String, ExtensionToolHandler)>,
    ) -> Self {
        Self {
            handlers: handlers.into_iter().collect(),
        }
    }
}

impl NativeToolExecutor for ExtensionToolExecutorRouter {
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
        if !matches!(&definition.owner, NativeToolOwner::Extension { .. }) {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }
        let Some(handler) = self.handlers.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        if handler.malformed {
            return Err(NativeToolExecutionError::MalformedResult);
        }

        Ok(NativeToolExecutionResult {
            request_id: request.request_id.clone(),
            byte_count: handler.response.len(),
            summary: handler.response.clone(),
            redacted: false,
            truncated: false,
        })
    }
}

/// Explicit allowlist policy for first native tool slices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolPermissionPolicy {
    allowed_fixture_tools: BTreeSet<String>,
    allowed_project_metadata_tools: BTreeSet<String>,
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
            allowed_project_metadata_tools: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn allow_project_metadata_tool(name: impl Into<String>) -> Self {
        Self::allow_project_metadata_tools([name])
    }

    #[must_use]
    pub fn allow_project_metadata_tools(
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            allowed_fixture_tools: BTreeSet::new(),
            allowed_project_metadata_tools: names.into_iter().map(Into::into).collect(),
        }
    }

    #[must_use]
    pub fn authorize(&self, definition: &NativeToolDefinition) -> NativeToolPermissionState {
        let allowed = match definition.risk {
            NativeToolRisk::FixtureSafe => self.allowed_fixture_tools.contains(&definition.name),
            NativeToolRisk::ReadsLocalMetadata => self
                .allowed_project_metadata_tools
                .contains(&definition.name),
            NativeToolRisk::ReadsLocalContent
            | NativeToolRisk::MutatesLocalState
            | NativeToolRisk::UsesNetwork
            | NativeToolRisk::RunsProcess => false,
        };

        if allowed {
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
    pub fn with_project_read_only_tools() -> Self {
        Self {
            definitions: vec![NativeToolDefinition::project_path_info()],
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&NativeToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    pub fn register_extension_tool(
        &mut self,
        definition: NativeToolDefinition,
    ) -> Result<(), NativeToolRegistrationError> {
        if self.get(&definition.name).is_some() {
            return Err(NativeToolRegistrationError::DuplicateToolName {
                name: definition.name,
            });
        }

        if !matches!(&definition.owner, NativeToolOwner::Extension { .. }) {
            return Err(NativeToolRegistrationError::UnsupportedOwner {
                name: definition.name,
            });
        }

        if definition.risk != NativeToolRisk::ReadsLocalMetadata {
            return Err(NativeToolRegistrationError::UnsupportedRisk {
                name: definition.name,
                risk: definition.risk,
            });
        }

        self.definitions.push(definition);
        Ok(())
    }

    #[must_use]
    pub fn provider_advertising_candidates<'a>(
        &self,
        policy: &NativeToolPermissionPolicy,
        routable_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<NativeToolDefinition> {
        let routable_tools = routable_tools.into_iter().collect::<BTreeSet<_>>();
        self.definitions
            .iter()
            .filter(|definition| {
                definition.provider_visibility == ProviderToolVisibility::Visible
                    && policy.authorize(definition) == NativeToolPermissionState::Allowed
                    && routable_tools.contains(definition.name.as_str())
                    && is_provider_advertising_routable(definition)
            })
            .cloned()
            .collect()
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
pub fn validate_provider_continuation_request(
    request: &ProviderContinuationRequest,
    policy: ProviderContinuationValidationPolicy,
) -> Result<(), ProviderContinuationValidationError> {
    for result in &request.tool_results {
        if policy.require_provider_call_id && result.provider_call_id.is_none() {
            return Err(ProviderContinuationValidationError::MissingProviderCallId {
                tool_request_id: result.tool_request_id.clone(),
            });
        }
        let actual_bytes = result.content.len();
        if actual_bytes > policy.max_result_content_bytes {
            return Err(ProviderContinuationValidationError::ResultContentTooLarge {
                tool_request_id: result.tool_request_id.clone(),
                max_bytes: policy.max_result_content_bytes,
                actual_bytes,
            });
        }
        if result.redacted && !policy.allow_redacted_results {
            return Err(
                ProviderContinuationValidationError::RedactedResultRejected {
                    tool_request_id: result.tool_request_id.clone(),
                },
            );
        }
        if result.truncated && !policy.allow_truncated_results {
            return Err(
                ProviderContinuationValidationError::TruncatedResultRejected {
                    tool_request_id: result.tool_request_id.clone(),
                },
            );
        }
    }
    Ok(())
}

pub fn build_provider_continuation_submission(
    request: &ProviderContinuationRequest,
    policy: ProviderContinuationValidationPolicy,
) -> Result<ProviderContinuationSubmission, ProviderContinuationMappingError> {
    validate_provider_continuation_request(request, policy)
        .map_err(ProviderContinuationMappingError::Validation)?;
    if request.tool_results.is_empty() {
        return Err(ProviderContinuationMappingError::EmptyToolResults);
    }

    let mut tool_results = Vec::with_capacity(request.tool_results.len());
    for result in &request.tool_results {
        if result.status != NativeToolOutcome::Completed {
            return Err(
                ProviderContinuationMappingError::UnsupportedToolResultStatus {
                    tool_request_id: result.tool_request_id.clone(),
                    status: result.status,
                },
            );
        }
        let Some(provider_call_id) = result.provider_call_id.clone() else {
            return Err(ProviderContinuationMappingError::Validation(
                ProviderContinuationValidationError::MissingProviderCallId {
                    tool_request_id: result.tool_request_id.clone(),
                },
            ));
        };
        tool_results.push(ProviderContinuationToolResult {
            tool_request_id: result.tool_request_id.clone(),
            provider_call_id,
            status: result.status,
            content: result.content.clone(),
            byte_count: result.byte_count,
            redacted: result.redacted,
            truncated: result.truncated,
            reason: result.reason.clone(),
        });
    }

    Ok(ProviderContinuationSubmission {
        turn_id: request.turn_id.clone(),
        model: request.model.clone(),
        prior_messages: request.prior_messages.clone(),
        tool_results,
        extensions: request.extensions.clone(),
    })
}

pub fn build_fixture_provider_tool_results(
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    executor: &impl NativeToolExecutor,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    NativeToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}

pub fn build_project_readonly_provider_tool_results(
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    project_root: NativeResourceRoot,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    let executor = ProjectReadOnlyToolExecutor::new(project_root);
    NativeToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor: &executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
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

fn native_tool_execution_error_label(error: &NativeToolExecutionError) -> &'static str {
    match error {
        NativeToolExecutionError::UnknownTool => "unknown_tool",
        NativeToolExecutionError::PermissionDenied => "permission_denied",
        NativeToolExecutionError::UnsupportedTool => "unsupported_tool",
        NativeToolExecutionError::MalformedResult => "malformed_result",
        NativeToolExecutionError::ResourcePath { error } => {
            native_resource_path_error_label(*error)
        }
    }
}

fn native_resource_path_error_label(error: NativeResourcePathError) -> &'static str {
    match error {
        NativeResourcePathError::RootUnavailable => "resource_path_root_unavailable",
        NativeResourcePathError::Missing => "resource_path_missing",
        NativeResourcePathError::EscapesRoot => "resource_path_outside_root",
        NativeResourcePathError::ExpectedFile => "resource_path_directory",
        NativeResourcePathError::ExpectedDirectory => "resource_path_not_directory",
    }
}
