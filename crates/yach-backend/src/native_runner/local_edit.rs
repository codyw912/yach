use std::path::PathBuf;

use tokio::sync::mpsc;
use yach_proto::{
    BackendEvent, LocalEditDecision, LocalEditFinishedOutcome, LocalEditOperationInput,
    LocalEditPreviewSummary, LocalEditReviewState, ServerEvent,
};

use crate::{
    NativeEditAccess, NativeEditAccessContext, NativeEditAccessError, NativeEditAccessReviewState,
    NativeEditHunk, NativeEditOperation, NativeEditPolicy, NativeEditPreview, NativeEditPreviewId,
    NativeEditTransactionRequest, NativeJsonlSessionStore, NativePermissionDecisionId,
    NativePermissionPolicy, NativeResourceRoot, NativeSessionEventSink, NativeSessionId,
    NativeSessionLog, NativeTurnId, native_edit_error_label,
};

pub(super) struct NativeLocalEditPrepareInput {
    pub(super) session_id: NativeSessionId,
    pub(super) request_id: String,
    pub(super) operation: LocalEditOperationInput,
    pub(super) turn_index: u64,
}

pub(super) fn native_local_edit_root(
    project_root: Option<PathBuf>,
) -> Result<NativeResourceRoot, String> {
    let root_path = project_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let root = NativeResourceRoot::project(&root_path).map_err(|error| {
        format!(
            "native dogfood: local edit root unavailable at {}: {error}",
            root_path.display()
        )
    })?;
    let (policy, _warnings) =
        crate::NativeSensitivePathPolicy::load_for_project(Some(root.canonical_path()));
    Ok(root.with_sensitive_policy(policy))
}

pub(super) fn handle_native_local_edit_prepare(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    edit_access: &mut NativeEditAccess,
    edit_root: Result<&NativeResourceRoot, &String>,
    input: NativeLocalEditPrepareInput,
) {
    let Ok(edit_root) = edit_root else {
        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
            preview_id: None,
            outcome: LocalEditFinishedOutcome::Failed,
            message: edit_root
                .err()
                .cloned()
                .unwrap_or_else(|| String::from("native dogfood: local edit root unavailable")),
        }));
        return;
    };
    let NativeLocalEditPrepareInput {
        session_id,
        request_id,
        operation,
        turn_index,
    } = input;
    let LocalEditRequestParts {
        request,
        path,
        operation,
    } = native_local_edit_request_from_input(operation);
    let mut log = NativeSessionLog::default();
    let context = NativeEditAccessContext {
        session_id,
        turn_id: NativeTurnId(format!("turn-{turn_index}")),
        permission_policy: NativePermissionPolicy::default_local_edit(),
        edit_policy: NativeEditPolicy::conservative(),
        tool_request_id: None,
    };

    match edit_access.prepare(edit_root, request, context, &mut log) {
        Ok(preview) => {
            if let Err(error) = store.append_events(&log.events) {
                let mut discard_log = NativeSessionLog::default();
                let _ = edit_access.reject(
                    &preview.preview_id,
                    &preview.permission_decision_id,
                    &mut discard_log,
                );
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: None,
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!(
                        "native dogfood: failed to persist local edit evidence: {error}"
                    ),
                }));
                return;
            }
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditPreviewReady {
                request_id,
                preview: native_local_edit_preview_summary(preview, path, operation),
            }));
        }
        Err(NativeEditAccessError::PermissionDenied { reason }) => {
            let outcome = if store.append_events(&log.events).is_ok() {
                LocalEditFinishedOutcome::Denied
            } else {
                LocalEditFinishedOutcome::Failed
            };
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome,
                message: format!("native dogfood: local edit denied: {reason}"),
            }));
        }
        Err(error) => {
            let _ = store.append_events(&log.events);
            let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                preview_id: None,
                outcome: LocalEditFinishedOutcome::Failed,
                message: native_local_edit_error_message(&error),
            }));
        }
    }
}

