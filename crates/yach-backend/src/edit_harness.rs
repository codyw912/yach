use crate::{
    NativeEditAppliedOperation, NativeEditApplyResult, NativeEditEngine, NativeEditError,
    NativeEditEvidenceOutcome, NativeEditEvidenceSummary, NativeEditOperationEvidence,
    NativeEditPolicy, NativeEditTransactionRequest, NativeResourceRoot, NativeSessionEvent,
    NativeSessionId, NativeSessionLog, NativeToolPayloadSummary, NativeToolRequestId, NativeTurnId,
    PreparedNativeEditOperation, PreparedNativeEditTransaction, native_edit_error_label,
};

#[expect(
    clippy::struct_field_names,
    reason = "session evidence context uses domain id names"
)]
pub(crate) struct NativeEditHarnessContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub tool_request_id: Option<NativeToolRequestId>,
}

pub(crate) struct NativeEditHarness;

impl NativeEditHarness {
    pub(crate) fn preview_and_apply(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        policy: NativeEditPolicy,
        log: &mut NativeSessionLog,
        context: NativeEditHarnessContext,
    ) -> Result<NativeEditApplyResult, NativeEditError> {
        Self::preview_and_apply_with_apply_policy(root, request, policy, policy, log, context)
    }

    pub(crate) fn preview_and_apply_with_apply_policy(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        preview_policy: NativeEditPolicy,
        apply_policy: NativeEditPolicy,
        log: &mut NativeSessionLog,
        context: NativeEditHarnessContext,
    ) -> Result<NativeEditApplyResult, NativeEditError> {
        let preview = match NativeEditEngine::preview(root, request, &preview_policy) {
            Ok(preview) => preview,
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: None,
                    outcome: NativeEditEvidenceOutcome::ValidationFailed,
                    reason: Some(native_edit_error_label(&error).to_owned()),
                    summary: None,
                });
                return Err(error);
            }
        };

        let prepared_summary = native_edit_prepared_evidence_summary(&preview);
        let transaction_id = preview.transaction_id.clone();
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: context.tool_request_id.clone(),
            transaction_id: transaction_id.clone(),
            summary: prepared_summary.clone(),
        });

        match NativeEditEngine::apply(root, preview, &apply_policy) {
            Ok(result) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(native_edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id,
                    turn_id: context.turn_id,
                    tool_request_id: context.tool_request_id,
                    transaction_id: Some(transaction_id),
                    outcome: NativeEditEvidenceOutcome::Failed,
                    reason: Some(native_edit_error_label(&error).to_owned()),
                    summary: Some(prepared_summary),
                });
                Err(error)
            }
        }
    }
}

pub(crate) fn native_edit_prepared_evidence_summary(
    transaction: &PreparedNativeEditTransaction,
) -> NativeEditEvidenceSummary {
    NativeEditEvidenceSummary {
        operation_count: transaction.operation_count,
        operations: transaction
            .operations
            .iter()
            .map(prepared_operation_evidence)
            .collect(),
        diff_summary: NativeToolPayloadSummary {
            summary: String::from("edit diff summary redacted"),
            byte_count: transaction.diff_summary_bytes,
            redacted: true,
            truncated: transaction.diff_summary_truncated,
        },
    }
}

pub(crate) fn native_edit_apply_evidence_summary(
    result: &NativeEditApplyResult,
) -> NativeEditEvidenceSummary {
    NativeEditEvidenceSummary {
        operation_count: result.operation_count,
        operations: result
            .operations
            .iter()
            .map(applied_operation_evidence)
            .collect(),
        diff_summary: NativeToolPayloadSummary {
            summary: String::from("edit diff summary redacted"),
            byte_count: result.diff_summary_bytes,
            redacted: true,
            truncated: result.diff_summary_truncated,
        },
    }
}

fn prepared_operation_evidence(
    operation: &PreparedNativeEditOperation,
) -> NativeEditOperationEvidence {
    match operation {
        PreparedNativeEditOperation::ModifyTextFile {
            relative_path,
            before_sha256,
            after_sha256,
            before_bytes,
            after_bytes,
            hunk_count,
            ..
        } => NativeEditOperationEvidence::ModifyTextFile {
            relative_path: relative_path.clone(),
            before_sha256: before_sha256.clone(),
            after_sha256: after_sha256.clone(),
            before_bytes: *before_bytes,
            after_bytes: *after_bytes,
            hunk_count: *hunk_count,
            bytes_written: None,
        },
        PreparedNativeEditOperation::CreateTextFile {
            relative_path,
            after_sha256,
            after_bytes,
            ..
        } => NativeEditOperationEvidence::CreateTextFile {
            relative_path: relative_path.clone(),
            after_sha256: after_sha256.clone(),
            after_bytes: *after_bytes,
            bytes_written: None,
        },
    }
}

fn applied_operation_evidence(
    operation: &NativeEditAppliedOperation,
) -> NativeEditOperationEvidence {
    match operation {
        NativeEditAppliedOperation::ModifyTextFile {
            relative_path,
            before_sha256,
            after_sha256,
            before_bytes,
            after_bytes,
            hunk_count,
            bytes_written,
        } => NativeEditOperationEvidence::ModifyTextFile {
            relative_path: relative_path.clone(),
            before_sha256: before_sha256.clone(),
            after_sha256: after_sha256.clone(),
            before_bytes: *before_bytes,
            after_bytes: *after_bytes,
            hunk_count: *hunk_count,
            bytes_written: Some(*bytes_written),
        },
        NativeEditAppliedOperation::CreateTextFile {
            relative_path,
            after_sha256,
            after_bytes,
            bytes_written,
        } => NativeEditOperationEvidence::CreateTextFile {
            relative_path: relative_path.clone(),
            after_sha256: after_sha256.clone(),
            after_bytes: *after_bytes,
            bytes_written: Some(*bytes_written),
        },
    }
}
