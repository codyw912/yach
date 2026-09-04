# Native Edit Transactions Apply Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add backend-only guarded apply support for already-previewed native edit transactions.

**Architecture:** Extend the existing `crates/yach-backend/src/edit.rs` transaction engine with a crate-internal `NativeEditEngine::apply`. Preview remains the only place that accepts provider-neutral edit requests; apply consumes a prepared transaction, revalidates filesystem state, writes exactly one create or modify operation through same-directory temporary files, and returns structured apply metadata. This slice remains backend-local: no public apply API, session evidence, tool registration, provider advertising, extension mutation, approval UI, delete/rename, benchmarks, or multi-operation atomicity.

**Tech Stack:** Rust 2024, `std::fs`, `std::io::Write`, `std::path`, existing `sha2`, `yach-backend`, `just dev cargo test`, `just test`.

---

## File Structure

Implementation files:

- Modify `crates/yach-backend/src/edit.rs`: apply result types, prepared after-image storage, `NativeEditEngine::apply`, same-directory temp file helpers, create no-overwrite publish, modify replacement, and focused tests.

No dependency changes are planned. Use `std::fs::OpenOptions::create_new` for temp files, `File::sync_all` best-effort where available, `std::fs::rename` for modify replacement, and `std::fs::hard_link` for create no-overwrite publish.

## Non-Goals For This Plan

- Do not add edit session events or JSONL evidence.
- Do not register built-in edit tools.
- Do not advertise edit tools to providers.
- Do not add extension-owned mutation tools.
- Do not add approval UI.
- Do not add delete, rename, chmod, symlink, binary edit, or directory creation.
- Do not allow multi-operation apply, even if a caller passes a permissive `NativeEditPolicy`.
- Do not expose `NativeEditEngine::apply` as a public API outside `yach-backend` until prepared transactions are sealed or a safe wrapper exists.
- Do not add Criterion benchmarks yet.

## Task 1: Apply API And Prepared After-Images

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`

- [ ] **Step 1: Add failing tests for the apply API surface**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/yach-backend/src/edit.rs`:

```rust
#[test]
fn native_edit_apply_creates_text_file_from_preview() {
    let project = TempProject::new("apply-create");
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
    assert!(!project.root().join("src/new.rs").exists());

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let apply_result = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert!(apply_result.is_ok());
    let Some(applied) = apply_result.ok() else {
        return;
    };
    assert_eq!(applied.outcome, NativeEditApplyOutcome::Completed);
    assert_eq!(applied.operation_count, 1);
    assert_eq!(
        std::fs::read_to_string(project.root().join("src/new.rs")).ok(),
        Some(String::from("pub fn created() {}\n"))
    );
    assert!(matches!(
        applied.operations.as_slice(),
        [NativeEditAppliedOperation::CreateTextFile {
            relative_path,
            after_sha256,
            after_bytes: 20,
            bytes_written: 20,
        }] if relative_path == "src/new.rs"
            && after_sha256 == &test_sha256_hex("pub fn created() {}\n")
    ));
}

#[test]
fn native_edit_apply_modifies_text_file_from_preview() {
    let project = TempProject::new("apply-modify");
    project.write("src/lib.rs", "pub fn old() {}\n");
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: String::from("src/lib.rs"),
                expected_sha256: test_sha256_hex("pub fn old() {}\n"),
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
    let apply_result = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

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
        [NativeEditAppliedOperation::ModifyTextFile {
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
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_ -- --nocapture
```

Expected: compile failures for missing `NativeEditEngine::apply`, `NativeEditApplyOutcome`, and `NativeEditAppliedOperation`.

- [ ] **Step 3: Add apply result types and prepared after-image fields**

Keep apply crate-internal for this slice. `PreparedNativeEditOperation` is publicly re-exported today, so storing full after-images in prepared operations would otherwise create a forgeable public full-file write API. The safe boundary for this plan is: preview constructs prepared transactions, `pub(crate)` apply consumes them inside `yach-backend`, and any future public/local harness wrapper gets its own design or sealing step.

In `crates/yach-backend/src/edit.rs`, extend `PreparedNativeEditOperation`:

