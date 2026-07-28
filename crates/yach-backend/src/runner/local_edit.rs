use std::path::PathBuf;

use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, LocalEditDecision, LocalEditFinishedOutcome, LocalEditOperationInput,
    LocalEditPreviewSummary, LocalEditReviewState, ServerEvent,
};

use crate::{
    EditAccess, EditAccessContext, EditAccessError, EditAccessReviewState, EditHunk, EditOperation,
    EditPolicy, EditPreview, EditPreviewId, EditTransactionRequest, JsonlSessionStore,
    PermissionDecisionId, PermissionPolicy, ResourceRoot, SessionEventSink, SessionId, SessionLog,
    TurnId, edit_error_label,
};

pub(super) struct LocalEditPrepareInput {
    pub(super) session_id: SessionId,
    pub(super) request_id: String,
    pub(super) operation: LocalEditOperationInput,
    pub(super) turn_index: u64,
}

pub(super) fn local_edit_root(project_root: Option<PathBuf>) -> Result<ResourceRoot, String> {
    let root_path = project_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = ResourceRoot::project(&root_path).map_err(|error| {
        format!(
            "local edit root unavailable at {}: {error}",
            root_path.display()
        )
    })?;
    let (policy, _warnings) =
        crate::SensitivePathPolicy::load_for_project(Some(root.canonical_path()));
    Ok(root.with_sensitive_policy(policy))
}

pub(super) fn handle_native_local_edit_prepare(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    edit_access: &mut EditAccess,
    edit_root: Result<&ResourceRoot, &String>,
    input: LocalEditPrepareInput,
) {
    let Ok(edit_root) = edit_root else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
            preview_id: None,
            outcome: LocalEditFinishedOutcome::Failed,
            message: edit_root
                .err()
                .cloned()
                .unwrap_or_else(|| String::from("local edit root unavailable")),
        }));
        return;
    };
    let LocalEditPrepareInput {
        session_id,
        request_id,
        operation,
        turn_index,
    } = input;
    let LocalEditRequestParts {
        request,
        path,
        operation,
    } = local_edit_request_from_input(operation);
    let mut log = SessionLog::default();
    let context = EditAccessContext {
        session_id,
        turn_id: TurnId(format!("turn-{turn_index}")),
        permission_policy: PermissionPolicy::default_local_edit(),
        edit_policy: EditPolicy::conservative(),
        tool_request_id: None,
    };

    match edit_access.prepare(edit_root, request, context, &mut log) {
        Ok(preview) => {
            if let Err(error) = store.append_events(&log.events) {
                let mut discard_log = SessionLog::default();
                let _ = edit_access.reject(
                    &preview.preview_id,
                    &preview.permission_decision_id,
                    &mut discard_log,
                );
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: None,
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!("failed to persist local edit evidence: {error}"),
                }));
                return;
            }
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditPreviewReady {
                request_id,
                preview: local_edit_preview_summary(preview, path, operation),
            }));
        }
        Err(EditAccessError::PermissionDenied { reason }) => {
            let outcome = if store.append_events(&log.events).is_ok() {
                LocalEditFinishedOutcome::Denied
            } else {
                LocalEditFinishedOutcome::Failed
            };
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome,
                message: format!("local edit denied: {reason}"),
            }));
        }
        Err(error) => {
            let _ = store.append_events(&log.events);
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome: LocalEditFinishedOutcome::Failed,
                message: local_edit_error_message(&error),
            }));
        }
    }
}

pub(super) fn handle_native_local_edit_decision(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &JsonlSessionStore,
    edit_access: &mut EditAccess,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
) {
    let preview_id = EditPreviewId(preview_id);
    let decision_id = PermissionDecisionId(permission_decision_id);
    match decision {
        LocalEditDecision::Apply => {
            match edit_access.apply_with_evidence_sink(&preview_id, &decision_id, store) {
                Ok((_, true)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from("local edit applied"),
                    }));
                }
                Ok((_, false)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from(
                            "local edit applied; completed evidence persist failed",
                        ),
                    }));
                }
                Err(error) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: local_edit_error_message(&error),
                    }));
                }
            }
        }
        LocalEditDecision::Reject => {
            let mut log = SessionLog::default();
            if let Err(error) = store.append_events(&[]) {
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: Some(preview_id.0),
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!("failed to persist local edit evidence: {error}"),
                }));
                return;
            }
            match edit_access.reject(&preview_id, &decision_id, &mut log) {
                Ok(()) => {
                    if let Err(error) = store.append_events(&log.events) {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                            preview_id: Some(preview_id.0),
                            outcome: LocalEditFinishedOutcome::Failed,
                            message: format!("failed to persist local edit evidence: {error}"),
                        }));
                        return;
                    }
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Rejected,
                        message: String::from("local edit rejected"),
                    }));
                }
                Err(error) => {
                    let _ = store.append_events(&log.events);
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: local_edit_error_message(&error),
                    }));
                }
            }
        }
    }
}

struct LocalEditRequestParts {
    request: EditTransactionRequest,
    path: String,
    operation: String,
}

fn local_edit_request_from_input(input: LocalEditOperationInput) -> LocalEditRequestParts {
    match input {
        LocalEditOperationInput::ModifyTextFile {
            path,
            expected_sha256,
            find,
            replace,
        } => LocalEditRequestParts {
            request: EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: path.clone(),
                    expected_sha256,
                    hunks: vec![EditHunk { find, replace }],
                }],
            },
            path,
            operation: String::from("modify_text_file"),
        },
        LocalEditOperationInput::CreateTextFile { path, content } => LocalEditRequestParts {
            request: EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: path.clone(),
                    content,
                }],
            },
            path,
            operation: String::from("create_text_file"),
        },
    }
}

pub(super) fn local_edit_preview_summary(
    preview: EditPreview,
    path: String,
    operation: String,
) -> LocalEditPreviewSummary {
    let review_state = local_edit_review_state(&preview.review_state);
    LocalEditPreviewSummary {
        preview_id: preview.preview_id.0,
        transaction_id: preview.transaction_id.0,
        permission_decision_id: preview.permission_decision_id.0,
        path,
        operation,
        review_state,
        diff_summary: preview.diff_summary,
        diff_summary_truncated: preview.diff_summary_truncated,
    }
}

const fn local_edit_review_state(review_state: &EditAccessReviewState) -> LocalEditReviewState {
    match review_state {
        EditAccessReviewState::Allowed => LocalEditReviewState::Allowed,
        EditAccessReviewState::NeedsUserApproval => LocalEditReviewState::NeedsUserApproval,
        EditAccessReviewState::AutoReviewUnavailable => LocalEditReviewState::AutoReviewUnavailable,
    }
}

pub(super) fn local_edit_error_message(error: &EditAccessError) -> String {
    match error {
        EditAccessError::PermissionDenied { reason } => {
            format!("local edit denied: {reason}")
        }
        EditAccessError::Preview(error) => {
            format!("local edit preview failed: {}", edit_error_label(error))
        }
        EditAccessError::Apply(error) => {
            format!("local edit apply failed: {}", edit_error_label(error))
        }
        EditAccessError::PreviewNotFound => String::from("stale local edit preview"),
        EditAccessError::DecisionMismatch => String::from("stale local edit permission decision"),
        EditAccessError::EvidencePersistFailed => {
            String::from("failed to persist local edit evidence")
        }
    }
}
