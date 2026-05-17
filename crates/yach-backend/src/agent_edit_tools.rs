use std::path::{Component, Path};

use crate::{
    NativeEditAccess, NativeEditAccessContext, NativeEditAccessError, NativeEditAccessReviewState,
    NativeEditHunk, NativeEditOperation, NativeEditPolicy, NativeEditPreview, NativeEditPreviewId,
    NativeEditTransactionRequest, NativePermissionDecisionId, NativePermissionPolicy,
    NativeProviderToolResult, NativeResourceRoot, NativeSessionEvent, NativeSessionEventSink,
    NativeSessionId, NativeSessionLog, NativeToolContinuationError, NativeToolError,
    NativeToolExecutionError, NativeToolOutcome, NativeToolPayloadSummary,
    NativeToolPermissionState, NativeToolRegistry, NativeToolRequestId, NativeTurnId,
    PendingNativeToolRequest, native_edit_read_existing_text, native_edit_sha256_hex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentEditToolRequest {
    pub transaction: NativeEditTransactionRequest,
    pub path: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentEditToolContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub permission_policy: NativePermissionPolicy,
    pub edit_policy: NativeEditPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAgentEditToolPrepared {
    Completed(NativeProviderToolResult),
    Denied(NativeProviderToolResult),
    NeedsUserReview {
        request_id: String,
        provider_call_id: String,
        preview: NativeEditPreview,
        path: String,
        operation: String,
    },
}

#[derive(Debug)]
pub struct PendingAgentEditToolReview {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub request_id: String,
    pub provider_call_id: String,
    pub preview_id: NativeEditPreviewId,
    pub permission_decision_id: NativePermissionDecisionId,
    pub path: String,
    pub operation: String,
}

pub fn prepare_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    context: NativeAgentEditToolContext,
    request: PendingNativeToolRequest,
) -> Result<NativeAgentEditToolPrepared, NativeToolContinuationError> {
    let mut prepare_log = NativeSessionLog::default();

    if request.turn_id != context.turn_id {
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            NativeToolError::MalformedArguments,
            String::from("turn_id_mismatch"),
        )?;
        return Err(NativeToolContinuationError::Validation(
            NativeToolError::MalformedArguments,
        ));
    }

    let validation = registry.validate_request_schema_only(&request);
    if let Err(error) = validation {
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            error.clone(),
            agent_edit_tool_error_label(&error),
        )?;
        return Err(NativeToolContinuationError::Validation(error));
    }

    let Some(provider_call_id) = request
        .provider_call_id
        .clone()
        .filter(|provider_call_id| !provider_call_id.is_empty())
    else {
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            NativeToolError::MalformedArguments,
            String::from("missing_provider_call_id"),
        )?;
        return Err(NativeToolContinuationError::Validation(
            NativeToolError::MalformedArguments,
        ));
    };

    prepare_log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: Some(provider_call_id.clone()),
        validation: Ok(()),
        permission: NativeToolPermissionState::Allowed,
        argument_summary: summarize_agent_edit_payload(&request.arguments),
    });

    let normalized =
        match normalize_agent_edit_tool_request(registry, root, &request, context.edit_policy) {
            Ok(normalized) => normalized,
            Err(error) => {
                if error == NativeToolError::PermissionDenied {
                    let path = string_argument(&request, "path")
                        .unwrap_or_else(|_| String::from("unknown"));
                    let result = provider_result(
                        &request.request_id,
                        Some(provider_call_id.clone()),
                        NativeToolOutcome::Denied,
                        denied_content(&request.request_id, &request.tool_name, &path),
                        Some(String::from("permission_denied")),
                    );
                    prepare_log.push(finished_event(
                        &context,
                        &request.request_id,
                        NativeToolOutcome::Denied,
                        Some(String::from("permission_denied")),
                        Some(result_summary(&result)),
                    ));
                    append_events(sink, &prepare_log.events)?;
                    return Ok(NativeAgentEditToolPrepared::Denied(result));
                }
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    NativeToolOutcome::ValidationFailed,
                    Some(agent_edit_tool_error_label(&error)),
                    None,
                ));
                append_events(sink, &prepare_log.events)?;
                return Err(NativeToolContinuationError::Validation(error));
            }
        };
    let edit_context = NativeEditAccessContext {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        permission_policy: context.permission_policy.clone(),
        edit_policy: context.edit_policy,
        tool_request_id: Some(NativeToolRequestId(request.request_id.clone())),
    };

    let preview =
        match edit_access.prepare(root, normalized.transaction, edit_context, &mut prepare_log) {
            Ok(preview) => preview,
            Err(NativeEditAccessError::PermissionDenied { reason }) => {
                let result = provider_result(
                    &request.request_id,
                    Some(provider_call_id.clone()),
                    NativeToolOutcome::Denied,
                    denied_content(&request.request_id, &normalized.operation, &normalized.path),
                    Some(reason.clone()),
                );
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    NativeToolOutcome::Denied,
                    Some(reason),
                    Some(result_summary(&result)),
                ));
                append_events(sink, &prepare_log.events)?;
                return Ok(NativeAgentEditToolPrepared::Denied(result));
            }
            Err(error) => {
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    NativeToolOutcome::Failed,
                    Some(agent_edit_access_error_label(&error)),
                    None,
                ));
                append_events(sink, &prepare_log.events)?;
                return Err(NativeToolContinuationError::Execution(
                    NativeToolExecutionError::MalformedResult,
                ));
            }
        };

    append_events(sink, &prepare_log.events)?;

    match preview.review_state {
        NativeEditAccessReviewState::Allowed => {
            let pending = PendingAgentEditToolReview {
                session_id: context.session_id,
                turn_id: context.turn_id,
                request_id: request.request_id,
                provider_call_id,
                preview_id: preview.preview_id.clone(),
                permission_decision_id: preview.permission_decision_id.clone(),
                path: normalized.path,
                operation: normalized.operation,
            };
            let result = apply_agent_edit_tool_review(edit_access, sink, pending)?;
            Ok(NativeAgentEditToolPrepared::Completed(result))
        }
        NativeEditAccessReviewState::NeedsUserApproval
        | NativeEditAccessReviewState::AutoReviewUnavailable => {
            Ok(NativeAgentEditToolPrepared::NeedsUserReview {
                request_id: request.request_id,
                provider_call_id,
                preview,
                path: normalized.path,
                operation: normalized.operation,
            })
        }
    }
}

