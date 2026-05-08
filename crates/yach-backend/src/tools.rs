use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeToolOutcome,
    NativeToolPayloadSummary, NativeToolRequestId, NativeTurnId, ProviderExtension,
    ProviderMessage, ProviderModel, ProviderToolCall,
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

/// Backend-owned request for a provider continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationRequest {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<NativeProviderToolResult>,
    pub extensions: Vec<ProviderExtension>,
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
            let execution = self
                .executor
                .execute(self.registry, &request, &validation)
                .map_err(NativeToolContinuationError::Execution)?;
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
