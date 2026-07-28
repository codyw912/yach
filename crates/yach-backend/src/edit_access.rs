use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::edit_harness::{edit_apply_evidence_summary, edit_prepared_evidence_summary};
use crate::{
    EditApplyResult, EditEngine, EditError, EditEvidenceOutcome, EditOperation, EditPolicy,
    EditTransactionId, EditTransactionRequest, PermissionActor, PermissionCapability,
    PermissionDecision, PermissionDecisionEngine, PermissionDecisionId, PermissionDecisionOutcome,
    PermissionDecisionSummary, PermissionPolicy, PermissionRequest, PermissionRisk,
    PermissionTargetSummary, PreparedEditTransaction, ResourceRoot, SessionEvent, SessionEventSink,
    SessionId, SessionLog, ToolRequestId, TurnId, edit_error_label,
};

static EDIT_PREVIEW_COUNTER: AtomicU64 = AtomicU64::new(0);
static EDIT_PERMISSION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditPreviewId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditAccessContext {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub permission_policy: PermissionPolicy,
    pub edit_policy: EditPolicy,
    pub tool_request_id: Option<ToolRequestId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditAccessReviewState {
    Allowed,
    NeedsUserApproval,
    AutoReviewUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPreview {
    pub preview_id: EditPreviewId,
    pub transaction_id: EditTransactionId,
    pub permission_decision_id: PermissionDecisionId,
    pub review_state: EditAccessReviewState,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditAccessError {
    PermissionDenied { reason: String },
    Preview(EditError),
    Apply(EditError),
    PreviewNotFound,
    DecisionMismatch,
    EvidencePersistFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditAccessPrepareDiagnostics {
    pub permission_decision_id: PermissionDecisionId,
    pub review_state: EditAccessReviewState,
    pub transaction_id: Option<EditTransactionId>,
    pub reason_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditAccessPrepareOutcome {
    pub preview: EditPreview,
    pub diagnostics: EditAccessPrepareDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditAccessPrepareError {
    PermissionDenied {
        reason: String,
        diagnostics: EditAccessPrepareDiagnostics,
    },
    Preview {
        error: EditError,
        diagnostics: EditAccessPrepareDiagnostics,
    },
}

#[derive(Debug)]
struct PendingEditPreview {
    context: EditAccessContext,
    root: ResourceRoot,
    prepared: PreparedEditTransaction,
    permission_decision_id: PermissionDecisionId,
    permission_summary: PermissionDecisionSummary,
}

#[derive(Debug, Default)]
pub struct EditAccess {
    pending: BTreeMap<String, PendingEditPreview>,
}

impl EditAccess {
    pub fn prepare(
        &mut self,
        root: &ResourceRoot,
        request: EditTransactionRequest,
        context: EditAccessContext,
        log: &mut SessionLog,
    ) -> Result<EditPreview, EditAccessError> {
        self.prepare_with_diagnostics(root, request, context, log)
            .map(|outcome| outcome.preview)
            .map_err(|error| match *error {
                EditAccessPrepareError::PermissionDenied { reason, .. } => {
                    EditAccessError::PermissionDenied { reason }
                }
                EditAccessPrepareError::Preview { error, .. } => EditAccessError::Preview(error),
            })
    }

    pub fn prepare_with_diagnostics(
        &mut self,
        root: &ResourceRoot,
        request: EditTransactionRequest,
        context: EditAccessContext,
        log: &mut SessionLog,
    ) -> Result<EditAccessPrepareOutcome, Box<EditAccessPrepareError>> {
        let permission_request = permission_request_from_edit(&request);
        let decision =
            PermissionDecisionEngine::decide(&permission_request, &context.permission_policy);
        let permission_summary = decision.summary(&permission_request, false);
        log.record_permission_decision(
            context.session_id.clone(),
            context.turn_id.clone(),
            permission_summary.clone(),
        );

        let permission_decision_id = decision.decision_id();
        let review_state = match &decision {
            PermissionDecision::Allowed { .. } => EditAccessReviewState::Allowed,
            PermissionDecision::NeedsUserReview { reason, .. }
                if reason == "auto_review_unavailable_fallback_ask" =>
            {
                EditAccessReviewState::AutoReviewUnavailable
            }
            PermissionDecision::NeedsUserReview { .. } => EditAccessReviewState::NeedsUserApproval,
            PermissionDecision::Denied { reason, .. } => {
                let diagnostics = EditAccessPrepareDiagnostics {
                    permission_decision_id,
                    review_state: EditAccessReviewState::NeedsUserApproval,
                    transaction_id: None,
                    reason_label: Some(reason.clone()),
                };
                return Err(Box::new(EditAccessPrepareError::PermissionDenied {
                    reason: reason.clone(),
                    diagnostics,
                }));
            }
        };

        let prepared = match EditEngine::preview(root, request, &context.edit_policy) {
            Ok(prepared) => prepared,
            Err(error) => {
                let reason_label = edit_error_label(&error).to_owned();
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: context.tool_request_id.clone(),
                    transaction_id: None,
                    outcome: EditEvidenceOutcome::ValidationFailed,
                    reason: Some(reason_label.clone()),
                    summary: None,
                });
                return Err(Box::new(EditAccessPrepareError::Preview {
                    error,
                    diagnostics: EditAccessPrepareDiagnostics {
                        permission_decision_id,
                        review_state,
                        transaction_id: None,
                        reason_label: Some(reason_label),
                    },
                }));
            }
        };
        let summary = edit_prepared_evidence_summary(&prepared);
        log.push(SessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: context.tool_request_id.clone(),
            transaction_id: prepared.transaction_id.clone(),
            summary,
        });

        let preview_id = EditPreviewId(next_edit_preview_id());
        let preview = EditPreview {
            preview_id: preview_id.clone(),
            transaction_id: prepared.transaction_id.clone(),
            permission_decision_id: permission_decision_id.clone(),
            review_state: review_state.clone(),
            operation_count: prepared.operation_count,
            diff_summary: prepared.diff_summary.clone(),
            diff_summary_truncated: prepared.diff_summary_truncated,
            diff_summary_bytes: prepared.diff_summary_bytes,
        };
        self.pending.insert(
            preview_id.0.clone(),
            PendingEditPreview {
                context,
                root: root.clone(),
                prepared,
                permission_decision_id: preview.permission_decision_id.clone(),
                permission_summary,
            },
        );
        Ok(EditAccessPrepareOutcome {
            diagnostics: EditAccessPrepareDiagnostics {
                permission_decision_id,
                review_state,
                transaction_id: Some(preview.transaction_id.clone()),
                reason_label: None,
            },
            preview,
        })
    }

    pub fn apply(
        &mut self,
        preview_id: &EditPreviewId,
        decision_id: &PermissionDecisionId,
        log: &mut SessionLog,
    ) -> Result<EditApplyResult, EditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(EditAccessError::PreviewNotFound)?;
        if &pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(EditAccessError::DecisionMismatch);
        }

        record_user_permission_override(
            log,
            &pending.context,
            &pending.permission_summary,
            PermissionDecisionOutcome::Allowed,
            "user_approved",
        );
        let transaction_id = pending.prepared.transaction_id.clone();
        match EditEngine::apply(
            &pending.root,
            pending.prepared,
            &pending.context.edit_policy,
        ) {
            Ok(result) => {
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id.clone(),
                    turn_id: pending.context.turn_id.clone(),
                    tool_request_id: pending.context.tool_request_id.clone(),
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id.clone(),
                    turn_id: pending.context.turn_id.clone(),
                    tool_request_id: pending.context.tool_request_id.clone(),
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Failed,
                    reason: Some(edit_error_label(&error).to_owned()),
                    summary: None,
                });
                Err(EditAccessError::Apply(error))
            }
        }
    }

    pub fn apply_with_evidence_sink(
        &mut self,
        preview_id: &EditPreviewId,
        decision_id: &PermissionDecisionId,
        sink: &impl SessionEventSink,
    ) -> Result<(EditApplyResult, bool), EditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(EditAccessError::PreviewNotFound)?;
        if &pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(EditAccessError::DecisionMismatch);
        }

        let transaction_id = pending.prepared.transaction_id.clone();
        let mut write_ahead_log = SessionLog::default();
        record_user_permission_override(
            &mut write_ahead_log,
            &pending.context,
            &pending.permission_summary,
            PermissionDecisionOutcome::Allowed,
            "user_approved",
        );
        write_ahead_log.push(SessionEvent::EditTransactionFinished {
            session_id: pending.context.session_id.clone(),
            turn_id: pending.context.turn_id.clone(),
            tool_request_id: pending.context.tool_request_id.clone(),
            transaction_id: Some(transaction_id.clone()),
            outcome: EditEvidenceOutcome::ApplyStarted,
            reason: Some(String::from("apply_started")),
            summary: Some(edit_prepared_evidence_summary(&pending.prepared)),
        });
        if sink.append_events(&write_ahead_log.events).is_err() {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(EditAccessError::EvidencePersistFailed);
        }

        match EditEngine::apply(
            &pending.root,
            pending.prepared,
            &pending.context.edit_policy,
        ) {
            Ok(result) => {
                let completed_log = [SessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id.clone(),
                    turn_id: pending.context.turn_id.clone(),
                    tool_request_id: pending.context.tool_request_id.clone(),
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(edit_apply_evidence_summary(&result)),
                }];
                let completed_evidence_persisted = sink.append_events(&completed_log).is_ok();
                Ok((result, completed_evidence_persisted))
            }
            Err(error) => {
                let failure_log = [SessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id.clone(),
                    turn_id: pending.context.turn_id.clone(),
                    tool_request_id: pending.context.tool_request_id.clone(),
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Failed,
                    reason: Some(edit_error_label(&error).to_owned()),
                    summary: None,
                }];
                let _ = sink.append_events(&failure_log);
                Err(EditAccessError::Apply(error))
            }
        }
    }

    pub fn reject(
        &mut self,
        preview_id: &EditPreviewId,
        decision_id: &PermissionDecisionId,
        log: &mut SessionLog,
    ) -> Result<(), EditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(EditAccessError::PreviewNotFound)?;
        if &pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(EditAccessError::DecisionMismatch);
        }

        record_user_permission_override(
            log,
            &pending.context,
            &pending.permission_summary,
            PermissionDecisionOutcome::Denied,
            "user_rejected",
        );
        log.push(SessionEvent::EditTransactionFinished {
            session_id: pending.context.session_id.clone(),
            turn_id: pending.context.turn_id.clone(),
            tool_request_id: pending.context.tool_request_id.clone(),
            transaction_id: Some(pending.prepared.transaction_id.clone()),
            outcome: EditEvidenceOutcome::Failed,
            reason: Some(String::from("user_rejected")),
            summary: Some(edit_prepared_evidence_summary(&pending.prepared)),
        });
        Ok(())
    }

    #[must_use]
    pub fn has_pending_preview(&self, preview_id: &EditPreviewId) -> bool {
        self.pending.contains_key(&preview_id.0)
    }
}

