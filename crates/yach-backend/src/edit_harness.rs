use crate::{
    EditAppliedOperation, EditApplyResult, EditEngine, EditError, EditEvidenceOutcome,
    EditEvidenceSummary, EditOperationEvidence, EditPolicy, EditTransactionRequest,
    PreparedEditOperation, PreparedEditTransaction, ResourceRoot, SessionEvent, SessionId,
    SessionLog, ToolPayloadSummary, ToolRequestId, TurnId, edit_error_label,
};

#[expect(
    clippy::struct_field_names,
    reason = "session evidence context uses domain id names"
)]
pub(crate) struct EditHarnessContext {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub tool_request_id: Option<ToolRequestId>,
}

pub(crate) struct EditHarness;

impl EditHarness {
    pub(crate) fn preview_and_apply(
        root: &ResourceRoot,
        request: EditTransactionRequest,
        policy: EditPolicy,
        log: &mut SessionLog,
        context: EditHarnessContext,
    ) -> Result<EditApplyResult, EditError> {
        Self::preview_and_apply_with_apply_policy(root, request, policy, policy, log, context)
    }

    pub(crate) fn preview_and_apply_with_apply_policy(
        root: &ResourceRoot,
        request: EditTransactionRequest,
        preview_policy: EditPolicy,
        apply_policy: EditPolicy,
        log: &mut SessionLog,
        context: EditHarnessContext,
    ) -> Result<EditApplyResult, EditError> {
        let preview = match EditEngine::preview(root, request, &preview_policy) {
            Ok(preview) => preview,
            Err(error) => {
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: None,
                    outcome: EditEvidenceOutcome::ValidationFailed,
                    reason: Some(edit_error_label(&error).to_owned()),
                    summary: None,
                });
                return Err(error);
            }
        };

        let prepared_summary = edit_prepared_evidence_summary(&preview);
        let transaction_id = preview.transaction_id.clone();
        log.push(SessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: context.tool_request_id.clone(),
            transaction_id: transaction_id.clone(),
            summary: prepared_summary.clone(),
        });

        match EditEngine::apply(root, preview, &apply_policy) {
            Ok(result) => {
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(SessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: Some(transaction_id),
                    outcome: EditEvidenceOutcome::Failed,
                    reason: Some(edit_error_label(&error).to_owned()),
                    summary: Some(prepared_summary),
                });
                Err(error)
            }
        }
    }
}

pub(crate) fn edit_prepared_evidence_summary(
    transaction: &PreparedEditTransaction,
) -> EditEvidenceSummary {
    EditEvidenceSummary {
        operation_count: transaction.operation_count,
        operations: transaction
            .operations
            .iter()
            .map(prepared_operation_evidence)
            .collect(),
        diff_summary: ToolPayloadSummary {
            summary: String::from("edit diff summary redacted"),
            byte_count: transaction.diff_summary_bytes,
            redacted: true,
            truncated: transaction.diff_summary_truncated,
        },
    }
}

pub(crate) fn edit_apply_evidence_summary(result: &EditApplyResult) -> EditEvidenceSummary {
    EditEvidenceSummary {
        operation_count: result.operation_count,
        operations: result
            .operations
            .iter()
            .map(applied_operation_evidence)
            .collect(),
        diff_summary: ToolPayloadSummary {
            summary: String::from("edit diff summary redacted"),
            byte_count: result.diff_summary_bytes,
            redacted: true,
            truncated: result.diff_summary_truncated,
        },
    }
}

fn prepared_operation_evidence(operation: &PreparedEditOperation) -> EditOperationEvidence {
    match operation {
        PreparedEditOperation::ModifyTextFile {
            relative_path,
            before_sha256,
            after_sha256,
            before_bytes,
            after_bytes,
            hunk_count,
            ..
        } => EditOperationEvidence::ModifyTextFile {
            relative_path: relative_path.clone(),
            before_sha256: before_sha256.clone(),
            after_sha256: after_sha256.clone(),
            before_bytes: *before_bytes,
            after_bytes: *after_bytes,
            hunk_count: *hunk_count,
            bytes_written: None,
        },
        PreparedEditOperation::CreateTextFile {
            relative_path,
            after_sha256,
            after_bytes,
            ..
        } => EditOperationEvidence::CreateTextFile {
            relative_path: relative_path.clone(),
            after_sha256: after_sha256.clone(),
            after_bytes: *after_bytes,
            bytes_written: None,
        },
    }
}

fn applied_operation_evidence(operation: &EditAppliedOperation) -> EditOperationEvidence {
    match operation {
        EditAppliedOperation::ModifyTextFile {
            relative_path,
            before_sha256,
            after_sha256,
            before_bytes,
            after_bytes,
            hunk_count,
            bytes_written,
        } => EditOperationEvidence::ModifyTextFile {
            relative_path: relative_path.clone(),
            before_sha256: before_sha256.clone(),
            after_sha256: after_sha256.clone(),
            before_bytes: *before_bytes,
            after_bytes: *after_bytes,
            hunk_count: *hunk_count,
            bytes_written: Some(*bytes_written),
        },
        EditAppliedOperation::CreateTextFile {
            relative_path,
            after_sha256,
            after_bytes,
            bytes_written,
        } => EditOperationEvidence::CreateTextFile {
            relative_path: relative_path.clone(),
            after_sha256: after_sha256.clone(),
            after_bytes: *after_bytes,
            bytes_written: Some(*bytes_written),
        },
    }
}