```rust
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
        after_content: String,
    },
    CreateTextFile {
        relative_path: String,
        resolved_path: PathBuf,
        after_sha256: String,
        after_bytes: usize,
        content: String,
    },
}
```

Add the apply result types below `PreparedNativeEditOperation`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditApplyResult {
    pub transaction_id: NativeEditTransactionId,
    pub outcome: NativeEditApplyOutcome,
    pub operations: Vec<NativeEditAppliedOperation>,
    pub operation_count: usize,
    pub diff_summary: String,
    pub diff_summary_truncated: bool,
    pub diff_summary_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEditApplyOutcome {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEditAppliedOperation {
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
```

Update the create preview branch to keep the content:

```rust
operations.push(PreparedNativeEditOperation::CreateTextFile {
    relative_path,
    resolved_path: resolved,
    after_sha256: sha256_hex(content.as_bytes()),
    after_bytes: content.len(),
    content,
});
```

Update the modify preview branch to keep the computed after-image:

```rust
let after_sha256 = sha256_hex(after.as_bytes());
let after_bytes = after.len();
diff_summary.push_str(&render_diff_summary(&relative_path, &before, &after));
operations.push(PreparedNativeEditOperation::ModifyTextFile {
    relative_path,
    resolved_path,
    before_sha256,
    after_sha256,
    before_bytes: before.len(),
    after_bytes,
    hunk_count: hunks.len(),
    after_content: after,
});
```

- [ ] **Step 4: Add an apply stub**

Inside `impl NativeEditEngine`, below `preview`, add:

```rust
pub(crate) fn apply(
    root: &NativeResourceRoot,
    transaction: PreparedNativeEditTransaction,
    policy: &NativeEditPolicy,
) -> Result<NativeEditApplyResult, NativeEditError> {
    let _ = (root, policy);
    Ok(NativeEditApplyResult {
        transaction_id: transaction.transaction_id,
        outcome: NativeEditApplyOutcome::Completed,
        operations: Vec::new(),
        operation_count: transaction.operation_count,
        diff_summary: transaction.diff_summary,
        diff_summary_truncated: transaction.diff_summary_truncated,
        diff_summary_bytes: transaction.diff_summary_bytes,
    })
}
```

- [ ] **Step 5: Run the focused tests and verify they fail behaviorally**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_ -- --nocapture
```

Expected: tests compile, then fail because apply does not write files and returns no applied operations.

- [ ] **Step 6: Commit the API shape**

```bash
git add crates/yach-backend/src/edit.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Add native edit apply API shape"
```

## Task 2: Create Apply With No-Overwrite Publish

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`

- [ ] **Step 1: Add race and no-overwrite tests for create apply**

Add these tests:

```rust
#[test]
fn native_edit_apply_create_fails_if_target_appears_after_preview() {
    let project = TempProject::new("apply-create-race");
    assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("from preview\n"),
            }],
        },
        &NativeEditPolicy::test(),
    );
    assert!(preview_result.is_ok());
    project.write("src/new.rs", "concurrent\n");

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert_eq!(
        error,
        Err(NativeEditError::TargetExists {
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
fn native_edit_apply_create_rejects_parent_symlink_added_after_preview() {
    let project = TempProject::new("apply-create-parent-link");
    assert!(std::fs::create_dir_all(project.root().join("real")).is_ok());
    assert!(std::fs::create_dir_all(project.root().join("link")).is_ok());
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("link/new.rs"),
                content: String::from("content\n"),
            }],
        },
        &NativeEditPolicy::test(),
    );
    assert!(preview_result.is_ok());
    assert!(std::fs::remove_dir(project.root().join("link")).is_ok());
    assert!(std::os::unix::fs::symlink(project.root().join("real"), project.root().join("link")).is_ok());

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert_eq!(
        error,
        Err(NativeEditError::SymlinkRejected {
            path: String::from("link/new.rs")
        })
    );
    assert!(!project.root().join("real/new.rs").exists());
}