fn record_user_permission_override(
    log: &mut SessionLog,
    context: &EditAccessContext,
    summary: &PermissionDecisionSummary,
    outcome: PermissionDecisionOutcome,
    reason: &str,
) {
    if summary.outcome != PermissionDecisionOutcome::NeedsUserReview {
        return;
    }
    let mut summary = summary.clone();
    summary.outcome = outcome;
    reason.clone_into(&mut summary.reason);
    summary.user_override = true;
    summary.rationale = None;
    log.record_permission_decision(context.session_id.clone(), context.turn_id.clone(), summary);
}

fn permission_request_from_edit(request: &EditTransactionRequest) -> PermissionRequest {
    let (operation, resource) = request.operations.first().map_or_else(
        || {
            (
                String::from("empty_edit_transaction"),
                String::from("<none>"),
            )
        },
        |operation| match operation {
            EditOperation::ModifyTextFile { path, .. } => (
                String::from("modify_text_file"),
                summarized_permission_resource(path),
            ),
            EditOperation::CreateTextFile { path, .. } => (
                String::from("create_text_file"),
                summarized_permission_resource(path),
            ),
        },
    );

    PermissionRequest {
        request_id: next_edit_permission_request_id(),
        actor: PermissionActor::UserLocalUi,
        capability: PermissionCapability::EditTransaction,
        target: PermissionTargetSummary {
            operation,
            resource,
        },
        risk: PermissionRisk::WorkspaceWrite,
        requested_reviewer: None,
    }
}

