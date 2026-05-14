use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::NativeResourceRoot;

static EDIT_TRANSACTION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditTransactionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeEditTransactionRequest {
    pub operations: Vec<NativeEditOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum NativeEditOperation {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        hunks: Vec<NativeEditHunk>,
    },
    CreateTextFile {
        path: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeEditHunk {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedNativeEditTransaction {
    pub transaction_id: NativeEditTransactionId,
    pub operations: Vec<PreparedNativeEditOperation>,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedNativeEditOperation {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeEditPolicy {
    pub max_operations: usize,
    pub max_file_bytes: u64,
    pub max_transaction_bytes: usize,
    pub max_diff_summary_bytes: usize,
    pub allow_create: bool,
    pub allow_modify: bool,
}

impl NativeEditPolicy {
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
pub enum NativeEditError {
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
    SymlinkRejected {
        path: String,
    },
    ExpectedFile {
        path: String,
    },
    UnsupportedMetadataPath {
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

pub struct NativeEditEngine;

impl NativeEditEngine {
    pub fn preview(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        policy: &NativeEditPolicy,
    ) -> Result<PreparedNativeEditTransaction, NativeEditError> {
        let operation_count = request.operations.len();
        if operation_count == 0 {
            return Err(NativeEditError::EmptyTransaction);
        }
        if operation_count > policy.max_operations {
            return Err(NativeEditError::TooManyOperations {
                max_operations: policy.max_operations,
                actual_operations: operation_count,
            });
        }

        let transaction_id = next_edit_transaction_id();
        let mut operations = Vec::new();
        for operation in request.operations {
            match operation {
                NativeEditOperation::CreateTextFile { path, content } => {
                    if !policy.allow_create {
                        return Err(NativeEditError::CreateDisabled);
                    }
                    let resolved = root.canonical_path().join(&path);
                    operations.push(PreparedNativeEditOperation::CreateTextFile {
                        relative_path: path,
                        resolved_path: resolved,
                        after_sha256: sha256_hex(content.as_bytes()),
                        after_bytes: content.len(),
                    });
                }
                NativeEditOperation::ModifyTextFile { path, .. } => {
                    if !policy.allow_modify {
                        return Err(NativeEditError::ModifyDisabled);
                    }
                    return Err(NativeEditError::TargetMissing { path });
                }
            }
        }

        Ok(PreparedNativeEditTransaction {
            transaction_id,
            operations,
            operation_count,
            diff_summary: String::new(),
            diff_summary_truncated: false,
            diff_summary_bytes: 0,
        })
    }
}

fn next_edit_transaction_id() -> NativeEditTransactionId {
    let sequence = EDIT_TRANSACTION_COUNTER.fetch_add(1, Ordering::Relaxed);
    NativeEditTransactionId(format!("edit-{sequence}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NativeResourceRoot;
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
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_edit_preview_prepares_create_without_writing_file() {
        let project = TempProject::new("create-smoke");
        std::fs::create_dir_all(project.root().join("src")).unwrap();
        let root = NativeResourceRoot::project(project.root()).unwrap();

        let preview = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &NativeEditPolicy::test(),
        )
        .unwrap();

        assert!(matches!(
            preview.operations.as_slice(),
            [PreparedNativeEditOperation::CreateTextFile { .. }]
        ));
        assert!(preview.transaction_id.0.starts_with("edit-"));
        assert_eq!(preview.operation_count, 1);
        assert!(!project.root().join("src/new.rs").exists());
    }
}