pub fn execute_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    context: NativeAgentEditToolContext,
    request: PendingNativeToolRequest,
) -> Result<NativeProviderToolResult, NativeToolContinuationError> {
    match prepare_agent_edit_tool_request(registry, root, edit_access, sink, context, request)? {
        NativeAgentEditToolPrepared::Completed(result)
        | NativeAgentEditToolPrepared::Denied(result) => Ok(result),
        NativeAgentEditToolPrepared::NeedsUserReview { .. } => Err(
            NativeToolContinuationError::Execution(NativeToolExecutionError::PermissionDenied),
        ),
    }
}

pub fn apply_agent_edit_tool_review(
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<NativeProviderToolResult, NativeToolContinuationError> {
    let preview_id = pending.preview_id.clone();
    let (apply_result, completed_evidence_persisted) = edit_access
        .apply_with_evidence_sink(&pending.preview_id, &pending.permission_decision_id, sink)
        .map_err(|_| {
            NativeToolContinuationError::Execution(NativeToolExecutionError::MalformedResult)
        })?;
    let mut result = provider_result(
        &pending.request_id,
        Some(pending.provider_call_id),
        NativeToolOutcome::Completed,
        applied_content(
            &pending.request_id,
            &preview_id,
            &apply_result.transaction_id.0,
            &pending.operation,
            &pending.path,
            apply_result.diff_summary_truncated,
        ),
        None,
    );
    let reason = if completed_evidence_persisted {
        None
    } else {
        Some(String::from("edit_evidence_persist_failed"))
    };
    let final_event = NativeSessionEvent::ToolExecutionFinished {
        session_id: pending.session_id,
        turn_id: pending.turn_id,
        tool_request_id: NativeToolRequestId(pending.request_id),
        outcome: NativeToolOutcome::Completed,
        reason,
        result_summary: Some(result_summary(&result)),
    };
    if append_event(sink, &final_event).is_err() {
        result.reason = Some(String::from("tool_evidence_persist_failed"));
    }
    Ok(result)
}

pub fn reject_agent_edit_tool_review(
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<NativeProviderToolResult, NativeToolContinuationError> {
    let mut log = NativeSessionLog::default();
    edit_access
        .reject(
            &pending.preview_id,
            &pending.permission_decision_id,
            &mut log,
        )
        .map_err(|_| {
            NativeToolContinuationError::Execution(NativeToolExecutionError::MalformedResult)
        })?;
    let result = provider_result(
        &pending.request_id,
        Some(pending.provider_call_id),
        NativeToolOutcome::Completed,
        rejected_content(&pending.request_id, &pending.operation, &pending.path),
        Some(String::from("user_rejected")),
    );
    log.push(NativeSessionEvent::ToolExecutionFinished {
        session_id: pending.session_id,
        turn_id: pending.turn_id,
        tool_request_id: NativeToolRequestId(pending.request_id),
        outcome: NativeToolOutcome::Completed,
        reason: Some(String::from("user_rejected")),
        result_summary: Some(result_summary(&result)),
    });
    append_events(sink, &log.events)?;
    Ok(result)
}

pub fn normalize_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
    edit_policy: NativeEditPolicy,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let definition = registry.validate_request_schema_only(request)?;

    match definition.name.as_str() {
        "edit_text_file" => normalize_edit_text_file(root, request, edit_policy),
        "create_text_file" => normalize_create_text_file(request),
        _ => Err(NativeToolError::UnknownTool),
    }
}

fn normalize_edit_text_file(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
    edit_policy: NativeEditPolicy,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let path = string_argument(request, "path")?;
    if agent_edit_metadata_path_denied(&path) {
        return Err(NativeToolError::PermissionDenied);
    }
    let find = string_argument(request, "find")?;
    let replace = string_argument(request, "replace")?;
    let (path, text) = native_edit_read_existing_text(root, &path, &edit_policy)
        .map_err(|_| NativeToolError::MalformedArguments)?;
    let expected_sha256 = native_edit_sha256_hex(text.as_bytes());

    Ok(NormalizedAgentEditToolRequest {
        transaction: NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: path.clone(),
                expected_sha256,
                hunks: vec![NativeEditHunk { find, replace }],
            }],
        },
        path,
        operation: String::from("edit_text_file"),
    })
}