#[cfg(unix)]
#[test]
fn native_edit_apply_create_treats_dangling_symlink_race_as_target_exists() {
    let project = TempProject::new("apply-create-dangling-link-race");
    assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("from preview\n"),
            }],
        },
        &NativeEditPolicy::test(),
    );
    assert!(preview_result.is_ok());
    assert!(std::os::unix::fs::symlink(
        project.root().join("missing-target.rs"),
        project.root().join("src/new.rs"),
    )
    .is_ok());

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert_eq!(
        error,
        Err(NativeEditError::TargetExists {
            path: String::from("src/new.rs")
        })
    );
    assert!(std::fs::symlink_metadata(project.root().join("src/new.rs")).is_ok());
}
```

- [ ] **Step 2: Run the create apply tests and verify the new ones fail**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_create -- --nocapture
```

Expected: create apply tests fail until real create application is implemented.

- [ ] **Step 3: Add temp file helpers**

Add imports at the top of `edit.rs`:

```rust
use std::fs::{File, OpenOptions};
use std::io::Write;
```

Add these helpers near the existing helper functions:

```rust
fn temp_path_for(target: &Path, transaction_id: &NativeEditTransactionId) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("target");
    let temp_name = format!(".{file_name}.{}.{}.tmp", transaction_id.0, std::process::id());
    target.with_file_name(temp_name)
}

fn write_temp_file(
    temp_path: &Path,
    content: &[u8],
    permissions: Option<std::fs::Permissions>,
    relative_path: &str,
) -> Result<File, NativeEditError> {
    let mut temp_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|_| NativeEditError::Io {
            path: relative_path.to_owned(),
        })?;

    if let Some(permissions) = permissions {
        temp_file
            .set_permissions(permissions)
            .map_err(|_| NativeEditError::Io {
                path: relative_path.to_owned(),
            })?;
    }

    temp_file
        .write_all(content)
        .and_then(|()| temp_file.sync_all())
        .map_err(|_| NativeEditError::Io {
            path: relative_path.to_owned(),
        })?;
    Ok(temp_file)
}

fn cleanup_temp_file(path: &Path) {
    let _ = std::fs::remove_file(path);
}
```

- [ ] **Step 4: Add a create operation applier**

Add:

```rust
fn apply_create_operation(
    root: &NativeResourceRoot,
    transaction_id: &NativeEditTransactionId,
    relative_path: String,
    resolved_path: PathBuf,
    after_sha256: String,
    after_bytes: usize,
    content: String,
    policy: &NativeEditPolicy,
) -> Result<NativeEditAppliedOperation, NativeEditError> {
    if !policy.allow_create {
        return Err(NativeEditError::CreateDisabled);
    }
    if u64::try_from(content.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(NativeEditError::FileTooLarge {
            path: relative_path,
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
        });
    }

    let (fresh_relative, fresh_resolved) = resolve_create_target(root, &relative_path)?;
    if fresh_relative != relative_path || fresh_resolved != resolved_path {
        return Err(NativeEditError::PathOutsideRoot {
            path: relative_path,
        });
    }
    if sha256_hex(content.as_bytes()) != after_sha256 || content.len() != after_bytes {
        return Err(NativeEditError::HashMismatch {
            path: relative_path,
            expected_sha256: after_sha256,
            actual_sha256: sha256_hex(content.as_bytes()),
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

    let publish_result = std::fs::hard_link(&temp_path, &resolved_path);
    cleanup_temp_file(&temp_path);
    if let Err(_error) = publish_result {
        if std::fs::symlink_metadata(&resolved_path).is_ok() {
            return Err(NativeEditError::TargetExists {
                path: relative_path,
            });
        }
        return Err(NativeEditError::Io {
            path: relative_path,
        });
    }

    Ok(NativeEditAppliedOperation::CreateTextFile {
        relative_path,
        after_sha256,
        after_bytes,
        bytes_written: content.len(),
    })
}
```

- [ ] **Step 5: Route create operations through apply**

Replace the apply stub body with operation routing:

