use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::ResourceRoot;

static EDIT_TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditTransactionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditTransactionRequest {
    pub operations: Vec<EditOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EditOperation {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        hunks: Vec<EditHunk>,
    },
    ReplaceTextFile {
        path: String,
        expected_sha256: String,
        content: String,
    },
    CreateTextFile {
        path: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditHunk {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedEditTransaction {
    pub transaction_id: EditTransactionId,
    pub operations: Vec<PreparedEditOperation>,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
    apply_payloads: Vec<PreparedEditApplyPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedEditOperation {
    ModifyTextFile {
        relative_path: String,
        resolved_path: PathBuf,
        before_sha256: String,
        after_sha256: String,
        before_bytes: usize,
        after_bytes: usize,
        hunk_count: usize,
    },
    CreateTextFile {
        relative_path: String,
        resolved_path: PathBuf,
        after_sha256: String,
        after_bytes: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PreparedEditApplyPayload {
    ModifyTextFile { after_content: String },
    CreateTextFile { content: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditApplyResult {
    pub transaction_id: EditTransactionId,
    pub outcome: EditApplyOutcome,
    pub operations: Vec<EditAppliedOperation>,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditApplyOutcome {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditAppliedOperation {
    ModifyTextFile {
        relative_path: String,
        before_sha256: String,
        after_sha256: String,
        before_bytes: usize,
        after_bytes: usize,
        hunk_count: usize,
        bytes_written: usize,
    },
    CreateTextFile {
        relative_path: String,
        after_sha256: String,
        after_bytes: usize,
        bytes_written: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditPolicy {
    pub max_operations: usize,
    pub max_file_bytes: u64,
    pub max_transaction_bytes: usize,
    pub max_diff_summary_bytes: usize,
    pub allow_create: bool,
    pub allow_modify: bool,
}

impl EditPolicy {
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_operations: 1,
            max_file_bytes: 256 * 1024,
            max_transaction_bytes: 128 * 1024,
            max_diff_summary_bytes: 32 * 1024,
            allow_create: true,
            allow_modify: true,
        }
    }
    #[must_use]
    pub const fn extension_proposal() -> Self {
        Self {
            max_operations: 16,
            max_file_bytes: 512 * 1024,
            max_transaction_bytes: 2 * 1024 * 1024,
            max_diff_summary_bytes: 64 * 1024,
            allow_create: false,
            allow_modify: true,
        }
    }

    #[must_use]
    pub const fn test() -> Self {
        Self {
            max_operations: 1,
            max_file_bytes: 1024,
            max_transaction_bytes: 1024,
            max_diff_summary_bytes: 1024,
            allow_create: true,
            allow_modify: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    EmptyTransaction,
    TooManyOperations {
        max_operations: usize,
        actual_operations: usize,
    },
    TransactionTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    CreateDisabled,
    ModifyDisabled,
    AbsolutePath {
        path: String,
    },
    PathTraversal {
        path: String,
    },
    PathOutsideRoot {
        path: String,
    },
    ParentMissing {
        path: String,
    },
    TargetMissing {
        path: String,
    },
    TargetExists {
        path: String,
    },
    DuplicateTarget {
        path: String,
    },
    SymlinkRejected {
        path: String,
    },
    ExpectedFile {
        path: String,
    },
    UnsupportedMetadataPath {
        path: String,
    },
    SensitivePathDenied {
        path: String,
    },
    UnsupportedFileType {
        path: String,
    },
    NotUtf8 {
        path: String,
    },
    FileTooLarge {
        path: String,
        max_bytes: u64,
        actual_bytes: u64,
    },
    HunkNotFound {
        path: String,
    },
    HunkAmbiguous {
        path: String,
    },
    EmptyHunks {
        path: String,
    },
    EmptyFind {
        path: String,
    },
    HashMismatch {
        path: String,
        expected_sha256: String,
        actual_sha256: String,
    },
    Io {
        path: String,
    },
}

pub struct EditEngine;

impl EditEngine {
    pub fn preview(
        root: &ResourceRoot,
        request: EditTransactionRequest,
        policy: &EditPolicy,
    ) -> Result<PreparedEditTransaction, EditError> {
        let operation_count = request.operations.len();
        if operation_count == 0 {
            return Err(EditError::EmptyTransaction);
        }
        if operation_count > policy.max_operations {
            return Err(EditError::TooManyOperations {
                max_operations: policy.max_operations,
                actual_operations: operation_count,
            });
        }
        let request_bytes = estimate_request_bytes(&request);
        if request_bytes > policy.max_transaction_bytes {
            return Err(EditError::TransactionTooLarge {
                max_bytes: policy.max_transaction_bytes,
                actual_bytes: request_bytes,
            });
        }

        let transaction_id = next_edit_transaction_id();
        let mut operations = Vec::new();
        let mut apply_payloads = Vec::new();
        let mut diff_summary = String::new();
        let mut seen_targets = BTreeSet::new();
        for operation in request.operations {
            match operation {
                EditOperation::CreateTextFile { path, content } => {
                    if !policy.allow_create {
                        return Err(EditError::CreateDisabled);
                    }
                    if u64::try_from(content.len())
                        .map_or(true, |actual| actual > policy.max_file_bytes)
                    {
                        return Err(EditError::FileTooLarge {
                            path,
                            max_bytes: policy.max_file_bytes,
                            actual_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                        });
                    }
                    let (relative_path, resolved) = resolve_create_target(root, &path)?;
                    reject_duplicate_target(&mut seen_targets, &relative_path)?;
                    diff_summary.push_str(&render_diff_summary(&relative_path, "", &content));
                    operations.push(PreparedEditOperation::CreateTextFile {
                        relative_path,
                        resolved_path: resolved,
                        after_sha256: sha256_hex(content.as_bytes()),
                        after_bytes: content.len(),
                    });
                    apply_payloads.push(PreparedEditApplyPayload::CreateTextFile { content });
                }
                EditOperation::ModifyTextFile {
                    path,
                    expected_sha256,
                    hunks,
                } => {
                    if !policy.allow_modify {
                        return Err(EditError::ModifyDisabled);
                    }
                    let (relative_path, resolved_path, before) =
                        read_existing_text(root, &path, policy)?;
                    reject_duplicate_target(&mut seen_targets, &relative_path)?;
                    let before_sha256 = sha256_hex(before.as_bytes());
                    if before_sha256 != expected_sha256 {
                        return Err(EditError::HashMismatch {
                            path: relative_path,
                            expected_sha256,
                            actual_sha256: before_sha256,
                        });
                    }
                    let after = apply_hunks(&relative_path, &before, &hunks)?;
                    if u64::try_from(after.len())
                        .map_or(true, |actual| actual > policy.max_file_bytes)
                    {
                        return Err(EditError::FileTooLarge {
                            path: relative_path,
                            max_bytes: policy.max_file_bytes,
                            actual_bytes: u64::try_from(after.len()).unwrap_or(u64::MAX),
                        });
                    }
                    let after_sha256 = sha256_hex(after.as_bytes());
                    let after_bytes = after.len();
                    diff_summary.push_str(&render_diff_summary(&relative_path, &before, &after));
                    operations.push(PreparedEditOperation::ModifyTextFile {
                        relative_path,
                        resolved_path,
                        before_sha256,
                        after_sha256,
                        before_bytes: before.len(),
                        after_bytes,
                        hunk_count: hunks.len(),
                    });
                    apply_payloads.push(PreparedEditApplyPayload::ModifyTextFile {
                        after_content: after,
                    });
                }
                EditOperation::ReplaceTextFile {
                    path,
                    expected_sha256,
                    content,
                } => {
                    if !policy.allow_modify {
                        return Err(EditError::ModifyDisabled);
                    }
                    if u64::try_from(content.len())
                        .map_or(true, |actual| actual > policy.max_file_bytes)
                    {
                        return Err(EditError::FileTooLarge {
                            path,
                            max_bytes: policy.max_file_bytes,
                            actual_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                        });
                    }
                    let (relative_path, resolved_path, before) =
                        read_existing_text(root, &path, policy)?;
                    reject_duplicate_target(&mut seen_targets, &relative_path)?;
                    let before_sha256 = sha256_hex(before.as_bytes());
                    if before_sha256 != expected_sha256 {
                        return Err(EditError::HashMismatch {
                            path: relative_path,
                            expected_sha256,
                            actual_sha256: before_sha256,
                        });
                    }
                    let after_sha256 = sha256_hex(content.as_bytes());
                    diff_summary.push_str(&render_diff_summary(&relative_path, &before, &content));
                    operations.push(PreparedEditOperation::ModifyTextFile {
                        relative_path,
                        resolved_path,
                        before_sha256,
                        after_sha256,
                        before_bytes: before.len(),
                        after_bytes: content.len(),
                        hunk_count: 1,
                    });
                    apply_payloads.push(PreparedEditApplyPayload::ModifyTextFile {
                        after_content: content,
                    });
                }
            }
        }
        let (diff_summary, diff_summary_truncated, diff_summary_bytes) =
            truncate_diff_summary(diff_summary, policy.max_diff_summary_bytes);

        Ok(PreparedEditTransaction {
            transaction_id,
            operations,
            operation_count,
            diff_summary,
            diff_summary_truncated,
            diff_summary_bytes,
            apply_payloads,
        })
    }

    pub(crate) fn apply(
        root: &ResourceRoot,
        transaction: PreparedEditTransaction,
        policy: &EditPolicy,
    ) -> Result<EditApplyResult, EditError> {
        let operation_count = transaction.operations.len();
        if operation_count == 0 {
            return Err(EditError::EmptyTransaction);
        }
        if operation_count > 1 {
            return apply_multiple_operations_atomically(root, transaction, policy);
        }
        if operation_count > policy.max_operations {
            return Err(EditError::TooManyOperations {
                max_operations: policy.max_operations,
                actual_operations: operation_count,
            });
        }
        if transaction.apply_payloads.len() != operation_count {
            return Err(EditError::Io {
                path: String::from("<edit-transaction>"),
            });
        }

        let Some(operation) = transaction.operations.into_iter().next() else {
            return Err(EditError::EmptyTransaction);
        };
        let Some(payload) = transaction.apply_payloads.into_iter().next() else {
            return Err(EditError::EmptyTransaction);
        };
        let applied_operation = match (operation, payload) {
            (
                operation @ PreparedEditOperation::CreateTextFile { .. },
                payload @ PreparedEditApplyPayload::CreateTextFile { .. },
            ) => apply_create_operation(
                root,
                &transaction.transaction_id,
                operation,
                payload,
                policy,
            )?,
            (
                operation @ PreparedEditOperation::ModifyTextFile { .. },
                payload @ PreparedEditApplyPayload::ModifyTextFile { .. },
            ) => apply_modify_operation(
                root,
                &transaction.transaction_id,
                operation,
                payload,
                policy,
            )?,
            _ => {
                return Err(EditError::Io {
                    path: String::from("<edit-transaction>"),
                });
            }
        };

        Ok(EditApplyResult {
            transaction_id: transaction.transaction_id,
            outcome: EditApplyOutcome::Completed,
            operations: vec![applied_operation],
            operation_count,
            diff_summary: transaction.diff_summary,
            diff_summary_truncated: transaction.diff_summary_truncated,
            diff_summary_bytes: transaction.diff_summary_bytes,
        })
    }
}
#[derive(Debug)]
enum StagedOperationKind {
    Create,
    Modify { backup_path: PathBuf },
}

#[derive(Debug)]
struct StagedOperation {
    relative_path: String,
    resolved_path: PathBuf,
    temp_path: PathBuf,
    expected_before_sha256: Option<String>,
    kind: StagedOperationKind,
    applied: EditAppliedOperation,
}

fn apply_multiple_operations_atomically(
    root: &ResourceRoot,
    transaction: PreparedEditTransaction,
    policy: &EditPolicy,
) -> Result<EditApplyResult, EditError> {
    let operation_count = transaction.operations.len();
    if operation_count > policy.max_operations {
        return Err(EditError::TooManyOperations {
            max_operations: policy.max_operations,
            actual_operations: operation_count,
        });
    }
    if transaction.apply_payloads.len() != operation_count {
        return Err(EditError::Io {
            path: String::from("<edit-transaction>"),
        });
    }

    let mut staged = Vec::with_capacity(operation_count);
    for (operation, payload) in transaction
        .operations
        .into_iter()
        .zip(transaction.apply_payloads)
    {
        match stage_operation(
            root,
            &transaction.transaction_id,
            operation,
            payload,
            policy,
        ) {
            Ok(operation) => staged.push(operation),
            Err(error) => {
                cleanup_staged_operations(&staged);
                return Err(error);
            }
        }
    }
    for operation in &staged {
        if let Err(error) = revalidate_staged_operation(root, operation, policy) {
            cleanup_staged_operations(&staged);
            return Err(error);
        }
    }

    for (published, operation) in staged.iter().enumerate() {
        let publish = match operation.kind {
            StagedOperationKind::Create => {
                std::fs::hard_link(&operation.temp_path, &operation.resolved_path)
            }
            StagedOperationKind::Modify { .. } => {
                std::fs::rename(&operation.temp_path, &operation.resolved_path)
            }
        };
        if publish.is_err() {
            rollback_staged_operations(&staged[..published]);
            cleanup_staged_operations(&staged);
            return Err(EditError::Io {
                path: operation.relative_path.clone(),
            });
        }
    }

    let operations = staged
        .iter()
        .map(|operation| operation.applied.clone())
        .collect();
    cleanup_staged_operations(&staged);
    Ok(EditApplyResult {
        transaction_id: transaction.transaction_id,
        outcome: EditApplyOutcome::Completed,
        operations,
        operation_count,
        diff_summary: transaction.diff_summary,
        diff_summary_truncated: transaction.diff_summary_truncated,
        diff_summary_bytes: transaction.diff_summary_bytes,
    })
}

fn stage_operation(
    root: &ResourceRoot,
    transaction_id: &EditTransactionId,
    operation: PreparedEditOperation,
    payload: PreparedEditApplyPayload,
    policy: &EditPolicy,
) -> Result<StagedOperation, EditError> {
    match (operation, payload) {
        (
            PreparedEditOperation::CreateTextFile {
                relative_path,
                resolved_path,
                after_sha256,
                after_bytes,
            },
            PreparedEditApplyPayload::CreateTextFile { content },
        ) => {
            if !policy.allow_create {
                return Err(EditError::CreateDisabled);
            }
            let (fresh_relative, fresh_resolved) = resolve_create_target(root, &relative_path)?;
            if fresh_relative != relative_path || fresh_resolved != resolved_path {
                return Err(EditError::PathOutsideRoot {
                    path: relative_path,
                });
            }
            let actual_after_sha256 = sha256_hex(content.as_bytes());
            if actual_after_sha256 != after_sha256 || content.len() != after_bytes {
                return Err(EditError::HashMismatch {
                    path: relative_path,
                    expected_sha256: after_sha256,
                    actual_sha256: actual_after_sha256,
                });
            }
            let temp_path = temp_path_for(&resolved_path, transaction_id);
            if let Err(error) =
                write_temp_file(&temp_path, content.as_bytes(), None, &relative_path)
            {
                cleanup_temp_file(&temp_path);
                return Err(error);
            }
            Ok(StagedOperation {
                relative_path: relative_path.clone(),
                resolved_path,
                temp_path,
                expected_before_sha256: None,
                kind: StagedOperationKind::Create,
                applied: EditAppliedOperation::CreateTextFile {
                    relative_path,
                    after_sha256,
                    after_bytes,
                    bytes_written: content.len(),
                },
            })
        }
        (
            PreparedEditOperation::ModifyTextFile {
                relative_path,
                resolved_path,
                before_sha256,
                after_sha256,
                before_bytes,
                after_bytes,
                hunk_count,
            },
            PreparedEditApplyPayload::ModifyTextFile { after_content },
        ) => {
            if !policy.allow_modify {
                return Err(EditError::ModifyDisabled);
            }
            let (fresh_relative, fresh_resolved, current_text) =
                read_existing_text(root, &relative_path, policy)?;
            let actual_before_sha256 = sha256_hex(current_text.as_bytes());
            if fresh_relative != relative_path || fresh_resolved != resolved_path {
                return Err(EditError::PathOutsideRoot {
                    path: relative_path,
                });
            }
            if actual_before_sha256 != before_sha256 || current_text.len() != before_bytes {
                return Err(EditError::HashMismatch {
                    path: relative_path,
                    expected_sha256: before_sha256,
                    actual_sha256: actual_before_sha256,
                });
            }
            let actual_after_sha256 = sha256_hex(after_content.as_bytes());
            if actual_after_sha256 != after_sha256 || after_content.len() != after_bytes {
                return Err(EditError::HashMismatch {
                    path: relative_path,
                    expected_sha256: after_sha256,
                    actual_sha256: actual_after_sha256,
                });
            }
            let permissions = std::fs::metadata(&resolved_path)
                .map(|metadata| metadata.permissions())
                .map_err(|_| EditError::Io {
                    path: relative_path.clone(),
                })?;
            let temp_path = temp_path_for(&resolved_path, transaction_id);
            if let Err(error) = write_temp_file(
                &temp_path,
                after_content.as_bytes(),
                Some(permissions),
                &relative_path,
            ) {
                cleanup_temp_file(&temp_path);
                return Err(error);
            }
            let backup_path = backup_path_for(&resolved_path, transaction_id);
            if std::fs::hard_link(&resolved_path, &backup_path).is_err() {
                cleanup_temp_file(&temp_path);
                cleanup_temp_file(&backup_path);
                return Err(EditError::Io {
                    path: relative_path,
                });
            }
            Ok(StagedOperation {
                relative_path: relative_path.clone(),
                resolved_path,
                temp_path,
                expected_before_sha256: Some(before_sha256.clone()),
                kind: StagedOperationKind::Modify { backup_path },
                applied: EditAppliedOperation::ModifyTextFile {
                    relative_path,
                    before_sha256,
                    after_sha256,
                    before_bytes,
                    after_bytes,
                    hunk_count,
                    bytes_written: after_content.len(),
                },
            })
        }
        _ => Err(EditError::Io {
            path: String::from("<edit-transaction>"),
        }),
    }
}

fn backup_path_for(target: &Path, transaction_id: &EditTransactionId) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("target");
    target.with_file_name(format!(
        ".{file_name}.{}.{}.bak",
        transaction_id.0,
        std::process::id()
    ))
}

fn revalidate_staged_operation(
    root: &ResourceRoot,
    operation: &StagedOperation,
    policy: &EditPolicy,
) -> Result<(), EditError> {
    match &operation.expected_before_sha256 {
        None => {
            let (relative_path, resolved_path) =
                resolve_create_target(root, &operation.relative_path)?;
            if relative_path != operation.relative_path || resolved_path != operation.resolved_path
            {
                return Err(EditError::PathOutsideRoot {
                    path: operation.relative_path.clone(),
                });
            }
        }
        Some(expected_sha256) => {
            let (relative_path, resolved_path, current_text) =
                read_existing_text(root, &operation.relative_path, policy)?;
            if relative_path != operation.relative_path || resolved_path != operation.resolved_path
            {
                return Err(EditError::PathOutsideRoot {
                    path: operation.relative_path.clone(),
                });
            }
            let actual_sha256 = sha256_hex(current_text.as_bytes());
            if actual_sha256 != *expected_sha256 {
                return Err(EditError::HashMismatch {
                    path: operation.relative_path.clone(),
                    expected_sha256: expected_sha256.clone(),
                    actual_sha256,
                });
            }
        }
    }
    Ok(())
}

fn rollback_staged_operations(operations: &[StagedOperation]) {
    for operation in operations.iter().rev() {
        match &operation.kind {
            StagedOperationKind::Create => {
                let _ = std::fs::remove_file(&operation.resolved_path);
            }
            StagedOperationKind::Modify { backup_path } => {
                let _ = std::fs::rename(backup_path, &operation.resolved_path);
            }
        }
    }
}

fn cleanup_staged_operations(operations: &[StagedOperation]) {
    for operation in operations {
        cleanup_temp_file(&operation.temp_path);
        if let StagedOperationKind::Modify { backup_path } = &operation.kind {
            cleanup_temp_file(backup_path);
        }
    }
}

pub(crate) fn edit_error_label(error: &EditError) -> &'static str {
    match error {
        EditError::EmptyTransaction => "empty_transaction",
        EditError::TooManyOperations { .. } => "too_many_operations",
        EditError::TransactionTooLarge { .. } => "transaction_too_large",
        EditError::CreateDisabled => "create_disabled",
        EditError::ModifyDisabled => "modify_disabled",
        EditError::AbsolutePath { .. } => "absolute_path",
        EditError::PathTraversal { .. } => "path_traversal",
        EditError::PathOutsideRoot { .. } => "path_outside_root",
        EditError::ParentMissing { .. } => "parent_missing",
        EditError::TargetMissing { .. } => "target_missing",
        EditError::TargetExists { .. } => "target_exists",
        EditError::DuplicateTarget { .. } => "duplicate_target",
        EditError::SymlinkRejected { .. } => "symlink_rejected",
        EditError::ExpectedFile { .. } => "expected_file",
        EditError::UnsupportedMetadataPath { .. } => "unsupported_metadata_path",
        EditError::SensitivePathDenied { .. } => "sensitive_path_denied",
        EditError::UnsupportedFileType { .. } => "unsupported_file_type",
        EditError::NotUtf8 { .. } => "not_utf8",
        EditError::FileTooLarge { .. } => "file_too_large",
        EditError::HunkNotFound { .. } => "hunk_not_found",
        EditError::HunkAmbiguous { .. } => "hunk_ambiguous",
        EditError::EmptyHunks { .. } => "empty_hunks",
        EditError::EmptyFind { .. } => "empty_find",
        EditError::HashMismatch { .. } => "hash_mismatch",
        EditError::Io { .. } => "io",
    }
}

fn estimate_request_bytes(request: &EditTransactionRequest) -> usize {
    serde_json::to_vec(request).map_or(usize::MAX, |bytes| bytes.len())
}

fn temp_path_for(target: &Path, transaction_id: &EditTransactionId) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("target");
    let temp_name = format!(
        ".{file_name}.{}.{}.tmp",
        transaction_id.0,
        std::process::id()
    );
    target.with_file_name(temp_name)
}

fn write_temp_file(
    temp_path: &Path,
    content: &[u8],
    permissions: Option<std::fs::Permissions>,
    relative_path: &str,
) -> Result<File, EditError> {
    let mut temp_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|_| EditError::Io {
            path: relative_path.to_owned(),
        })?;

    if let Some(permissions) = permissions {
        temp_file
            .set_permissions(permissions)
            .map_err(|_| EditError::Io {
                path: relative_path.to_owned(),
            })?;
    }

    temp_file
        .write_all(content)
        .and_then(|()| temp_file.sync_all())
        .map_err(|_| EditError::Io {
            path: relative_path.to_owned(),
        })?;
    Ok(temp_file)
}

fn cleanup_temp_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}

fn apply_create_operation(
    root: &ResourceRoot,
    transaction_id: &EditTransactionId,
    operation: PreparedEditOperation,
    payload: PreparedEditApplyPayload,
    policy: &EditPolicy,
) -> Result<EditAppliedOperation, EditError> {
    let PreparedEditOperation::CreateTextFile {
        relative_path,
        resolved_path,
        after_sha256,
        after_bytes,
    } = operation
    else {
        unreachable!("apply_create_operation only accepts prepared create operations");
    };
    let PreparedEditApplyPayload::CreateTextFile { content } = payload else {
        unreachable!("apply_create_operation only accepts prepared create payloads");
    };

    if !policy.allow_create {
        return Err(EditError::CreateDisabled);
    }
    if u64::try_from(content.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(EditError::FileTooLarge {
            path: relative_path,
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
        });
    }

    let (fresh_relative, fresh_resolved) = resolve_create_target(root, &relative_path)?;
    if fresh_relative != relative_path || fresh_resolved != resolved_path {
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }
    let actual_after_sha256 = sha256_hex(content.as_bytes());
    if actual_after_sha256 != after_sha256 || content.len() != after_bytes {
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: after_sha256,
            actual_sha256: actual_after_sha256,
        });
    }

    let (_, fresh_resolved) = resolve_create_target(root, &relative_path)?;
    if fresh_resolved != resolved_path {
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }

    let temp_path = temp_path_for(&resolved_path, transaction_id);
    match write_temp_file(&temp_path, content.as_bytes(), None, &relative_path) {
        Ok(_temp_file) => {}
        Err(error) => {
            cleanup_temp_file(&temp_path);
            return Err(error);
        }
    }

    let (_, fresh_resolved) = resolve_create_target(root, &relative_path)?;
    if fresh_resolved != resolved_path {
        cleanup_temp_file(&temp_path);
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }

    let publish_result = std::fs::hard_link(&temp_path, &resolved_path);
    cleanup_temp_file(&temp_path);
    if publish_result.is_err() {
        if std::fs::symlink_metadata(&resolved_path).is_ok() {
            return Err(EditError::TargetExists {
                path: relative_path,
            });
        }
        return Err(EditError::Io {
            path: relative_path,
        });
    }

    Ok(EditAppliedOperation::CreateTextFile {
        relative_path,
        after_sha256,
        after_bytes,
        bytes_written: content.len(),
    })
}

fn apply_modify_operation(
    root: &ResourceRoot,
    transaction_id: &EditTransactionId,
    operation: PreparedEditOperation,
    payload: PreparedEditApplyPayload,
    policy: &EditPolicy,
) -> Result<EditAppliedOperation, EditError> {
    let PreparedEditOperation::ModifyTextFile {
        relative_path,
        resolved_path,
        before_sha256,
        after_sha256,
        before_bytes,
        after_bytes,
        hunk_count,
    } = operation
    else {
        unreachable!("apply_modify_operation only accepts prepared modify operations");
    };
    let PreparedEditApplyPayload::ModifyTextFile { after_content } = payload else {
        unreachable!("apply_modify_operation only accepts prepared modify payloads");
    };

    if !policy.allow_modify {
        return Err(EditError::ModifyDisabled);
    }

    let (fresh_relative, fresh_resolved, current_text) =
        read_existing_text(root, &relative_path, policy)?;
    if fresh_relative != relative_path || fresh_resolved != resolved_path {
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }

    let actual_before_sha256 = sha256_hex(current_text.as_bytes());
    if actual_before_sha256 != before_sha256 {
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: actual_before_sha256,
        });
    }
    if current_text.len() != before_bytes {
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: sha256_hex(current_text.as_bytes()),
        });
    }
    let actual_after_sha256 = sha256_hex(after_content.as_bytes());
    if actual_after_sha256 != after_sha256 || after_content.len() != after_bytes {
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: after_sha256,
            actual_sha256: actual_after_sha256,
        });
    }
    if u64::try_from(after_content.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(EditError::FileTooLarge {
            path: relative_path,
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(after_content.len()).unwrap_or(u64::MAX),
        });
    }

    let (_, final_resolved, final_text) = read_existing_text(root, &relative_path, policy)?;
    if final_resolved != resolved_path {
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }
    let final_before_sha256 = sha256_hex(final_text.as_bytes());
    if final_before_sha256 != before_sha256 {
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: final_before_sha256,
        });
    }

    let permissions = std::fs::metadata(&resolved_path)
        .map(|metadata| metadata.permissions())
        .map_err(|_| EditError::Io {
            path: relative_path.clone(),
        })?;
    let temp_path = temp_path_for(&resolved_path, transaction_id);
    match write_temp_file(
        &temp_path,
        after_content.as_bytes(),
        Some(permissions),
        &relative_path,
    ) {
        Ok(_temp_file) => {}
        Err(error) => {
            cleanup_temp_file(&temp_path);
            return Err(error);
        }
    }

    let (_, final_resolved, final_text) = read_existing_text(root, &relative_path, policy)?;
    if final_resolved != resolved_path {
        cleanup_temp_file(&temp_path);
        return Err(EditError::PathOutsideRoot {
            path: relative_path,
        });
    }
    let final_before_sha256 = sha256_hex(final_text.as_bytes());
    if final_before_sha256 != before_sha256 {
        cleanup_temp_file(&temp_path);
        return Err(EditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: final_before_sha256,
        });
    }

    std::fs::rename(&temp_path, &resolved_path).map_err(|_| {
        cleanup_temp_file(&temp_path);
        EditError::Io {
            path: relative_path.clone(),
        }
    })?;

    Ok(EditAppliedOperation::ModifyTextFile {
        relative_path,
        before_sha256,
        after_sha256,
        before_bytes,
        after_bytes,
        hunk_count,
        bytes_written: after_content.len(),
    })
}