fn normalize_create_text_file(
    request: &PendingNativeToolRequest,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let path = string_argument(request, "path")?;
    if agent_edit_metadata_path_denied(&path) {
        return Err(NativeToolError::PermissionDenied);
    }
    let content = string_argument(request, "content")?;

    Ok(NormalizedAgentEditToolRequest {
        transaction: NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: path.clone(),
                content,
            }],
        },
        path,
        operation: String::from("create_text_file"),
    })
}

fn agent_edit_metadata_path_denied(path: &str) -> bool {
    let mut components = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => components.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    components.as_slice() == [".yach", "APPEND_SYSTEM.md"]
}

fn string_argument(
    request: &PendingNativeToolRequest,
    field: &str,
) -> Result<String, NativeToolError> {
    request
        .arguments
        .as_object()
        .and_then(|arguments| arguments.get(field))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(NativeToolError::MalformedArguments)
}

fn provider_result(
    request_id: &str,
    provider_call_id: Option<String>,
    status: NativeToolOutcome,
    content: String,
    reason: Option<String>,
) -> NativeProviderToolResult {
    NativeProviderToolResult {
        tool_request_id: request_id.to_owned(),
        provider_call_id,
        status,
        byte_count: content.len(),
        content,
        redacted: true,
        truncated: false,
        reason,
    }
}