```rust
let operation_count = transaction.operations.len();
if operation_count == 0 {
    return Err(NativeEditError::EmptyTransaction);
}
if operation_count != 1 {
    return Err(NativeEditError::TooManyOperations {
        max_operations: 1,
        actual_operations: operation_count,
    });
}
if operation_count > policy.max_operations {
    return Err(NativeEditError::TooManyOperations {
        max_operations: policy.max_operations,
        actual_operations: operation_count,
    });
}

let mut operations = Vec::new();
for operation in transaction.operations {
    match operation {
        PreparedNativeEditOperation::CreateTextFile {
            relative_path,
            resolved_path,
            after_sha256,
            after_bytes,
            content,
        } => {
            operations.push(apply_create_operation(
                root,
                &transaction.transaction_id,
                relative_path,
                resolved_path,
                after_sha256,
                after_bytes,
                content,
                policy,
            )?);
        }
        PreparedNativeEditOperation::ModifyTextFile { .. } => {
            return Err(NativeEditError::ModifyDisabled);
        }
    }
}

Ok(NativeEditApplyResult {
    transaction_id: transaction.transaction_id,
    outcome: NativeEditApplyOutcome::Completed,
    operations,
    operation_count,
    diff_summary: transaction.diff_summary,
    diff_summary_truncated: transaction.diff_summary_truncated,
    diff_summary_bytes: transaction.diff_summary_bytes,
})
```

- [ ] **Step 6: Run create apply tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_create -- --nocapture
```

Expected: create apply tests pass. Modify apply tests may still fail.

- [ ] **Step 7: Commit create apply**

```bash
git add crates/yach-backend/src/edit.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Apply native edit create transactions"
```

## Task 3: Modify Apply With Final Hash Guard And Replacement

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`

- [ ] **Step 1: Add modify race, symlink, and permission tests**

Add:

```rust
#[test]
fn native_edit_apply_modify_fails_if_file_changed_after_preview() {
    let project = TempProject::new("apply-modify-race");
    project.write("src/lib.rs", "pub fn old() {}\n");
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: String::from("src/lib.rs"),
                expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                hunks: vec![NativeEditHunk {
                    find: String::from("old"),
                    replace: String::from("new"),
                }],
            }],
        },
        &NativeEditPolicy::test(),
    );
    assert!(preview_result.is_ok());
    project.write("src/lib.rs", "pub fn concurrent() {}\n");

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert_eq!(
        error,
        Err(NativeEditError::HashMismatch {
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
fn native_edit_apply_modify_rejects_target_symlink_added_after_preview() {
    let project = TempProject::new("apply-modify-link");
    project.write("src/lib.rs", "pub fn old() {}\n");
    project.write("src/other.rs", "pub fn other() {}\n");
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: String::from("src/lib.rs"),
                expected_sha256: test_sha256_hex("pub fn old() {}\n"),
                hunks: vec![NativeEditHunk {
                    find: String::from("old"),
                    replace: String::from("new"),
                }],
            }],
        },
        &NativeEditPolicy::test(),
    );
    assert!(preview_result.is_ok());
    assert!(std::fs::remove_file(project.root().join("src/lib.rs")).is_ok());
    assert!(std::os::unix::fs::symlink(project.root().join("src/other.rs"), project.root().join("src/lib.rs")).is_ok());

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());

    assert_eq!(
        error,
        Err(NativeEditError::SymlinkRejected {
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
fn native_edit_apply_modify_preserves_unix_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let project = TempProject::new("apply-modify-mode");
    project.write("script.sh", "echo old\n");
    assert!(std::fs::set_permissions(
        project.root().join("script.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .is_ok());
    let Some(root) = native_root(&project) else {
        return;
    };

    let preview_result = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::ModifyTextFile {
                path: String::from("script.sh"),
                expected_sha256: test_sha256_hex("echo old\n"),
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
    let apply_result = NativeEditEngine::apply(&root, preview, &NativeEditPolicy::test());
    assert!(apply_result.is_ok());

    let mode = std::fs::metadata(project.root().join("script.sh"))
        .map(|metadata| metadata.permissions().mode() & 0o777)
        .ok();
    assert_eq!(mode, Some(0o755));
}
```

