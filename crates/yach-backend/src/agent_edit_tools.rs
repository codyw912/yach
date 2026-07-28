use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::{
    EditAccess, EditAccessContext, EditAccessPrepareError, EditAccessReviewState, EditError,
    EditHunk, EditOperation, EditPolicy, EditPreview, EditPreviewId, EditTraceId, EditTraceOutcome,
    EditTracePhase, EditTraceRecord, EditTraceSource, EditTransactionId, EditTransactionRequest,
    MetricAttribute, PendingToolRequest, PermissionDecisionId, PermissionPolicy,
    ProviderToolResult, ResourceRoot, SessionEvent, SessionEventSink, SessionId, SessionLog,
    ToolContinuationError, ToolError, ToolExecutionError, ToolOutcome, ToolPayloadSummary,
    ToolPermissionState, ToolRegistry, ToolRequestId, TurnId, edit_error_label,
    edit_read_existing_text, edit_sha256_hex,
};

static AGENT_EDIT_TRACE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentEditToolRequest {
    pub transaction: EditTransactionRequest,
    pub path: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEditToolContext {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub permission_policy: PermissionPolicy,
    pub edit_policy: EditPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEditToolPrepared {
    Completed {
        trace_id: EditTraceId,
        result: ProviderToolResult,
    },
    /// Preview failed for a recoverable reason (target exists, hash mismatch,
    /// missing target, ...). The result carries a failed status with
    /// actionable guidance so the provider loop can continue instead of
    /// aborting the turn.
    Failed {
        trace_id: EditTraceId,
        result: ProviderToolResult,
    },
    Denied {
        trace_id: EditTraceId,
        result: ProviderToolResult,
    },
    NeedsUserReview {
        trace_id: EditTraceId,
        request_id: String,
        provider_call_id: String,
        preview: EditPreview,
        path: String,
        operation: String,
    },
}

#[derive(Debug)]
pub struct PendingAgentEditToolReview {
    pub trace_id: EditTraceId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub request_id: String,
    pub provider_call_id: String,
    pub preview_id: EditPreviewId,
    pub permission_decision_id: PermissionDecisionId,
    pub path: String,
    pub operation: String,
}

pub fn prepare_agent_edit_tool_request(
    registry: &ToolRegistry,
    root: &ResourceRoot,
    edit_access: &mut EditAccess,
    sink: &impl SessionEventSink,
    context: AgentEditToolContext,
    request: PendingToolRequest,
) -> Result<AgentEditToolPrepared, ToolContinuationError> {
    let trace_id = next_agent_edit_trace_id();
    let mut prepare_log = SessionLog::default();
    let tool_request_id = ToolRequestId(request.request_id.clone());
    let operation = trace_operation(&request.tool_name);

    if request.turn_id != context.turn_id {
        record_agent_edit_trace(
            &mut prepare_log,
            &context,
            AgentEditTraceInput {
                trace_id: &trace_id,
                request: &request,
                phase: EditTracePhase::ToolValidation,
                outcome: EditTraceOutcome::Failed,
                started: Instant::now(),
                reason_label: Some(String::from("turn_id_mismatch")),
                preview_id: None,
                permission_decision_id: None,
                transaction_id: None,
                attributes: trace_operation_attributes(operation.as_deref()),
            },
        );
        record_result_shaping_trace(
            &mut prepare_log,
            &context,
            &trace_id,
            &request,
            EditTraceOutcome::Failed,
            Some(String::from("validation_failed")),
            operation.as_deref(),
        );
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            ToolError::MalformedArguments,
            String::from("turn_id_mismatch"),
        )?;
        return Err(ToolContinuationError::Validation(
            ToolError::MalformedArguments,
        ));
    }

    let validation = registry.validate_request_schema_only(&request);
    if let Err(error) = validation {
        record_agent_edit_trace(
            &mut prepare_log,
            &context,
            AgentEditTraceInput {
                trace_id: &trace_id,
                request: &request,
                phase: EditTracePhase::ToolValidation,
                outcome: EditTraceOutcome::Failed,
                started: Instant::now(),
                reason_label: Some(agent_edit_tool_error_label(&error)),
                preview_id: None,
                permission_decision_id: None,
                transaction_id: None,
                attributes: trace_operation_attributes(operation.as_deref()),
            },
        );
        record_result_shaping_trace(
            &mut prepare_log,
            &context,
            &trace_id,
            &request,
            EditTraceOutcome::Failed,
            Some(String::from("validation_failed")),
            operation.as_deref(),
        );
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            error.clone(),
            agent_edit_tool_error_label(&error),
        )?;
        return Err(ToolContinuationError::Validation(error));
    }

    let Some(provider_call_id) = request
        .provider_call_id
        .clone()
        .filter(|provider_call_id| !provider_call_id.is_empty())
    else {
        record_agent_edit_trace(
            &mut prepare_log,
            &context,
            AgentEditTraceInput {
                trace_id: &trace_id,
                request: &request,
                phase: EditTracePhase::ToolValidation,
                outcome: EditTraceOutcome::Failed,
                started: Instant::now(),
                reason_label: Some(String::from("missing_provider_call_id")),
                preview_id: None,
                permission_decision_id: None,
                transaction_id: None,
                attributes: trace_operation_attributes(operation.as_deref()),
            },
        );
        record_result_shaping_trace(
            &mut prepare_log,
            &context,
            &trace_id,
            &request,
            EditTraceOutcome::Failed,
            Some(String::from("validation_failed")),
            operation.as_deref(),
        );
        append_validation_failure(
            &mut prepare_log,
            sink,
            &context,
            &request,
            ToolError::MalformedArguments,
            String::from("missing_provider_call_id"),
        )?;
        return Err(ToolContinuationError::Validation(
            ToolError::MalformedArguments,
        ));
    };
    record_agent_edit_trace(
        &mut prepare_log,
        &context,
        AgentEditTraceInput {
            trace_id: &trace_id,
            request: &request,
            phase: EditTracePhase::ToolValidation,
            outcome: EditTraceOutcome::Completed,
            started: Instant::now(),
            reason_label: None,
            preview_id: None,
            permission_decision_id: None,
            transaction_id: None,
            attributes: trace_operation_attributes(operation.as_deref()),
        },
    );

    prepare_log.push(SessionEvent::ToolRequestRecorded {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: tool_request_id.clone(),
        tool_name: request.tool_name.clone(),
        provider_call_id: Some(provider_call_id.clone()),
        validation: Ok(()),
        permission: ToolPermissionState::Allowed,
        argument_summary: summarize_agent_edit_payload(&request.arguments),
        argument_content: Some(request.arguments.to_string()),
    });

    // Sensitive-path check before normalization: normalization reads the
    // target file to compute its hash, and a denied path must fail
    // recoverably instead of surfacing as a validation error.
    if sensitive_denied_request_path(root, &request) {
        let reason = String::from("sensitive_path_denied");
        let result = provider_result(
            &request.request_id,
            Some(provider_call_id.clone()),
            ToolOutcome::Failed,
            failed_content(
                &request.request_id,
                &request.tool_name,
                &reason,
                agent_edit_failure_guidance(&reason),
            ),
            Some(reason.clone()),
        );
        record_result_shaping_trace(
            &mut prepare_log,
            &context,
            &trace_id,
            &request,
            EditTraceOutcome::Failed,
            Some(reason.clone()),
            operation.as_deref(),
        );
        prepare_log.push(finished_event(
            &context,
            &request.request_id,
            ToolOutcome::Failed,
            Some(reason),
            Some(result_summary(&result)),
            Some(result.content.clone()),
        ));
        append_events(sink, &prepare_log.events)?;
        return Ok(AgentEditToolPrepared::Failed { trace_id, result });
    }

    let normalized =
        match normalize_agent_edit_tool_request(registry, root, &request, context.edit_policy) {
            Ok(normalized) => {
                record_agent_edit_trace(
                    &mut prepare_log,
                    &context,
                    AgentEditTraceInput {
                        trace_id: &trace_id,
                        request: &request,
                        phase: EditTracePhase::ArgumentNormalization,
                        outcome: EditTraceOutcome::Completed,
                        started: Instant::now(),
                        reason_label: None,
                        preview_id: None,
                        permission_decision_id: None,
                        transaction_id: None,
                        attributes: trace_operation_attributes(Some(&normalized.operation)),
                    },
                );
                normalized
            }
            Err(error) => {
                let normalization_outcome = if error == ToolError::PermissionDenied {
                    EditTraceOutcome::Denied
                } else {
                    EditTraceOutcome::Failed
                };
                record_agent_edit_trace(
                    &mut prepare_log,
                    &context,
                    AgentEditTraceInput {
                        trace_id: &trace_id,
                        request: &request,
                        phase: EditTracePhase::ArgumentNormalization,
                        outcome: normalization_outcome,
                        started: Instant::now(),
                        reason_label: Some(agent_edit_tool_error_label(&error)),
                        preview_id: None,
                        permission_decision_id: None,
                        transaction_id: None,
                        attributes: trace_operation_attributes(operation.as_deref()),
                    },
                );
                if error == ToolError::PermissionDenied {
                    let path = string_argument(&request, "path")
                        .unwrap_or_else(|_| String::from("unknown"));
                    let result = provider_result(
                        &request.request_id,
                        Some(provider_call_id.clone()),
                        ToolOutcome::Denied,
                        denied_content(&request.request_id, &request.tool_name, &path),
                        Some(String::from("permission_denied")),
                    );
                    record_result_shaping_trace(
                        &mut prepare_log,
                        &context,
                        &trace_id,
                        &request,
                        EditTraceOutcome::Denied,
                        Some(String::from("permission_denied")),
                        operation.as_deref(),
                    );
                    prepare_log.push(finished_event(
                        &context,
                        &request.request_id,
                        ToolOutcome::Denied,
                        Some(String::from("permission_denied")),
                        Some(result_summary(&result)),
                        Some(result.content.clone()),
                    ));
                    append_events(sink, &prepare_log.events)?;
                    return Ok(AgentEditToolPrepared::Denied { trace_id, result });
                }
                record_result_shaping_trace(
                    &mut prepare_log,
                    &context,
                    &trace_id,
                    &request,
                    EditTraceOutcome::Failed,
                    Some(String::from("validation_failed")),
                    operation.as_deref(),
                );
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    ToolOutcome::ValidationFailed,
                    Some(agent_edit_tool_error_label(&error)),
                    None,
                    None,
                ));
                append_events(sink, &prepare_log.events)?;
                return Err(ToolContinuationError::Validation(error));
            }
        };
    let edit_context = EditAccessContext {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        permission_policy: context.permission_policy.clone(),
        edit_policy: context.edit_policy,
        tool_request_id: Some(tool_request_id),
    };

    let prepare_started = Instant::now();
    let preview = match edit_access.prepare_with_diagnostics(
        root,
        normalized.transaction,
        edit_context,
        &mut prepare_log,
    ) {
        Ok(outcome) => {
            record_permission_decision_trace(
                &mut prepare_log,
                &context,
                PermissionDecisionTraceInput {
                    trace_id: &trace_id,
                    request: &request,
                    outcome: EditTraceOutcome::Completed,
                    started: prepare_started,
                    reason_label: None,
                    permission_decision_id: &outcome.diagnostics.permission_decision_id,
                    review_state: &outcome.diagnostics.review_state,
                    transaction_id: outcome.diagnostics.transaction_id.as_ref(),
                    operation: &normalized.operation,
                },
            );
            record_preview_trace(
                &mut prepare_log,
                &context,
                PreviewTraceInput {
                    trace_id: &trace_id,
                    request: &request,
                    outcome: EditTraceOutcome::Completed,
                    started: prepare_started,
                    reason_label: None,
                    preview_id: Some(&outcome.preview.preview_id),
                    permission_decision_id: &outcome.diagnostics.permission_decision_id,
                    transaction_id: Some(&outcome.preview.transaction_id),
                    operation: &normalized.operation,
                },
            );
            outcome.preview
        }
        Err(error) => match *error {
            EditAccessPrepareError::PermissionDenied {
                reason,
                diagnostics,
            } => {
                record_permission_decision_trace(
                    &mut prepare_log,
                    &context,
                    PermissionDecisionTraceInput {
                        trace_id: &trace_id,
                        request: &request,
                        outcome: EditTraceOutcome::Denied,
                        started: prepare_started,
                        reason_label: Some(reason.clone()),
                        permission_decision_id: &diagnostics.permission_decision_id,
                        review_state: &diagnostics.review_state,
                        transaction_id: diagnostics.transaction_id.as_ref(),
                        operation: &normalized.operation,
                    },
                );
                let result = provider_result(
                    &request.request_id,
                    Some(provider_call_id.clone()),
                    ToolOutcome::Denied,
                    denied_content(&request.request_id, &normalized.operation, &normalized.path),
                    Some(reason.clone()),
                );
                record_result_shaping_trace(
                    &mut prepare_log,
                    &context,
                    &trace_id,
                    &request,
                    EditTraceOutcome::Denied,
                    Some(reason.clone()),
                    Some(&normalized.operation),
                );
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    ToolOutcome::Denied,
                    Some(reason),
                    Some(result_summary(&result)),
                    Some(result.content.clone()),
                ));
                append_events(sink, &prepare_log.events)?;
                return Ok(AgentEditToolPrepared::Denied { trace_id, result });
            }
            EditAccessPrepareError::Preview { error, diagnostics } => {
                record_permission_decision_trace(
                    &mut prepare_log,
                    &context,
                    PermissionDecisionTraceInput {
                        trace_id: &trace_id,
                        request: &request,
                        outcome: EditTraceOutcome::Completed,
                        started: prepare_started,
                        reason_label: None,
                        permission_decision_id: &diagnostics.permission_decision_id,
                        review_state: &diagnostics.review_state,
                        transaction_id: diagnostics.transaction_id.as_ref(),
                        operation: &normalized.operation,
                    },
                );
                record_preview_trace(
                    &mut prepare_log,
                    &context,
                    PreviewTraceInput {
                        trace_id: &trace_id,
                        request: &request,
                        outcome: EditTraceOutcome::Failed,
                        started: prepare_started,
                        reason_label: diagnostics.reason_label.clone(),
                        preview_id: None,
                        permission_decision_id: &diagnostics.permission_decision_id,
                        transaction_id: None,
                        operation: &normalized.operation,
                    },
                );
                let reason = agent_edit_access_prepare_error_label(&error);
                let result = provider_result(
                    &request.request_id,
                    Some(provider_call_id.clone()),
                    ToolOutcome::Failed,
                    failed_content(
                        &request.request_id,
                        &normalized.operation,
                        &reason,
                        agent_edit_failure_guidance(&reason),
                    ),
                    Some(reason.clone()),
                );
                record_result_shaping_trace(
                    &mut prepare_log,
                    &context,
                    &trace_id,
                    &request,
                    EditTraceOutcome::Failed,
                    Some(reason.clone()),
                    Some(&normalized.operation),
                );
                prepare_log.push(finished_event(
                    &context,
                    &request.request_id,
                    ToolOutcome::Failed,
                    Some(reason),
                    Some(result_summary(&result)),
                    Some(result.content.clone()),
                ));
                append_events(sink, &prepare_log.events)?;
                return Ok(AgentEditToolPrepared::Failed { trace_id, result });
            }
        },
    };

    append_events(sink, &prepare_log.events)?;

    match preview.review_state {
        EditAccessReviewState::Allowed => {
            let pending = PendingAgentEditToolReview {
                trace_id: trace_id.clone(),
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
            Ok(AgentEditToolPrepared::Completed { trace_id, result })
        }
        EditAccessReviewState::NeedsUserApproval | EditAccessReviewState::AutoReviewUnavailable => {
            Ok(AgentEditToolPrepared::NeedsUserReview {
                trace_id,
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
    registry: &ToolRegistry,
    root: &ResourceRoot,
    edit_access: &mut EditAccess,
    sink: &impl SessionEventSink,
    context: AgentEditToolContext,
    request: PendingToolRequest,
) -> Result<ProviderToolResult, ToolContinuationError> {
    match prepare_agent_edit_tool_request(registry, root, edit_access, sink, context, request)? {
        AgentEditToolPrepared::Completed { result, .. }
        | AgentEditToolPrepared::Failed { result, .. }
        | AgentEditToolPrepared::Denied { result, .. } => Ok(result),
        AgentEditToolPrepared::NeedsUserReview { .. } => Err(ToolContinuationError::Execution(
            ToolExecutionError::PermissionDenied,
        )),
    }
}

pub fn apply_agent_edit_tool_review(
    edit_access: &mut EditAccess,
    sink: &impl SessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<ProviderToolResult, ToolContinuationError> {
    let preview_id = pending.preview_id.clone();
    let apply_started = Instant::now();
    let (apply_result, completed_evidence_persisted) = edit_access
        .apply_with_evidence_sink(&pending.preview_id, &pending.permission_decision_id, sink)
        .map_err(|_| ToolContinuationError::Execution(ToolExecutionError::MalformedResult))?;
    let mut result = provider_result(
        &pending.request_id,
        Some(pending.provider_call_id.clone()),
        ToolOutcome::Completed,
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
    let mut trace_log = SessionLog::default();
    record_review_trace(
        &mut trace_log,
        ReviewTraceInput {
            pending: &pending,
            phase: EditTracePhase::Apply,
            outcome: EditTraceOutcome::Completed,
            started: apply_started,
            reason_label: reason.clone(),
            transaction_id: Some(apply_result.transaction_id.clone()),
            attributes: vec![
                trace_attribute("operation", pending.operation.clone()),
                trace_attribute(
                    "evidence_persisted",
                    completed_evidence_persisted.to_string(),
                ),
            ],
        },
    );
    record_review_trace(
        &mut trace_log,
        ReviewTraceInput {
            pending: &pending,
            phase: EditTracePhase::ResultShaping,
            outcome: EditTraceOutcome::Completed,
            started: Instant::now(),
            reason_label: None,
            transaction_id: Some(apply_result.transaction_id.clone()),
            attributes: trace_operation_attributes(Some(&pending.operation)),
        },
    );
    let final_event = SessionEvent::ToolExecutionFinished {
        session_id: pending.session_id.clone(),
        turn_id: pending.turn_id.clone(),
        tool_request_id: ToolRequestId(pending.request_id),
        outcome: ToolOutcome::Completed,
        reason,
        result_summary: Some(result_summary(&result)),
        result_content: Some(result.content.clone()),
    };
    if append_event(sink, &final_event).is_err() {
        result.reason = Some(String::from("tool_evidence_persist_failed"));
    }
    let _ = append_events(sink, &trace_log.events);
    Ok(result)
}

pub fn reject_agent_edit_tool_review(
    edit_access: &mut EditAccess,
    sink: &impl SessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<ProviderToolResult, ToolContinuationError> {
    let mut log = SessionLog::default();
    let reject_started = Instant::now();
    edit_access
        .reject(
            &pending.preview_id,
            &pending.permission_decision_id,
            &mut log,
        )
        .map_err(|_| ToolContinuationError::Execution(ToolExecutionError::MalformedResult))?;
    let result = provider_result(
        &pending.request_id,
        Some(pending.provider_call_id.clone()),
        ToolOutcome::Completed,
        rejected_content(&pending.request_id, &pending.operation, &pending.path),
        Some(String::from("user_rejected")),
    );
    record_review_trace(
        &mut log,
        ReviewTraceInput {
            pending: &pending,
            phase: EditTracePhase::Reject,
            outcome: EditTraceOutcome::Rejected,
            started: reject_started,
            reason_label: Some(String::from("user_rejected")),
            transaction_id: None,
            attributes: trace_operation_attributes(Some(&pending.operation)),
        },
    );
    record_review_trace(
        &mut log,
        ReviewTraceInput {
            pending: &pending,
            phase: EditTracePhase::ResultShaping,
            outcome: EditTraceOutcome::Rejected,
            started: Instant::now(),
            reason_label: Some(String::from("user_rejected")),
            transaction_id: None,
            attributes: trace_operation_attributes(Some(&pending.operation)),
        },
    );
    log.push(SessionEvent::ToolExecutionFinished {
        session_id: pending.session_id,
        turn_id: pending.turn_id,
        tool_request_id: ToolRequestId(pending.request_id),
        outcome: ToolOutcome::Completed,
        reason: Some(String::from("user_rejected")),
        result_summary: Some(result_summary(&result)),
        result_content: Some(result.content.clone()),
    });
    append_events(sink, &log.events)?;
    Ok(result)
}

pub fn normalize_agent_edit_tool_request(
    registry: &ToolRegistry,
    root: &ResourceRoot,
    request: &PendingToolRequest,
    edit_policy: EditPolicy,
) -> Result<NormalizedAgentEditToolRequest, ToolError> {
    let definition = registry.validate_request_schema_only(request)?;

    match definition.name.as_str() {
        "edit_text_file" => normalize_edit_text_file(root, request, edit_policy),
        "create_text_file" => normalize_create_text_file(request),
        _ => Err(ToolError::UnknownTool),
    }
}

fn normalize_edit_text_file(
    root: &ResourceRoot,
    request: &PendingToolRequest,
    edit_policy: EditPolicy,
) -> Result<NormalizedAgentEditToolRequest, ToolError> {
    let path = string_argument(request, "path")?;
    if agent_edit_metadata_path_denied(&path) {
        return Err(ToolError::PermissionDenied);
    }
    let find = string_argument(request, "find")?;
    let replace = string_argument(request, "replace")?;
    let (path, text) = edit_read_existing_text(root, &path, &edit_policy)
        .map_err(|_| ToolError::MalformedArguments)?;
    let expected_sha256 = edit_sha256_hex(text.as_bytes());

    Ok(NormalizedAgentEditToolRequest {
        transaction: EditTransactionRequest {
            operations: vec![EditOperation::ModifyTextFile {
                path: path.clone(),
                expected_sha256,
                hunks: vec![EditHunk { find, replace }],
            }],
        },
        path,
        operation: String::from("edit_text_file"),
    })
}

fn normalize_create_text_file(
    request: &PendingToolRequest,
) -> Result<NormalizedAgentEditToolRequest, ToolError> {
    let path = string_argument(request, "path")?;
    if agent_edit_metadata_path_denied(&path) {
        return Err(ToolError::PermissionDenied);
    }
    let content = string_argument(request, "content")?;

    Ok(NormalizedAgentEditToolRequest {
        transaction: EditTransactionRequest {
            operations: vec![EditOperation::CreateTextFile {
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

/// Whether the request's `path` argument hits the sensitive-file deny list.
/// Uses only normal path components; traversal and absolute paths are
/// rejected later by edit path validation regardless.
fn sensitive_denied_request_path(root: &ResourceRoot, request: &PendingToolRequest) -> bool {
    let Ok(path) = string_argument(request, "path") else {
        return false;
    };
    let normalized: std::path::PathBuf = Path::new(&path)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    !normalized.as_os_str().is_empty() && root.sensitive_denies(&normalized)
}

fn string_argument(request: &PendingToolRequest, field: &str) -> Result<String, ToolError> {
    request
        .arguments
        .as_object()
        .and_then(|arguments| arguments.get(field))
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(ToolError::MalformedArguments)
}

fn provider_result(
    request_id: &str,
    provider_call_id: Option<String>,
    status: ToolOutcome,
    content: String,
    reason: Option<String>,
) -> ProviderToolResult {
    ProviderToolResult {
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
    preview_id: &EditPreviewId,
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

fn failed_content(request_id: &str, operation: &str, error: &str, guidance: &str) -> String {
    serde_json::json!({
        "outcome": "failed",
        "tool_request_id": request_id,
        "operation": operation,
        "error": error,
        "guidance": guidance,
    })
    .to_string()
}

/// Actionable next-step guidance for recoverable edit failures. Cheap models
/// follow explicit next-step instructions in tool errors far better than
/// they infer them, so each message states the concrete recovery action.
fn agent_edit_failure_guidance(error_label: &str) -> &'static str {
    match error_label {
        "target_exists" => {
            "The target file already exists and may have changed outside this \
conversation. Use read_text_file to inspect the current contents before deciding \
whether to edit it with edit_text_file or choose a different path."
        }
        "hash_mismatch" => {
            "The file changed since it was last read. Use read_text_file to get the \
current contents, then retry the edit against that fresh state."
        }
        "target_missing" | "parent_missing" => {
            "The target path does not currently exist. Use list_project_paths or \
read_text_file to verify the current project state before retrying."
        }
        "hunk_not_found" => {
            "The exact text to replace was not found in the current file. Use \
read_text_file to get the current contents and retry with text that matches exactly."
        }
        "sensitive_path_denied" => {
            "This path matches the sensitive-file deny list, so tools cannot read or \
modify it. If access is intended, ask the user to allow the path under files.allow in \
.yach/config.json and retry."
        }
        _ => {
            "Use read_text_file or list_project_paths to verify the current project \
state before retrying."
        }
    }
}

fn finished_event(
    context: &AgentEditToolContext,
    request_id: &str,
    outcome: ToolOutcome,
    reason: Option<String>,
    result_summary: Option<ToolPayloadSummary>,
    result_content: Option<String>,
) -> SessionEvent {
    SessionEvent::ToolExecutionFinished {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: ToolRequestId(request_id.to_owned()),
        outcome,
        reason,
        result_summary,
        result_content,
    }
}

fn append_validation_failure(
    log: &mut SessionLog,
    sink: &impl SessionEventSink,
    context: &AgentEditToolContext,
    request: &PendingToolRequest,
    error: ToolError,
    reason: String,
) -> Result<(), ToolContinuationError> {
    log.push(SessionEvent::ToolRequestRecorded {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: ToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: request.provider_call_id.clone(),
        validation: Err(error),
        permission: ToolPermissionState::Denied,
        argument_summary: summarize_agent_edit_payload(&request.arguments),
        argument_content: None,
    });
    log.push(finished_event(
        context,
        &request.request_id,
        ToolOutcome::ValidationFailed,
        Some(reason),
        None,
        None,
    ));
    append_events(sink, &log.events)
}

fn summarize_agent_edit_payload(value: &serde_json::Value) -> ToolPayloadSummary {
    let byte_count = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    ToolPayloadSummary {
        summary: String::from("tool payload redacted"),
        byte_count,
        redacted: true,
        truncated: false,
    }
}

fn result_summary(result: &ProviderToolResult) -> ToolPayloadSummary {
    ToolPayloadSummary {
        summary: String::from("agent edit result redacted"),
        byte_count: result.byte_count,
        redacted: true,
        truncated: false,
    }
}

fn append_event(
    sink: &impl SessionEventSink,
    event: &SessionEvent,
) -> Result<(), ToolContinuationError> {
    sink.append_event(event)
        .map_err(|_| ToolContinuationError::Execution(ToolExecutionError::MalformedResult))
}

fn append_events(
    sink: &impl SessionEventSink,
    events: &[SessionEvent],
) -> Result<(), ToolContinuationError> {
    sink.append_events(events)
        .map_err(|_| ToolContinuationError::Execution(ToolExecutionError::MalformedResult))
}

struct AgentEditTraceInput<'a> {
    trace_id: &'a EditTraceId,
    request: &'a PendingToolRequest,
    phase: EditTracePhase,
    outcome: EditTraceOutcome,
    started: Instant,
    reason_label: Option<String>,
    preview_id: Option<EditPreviewId>,
    permission_decision_id: Option<PermissionDecisionId>,
    transaction_id: Option<EditTransactionId>,
    attributes: Vec<MetricAttribute>,
}

struct PermissionDecisionTraceInput<'a> {
    trace_id: &'a EditTraceId,
    request: &'a PendingToolRequest,
    outcome: EditTraceOutcome,
    started: Instant,
    reason_label: Option<String>,
    permission_decision_id: &'a PermissionDecisionId,
    review_state: &'a EditAccessReviewState,
    transaction_id: Option<&'a EditTransactionId>,
    operation: &'a str,
}

struct PreviewTraceInput<'a> {
    trace_id: &'a EditTraceId,
    request: &'a PendingToolRequest,
    outcome: EditTraceOutcome,
    started: Instant,
    reason_label: Option<String>,
    preview_id: Option<&'a EditPreviewId>,
    permission_decision_id: &'a PermissionDecisionId,
    transaction_id: Option<&'a EditTransactionId>,
    operation: &'a str,
}

struct ReviewTraceInput<'a> {
    pending: &'a PendingAgentEditToolReview,
    phase: EditTracePhase,
    outcome: EditTraceOutcome,
    started: Instant,
    reason_label: Option<String>,
    transaction_id: Option<EditTransactionId>,
    attributes: Vec<MetricAttribute>,
}

fn record_agent_edit_trace(
    log: &mut SessionLog,
    context: &AgentEditToolContext,
    input: AgentEditTraceInput<'_>,
) {
    log.record_edit_trace(
        context.session_id.clone(),
        context.turn_id.clone(),
        EditTraceRecord {
            trace_id: input.trace_id.clone(),
            phase: input.phase,
            source: EditTraceSource::ProviderTool,
            tool_name: Some(input.request.tool_name.clone()),
            tool_request_id: Some(ToolRequestId(input.request.request_id.clone())),
            provider_call_id: input.request.provider_call_id.clone(),
            preview_id: input.preview_id,
            permission_decision_id: input.permission_decision_id,
            transaction_id: input.transaction_id,
            outcome: input.outcome,
            duration_ms: duration_ms(input.started),
            reason_label: input.reason_label,
            attributes: input.attributes,
        },
    );
}

fn record_permission_decision_trace(
    log: &mut SessionLog,
    context: &AgentEditToolContext,
    input: PermissionDecisionTraceInput<'_>,
) {
    record_agent_edit_trace(
        log,
        context,
        AgentEditTraceInput {
            trace_id: input.trace_id,
            request: input.request,
            phase: EditTracePhase::PermissionDecision,
            outcome: input.outcome,
            started: input.started,
            reason_label: input.reason_label,
            preview_id: None,
            permission_decision_id: Some(input.permission_decision_id.clone()),
            transaction_id: input.transaction_id.cloned(),
            attributes: vec![
                trace_attribute("operation", input.operation),
                trace_attribute("review_state", review_state_label(input.review_state)),
            ],
        },
    );
}

fn record_preview_trace(
    log: &mut SessionLog,
    context: &AgentEditToolContext,
    input: PreviewTraceInput<'_>,
) {
    record_agent_edit_trace(
        log,
        context,
        AgentEditTraceInput {
            trace_id: input.trace_id,
            request: input.request,
            phase: EditTracePhase::Preview,
            outcome: input.outcome,
            started: input.started,
            reason_label: input.reason_label,
            preview_id: input.preview_id.cloned(),
            permission_decision_id: Some(input.permission_decision_id.clone()),
            transaction_id: input.transaction_id.cloned(),
            attributes: trace_operation_attributes(Some(input.operation)),
        },
    );
}

fn record_result_shaping_trace(
    log: &mut SessionLog,
    context: &AgentEditToolContext,
    trace_id: &EditTraceId,
    request: &PendingToolRequest,
    outcome: EditTraceOutcome,
    reason_label: Option<String>,
    operation: Option<&str>,
) {
    record_agent_edit_trace(
        log,
        context,
        AgentEditTraceInput {
            trace_id,
            request,
            phase: EditTracePhase::ResultShaping,
            outcome,
            started: Instant::now(),
            reason_label,
            preview_id: None,
            permission_decision_id: None,
            transaction_id: None,
            attributes: trace_operation_attributes(operation),
        },
    );
}

fn record_review_trace(log: &mut SessionLog, input: ReviewTraceInput<'_>) {
    let pending = input.pending;
    log.record_edit_trace(
        pending.session_id.clone(),
        pending.turn_id.clone(),
        EditTraceRecord {
            trace_id: pending.trace_id.clone(),
            phase: input.phase,
            source: EditTraceSource::ProviderTool,
            tool_name: Some(pending.operation.clone()),
            tool_request_id: Some(ToolRequestId(pending.request_id.clone())),
            provider_call_id: Some(pending.provider_call_id.clone()),
            preview_id: Some(pending.preview_id.clone()),
            permission_decision_id: Some(pending.permission_decision_id.clone()),
            transaction_id: input.transaction_id,
            outcome: input.outcome,
            duration_ms: duration_ms(input.started),
            reason_label: input.reason_label,
            attributes: input.attributes,
        },
    );
}

fn trace_operation(tool_name: &str) -> Option<String> {
    matches!(tool_name, "edit_text_file" | "create_text_file").then(|| tool_name.to_owned())
}

fn trace_operation_attributes(operation: Option<&str>) -> Vec<MetricAttribute> {
    operation
        .map(|operation| vec![trace_attribute("operation", operation)])
        .unwrap_or_default()
}

fn trace_attribute(key: &str, value: impl Into<String>) -> MetricAttribute {
    MetricAttribute {
        key: key.to_owned(),
        value: value.into(),
    }
}

fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn review_state_label(review_state: &EditAccessReviewState) -> &'static str {
    match review_state {
        EditAccessReviewState::Allowed => "allowed",
        EditAccessReviewState::NeedsUserApproval => "needs_user_approval",
        EditAccessReviewState::AutoReviewUnavailable => "auto_review_unavailable",
    }
}

fn agent_edit_access_prepare_error_label(error: &EditError) -> String {
    edit_error_label(error).to_owned()
}

fn agent_edit_tool_error_label(error: &ToolError) -> String {
    match error {
        ToolError::UnknownTool => String::from("unknown_tool"),
        ToolError::MalformedArguments => String::from("malformed_arguments"),
        ToolError::ArgumentsTooLarge => String::from("arguments_too_large"),
        ToolError::MissingRequiredField { .. } => String::from("missing_required_field"),
        ToolError::InvalidFieldType { .. } => String::from("invalid_field_type"),
        ToolError::UnexpectedField { .. } => String::from("unexpected_field"),
        ToolError::PermissionDenied => String::from("permission_denied"),
    }
}

fn next_agent_edit_trace_id() -> EditTraceId {
    let next = AGENT_EDIT_TRACE_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    EditTraceId(format!("edit-trace-{next}"))
}