pub(super) fn handle_native_local_edit_decision(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    store: &NativeJsonlSessionStore,
    edit_access: &mut NativeEditAccess,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
) {
    let preview_id = NativeEditPreviewId(preview_id);
    let decision_id = NativePermissionDecisionId(permission_decision_id);
    match decision {
        LocalEditDecision::Apply => {
            match edit_access.apply_with_evidence_sink(&preview_id, &decision_id, store) {
                Ok((_, true)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from("native dogfood: local edit applied"),
                    }));
                }
                Ok((_, false)) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Applied,
                        message: String::from(
                            "native dogfood: local edit applied; completed evidence persist failed",
                        ),
                    }));
                }
                Err(error) => {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: native_local_edit_error_message(&error),
                    }));
                }
            }
        }
        LocalEditDecision::Reject => {
            let mut log = NativeSessionLog::default();
            if let Err(error) = store.append_events(&[]) {
                let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                    preview_id: Some(preview_id.0),
                    outcome: LocalEditFinishedOutcome::Failed,
                    message: format!(
                        "native dogfood: failed to persist local edit evidence: {error}"
                    ),
                }));
                return;
            }
            match edit_access.reject(&preview_id, &decision_id, &mut log) {
                Ok(()) => {
                    if let Err(error) = store.append_events(&log.events) {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                            preview_id: Some(preview_id.0),
                            outcome: LocalEditFinishedOutcome::Failed,
                            message: format!(
                                "native dogfood: failed to persist local edit evidence: {error}"
                            ),
                        }));
                        return;
                    }
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Rejected,
                        message: String::from("native dogfood: local edit rejected"),
                    }));
                }
                Err(error) => {
                    let _ = store.append_events(&log.events);
                    let _ = tx.send(BackendEvent::Server(ServerEvent::LocalEditFinished {
                        preview_id: Some(preview_id.0),
                        outcome: LocalEditFinishedOutcome::Failed,
                        message: native_local_edit_error_message(&error),
                    }));
                }
            }
        }
    }
}

struct LocalEditRequestParts {
    request: NativeEditTransactionRequest,
    path: String,
    operation: String,
}

fn native_local_edit_request_from_input(input: LocalEditOperationInput) -> LocalEditRequestParts {
    match input {
        LocalEditOperationInput::ModifyTextFile {
            path,
            expected_sha256,
            find,
            replace,
        } => LocalEditRequestParts {
            request: NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: path.clone(),
                    expected_sha256,
                    hunks: vec![NativeEditHunk { find, replace }],
                }],
            },
            path,
            operation: String::from("modify_text_file"),
        },
        LocalEditOperationInput::CreateTextFile { path, content } => LocalEditRequestParts {
            request: NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: path.clone(),
                    content,
                }],
            },
            path,
            operation: String::from("create_text_file"),
        },
    }
}

pub(super) fn native_local_edit_preview_summary(
    preview: NativeEditPreview,
    path: String,
    operation: String,
) -> LocalEditPreviewSummary {
    let review_state = native_local_edit_review_state(&preview.review_state);
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

const fn native_local_edit_review_state(
    review_state: &NativeEditAccessReviewState,
) -> LocalEditReviewState {
    match review_state {
        NativeEditAccessReviewState::Allowed => LocalEditReviewState::Allowed,
        NativeEditAccessReviewState::NeedsUserApproval => LocalEditReviewState::NeedsUserApproval,
        NativeEditAccessReviewState::AutoReviewUnavailable => {
            LocalEditReviewState::AutoReviewUnavailable
        }
    }
}

pub(super) fn native_local_edit_error_message(error: &NativeEditAccessError) -> String {
    match error {
        NativeEditAccessError::PermissionDenied { reason } => {
            format!("native dogfood: local edit denied: {reason}")
        }
        NativeEditAccessError::Preview(error) => {
            format!(
                "native dogfood: local edit preview failed: {}",
                native_edit_error_label(error)
            )
        }
        NativeEditAccessError::Apply(error) => {
            format!(
                "native dogfood: local edit apply failed: {}",
                native_edit_error_label(error)
            )
        }
        NativeEditAccessError::PreviewNotFound => {
            String::from("native dogfood: stale local edit preview")
        }
        NativeEditAccessError::DecisionMismatch => {
            String::from("native dogfood: stale local edit permission decision")
        }
        NativeEditAccessError::EvidencePersistFailed => {
            String::from("native dogfood: failed to persist local edit evidence")
        }
    }
}