fn summarized_permission_resource(path: &str) -> String {
    let parsed = std::path::Path::new(path);
    if parsed.is_absolute() {
        return String::from("<absolute_path>");
    }
    let mut components = Vec::new();
    for component in parsed.components() {
        match component {
            std::path::Component::Normal(part) => {
                if let Some(part) = part.to_str() {
                    components.push(part);
                } else {
                    return String::from("<redacted_resource>");
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => return String::from("<path_traversal>"),
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return String::from("<absolute_path>");
            }
        }
    }
    if components.is_empty() {
        return String::from("<empty_path>");
    }
    if components.first() == Some(&".git")
        || components.first() == Some(&"target")
        || components.as_slice().starts_with(&[".yach", "sessions"])
    {
        return String::from("<metadata_path>");
    }
    path.to_owned()
}

fn next_edit_preview_id() -> String {
    let next = EDIT_PREVIEW_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("edit-preview-{next}")
}

fn next_edit_permission_request_id() -> String {
    let next = EDIT_PERMISSION_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("edit-permission-request-{next}")
}

#[cfg(test)]
mod tests {
    use super::{EditAccess, EditAccessContext, EditAccessError, EditAccessReviewState};
    use crate::{
        EditEvidenceOutcome, EditHunk, EditOperation, EditPolicy, EditPreviewId,
        EditTransactionRequest, PermissionDecisionId, PermissionDecisionOutcome, PermissionMode,
        PermissionPolicy, ResourceRoot, SessionEvent, SessionId, SessionLog, TurnId,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "yach-edit-access-{name}-{}-{sequence}",
                std::process::id()
            ));
            assert!(std::fs::create_dir_all(&root).is_ok());
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn context(mode: PermissionMode) -> EditAccessContext {
        EditAccessContext {
            session_id: SessionId(String::from("default")),
            turn_id: TurnId(String::from("turn-1")),
            permission_policy: PermissionPolicy::for_edit_mode(mode),
            edit_policy: EditPolicy::test(),
            tool_request_id: None,
        }
    }

    fn resource_root(project: &TempProject) -> Option<ResourceRoot> {
        let root = ResourceRoot::project(project.root());
        assert!(root.is_ok());
        root.ok()
    }

    fn write_file(project: &TempProject, relative_path: &str, content: &str) {
        assert!(std::fs::write(project.root().join(relative_path), content).is_ok());
    }

    fn modify_request() -> EditTransactionRequest {
        EditTransactionRequest {
            operations: vec![EditOperation::ModifyTextFile {
                path: String::from("file.txt"),
                expected_sha256: crate::sha256_hex_for_test("hello\n"),
                hunks: vec![EditHunk {
                    find: String::from("hello"),
                    replace: String::from("goodbye"),
                }],
            }],
        }
    }

    #[test]
    fn prepare_in_ask_mode_keeps_transaction_pending() {
        let project = TempProject::new("ask-pending");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();

        let preview = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Ask),
            &mut log,
        );

        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };
        assert_eq!(
            preview.review_state,
            EditAccessReviewState::NeedsUserApproval
        );
        assert!(access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
        assert!(
            log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::PermissionDecisionRecorded { .. }))
        );
        assert!(
            log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn prepare_with_diagnostics_reports_permission_and_preview_ids() {
        let project = TempProject::new("diagnostics");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();

        let outcome = access.prepare_with_diagnostics(
            &root,
            modify_request(),
            context(PermissionMode::Ask),
            &mut log,
        );

        assert!(outcome.is_ok());
        let Some(outcome) = outcome.ok() else {
            return;
        };
        assert_eq!(
            outcome.diagnostics.permission_decision_id,
            outcome.preview.permission_decision_id
        );
        assert_eq!(
            outcome.diagnostics.transaction_id.as_ref(),
            Some(&outcome.preview.transaction_id)
        );
    }

    #[test]
    fn apply_consumes_pending_preview_and_records_evidence() {
        let project = TempProject::new("apply");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Ask),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        let result = access.apply(
            &preview.preview_id,
            &preview.permission_decision_id,
            &mut log,
        );

        assert!(result.is_ok());
        let Some(result) = result.ok() else {
            return;
        };
        assert_eq!(result.transaction_id, preview.transaction_id);
        assert!(!access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("goodbye\n")
        );
        assert!(
            log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::EditTransactionFinished { .. }))
        );
    }

    #[test]
    fn allow_mode_preview_can_apply_through_facade() {
        let project = TempProject::new("allow");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Allow),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        assert_eq!(preview.review_state, EditAccessReviewState::Allowed);
        let apply = access.apply(
            &preview.preview_id,
            &preview.permission_decision_id,
            &mut log,
        );

        assert!(apply.is_ok());
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("goodbye\n")
        );
    }

    #[test]
    fn reject_consumes_pending_preview_without_writing() {
        let project = TempProject::new("reject");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Ask),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        let reject = access.reject(
            &preview.preview_id,
            &preview.permission_decision_id,
            &mut log,
        );

        assert!(reject.is_ok());
        assert!(!access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
        assert!(log.events.iter().any(|event| matches!(
            event,
            SessionEvent::PermissionDecisionRecorded { summary, .. }
                if summary.outcome == PermissionDecisionOutcome::Denied
                    && summary.reason == "user_rejected"
                    && summary.user_override
        )));
    }

    #[test]
    fn deny_mode_rejects_before_preview() {
        let project = TempProject::new("deny");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();

        let error = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Deny),
            &mut log,
        );

        assert!(matches!(
            error,
            Err(EditAccessError::PermissionDenied { .. })
        ));
        assert!(
            log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::PermissionDecisionRecorded { .. }))
        );
        assert!(
            !log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn permission_evidence_redacts_absolute_request_paths_before_preview() {
        let project = TempProject::new("absolute-path");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let absolute_path = project
            .root()
            .join("file.txt")
            .to_string_lossy()
            .into_owned();

        let error = access.prepare(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: absolute_path,
                    expected_sha256: crate::sha256_hex_for_test("hello\n"),
                    hunks: vec![EditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            },
            context(PermissionMode::Deny),
            &mut log,
        );

        assert!(error.is_err());
        let summary = log.events.iter().find_map(|event| match event {
            SessionEvent::PermissionDecisionRecorded { summary, .. } => Some(summary),
            _ => None,
        });
        assert_eq!(
            summary.map(|summary| summary.target.resource.as_str()),
            Some("<absolute_path>")
        );
    }

    #[test]
    fn permission_prompt_uses_sanitized_resource_before_preview() {
        let absolute_path = String::from("/tmp/secret.txt");
        let request = EditTransactionRequest {
            operations: vec![EditOperation::ModifyTextFile {
                path: absolute_path,
                expected_sha256: crate::sha256_hex_for_test("hello\n"),
                hunks: vec![EditHunk {
                    find: String::from("hello"),
                    replace: String::from("goodbye"),
                }],
            }],
        };

        let permission_request = super::permission_request_from_edit(&request);
        let decision = crate::PermissionDecisionEngine::decide(
            &permission_request,
            &PermissionPolicy::for_edit_mode(PermissionMode::Ask),
        );

        let crate::PermissionDecision::NeedsUserReview { prompt, .. } = decision else {
            assert!(matches!(
                decision,
                crate::PermissionDecision::NeedsUserReview { .. }
            ));
            return;
        };
        assert_eq!(permission_request.target.resource, "<absolute_path>");
        assert!(prompt.body.contains("<absolute_path>"));
        assert!(!prompt.body.contains("/tmp/secret.txt"));
    }

    #[test]
    fn permission_prompt_redacts_metadata_resources_before_preview() {
        for path in [
            ".git/config",
            "target/debug/output",
            "./.yach/sessions/session.jsonl",
        ] {
            let request = EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from(path),
                    expected_sha256: crate::sha256_hex_for_test("hello\n"),
                    hunks: vec![EditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            };

            let permission_request = super::permission_request_from_edit(&request);
            let decision = crate::PermissionDecisionEngine::decide(
                &permission_request,
                &PermissionPolicy::for_edit_mode(PermissionMode::Ask),
            );

            let crate::PermissionDecision::NeedsUserReview { prompt, .. } = decision else {
                assert!(matches!(
                    decision,
                    crate::PermissionDecision::NeedsUserReview { .. }
                ));
                return;
            };
            assert_eq!(permission_request.target.resource, "<metadata_path>");
            assert!(prompt.body.contains("<metadata_path>"));
            assert!(!prompt.body.contains(path));
        }
    }

    #[test]
    fn preview_validation_failure_records_edit_evidence() {
        let project = TempProject::new("preview-validation-failure");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();

        let error = access.prepare(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("file.txt"),
                    expected_sha256: String::from("wrong"),
                    hunks: vec![EditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            },
            context(PermissionMode::Ask),
            &mut log,
        );

        assert!(matches!(error, Err(EditAccessError::Preview(_))));
        assert!(log.events.iter().any(|event| matches!(
            event,
            SessionEvent::EditTransactionFinished {
                transaction_id: None,
                outcome: EditEvidenceOutcome::ValidationFailed,
                reason: Some(reason),
                summary: None,
                ..
            } if reason == "hash_mismatch"
        )));
        assert!(
            !log.events
                .iter()
                .any(|event| matches!(event, SessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn stale_preview_id_fails_safely() {
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let error = access.apply(
            &EditPreviewId(String::from("missing")),
            &PermissionDecisionId(String::from("permission-decision-missing")),
            &mut log,
        );

        assert_eq!(error, Err(EditAccessError::PreviewNotFound));
    }

    #[test]
    fn decision_mismatch_keeps_pending_preview_for_later_decision() {
        let project = TempProject::new("decision-mismatch");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut access = EditAccess::default();
        let mut log = SessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(PermissionMode::Ask),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        let error = access.apply(
            &preview.preview_id,
            &PermissionDecisionId(String::from("permission-decision-wrong")),
            &mut log,
        );

        assert_eq!(error, Err(EditAccessError::DecisionMismatch));
        assert!(access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
    }
}