fn applied_content(
    request_id: &str,
    preview_id: &NativeEditPreviewId,
    transaction_id: &str,
    operation: &str,
    _path: &str,
    diff_summary_truncated: bool,
) -> String {
    serde_json::json!({
        "outcome": "applied",
        "tool_request_id": request_id,
        "preview_id": preview_id.0,
        "transaction_id": transaction_id,
        "operation": operation,
        "diff_summary_truncated": diff_summary_truncated,
    })
    .to_string()
}

fn rejected_content(request_id: &str, operation: &str, _path: &str) -> String {
    serde_json::json!({
        "outcome": "rejected",
        "tool_request_id": request_id,
        "operation": operation,
    })
    .to_string()
}

fn denied_content(request_id: &str, operation: &str, _path: &str) -> String {
    serde_json::json!({
        "outcome": "denied",
        "tool_request_id": request_id,
        "operation": operation,
    })
    .to_string()
}

fn finished_event(
    context: &NativeAgentEditToolContext,
    request_id: &str,
    outcome: NativeToolOutcome,
    reason: Option<String>,
    result_summary: Option<NativeToolPayloadSummary>,
) -> NativeSessionEvent {
    NativeSessionEvent::ToolExecutionFinished {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request_id.to_owned()),
        outcome,
        reason,
        result_summary,
    }
}

fn append_validation_failure(
    log: &mut NativeSessionLog,
    sink: &impl NativeSessionEventSink,
    context: &NativeAgentEditToolContext,
    request: &PendingNativeToolRequest,
    error: NativeToolError,
    reason: String,
) -> Result<(), NativeToolContinuationError> {
    log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: request.provider_call_id.clone(),
        validation: Err(error),
        permission: NativeToolPermissionState::Denied,
        argument_summary: summarize_agent_edit_payload(&request.arguments),
    });
    log.push(finished_event(
        context,
        &request.request_id,
        NativeToolOutcome::ValidationFailed,
        Some(reason),
        None,
    ));
    append_events(sink, &log.events)
}

fn summarize_agent_edit_payload(value: &serde_json::Value) -> NativeToolPayloadSummary {
    let byte_count = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    NativeToolPayloadSummary {
        summary: String::from("tool payload redacted"),
        byte_count,
        redacted: true,
        truncated: false,
    }
}

fn result_summary(result: &NativeProviderToolResult) -> NativeToolPayloadSummary {
    NativeToolPayloadSummary {
        summary: String::from("agent edit result redacted"),
        byte_count: result.byte_count,
        redacted: true,
        truncated: false,
    }
}

fn append_event(
    sink: &impl NativeSessionEventSink,
    event: &NativeSessionEvent,
) -> Result<(), NativeToolContinuationError> {
    sink.append_event(event).map_err(|_| {
        NativeToolContinuationError::Execution(NativeToolExecutionError::MalformedResult)
    })
}

fn append_events(
    sink: &impl NativeSessionEventSink,
    events: &[NativeSessionEvent],
) -> Result<(), NativeToolContinuationError> {
    sink.append_events(events).map_err(|_| {
        NativeToolContinuationError::Execution(NativeToolExecutionError::MalformedResult)
    })
}

fn agent_edit_access_error_label(error: &NativeEditAccessError) -> String {
    match error {
        NativeEditAccessError::PermissionDenied { .. } => String::from("permission_denied"),
        NativeEditAccessError::Preview(_) => String::from("preview_failed"),
        NativeEditAccessError::Apply(_) => String::from("apply_failed"),
        NativeEditAccessError::PreviewNotFound => String::from("preview_not_found"),
        NativeEditAccessError::DecisionMismatch => String::from("decision_mismatch"),
        NativeEditAccessError::EvidencePersistFailed => String::from("evidence_persist_failed"),
    }
}

fn agent_edit_tool_error_label(error: &NativeToolError) -> String {
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
