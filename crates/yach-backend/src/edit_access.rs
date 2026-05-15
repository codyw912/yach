use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::edit_harness::{
    native_edit_apply_evidence_summary, native_edit_prepared_evidence_summary,
};
use crate::{
    NativeEditApplyResult, NativeEditEngine, NativeEditError, NativeEditEvidenceOutcome,
    NativeEditOperation, NativeEditPolicy, NativeEditTransactionId, NativeEditTransactionRequest,
    NativePermissionActor, NativePermissionCapability, NativePermissionDecision,
    NativePermissionDecisionEngine, NativePermissionDecisionId, NativePermissionPolicy,
    NativePermissionRequest, NativePermissionRisk, NativePermissionTargetSummary,
    NativeResourceRoot, NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeToolRequestId,
    NativeTurnId, PreparedNativeEditTransaction, native_edit_error_label,
};

static EDIT_PREVIEW_COUNTER: AtomicU64 = AtomicU64::new(0);
static EDIT_PERMISSION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditPreviewId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditAccessContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub permission_policy: NativePermissionPolicy,
    pub edit_policy: NativeEditPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditAccessReviewState {
    Allowed,
    NeedsUserApproval,
    AutoReviewUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditPreview {
    pub preview_id: NativeEditPreviewId,
    pub transaction_id: NativeEditTransactionId,
    pub permission_decision_id: NativePermissionDecisionId,
    pub review_state: NativeEditAccessReviewState,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEditAccessError {
    PermissionDenied { reason: String },
    Preview(NativeEditError),
    Apply(NativeEditError),
    PreviewNotFound,
    DecisionMismatch,
}

#[derive(Debug)]
struct PendingNativeEditPreview {
    context: NativeEditAccessContext,
    root: NativeResourceRoot,
    prepared: PreparedNativeEditTransaction,
    permission_decision_id: NativePermissionDecisionId,
}

#[derive(Debug, Default)]
pub struct NativeEditAccess {
    pending: BTreeMap<String, PendingNativeEditPreview>,
}

impl NativeEditAccess {
    pub fn prepare(
        &mut self,
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        context: NativeEditAccessContext,
        log: &mut NativeSessionLog,
    ) -> Result<NativeEditPreview, NativeEditAccessError> {
        let permission_request = permission_request_from_edit(&request);
        let decision =
            NativePermissionDecisionEngine::decide(&permission_request, &context.permission_policy);
        log.record_permission_decision(
            context.session_id.clone(),
            context.turn_id.clone(),
            decision.summary(&permission_request, false),
        );

        let review_state = match &decision {
            NativePermissionDecision::Allowed { .. } => NativeEditAccessReviewState::Allowed,
            NativePermissionDecision::NeedsUserReview { reason, .. }
                if reason == "auto_review_unavailable_fallback_ask" =>
            {
                NativeEditAccessReviewState::AutoReviewUnavailable
            }
            NativePermissionDecision::NeedsUserReview { .. } => {
                NativeEditAccessReviewState::NeedsUserApproval
            }
            NativePermissionDecision::Denied { reason, .. } => {
                return Err(NativeEditAccessError::PermissionDenied {
                    reason: reason.clone(),
                });
            }
        };

        let prepared = match NativeEditEngine::preview(root, request, &context.edit_policy) {
            Ok(prepared) => prepared,
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: None::<NativeToolRequestId>,
                    transaction_id: None,
                    outcome: NativeEditEvidenceOutcome::ValidationFailed,
                    reason: Some(native_edit_error_label(&error).to_owned()),
                    summary: None,
                });
                return Err(NativeEditAccessError::Preview(error));
            }
        };
        let summary = native_edit_prepared_evidence_summary(&prepared);
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: None::<NativeToolRequestId>,
            transaction_id: prepared.transaction_id.clone(),
            summary,
        });

        let preview_id = NativeEditPreviewId(next_edit_preview_id());
        let preview = NativeEditPreview {
            preview_id: preview_id.clone(),
            transaction_id: prepared.transaction_id.clone(),
            permission_decision_id: decision.decision_id(),
            review_state,
            operation_count: prepared.operation_count,
            diff_summary: prepared.diff_summary.clone(),
            diff_summary_truncated: prepared.diff_summary_truncated,
            diff_summary_bytes: prepared.diff_summary_bytes,
        };
        self.pending.insert(
            preview_id.0.clone(),
            PendingNativeEditPreview {
                context,
                root: root.clone(),
                prepared,
                permission_decision_id: preview.permission_decision_id.clone(),
            },
        );
        Ok(preview)
    }

    pub fn apply(
        &mut self,
        preview_id: &NativeEditPreviewId,
        decision_id: &NativePermissionDecisionId,
        log: &mut NativeSessionLog,
    ) -> Result<NativeEditApplyResult, NativeEditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(NativeEditAccessError::PreviewNotFound)?;
        if &pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(NativeEditAccessError::DecisionMismatch);
        }

        let transaction_id = pending.prepared.transaction_id.clone();
        match NativeEditEngine::apply(
            &pending.root,
            pending.prepared,
            &pending.context.edit_policy,
        ) {
            Ok(result) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id,
                    turn_id: pending.context.turn_id,
                    tool_request_id: None::<NativeToolRequestId>,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(native_edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: pending.context.session_id,
                    turn_id: pending.context.turn_id,
                    tool_request_id: None::<NativeToolRequestId>,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Failed,
                    reason: Some(native_edit_error_label(&error).to_owned()),
                    summary: None,
                });
                Err(NativeEditAccessError::Apply(error))
            }
        }
    }

    pub fn reject(
        &mut self,
        preview_id: &NativeEditPreviewId,
        decision_id: &NativePermissionDecisionId,
        log: &mut NativeSessionLog,
    ) -> Result<(), NativeEditAccessError> {
        let pending = self
            .pending
            .remove(&preview_id.0)
            .ok_or(NativeEditAccessError::PreviewNotFound)?;
        if &pending.permission_decision_id != decision_id {
            self.pending.insert(preview_id.0.clone(), pending);
            return Err(NativeEditAccessError::DecisionMismatch);
        }

        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: pending.context.session_id,
            turn_id: pending.context.turn_id,
            tool_request_id: None::<NativeToolRequestId>,
            transaction_id: Some(pending.prepared.transaction_id.clone()),
            outcome: NativeEditEvidenceOutcome::Failed,
            reason: Some(String::from("user_rejected")),
            summary: Some(native_edit_prepared_evidence_summary(&pending.prepared)),
        });
        Ok(())
    }

    #[must_use]
    pub fn has_pending_preview(&self, preview_id: &NativeEditPreviewId) -> bool {
        self.pending.contains_key(&preview_id.0)
    }
}

