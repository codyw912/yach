use std::path::{Component, Path, PathBuf};
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
        let request_bytes = estimate_request_bytes(&request);
        if request_bytes > policy.max_transaction_bytes {
            return Err(NativeEditError::TransactionTooLarge {
                max_bytes: policy.max_transaction_bytes,
                actual_bytes: request_bytes,
            });
        }

        let transaction_id = next_edit_transaction_id();
        let mut operations = Vec::new();
        let mut diff_summary = String::new();
        for operation in request.operations {
            match operation {
                NativeEditOperation::CreateTextFile { path, content } => {
                    if !policy.allow_create {
                        return Err(NativeEditError::CreateDisabled);
                    }
                    if u64::try_from(content.len())
                        .map_or(true, |actual| actual > policy.max_file_bytes)
                    {
                        return Err(NativeEditError::FileTooLarge {
                            path,
                            max_bytes: policy.max_file_bytes,
                            actual_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                        });
                    }
                    let (relative_path, resolved) = resolve_create_target(root, &path)?;
                    diff_summary.push_str(&render_diff_summary(&relative_path, "", &content));
                    operations.push(PreparedNativeEditOperation::CreateTextFile {
                        relative_path,
                        resolved_path: resolved,
                        after_sha256: sha256_hex(content.as_bytes()),
                        after_bytes: content.len(),
                    });
                }
                NativeEditOperation::ModifyTextFile {
                    path,
                    expected_sha256,
                    hunks,
                } => {
                    if !policy.allow_modify {
                        return Err(NativeEditError::ModifyDisabled);
                    }
                    let (relative_path, resolved_path, before) =
                        read_existing_text(root, &path, policy)?;
                    let before_sha256 = sha256_hex(before.as_bytes());
                    if before_sha256 != expected_sha256 {
                        return Err(NativeEditError::HashMismatch {
                            path: relative_path,
                            expected_sha256,
                            actual_sha256: before_sha256,
                        });
                    }
                    let after = apply_hunks(&relative_path, &before, &hunks)?;
                    if u64::try_from(after.len())
                        .map_or(true, |actual| actual > policy.max_file_bytes)
                    {
                        return Err(NativeEditError::FileTooLarge {
                            path: relative_path,
                            max_bytes: policy.max_file_bytes,
                            actual_bytes: u64::try_from(after.len()).unwrap_or(u64::MAX),
                        });
                    }
                    diff_summary.push_str(&render_diff_summary(&relative_path, &before, &after));
                    operations.push(PreparedNativeEditOperation::ModifyTextFile {
                        relative_path,
                        resolved_path,
                        before_sha256,
                        after_sha256: sha256_hex(after.as_bytes()),
                        before_bytes: before.len(),
                        after_bytes: after.len(),
                        hunk_count: hunks.len(),
                    });
                }
            }
        }
        let (diff_summary, diff_summary_truncated, diff_summary_bytes) =
            truncate_diff_summary(diff_summary, policy.max_diff_summary_bytes);

        Ok(PreparedNativeEditTransaction {
            transaction_id,
            operations,
            operation_count,
            diff_summary,
            diff_summary_truncated,
            diff_summary_bytes,
        })
    }
}

fn estimate_request_bytes(request: &NativeEditTransactionRequest) -> usize {
    serde_json::to_vec(request).map_or(usize::MAX, |bytes| bytes.len())
}

fn validate_relative_path(path: &str) -> Result<PathBuf, NativeEditError> {
    let path_ref = Path::new(path);
    if path_ref.is_absolute() {
        return Err(NativeEditError::AbsolutePath {
            path: path.to_owned(),
        });
    }

    let mut normalized = PathBuf::new();
    for component in path_ref.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(NativeEditError::PathTraversal {
                    path: path.to_owned(),
                });
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(NativeEditError::AbsolutePath {
                    path: path.to_owned(),
                });
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(NativeEditError::PathTraversal {
            path: path.to_owned(),
        });
    }

    reject_metadata_path(path, &normalized)?;
    Ok(normalized)
}

fn reject_metadata_path(original: &str, normalized: &Path) -> Result<(), NativeEditError> {
    let components = normalized
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();

    if components.first() == Some(&".git")
        || components.first() == Some(&"target")
        || components
            .as_slice()
            .starts_with(&[".yach", "native-sessions"])
    {
        return Err(NativeEditError::UnsupportedMetadataPath {
            path: original.to_owned(),
        });
    }

    Ok(())
}