- [ ] **Step 2: Run modify apply tests and verify failures**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_modify -- --nocapture
```

Expected: modify apply tests fail until modify routing and replacement are implemented.

- [ ] **Step 3: Add the modify operation applier**

Add:

```rust
fn apply_modify_operation(
    root: &NativeResourceRoot,
    transaction_id: &NativeEditTransactionId,
    relative_path: String,
    resolved_path: PathBuf,
    before_sha256: String,
    after_sha256: String,
    before_bytes: usize,
    after_bytes: usize,
    hunk_count: usize,
    after_content: String,
    policy: &NativeEditPolicy,
) -> Result<NativeEditAppliedOperation, NativeEditError> {
    if !policy.allow_modify {
        return Err(NativeEditError::ModifyDisabled);
    }

    let (fresh_relative, fresh_resolved, current_text) =
        read_existing_text(root, &relative_path, policy)?;
    if fresh_relative != relative_path || fresh_resolved != resolved_path {
        return Err(NativeEditError::PathOutsideRoot {
            path: relative_path,
        });
    }

    let actual_before_sha256 = sha256_hex(current_text.as_bytes());
    if actual_before_sha256 != before_sha256 {
        return Err(NativeEditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: actual_before_sha256,
        });
    }
    if current_text.len() != before_bytes {
        return Err(NativeEditError::HashMismatch {
            path: relative_path,
            expected_sha256: before_sha256,
            actual_sha256: sha256_hex(current_text.as_bytes()),
        });
    }
    if sha256_hex(after_content.as_bytes()) != after_sha256 || after_content.len() != after_bytes {
        return Err(NativeEditError::HashMismatch {
            path: relative_path,
            expected_sha256: after_sha256,
            actual_sha256: sha256_hex(after_content.as_bytes()),
        });
    }
    if u64::try_from(after_content.len()).map_or(true, |actual| actual > policy.max_file_bytes) {
        return Err(NativeEditError::FileTooLarge {
            path: relative_path,
            max_bytes: policy.max_file_bytes,
            actual_bytes: u64::try_from(after_content.len()).unwrap_or(u64::MAX),
        });
    }

    let permissions = std::fs::metadata(&resolved_path)
        .map(|metadata| metadata.permissions())
        .map_err(|_| NativeEditError::Io {
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

    std::fs::rename(&temp_path, &resolved_path).map_err(|_| {
        cleanup_temp_file(&temp_path);
        NativeEditError::Io {
            path: relative_path.clone(),
        }
    })?;

    Ok(NativeEditAppliedOperation::ModifyTextFile {
        relative_path,
        before_sha256,
        after_sha256,
        before_bytes,
        after_bytes,
        hunk_count,
        bytes_written: after_content.len(),
    })
}
```

- [ ] **Step 4: Route modify operations through apply**

Replace the temporary `ModifyTextFile` error branch in `NativeEditEngine::apply` with:

```rust
PreparedNativeEditOperation::ModifyTextFile {
    relative_path,
    resolved_path,
    before_sha256,
    after_sha256,
    before_bytes,
    after_bytes,
    hunk_count,
    after_content,
} => {
    operations.push(apply_modify_operation(
        root,
        &transaction.transaction_id,
        relative_path,
        resolved_path,
        before_sha256,
        after_sha256,
        before_bytes,
        after_bytes,
        hunk_count,
        after_content,
        policy,
    )?);
}
```

- [ ] **Step 5: Run modify and create apply tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_ -- --nocapture
```

Expected: create and modify apply tests pass.

- [ ] **Step 6: Commit modify apply**

```bash
git add crates/yach-backend/src/edit.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Apply native edit modify transactions"
```

## Task 4: Apply Policy, Result Metadata, And Failure Boundaries

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`

- [ ] **Step 1: Add final policy and result tests**

Add:

```rust
#[test]
fn native_edit_apply_rejects_multiple_operations_even_when_policy_allows_them() {
    let project = TempProject::new("apply-multi-op");
    assert!(std::fs::create_dir_all(project.root().join("src")).is_ok());
    let Some(root) = native_root(&project) else {
        return;
    };
    let mut preview_policy = NativeEditPolicy::test();
    preview_policy.max_operations = 2;

    let preview_result = NativeEditEngine::preview(
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
        &preview_policy,
    );
    assert!(preview_result.is_ok());

    let Some(preview) = preview_result.ok() else {
        return;
    };
    let error = NativeEditEngine::apply(&root, preview, &preview_policy);

    assert_eq!(
        error,
        Err(NativeEditError::TooManyOperations {
            max_operations: 1,
            actual_operations: 2
        })
    );
    assert!(!project.root().join("src/a.rs").exists());
    assert!(!project.root().join("src/b.rs").exists());
}

#[test]
fn native_edit_apply_result_preserves_bounded_diff_summary() {
    let project = TempProject::new("apply-diff-summary");
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

    let apply_result = NativeEditEngine::apply(&root, preview, &policy);
    assert!(apply_result.is_ok());
    let Some(applied) = apply_result.ok() else {
        return;
    };
    assert!(applied.diff_summary_truncated);
    assert!(applied.diff_summary_bytes <= policy.max_diff_summary_bytes);
    assert!(applied.diff_summary.contains("[diff truncated]"));
}
```

- [ ] **Step 2: Run policy/result tests and verify failures if any**

Run:

```bash
just dev cargo test -p yach-backend native_edit_apply_rejects_multiple_operations_even_when_policy_allows_them -- --nocapture
just dev cargo test -p yach-backend native_edit_apply_result_preserves_bounded_diff_summary -- --nocapture
```

Expected: tests pass if earlier routing already preserved policy and diff metadata. If they fail, fix only the policy/result behavior described by the test.

- [ ] **Step 3: Review public error paths for absolute path leakage**

Inspect all new apply errors. The implementation must return `relative_path` from prepared operations, never `resolved_path.display().to_string()`.

Run:

```bash
rg -n "display\\(|to_string_lossy\\(" crates/yach-backend/src/edit.rs
```

Expected: any matches are existing helper code for normalized relative paths or no matches in new apply error construction. Do not add absolute path strings to `NativeEditError`.

- [ ] **Step 4: Run all native edit tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit -- --nocapture
```

Expected: all native edit tests pass.

- [ ] **Step 5: Commit apply policy coverage**

```bash
git add crates/yach-backend/src/edit.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Cover native edit apply policy"
```

## Task 5: Final Verification And Cleanup

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`

- [ ] **Step 1: Format and run backend tests**

Run:

```bash
just dev cargo fmt --check
just dev cargo test -p yach-backend native_edit -- --nocapture
just dev cargo test -p yach-backend
```

Expected: formatting check passes, all native edit tests pass, and all backend tests pass.

- [ ] **Step 2: Run workspace tests and backend clippy**

Run:

```bash
just test
just dev cargo clippy -p yach-backend --lib -- -D warnings
git diff --check
```

Expected: workspace tests pass, backend lib clippy passes, and `git diff --check` reports no whitespace errors.

- [ ] **Step 3: Self-review apply scope**

Confirm these are true before opening a PR:

- `NativeEditEngine::apply` exists and consumes `PreparedNativeEditTransaction`.
- `NativeEditEngine::apply` is `pub(crate)` and remains unavailable as a public full-file mutation API.
- Apply supports exactly the prepared `CreateTextFile` and `ModifyTextFile` operation kinds.
- Apply hard-rejects anything other than exactly one operation, even if policy allows more.
- Create apply fails closed if the target appears after preview and never overwrites concurrent content.
- Modify apply re-reads the target and rejects stale previews through `HashMismatch`.
- Modify apply preserves ordinary permissions.
- Apply does not persist full file bodies to session logs.
- No provider-visible edit tool, extension mutation tool, approval UI, or benchmark was added.

- [ ] **Step 4: Commit any cleanup**

If formatting or review required code changes:

```bash
git add crates/yach-backend/src/edit.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Finalize native edit apply"
```

Skip this commit if the tree is already clean.

## Completion Checklist

- `NativeEditEngine::apply` applies prepared create transactions.
- `NativeEditEngine::apply` applies prepared modify transactions.
- `NativeEditEngine::apply` remains crate-internal for this slice.
- Preview remains read-only.
- Apply revalidates paths and does not trust prepared resolved paths as authority.
- Modify apply verifies the current file hash immediately before replacement.
- Create apply fails if the target exists at publish time.
- Create apply treats a dangling symlink target race as `TargetExists`.
- Failed validation does not write target files.
- Modify apply preserves ordinary file permissions.
- Apply returns structured relative-path metadata, hashes, byte counts, operation count, and bounded diff summary fields.
- No full file bodies are persisted beyond the in-memory prepared transaction.
- `just dev cargo test -p yach-backend native_edit -- --nocapture` passes.
- `just dev cargo test -p yach-backend` passes.
- `just test` passes.
- `just dev cargo clippy -p yach-backend --lib -- -D warnings` passes.