fn reject_duplicate_target(
    seen_targets: &mut BTreeSet<String>,
    relative_path: &str,
) -> Result<(), EditError> {
    if seen_targets.insert(relative_path.to_owned()) {
        return Ok(());
    }

    Err(EditError::DuplicateTarget {
        path: relative_path.to_owned(),
    })
}

fn validate_relative_path(path: &str) -> Result<PathBuf, EditError> {
    let path_ref = Path::new(path);
    if path_ref.is_absolute() {
        return Err(EditError::AbsolutePath {
            path: path.to_owned(),
        });
    }

    let mut normalized = PathBuf::new();
    for component in path_ref.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(EditError::PathTraversal {
                    path: path.to_owned(),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(EditError::AbsolutePath {
                    path: path.to_owned(),
                });
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(EditError::PathTraversal {
            path: path.to_owned(),
        });
    }

    reject_metadata_path(path, &normalized)?;
    Ok(normalized)
}

fn reject_sensitive_path(
    root: &ResourceRoot,
    original: &str,
    normalized: &Path,
) -> Result<(), EditError> {
    if root.sensitive_denies(normalized) {
        return Err(EditError::SensitivePathDenied {
            path: original.to_owned(),
        });
    }
    Ok(())
}

fn reject_metadata_path(original: &str, normalized: &Path) -> Result<(), EditError> {
    let components = normalized
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();

    if components.first() == Some(&".git")
        || components.first() == Some(&"target")
        || components.as_slice().starts_with(&[".yach", "sessions"])
    {
        return Err(EditError::UnsupportedMetadataPath {
            path: original.to_owned(),
        });
    }

    Ok(())
}

fn resolve_create_target(root: &ResourceRoot, path: &str) -> Result<(String, PathBuf), EditError> {
    let normalized = validate_relative_path(path)?;
    reject_sensitive_path(root, path, &normalized)?;
    let parent = normalized.parent().unwrap_or_else(|| Path::new(""));
    reject_symlinked_parent(root, path, parent)?;
    let canonical_parent = root
        .canonical_path()
        .join(parent)
        .canonicalize()
        .map_err(|_| EditError::ParentMissing {
            path: path.to_owned(),
        })?;

    if !canonical_parent.starts_with(root.canonical_path()) {
        return Err(EditError::PathOutsideRoot {
            path: path.to_owned(),
        });
    }
    if !canonical_parent.is_dir() {
        return Err(EditError::UnsupportedFileType {
            path: path.to_owned(),
        });
    }

    let file_name = normalized
        .file_name()
        .ok_or_else(|| EditError::PathTraversal {
            path: path.to_owned(),
        })?;
    let resolved = canonical_parent.join(file_name);
    if std::fs::symlink_metadata(&resolved).is_ok() {
        return Err(EditError::TargetExists {
            path: path.to_owned(),
        });
    }

    Ok((path_to_slash_string(&normalized), resolved))
}

fn read_existing_text(
    root: &ResourceRoot,
    path: &str,
    policy: &EditPolicy,
) -> Result<(String, PathBuf, String), EditError> {
    let normalized = validate_relative_path(path)?;
    reject_sensitive_path(root, path, &normalized)?;
    let relative_path = path_to_slash_string(&normalized);
    let parent = normalized.parent().unwrap_or_else(|| Path::new(""));
    reject_symlinked_parent(root, path, parent)?;
    let unresolved = root.canonical_path().join(&normalized);
    let link_metadata =
        std::fs::symlink_metadata(&unresolved).map_err(|_| EditError::TargetMissing {
            path: path.to_owned(),
        })?;
    if link_metadata.file_type().is_symlink() {
        return Err(EditError::SymlinkRejected {
            path: path.to_owned(),
        });
    }

    let resolved = root
        .resolve_file(&normalized)
        .map_err(|error| match error {
            crate::ResourcePathError::Missing => EditError::TargetMissing {
                path: path.to_owned(),
            },
            crate::ResourcePathError::EscapesRoot => EditError::PathOutsideRoot {
                path: path.to_owned(),
            },
            crate::ResourcePathError::ExpectedFile => EditError::ExpectedFile {
                path: path.to_owned(),
            },
            crate::ResourcePathError::SensitiveDenied => EditError::SensitivePathDenied {
                path: path.to_owned(),
            },
            crate::ResourcePathError::RootUnavailable
            | crate::ResourcePathError::ExpectedDirectory => EditError::Io {
                path: path.to_owned(),
            },
        })?;

    let metadata = std::fs::metadata(&resolved).map_err(|_| EditError::Io {
        path: path.to_owned(),
    })?;
    if metadata.len() > policy.max_file_bytes {
        return Err(EditError::FileTooLarge {
            path: path.to_owned(),
            max_bytes: policy.max_file_bytes,
            actual_bytes: metadata.len(),
        });
    }
    let bytes = std::fs::read(&resolved).map_err(|_| EditError::Io {
        path: path.to_owned(),
    })?;
    if u64::try_from(bytes.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(EditError::FileTooLarge {
            path: path.to_owned(),
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    let text = String::from_utf8(bytes).map_err(|_| EditError::NotUtf8 {
        path: path.to_owned(),
    })?;
    Ok((relative_path, resolved, text))
}

pub(crate) fn edit_read_existing_text(
    root: &ResourceRoot,
    path: &str,
    policy: &EditPolicy,
) -> Result<(String, String), EditError> {
    let (relative_path, _resolved_path, text) = read_existing_text(root, path, policy)?;
    Ok((relative_path, text))
}

fn apply_hunks(
    relative_path: &str,
    original: &str,
    hunks: &[EditHunk],
) -> Result<String, EditError> {
    if hunks.is_empty() {
        return Err(EditError::EmptyHunks {
            path: relative_path.to_owned(),
        });
    }

    let mut text = original.to_owned();
    for hunk in hunks {
        if hunk.find.is_empty() {
            return Err(EditError::EmptyFind {
                path: relative_path.to_owned(),
            });
        }
        let matches = overlapping_match_indices(&text, &hunk.find);
        match matches.as_slice() {
            [] => {
                return Err(EditError::HunkNotFound {
                    path: relative_path.to_owned(),
                });
            }
            [_] => {
                text = text.replacen(&hunk.find, &hunk.replace, 1);
            }
            _ => {
                return Err(EditError::HunkAmbiguous {
                    path: relative_path.to_owned(),
                });
            }
        }
    }
    Ok(text)
}

fn overlapping_match_indices(text: &str, find: &str) -> Vec<usize> {
    text.char_indices()
        .filter_map(|(index, _)| text[index..].starts_with(find).then_some(index))
        .collect()
}

fn render_diff_summary(relative_path: &str, before: &str, after: &str) -> String {
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(4)
        .header(relative_path, relative_path)
        .to_string()
}

fn truncate_diff_summary(summary: String, max_bytes: usize) -> (String, bool, usize) {
    if summary.len() <= max_bytes {
        let byte_count = summary.len();
        return (summary, false, byte_count);
    }

    let marker = "\n[diff truncated]\n";
    if max_bytes <= marker.len() {
        let mut boundary = max_bytes;
        while !marker.is_char_boundary(boundary) {
            boundary = boundary.saturating_sub(1);
        }
        let truncated = marker[..boundary].to_owned();
        let byte_count = truncated.len();
        return (truncated, true, byte_count);
    }

    let mut boundary = max_bytes - marker.len();
    while !summary.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    let mut truncated = summary[..boundary].to_owned();
    truncated.push_str(marker);
    let byte_count = truncated.len();
    (truncated, true, byte_count)
}

fn reject_symlinked_parent(
    root: &ResourceRoot,
    original: &str,
    parent: &Path,
) -> Result<(), EditError> {
    let mut current = root.canonical_path().to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        let metadata =
            std::fs::symlink_metadata(&current).map_err(|_| EditError::ParentMissing {
                path: original.to_owned(),
            })?;
        if metadata.file_type().is_symlink() {
            return Err(EditError::SymlinkRejected {
                path: original.to_owned(),
            });
        }
    }
    Ok(())
}

fn path_to_slash_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn next_edit_transaction_id() -> EditTransactionId {
    let sequence = EDIT_TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    EditTransactionId(format!("edit-{sequence}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

pub(crate) fn edit_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

#[cfg(test)]
pub(crate) fn sha256_hex_for_test(content: &str) -> String {
    sha256_hex(content.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceRoot;
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
                "yach-edit-preview-{name}-{}-{sequence}",
                std::process::id()
            ));
            assert!(std::fs::create_dir_all(&root).is_ok());
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                assert!(std::fs::create_dir_all(parent).is_ok());
            }
            assert!(std::fs::write(path, content).is_ok());
        }
        fn read(&self, relative_path: &str) -> String {
            std::fs::read_to_string(self.root.join(relative_path)).unwrap_or_default()
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn edit_preview_prepares_create_without_writing_file() {
        let project = TempProject::new("create-smoke");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };

        assert!(matches!(
            preview.operations.as_slice(),
            [PreparedEditOperation::CreateTextFile { .. }]
        ));
        assert!(preview.transaction_id.0.starts_with("edit-"));
        assert_eq!(preview.operation_count, 1);
        assert!(!project.root().join("src/new.rs").exists());
    }

    #[test]
    fn edit_apply_creates_text_file_from_preview() {
        let project = TempProject::new("apply-create");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        assert!(!project.root().join("src/new.rs").exists());

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let apply_result = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert!(apply_result.is_ok());
        let Some(applied) = apply_result.ok() else {
            return;
        };
        assert_eq!(applied.outcome, EditApplyOutcome::Completed);
        assert_eq!(applied.operation_count, 1);
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/new.rs")).ok(),
            Some(String::from("pub fn created() {}\n"))
        );
        assert!(matches!(
            applied.operations.as_slice(),
            [EditAppliedOperation::CreateTextFile {
                relative_path,
                after_sha256,
                after_bytes: 20,
                bytes_written: 20,
            }] if relative_path == "src/new.rs"
                && after_sha256 == &test_sha256_hex("pub fn created() {}\n")
        ));
    }

    #[test]
    fn edit_apply_create_fails_if_target_appears_after_preview() {
        let project = TempProject::new("apply-create-race");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("from preview\n"),
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        project.write("src/new.rs", "concurrent\n");

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let error = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert_eq!(
            error,
            Err(EditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/new.rs")).ok(),
            Some(String::from("concurrent\n"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_apply_create_rejects_parent_symlink_added_after_preview() {
        let project = TempProject::new("apply-create-parent-link");
        assert!(std::fs::create_dir_all(project.root().join("real")).is_ok());
        assert!(std::fs::create_dir_all(project.root().join("link")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("link/new.rs"),
                    content: String::from("content\n"),
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        assert!(std::fs::remove_dir(project.root().join("link")).is_ok());
        assert!(
            std::os::unix::fs::symlink(project.root().join("real"), project.root().join("link"))
                .is_ok()
        );

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let error = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert_eq!(
            error,
            Err(EditError::SymlinkRejected {
                path: String::from("link/new.rs")
            })
        );
        assert!(!project.root().join("real/new.rs").exists());
    }

    #[cfg(unix)]
    #[test]
    fn edit_apply_create_treats_dangling_symlink_race_as_target_exists() {
        let project = TempProject::new("apply-create-dangling-link-race");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("from preview\n"),
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("missing-target.rs"),
                project.root().join("src/new.rs"),
            )
            .is_ok()
        );

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let error = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert_eq!(
            error,
            Err(EditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
        assert!(std::fs::symlink_metadata(project.root().join("src/new.rs")).is_ok());
    }

    #[test]
    fn edit_preview_rejects_absolute_create_path() {
        let project = TempProject::new("absolute-create");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("/tmp/outside.rs"),
                    content: String::from("outside"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::AbsolutePath {
                path: String::from("/tmp/outside.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_parent_traversal_create_path() {
        let project = TempProject::new("traversal-create");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("../outside.rs"),
                    content: String::from("outside"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::PathTraversal {
                path: String::from("../outside.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_missing_create_parent() {
        let project = TempProject::new("missing-parent");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("missing/new.rs"),
                    content: String::from("content"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::ParentMissing {
                path: String::from("missing/new.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_file_create_parent() {
        let project = TempProject::new("file-parent");
        project.write("src", "not a directory");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("content"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::UnsupportedFileType {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_existing_create_target() {
        let project = TempProject::new("target-exists");
        project.write("src/new.rs", "existing");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("replacement"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_metadata_and_root_target_paths() {
        let project = TempProject::new("metadata-paths");
        assert!(std::fs::create_dir_all(project.root().join(".yach")).is_ok());
        assert!(std::fs::create_dir_all(project.root().join("target")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        for path in [
            ".git/config",
            ".yach/sessions/session.jsonl",
            "target/out.rs",
        ] {
            let error = EditEngine::preview(
                &root,
                EditTransactionRequest {
                    operations: vec![EditOperation::CreateTextFile {
                        path: String::from(path),
                        content: String::from("content"),
                    }],
                },
                &EditPolicy::test(),
            );

            assert_eq!(
                error,
                Err(EditError::UnsupportedMetadataPath {
                    path: String::from(path)
                })
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn edit_preview_rejects_create_through_symlink_parent() {
        let project = TempProject::new("symlink-parent-create");
        assert!(std::fs::create_dir_all(project.root().join("real")).is_ok());
        assert!(
            std::os::unix::fs::symlink(project.root().join("real"), project.root().join("link"))
                .is_ok()
        );
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("link/new.rs"),
                    content: String::from("content"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::SymlinkRejected {
                path: String::from("link/new.rs")
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_preview_rejects_dangling_symlink_create_target_as_existing() {
        let project = TempProject::new("dangling-create-target");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("missing-target.rs"),
                project.root().join("src/new.rs"),
            )
            .is_ok()
        );
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("content"),
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn edit_preview_prepares_modify_with_hashes_and_diff_summary() {
        let project = TempProject::new("modify-preview");
        project.write("src/lib.rs", "pub fn old() {}\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let expected_sha256 = test_sha256_hex("pub fn old() {}\n");

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256,
                    hunks: vec![EditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );

        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };
        assert!(matches!(
            preview.operations.as_slice(),
            [PreparedEditOperation::ModifyTextFile {
                relative_path,
                before_bytes: 16,
                after_bytes: 16,
                hunk_count: 1,
                ..
            }] if relative_path == "src/lib.rs"
        ));
        assert!(preview.diff_summary.contains("--- src/lib.rs"));
        assert!(preview.diff_summary.contains("+++ src/lib.rs"));
        assert!(preview.diff_summary.contains("-pub fn old() {}"));
        assert!(preview.diff_summary.contains("+pub fn new() {}"));
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/lib.rs"))
                .ok()
                .as_deref(),
            Some("pub fn old() {}\n")
        );
    }

    #[test]
    fn edit_apply_modifies_text_file_from_preview() {
        let project = TempProject::new("apply-modify");
        project.write("src/lib.rs", "pub fn old() {}\n");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                    hunks: vec![EditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let apply_result = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert!(apply_result.is_ok());
        let Some(applied) = apply_result.ok() else {
            return;
        };
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/lib.rs")).ok(),
            Some(String::from("pub fn new() {}\n"))
        );
        assert!(matches!(
            applied.operations.as_slice(),
            [EditAppliedOperation::ModifyTextFile {
                relative_path,
                before_sha256,
                after_sha256,
                before_bytes: 16,
                after_bytes: 16,
                hunk_count: 1,
                bytes_written: 16,
            }] if relative_path == "src/lib.rs"
                && before_sha256 == &test_sha256_hex("pub fn old() {}\n")
                && after_sha256 == &test_sha256_hex("pub fn new() {}\n")
        ));
    }

    #[test]
    fn edit_apply_modify_fails_if_file_changed_after_preview() {
        let project = TempProject::new("apply-modify-race");
        project.write("src/lib.rs", "pub fn old() {}\n");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                    hunks: vec![EditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        project.write("src/lib.rs", "pub fn concurrent() {}\n");

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let error = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert_eq!(
            error,
            Err(EditError::HashMismatch {
                path: String::from("src/lib.rs"),
                expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                actual_sha256: test_sha256_hex("pub fn concurrent() {}\n")
            })
        );
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/lib.rs")).ok(),
            Some(String::from("pub fn concurrent() {}\n"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_apply_modify_rejects_target_symlink_added_after_preview() {
        let project = TempProject::new("apply-modify-link");
        project.write("src/lib.rs", "pub fn old() {}\n");
        project.write("src/other.rs", "pub fn other() {}\n");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                    hunks: vec![EditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        assert!(std::fs::remove_file(project.root().join("src/lib.rs")).is_ok());
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("src/other.rs"),
                project.root().join("src/lib.rs"),
            )
            .is_ok()
        );

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let error = EditEngine::apply(&root, preview, &EditPolicy::test());

        assert_eq!(
            error,
            Err(EditError::SymlinkRejected {
                path: String::from("src/lib.rs")
            })
        );
        assert_eq!(
            std::fs::read_to_string(project.root().join("src/other.rs")).ok(),
            Some(String::from("pub fn other() {}\n"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_apply_modify_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let project = TempProject::new("apply-modify-mode");
        project.write("script.sh", "echo old\n");
        assert!(
            std::fs::set_permissions(
                project.root().join("script.sh"),
                std::fs::Permissions::from_mode(0o755),
            )
            .is_ok()
        );
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("script.sh"),
                    expected_sha256: test_sha256_hex("echo old\n"),
                    hunks: vec![EditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );
        assert!(preview_result.is_ok());

        let Some(preview) = preview_result.ok() else {
            return;
        };
        let apply_result = EditEngine::apply(&root, preview, &EditPolicy::test());
        assert!(apply_result.is_ok());

        let mode = std::fs::metadata(project.root().join("script.sh"))
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .ok();
        assert_eq!(mode, Some(0o755));
    }

    #[test]
    fn edit_preview_rejects_hash_mismatch() {
        let project = TempProject::new("hash-mismatch");
        project.write("src/lib.rs", "actual\n");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("expected\n"),
                    hunks: vec![EditHunk {
                        find: String::from("actual"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );

        assert!(matches!(
            error,
            Err(EditError::HashMismatch { path, .. }) if path == "src/lib.rs"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn edit_preview_rejects_symlink_modify_target() {
        let project = TempProject::new("symlink-modify");
        project.write("src/real.rs", "real\n");
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("src/real.rs"),
                project.root().join("src/link.rs"),
            )
            .is_ok()
        );
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/link.rs"),
                    expected_sha256: test_sha256_hex("real\n"),
                    hunks: vec![EditHunk {
                        find: String::from("real"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::SymlinkRejected {
                path: String::from("src/link.rs")
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn edit_preview_rejects_modify_through_symlinked_metadata_parent() {
        let project = TempProject::new("symlink-metadata-parent-modify");
        project.write(".git/config", "protected\n");
        assert!(
            std::os::unix::fs::symlink(project.root().join(".git"), project.root().join("link"))
                .is_ok()
        );
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("link/config"),
                    expected_sha256: test_sha256_hex("protected\n"),
                    hunks: vec![EditHunk {
                        find: String::from("protected"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::SymlinkRejected {
                path: String::from("link/config")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_empty_or_ambiguous_hunks() {
        let project = TempProject::new("hunk-policy");
        project.write("src/lib.rs", "same same\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let expected_sha256 = test_sha256_hex("same same\n");

        let empty = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: expected_sha256.clone(),
                    hunks: Vec::new(),
                }],
            },
            &EditPolicy::test(),
        );
        assert_eq!(
            empty,
            Err(EditError::EmptyHunks {
                path: String::from("src/lib.rs")
            })
        );

        let ambiguous = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256,
                    hunks: vec![EditHunk {
                        find: String::from("same"),
                        replace: String::from("other"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );
        assert_eq!(
            ambiguous,
            Err(EditError::HunkAmbiguous {
                path: String::from("src/lib.rs")
            })
        );
    }

    #[test]
    fn edit_preview_rejects_overlapping_ambiguous_hunks() {
        let project = TempProject::new("overlapping-hunk-policy");
        project.write("src/lib.rs", "aaa\n");
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("aaa\n"),
                    hunks: vec![EditHunk {
                        find: String::from("aa"),
                        replace: String::from("b"),
                    }],
                }],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::HunkAmbiguous {
                path: String::from("src/lib.rs")
            })
        );
    }

    #[test]
    fn edit_preview_create_includes_diff_summary() {
        let project = TempProject::new("create-diff");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &EditPolicy::test(),
        );

        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };
        assert!(preview.diff_summary.contains("--- src/new.rs"));
        assert!(preview.diff_summary.contains("+++ src/new.rs"));
        assert!(preview.diff_summary.contains("+pub fn created() {}"));
    }

    #[test]
    fn edit_preview_rejects_multiple_operations_by_policy() {
        let project = TempProject::new("multi-op");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![
                    EditOperation::CreateTextFile {
                        path: String::from("src/a.rs"),
                        content: String::from("a"),
                    },
                    EditOperation::CreateTextFile {
                        path: String::from("src/b.rs"),
                        content: String::from("b"),
                    },
                ],
            },
            &EditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(EditError::TooManyOperations {
                max_operations: 1,
                actual_operations: 2
            })
        );
    }

    #[test]
    fn edit_preview_rejects_duplicate_targets_when_multi_op_policy_allows_them() {
        let project = TempProject::new("duplicate-target");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut policy = EditPolicy::test();
        policy.max_operations = 2;

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![
                    EditOperation::CreateTextFile {
                        path: String::from("src/new.rs"),
                        content: String::from("a"),
                    },
                    EditOperation::CreateTextFile {
                        path: String::from("src/new.rs"),
                        content: String::from("b"),
                    },
                ],
            },
            &policy,
        );

        assert_eq!(
            error,
            Err(EditError::DuplicateTarget {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn edit_apply_replaces_multiple_files_in_one_transaction() {
        let project = TempProject::new("apply-multi-op");
        project.write("src/a.rs", "alpha\n");
        project.write("src/b.rs", "");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let policy = EditPolicy::extension_proposal();
        let preview = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![
                    EditOperation::ReplaceTextFile {
                        path: String::from("src/a.rs"),
                        expected_sha256: test_sha256_hex("alpha\n"),
                        content: String::from("updated alpha\n"),
                    },
                    EditOperation::ReplaceTextFile {
                        path: String::from("src/b.rs"),
                        expected_sha256: test_sha256_hex(""),
                        content: String::from("created from empty\n"),
                    },
                ],
            },
            &policy,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };

        let applied = EditEngine::apply(&root, preview, &policy);
        assert!(applied.is_ok());
        assert_eq!(project.read("src/a.rs"), "updated alpha\n");
        assert_eq!(project.read("src/b.rs"), "created from empty\n");
    }

    #[test]
    fn edit_apply_stale_member_leaves_all_files_unchanged() {
        let project = TempProject::new("apply-multi-stale");
        project.write("src/a.rs", "alpha\n");
        project.write("src/b.rs", "beta\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let policy = EditPolicy::extension_proposal();
        let preview = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![
                    EditOperation::ReplaceTextFile {
                        path: String::from("src/a.rs"),
                        expected_sha256: test_sha256_hex("alpha\n"),
                        content: String::from("updated alpha\n"),
                    },
                    EditOperation::ReplaceTextFile {
                        path: String::from("src/b.rs"),
                        expected_sha256: test_sha256_hex("beta\n"),
                        content: String::from("updated beta\n"),
                    },
                ],
            },
            &policy,
        );
        assert!(preview.is_ok());
        let Some(preview) = preview.ok() else {
            return;
        };
        project.write("src/b.rs", "changed after preview\n");

        let applied = EditEngine::apply(&root, preview, &policy);
        assert!(matches!(applied, Err(EditError::HashMismatch { .. })));
        assert_eq!(project.read("src/a.rs"), "alpha\n");
        assert_eq!(project.read("src/b.rs"), "changed after preview\n");
    }

    #[test]
    fn edit_apply_result_preserves_bounded_diff_summary() {
        let project = TempProject::new("apply-diff-summary");
        project.write("src/lib.rs", "alpha\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut policy = EditPolicy::test();
        policy.max_diff_summary_bytes = 20;

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("alpha\n"),
                    hunks: vec![EditHunk {
                        find: String::from("alpha"),
                        replace: String::from("beta"),
                    }],
                }],
            },
            &policy,
        );
        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };

        let apply_result = EditEngine::apply(&root, preview, &policy);
        assert!(apply_result.is_ok());
        let Some(applied) = apply_result.ok() else {
            return;
        };
        assert!(applied.diff_summary_truncated);
        assert!(applied.diff_summary_bytes <= policy.max_diff_summary_bytes);
        assert!(applied.diff_summary.contains("[diff truncated]"));
    }

    #[test]
    fn edit_preview_diff_shows_only_changed_hunks_and_nearby_context() {
        let before = "far-before\na\nb\nc\nd\ne\nold\nf\ng\nh\ni\nj\nfar-after\n";
        let after = "far-before\na\nb\nc\nd\ne\nnew\nf\ng\nh\ni\nj\nfar-after\n";

        let diff = render_diff_summary("src/lib.rs", before, after);

        assert!(diff.contains("--- src/lib.rs"));
        assert!(diff.contains("+++ src/lib.rs"));
        assert!(diff.contains("@@"));
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
        assert!(!diff.contains("far-before"));
        assert!(!diff.contains("far-after"));
    }

    #[test]
    fn edit_preview_truncates_large_diff_summary() {
        let project = TempProject::new("diff-truncate");
        project.write("src/lib.rs", "alpha\n");
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut policy = EditPolicy::test();
        policy.max_diff_summary_bytes = 20;

        let preview_result = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("alpha\n"),
                    hunks: vec![EditHunk {
                        find: String::from("alpha"),
                        replace: String::from("beta"),
                    }],
                }],
            },
            &policy,
        );

        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };
        assert!(preview.diff_summary_truncated);
        assert!(preview.diff_summary_bytes <= policy.max_diff_summary_bytes);
        assert!(preview.diff_summary.len() <= policy.max_diff_summary_bytes);
        assert!(preview.diff_summary.contains("[diff truncated]"));
    }

    #[test]
    fn edit_preview_rejects_serialized_transaction_too_large() {
        let project = TempProject::new("transaction-too-large");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = resource_root(&project) else {
            return;
        };
        let mut policy = EditPolicy::test();
        policy.max_transaction_bytes = 8;

        let error = EditEngine::preview(
            &root,
            EditTransactionRequest {
                operations: vec![EditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("this request is intentionally too large"),
                }],
            },
            &policy,
        );

        assert!(matches!(
            error,
            Err(EditError::TransactionTooLarge {
                max_bytes: 8,
                actual_bytes
            }) if actual_bytes > 8
        ));
    }

    fn test_sha256_hex(text: &str) -> String {
        sha256_hex(text.as_bytes())
    }

    fn resource_root(project: &TempProject) -> Option<ResourceRoot> {
        let root = ResourceRoot::project(project.root());
        assert!(root.is_ok());
        root.ok()
    }
}