fn permission_request_from_edit(request: &NativeEditTransactionRequest) -> NativePermissionRequest {
    let (operation, resource) = request.operations.first().map_or_else(
        || {
            (
                String::from("empty_edit_transaction"),
                String::from("<none>"),
            )
        },
        |operation| match operation {
            NativeEditOperation::ModifyTextFile { path, .. } => (
                String::from("modify_text_file"),
                summarized_permission_resource(path),
            ),
            NativeEditOperation::CreateTextFile { path, .. } => (
                String::from("create_text_file"),
                summarized_permission_resource(path),
            ),
        },
    );

    NativePermissionRequest {
        request_id: next_edit_permission_request_id(),
        actor: NativePermissionActor::UserLocalUi,
        capability: NativePermissionCapability::EditTransaction,
        target: NativePermissionTargetSummary {
            operation,
            resource,
        },
        risk: NativePermissionRisk::WorkspaceWrite,
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
        || components
            .as_slice()
            .starts_with(&[".yach", "native-sessions"])
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
    use super::{
        NativeEditAccess, NativeEditAccessContext, NativeEditAccessError,
        NativeEditAccessReviewState,
    };
    use crate::{
        NativeEditEvidenceOutcome, NativeEditHunk, NativeEditOperation, NativeEditPolicy,
        NativeEditPreviewId, NativeEditTransactionRequest, NativePermissionDecisionId,
        NativePermissionMode, NativePermissionPolicy, NativeResourceRoot, NativeSessionEvent,
        NativeSessionId, NativeSessionLog, NativeTurnId,
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

    fn context(mode: NativePermissionMode) -> NativeEditAccessContext {
        NativeEditAccessContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(mode),
            edit_policy: NativeEditPolicy::test(),
        }
    }

    fn native_root(project: &TempProject) -> Option<NativeResourceRoot> {
        let root = NativeResourceRoot::project(project.root());
        assert!(root.is_ok());
        root.ok()
    }

    fn write_file(project: &TempProject, relative_path: &str, content: &str) {
        assert!(std::fs::write(project.root().join(relative_path), content).is_ok());
    }

    fn modify_request() -> NativeEditTransactionRequest {
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: String::from("file.txt"),
                expected_sha256: crate::sha256_hex_for_test("hello\n"),
                hunks: vec![NativeEditHunk {
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
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();

        let preview = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Ask),
            &mut log,
        );

        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };
        assert_eq!(
            preview.review_state,
            NativeEditAccessReviewState::NeedsUserApproval
        );
        assert!(access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
        assert!(
            log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::PermissionDecisionRecorded { .. }
            ))
        );
        assert!(
            log.events
                .iter()
                .any(|event| matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn apply_consumes_pending_preview_and_records_evidence() {
        let project = TempProject::new("apply");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Ask),
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
                .any(|event| matches!(event, NativeSessionEvent::EditTransactionFinished { .. }))
        );
    }

    #[test]
    fn allow_mode_preview_can_apply_through_facade() {
        let project = TempProject::new("allow");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Allow),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        assert_eq!(preview.review_state, NativeEditAccessReviewState::Allowed);
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
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Ask),
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
    }

    #[test]
    fn deny_mode_rejects_before_preview() {
        let project = TempProject::new("deny");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();

        let error = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Deny),
            &mut log,
        );

        assert!(matches!(
            error,
            Err(NativeEditAccessError::PermissionDenied { .. })
        ));
        assert!(
            log.events.iter().any(|event| matches!(
                event,
                NativeSessionEvent::PermissionDecisionRecorded { .. }
            ))
        );
        assert!(
            !log.events
                .iter()
                .any(|event| matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn permission_evidence_redacts_absolute_request_paths_before_preview() {
        let project = TempProject::new("absolute-path");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let absolute_path = project
            .root()
            .join("file.txt")
            .to_string_lossy()
            .into_owned();

        let error = access.prepare(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: absolute_path,
                    expected_sha256: crate::sha256_hex_for_test("hello\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            },
            context(NativePermissionMode::Deny),
            &mut log,
        );

        assert!(error.is_err());
        let summary = log.events.iter().find_map(|event| match event {
            NativeSessionEvent::PermissionDecisionRecorded { summary, .. } => Some(summary),
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
        let request = NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: absolute_path,
                expected_sha256: crate::sha256_hex_for_test("hello\n"),
                hunks: vec![NativeEditHunk {
                    find: String::from("hello"),
                    replace: String::from("goodbye"),
                }],
            }],
        };

        let permission_request = super::permission_request_from_edit(&request);
        let decision = crate::NativePermissionDecisionEngine::decide(
            &permission_request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
        );

        let crate::NativePermissionDecision::NeedsUserReview { prompt, .. } = decision else {
            assert!(matches!(
                decision,
                crate::NativePermissionDecision::NeedsUserReview { .. }
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
            "./.yach/native-sessions/session.jsonl",
        ] {
            let request = NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from(path),
                    expected_sha256: crate::sha256_hex_for_test("hello\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            };

            let permission_request = super::permission_request_from_edit(&request);
            let decision = crate::NativePermissionDecisionEngine::decide(
                &permission_request,
                &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
            );

            let crate::NativePermissionDecision::NeedsUserReview { prompt, .. } = decision else {
                assert!(matches!(
                    decision,
                    crate::NativePermissionDecision::NeedsUserReview { .. }
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
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();

        let error = access.prepare(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("file.txt"),
                    expected_sha256: String::from("wrong"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("hello"),
                        replace: String::from("goodbye"),
                    }],
                }],
            },
            context(NativePermissionMode::Ask),
            &mut log,
        );

        assert!(matches!(error, Err(NativeEditAccessError::Preview(_))));
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EditTransactionFinished {
                transaction_id: None,
                outcome: NativeEditEvidenceOutcome::ValidationFailed,
                reason: Some(reason),
                summary: None,
                ..
            } if reason == "hash_mismatch"
        )));
        assert!(
            !log.events
                .iter()
                .any(|event| matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }))
        );
    }

    #[test]
    fn stale_preview_id_fails_safely() {
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let error = access.apply(
            &NativeEditPreviewId(String::from("missing")),
            &NativePermissionDecisionId(String::from("permission-decision-missing")),
            &mut log,
        );

        assert_eq!(error, Err(NativeEditAccessError::PreviewNotFound));
    }

    #[test]
    fn decision_mismatch_keeps_pending_preview_for_later_decision() {
        let project = TempProject::new("decision-mismatch");
        write_file(&project, "file.txt", "hello\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut access = NativeEditAccess::default();
        let mut log = NativeSessionLog::default();
        let preview = access.prepare(
            &root,
            modify_request(),
            context(NativePermissionMode::Ask),
            &mut log,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        let error = access.apply(
            &preview.preview_id,
            &NativePermissionDecisionId(String::from("permission-decision-wrong")),
            &mut log,
        );

        assert_eq!(error, Err(NativeEditAccessError::DecisionMismatch));
        assert!(access.has_pending_preview(&preview.preview_id));
        assert_eq!(
            std::fs::read_to_string(project.root().join("file.txt"))
                .ok()
                .as_deref(),
            Some("hello\n")
        );
    }
}