fn resolve_create_target(
    root: &NativeResourceRoot,
    path: &str,
) -> Result<(String, PathBuf), NativeEditError> {
    let normalized = validate_relative_path(path)?;
    let parent = normalized.parent().unwrap_or_else(|| Path::new(""));
    reject_symlinked_parent(root, path, parent)?;
    let canonical_parent = root
        .canonical_path()
        .join(parent)
        .canonicalize()
        .map_err(|_| NativeEditError::ParentMissing {
            path: path.to_owned(),
        })?;

    if !canonical_parent.starts_with(root.canonical_path()) {
        return Err(NativeEditError::PathOutsideRoot {
            path: path.to_owned(),
        });
    }
    if !canonical_parent.is_dir() {
        return Err(NativeEditError::UnsupportedFileType {
            path: path.to_owned(),
        });
    }

    let file_name = normalized
        .file_name()
        .ok_or_else(|| NativeEditError::PathTraversal {
            path: path.to_owned(),
        })?;
    let resolved = canonical_parent.join(file_name);
    if std::fs::symlink_metadata(&resolved).is_ok() {
        return Err(NativeEditError::TargetExists {
            path: path.to_owned(),
        });
    }

    Ok((path_to_slash_string(&normalized), resolved))
}

fn read_existing_text(
    root: &NativeResourceRoot,
    path: &str,
    policy: &NativeEditPolicy,
) -> Result<(String, PathBuf, String), NativeEditError> {
    let normalized = validate_relative_path(path)?;
    let relative_path = path_to_slash_string(&normalized);
    let parent = normalized.parent().unwrap_or_else(|| Path::new(""));
    reject_symlinked_parent(root, path, parent)?;
    let unresolved = root.canonical_path().join(&normalized);
    let link_metadata =
        std::fs::symlink_metadata(&unresolved).map_err(|_| NativeEditError::TargetMissing {
            path: path.to_owned(),
        })?;
    if link_metadata.file_type().is_symlink() {
        return Err(NativeEditError::SymlinkRejected {
            path: path.to_owned(),
        });
    }

    let resolved = root
        .resolve_file(&normalized)
        .map_err(|error| match error {
            crate::NativeResourcePathError::Missing => NativeEditError::TargetMissing {
                path: path.to_owned(),
            },
            crate::NativeResourcePathError::EscapesRoot => NativeEditError::PathOutsideRoot {
                path: path.to_owned(),
            },
            crate::NativeResourcePathError::ExpectedFile => NativeEditError::ExpectedFile {
                path: path.to_owned(),
            },
            crate::NativeResourcePathError::RootUnavailable
            | crate::NativeResourcePathError::ExpectedDirectory => NativeEditError::Io {
                path: path.to_owned(),
            },
        })?;

    let metadata = std::fs::metadata(&resolved).map_err(|_| NativeEditError::Io {
        path: path.to_owned(),
    })?;
    if metadata.len() > policy.max_file_bytes {
        return Err(NativeEditError::FileTooLarge {
            path: path.to_owned(),
            max_bytes: policy.max_file_bytes,
            actual_bytes: metadata.len(),
        });
    }
    let bytes = std::fs::read(&resolved).map_err(|_| NativeEditError::Io {
        path: path.to_owned(),
    })?;
    if u64::try_from(bytes.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(NativeEditError::FileTooLarge {
            path: path.to_owned(),
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    let text = String::from_utf8(bytes).map_err(|_| NativeEditError::NotUtf8 {
        path: path.to_owned(),
    })?;
    Ok((relative_path, resolved, text))
}

fn apply_hunks(
    relative_path: &str,
    original: &str,
    hunks: &[NativeEditHunk],
) -> Result<String, NativeEditError> {
    if hunks.is_empty() {
        return Err(NativeEditError::EmptyHunks {
            path: relative_path.to_owned(),
        });
    }

    let mut text = original.to_owned();
    for hunk in hunks {
        if hunk.find.is_empty() {
            return Err(NativeEditError::EmptyFind {
                path: relative_path.to_owned(),
            });
        }
        let matches = overlapping_match_indices(&text, &hunk.find);
        match matches.as_slice() {
            [] => {
                return Err(NativeEditError::HunkNotFound {
                    path: relative_path.to_owned(),
                });
            }
            [_] => {
                text = text.replacen(&hunk.find, &hunk.replace, 1);
            }
            _ => {
                return Err(NativeEditError::HunkAmbiguous {
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
    if before == after {
        return format!("--- {relative_path}\n+++ {relative_path}\n");
    }

    let mut diff = format!("--- {relative_path}\n+++ {relative_path}\n");
    for line in before.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in after.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
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
    root: &NativeResourceRoot,
    original: &str,
    parent: &Path,
) -> Result<(), NativeEditError> {
    let mut current = root.canonical_path().to_path_buf();
    for component in parent.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        let metadata =
            std::fs::symlink_metadata(&current).map_err(|_| NativeEditError::ParentMissing {
                path: original.to_owned(),
            })?;
        if metadata.file_type().is_symlink() {
            return Err(NativeEditError::SymlinkRejected {
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
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn native_edit_preview_prepares_create_without_writing_file() {
        let project = TempProject::new("create-smoke");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = native_root(&project) else {
            return;
        };

        let preview_result = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &NativeEditPolicy::test(),
        );
        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };

        assert!(matches!(
            preview.operations.as_slice(),
            [PreparedNativeEditOperation::CreateTextFile { .. }]
        ));
        assert!(preview.transaction_id.0.starts_with("edit-"));
        assert_eq!(preview.operation_count, 1);
        assert!(!project.root().join("src/new.rs").exists());
    }

    #[test]
    fn native_edit_preview_rejects_absolute_create_path() {
        let project = TempProject::new("absolute-create");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("/tmp/outside.rs"),
                    content: String::from("outside"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::AbsolutePath {
                path: String::from("/tmp/outside.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_parent_traversal_create_path() {
        let project = TempProject::new("traversal-create");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("../outside.rs"),
                    content: String::from("outside"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::PathTraversal {
                path: String::from("../outside.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_missing_create_parent() {
        let project = TempProject::new("missing-parent");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("missing/new.rs"),
                    content: String::from("content"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::ParentMissing {
                path: String::from("missing/new.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_file_create_parent() {
        let project = TempProject::new("file-parent");
        project.write("src", "not a directory");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("content"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::UnsupportedFileType {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_existing_create_target() {
        let project = TempProject::new("target-exists");
        project.write("src/new.rs", "existing");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("replacement"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_metadata_and_root_target_paths() {
        let project = TempProject::new("metadata-paths");
        assert!(std::fs::create_dir_all(project.root().join(".yach")).is_ok());
        assert!(std::fs::create_dir_all(project.root().join("target")).is_ok());
        let Some(root) = native_root(&project) else {
            return;
        };

        for path in [
            ".git/config",
            ".yach/native-sessions/session.jsonl",
            "target/out.rs",
        ] {
            let error = NativeEditEngine::preview(
                &root,
                NativeEditTransactionRequest {
                    operations: vec![NativeEditOperation::CreateTextFile {
                        path: String::from(path),
                        content: String::from("content"),
                    }],
                },
                &NativeEditPolicy::test(),
            );

            assert_eq!(
                error,
                Err(NativeEditError::UnsupportedMetadataPath {
                    path: String::from(path)
                })
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_edit_preview_rejects_create_through_symlink_parent() {
        let project = TempProject::new("symlink-parent-create");
        assert!(std::fs::create_dir_all(project.root().join("real")).is_ok());
        assert!(
            std::os::unix::fs::symlink(project.root().join("real"), project.root().join("link"))
                .is_ok()
        );
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("link/new.rs"),
                    content: String::from("content"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::SymlinkRejected {
                path: String::from("link/new.rs")
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_edit_preview_rejects_dangling_symlink_create_target_as_existing() {
        let project = TempProject::new("dangling-create-target");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("missing-target.rs"),
                project.root().join("src/new.rs"),
            )
            .is_ok()
        );
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("content"),
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::TargetExists {
                path: String::from("src/new.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_prepares_modify_with_hashes_and_diff_summary() {
        let project = TempProject::new("modify-preview");
        project.write("src/lib.rs", "pub fn old() {}\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let expected_sha256 = test_sha256_hex("pub fn old() {}\n");

        let preview_result = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256,
                    hunks: vec![NativeEditHunk {
                        find: String::from("old"),
                        replace: String::from("new"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert!(preview_result.is_ok());
        let Some(preview) = preview_result.ok() else {
            return;
        };
        assert!(matches!(
            preview.operations.as_slice(),
            [PreparedNativeEditOperation::ModifyTextFile {
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
    fn native_edit_preview_rejects_hash_mismatch() {
        let project = TempProject::new("hash-mismatch");
        project.write("src/lib.rs", "actual\n");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("expected\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("actual"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert!(matches!(
            error,
            Err(NativeEditError::HashMismatch { path, .. }) if path == "src/lib.rs"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn native_edit_preview_rejects_symlink_modify_target() {
        let project = TempProject::new("symlink-modify");
        project.write("src/real.rs", "real\n");
        assert!(
            std::os::unix::fs::symlink(
                project.root().join("src/real.rs"),
                project.root().join("src/link.rs"),
            )
            .is_ok()
        );
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/link.rs"),
                    expected_sha256: test_sha256_hex("real\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("real"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::SymlinkRejected {
                path: String::from("src/link.rs")
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_edit_preview_rejects_modify_through_symlinked_metadata_parent() {
        let project = TempProject::new("symlink-metadata-parent-modify");
        project.write(".git/config", "protected\n");
        assert!(
            std::os::unix::fs::symlink(project.root().join(".git"), project.root().join("link"))
                .is_ok()
        );
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("link/config"),
                    expected_sha256: test_sha256_hex("protected\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("protected"),
                        replace: String::from("changed"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::SymlinkRejected {
                path: String::from("link/config")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_empty_or_ambiguous_hunks() {
        let project = TempProject::new("hunk-policy");
        project.write("src/lib.rs", "same same\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let expected_sha256 = test_sha256_hex("same same\n");

        let empty = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: expected_sha256.clone(),
                    hunks: Vec::new(),
                }],
            },
            &NativeEditPolicy::test(),
        );
        assert_eq!(
            empty,
            Err(NativeEditError::EmptyHunks {
                path: String::from("src/lib.rs")
            })
        );

        let ambiguous = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256,
                    hunks: vec![NativeEditHunk {
                        find: String::from("same"),
                        replace: String::from("other"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );
        assert_eq!(
            ambiguous,
            Err(NativeEditError::HunkAmbiguous {
                path: String::from("src/lib.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_rejects_overlapping_ambiguous_hunks() {
        let project = TempProject::new("overlapping-hunk-policy");
        project.write("src/lib.rs", "aaa\n");
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("aaa\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("aa"),
                        replace: String::from("b"),
                    }],
                }],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::HunkAmbiguous {
                path: String::from("src/lib.rs")
            })
        );
    }

    #[test]
    fn native_edit_preview_create_includes_diff_summary() {
        let project = TempProject::new("create-diff");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = native_root(&project) else {
            return;
        };

        let preview_result = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("pub fn created() {}\n"),
                }],
            },
            &NativeEditPolicy::test(),
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
    fn native_edit_preview_rejects_multiple_operations_by_policy() {
        let project = TempProject::new("multi-op");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = native_root(&project) else {
            return;
        };

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![
                    NativeEditOperation::CreateTextFile {
                        path: String::from("src/a.rs"),
                        content: String::from("a"),
                    },
                    NativeEditOperation::CreateTextFile {
                        path: String::from("src/b.rs"),
                        content: String::from("b"),
                    },
                ],
            },
            &NativeEditPolicy::test(),
        );

        assert_eq!(
            error,
            Err(NativeEditError::TooManyOperations {
                max_operations: 1,
                actual_operations: 2
            })
        );
    }

    #[test]
    fn native_edit_preview_truncates_large_diff_summary() {
        let project = TempProject::new("diff-truncate");
        project.write("src/lib.rs", "alpha\n");
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut policy = NativeEditPolicy::test();
        policy.max_diff_summary_bytes = 20;

        let preview_result = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("src/lib.rs"),
                    expected_sha256: test_sha256_hex("alpha\n"),
                    hunks: vec![NativeEditHunk {
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
    fn native_edit_preview_rejects_serialized_transaction_too_large() {
        let project = TempProject::new("transaction-too-large");
        assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
        let Some(root) = native_root(&project) else {
            return;
        };
        let mut policy = NativeEditPolicy::test();
        policy.max_transaction_bytes = 8;

        let error = NativeEditEngine::preview(
            &root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::CreateTextFile {
                    path: String::from("src/new.rs"),
                    content: String::from("this request is intentionally too large"),
                }],
            },
            &policy,
        );

        assert!(matches!(
            error,
            Err(NativeEditError::TransactionTooLarge {
                max_bytes: 8,
                actual_bytes
            }) if actual_bytes > 8
        ));
    }

    fn test_sha256_hex(text: &str) -> String {
        sha256_hex(text.as_bytes())
    }

    fn native_root(project: &TempProject) -> Option<NativeResourceRoot> {
        let root = NativeResourceRoot::project(project.root());
        assert!(root.is_ok());
        root.ok()
    }
}
