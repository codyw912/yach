# Native Edit Evidence And Local Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add redacted native edit session evidence and a backend-local harness that wraps preview/apply without provider-visible mutation.

**Architecture:** Extend `NativeSessionEvent` with explicit edit prepared/finished records, then add a crate-local `edit_harness` module that calls `NativeEditEngine::preview` and `NativeEditEngine::apply` while appending evidence to `NativeSessionLog`. Keep provider advertising, native tool registration, extensions, approval UI, and public apply exposure unchanged.

**Tech Stack:** Rust 2024, serde JSONL session events, existing `yach-backend` edit/session/resource primitives, `just dev cargo test`, `just test`.

---

## File Structure

Implementation files:

- Modify `crates/yach-backend/src/edit.rs`: derive serde traits for `NativeEditTransactionId` and add a crate-visible edit error label helper.
- Modify `crates/yach-backend/src/session.rs`: add edit evidence summary types and `NativeSessionEvent` variants.
- Create `crates/yach-backend/src/edit_harness.rs`: crate-local harness context, summary conversion, and preview/apply orchestration.
- Modify `crates/yach-backend/src/native_runner.rs`: update exhaustive session-event projections so edit evidence is ignored by provider transcript reconstruction and UI session-message/stat views.
- Modify `crates/yach-backend/src/lib.rs`: wire the module and add focused integration tests.

No dependency changes are planned.

## Non-Goals For This Plan

- Do not register an edit tool in `NativeToolRegistry`.
- Do not add provider-advertised edit/write tools.
- Do not change provider continuation behavior.
- Do not add extension mutation support.
- Do not add approval UI.
- Do not add CLI commands.
- Do not make `NativeEditEngine::apply` public.
- Do not add benchmarks in this slice.

## Task 1: Serializable Edit Evidence Events

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing JSONL evidence round-trip test**

Add this test inside the existing `#[cfg(test)] mod tests` in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_session_log_preserves_edit_transaction_evidence_jsonl() {
    let log_path = temp_resource_dir("native-edit-evidence-jsonl").join("session.jsonl");
    let mut log = NativeSessionLog::default();
    let summary = NativeEditEvidenceSummary {
        operation_count: 1,
        operations: vec![NativeEditOperationEvidence::ModifyTextFile {
            relative_path: String::from("src/lib.rs"),
            before_sha256: String::from("before"),
            after_sha256: String::from("after"),
            before_bytes: 12,
            after_bytes: 13,
            hunk_count: 1,
            bytes_written: None,
        }],
        diff_summary: NativeToolPayloadSummary {
            summary: String::from("--- src/lib.rs\n+++ src/lib.rs\n-red\n+green\n"),
            byte_count: 43,
            redacted: false,
            truncated: false,
        },
    };

    log.push(NativeSessionEvent::EditTransactionPrepared {
        session_id: NativeSessionId(String::from("session-edit")),
        turn_id: NativeTurnId(String::from("turn-7")),
        tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
        transaction_id: NativeEditTransactionId(String::from("edit-7")),
        summary: summary.clone(),
    });
    let finished_summary = NativeEditEvidenceSummary {
        operation_count: 1,
        operations: vec![NativeEditOperationEvidence::ModifyTextFile {
            relative_path: String::from("src/lib.rs"),
            before_sha256: String::from("before"),
            after_sha256: String::from("after"),
            before_bytes: 12,
            after_bytes: 13,
            hunk_count: 1,
            bytes_written: Some(13),
        }],
        diff_summary: summary.diff_summary.clone(),
    };

    log.push(NativeSessionEvent::EditTransactionFinished {
        session_id: NativeSessionId(String::from("session-edit")),
        turn_id: NativeTurnId(String::from("turn-7")),
        tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
        transaction_id: Some(NativeEditTransactionId(String::from("edit-7"))),
        outcome: NativeEditEvidenceOutcome::Completed,
        reason: None,
        summary: Some(finished_summary),
    });

    assert!(log.write_to_file(&log_path).is_ok());
    let loaded = NativeSessionLog::load_from_file(&log_path);

    assert_eq!(loaded, Ok(log));
    assert!(std::fs::remove_dir_all(log_path.parent().unwrap()).is_ok());
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_edit_transaction_evidence_jsonl -- --nocapture
```

Expected: compile failure for missing `NativeEditEvidenceSummary`, `NativeEditOperationEvidence`, `NativeEditEvidenceOutcome`, and the new `NativeSessionEvent` variants.

- [ ] **Step 3: Add serde support for edit transaction IDs**

In `crates/yach-backend/src/edit.rs`, change the serde import and derive:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditTransactionId(pub String);
```

- [ ] **Step 4: Add edit evidence summary types**

In `crates/yach-backend/src/session.rs`, extend the import:

```rust
use crate::static_context::{NativeStaticContextOmission, NativeStaticContextSummary};
use crate::{
    NativeEditTransactionId, NativeToolError, NativeToolPermissionState,
};
```

Add these types above `NativeSessionEvent`:

```rust
/// Redacted edit transaction summary persisted in native session logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditEvidenceSummary {
    pub operation_count: usize,
    pub operations: Vec<NativeEditOperationEvidence>,
    pub diff_summary: NativeToolPayloadSummary,
}

/// Redacted per-operation edit evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NativeEditOperationEvidence {
    ModifyTextFile {
        relative_path: String,
        before_sha256: String,
        after_sha256: String,
        before_bytes: usize,
        after_bytes: usize,
        hunk_count: usize,
        bytes_written: Option<usize>,
    },
    CreateTextFile {
        relative_path: String,
        after_sha256: String,
        after_bytes: usize,
        bytes_written: Option<usize>,
    },
}

/// Categorical edit transaction outcome for durable evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditEvidenceOutcome {
    Completed,
    ValidationFailed,
    Failed,
}
```

- [ ] **Step 5: Add edit session event variants**

In `NativeSessionEvent`, add:

```rust
    EditTransactionPrepared {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: Option<NativeToolRequestId>,
        transaction_id: NativeEditTransactionId,
        summary: NativeEditEvidenceSummary,
    },
    EditTransactionFinished {
        session_id: NativeSessionId,
        turn_id: NativeTurnId,
        tool_request_id: Option<NativeToolRequestId>,
        transaction_id: Option<NativeEditTransactionId>,
        outcome: NativeEditEvidenceOutcome,
        reason: Option<String>,
        summary: Option<NativeEditEvidenceSummary>,
    },
```

Update the `last_entry_id` and `transcript_messages` matches so both edit event variants return `None`. Update `event_turn_id` so both edit event variants return `Some(turn_id)`.

- [ ] **Step 6: Update native runner session-event projections**

In `crates/yach-backend/src/native_runner.rs`, update every exhaustive match over `NativeSessionEvent` that currently ignores tool/static/metric events to also ignore edit evidence events unless the match is specifically recording the event.

The important production projections are:

- provider transcript reconstruction around `build_provider_messages_from_log`;
- provider/static-context resume helpers that find entry text;
- `send_native_session_messages`;
- `send_native_session_stats`.

In each ignored-event arm that currently looks like this:

```rust
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. } => None,
```

add the two edit variants:

```rust
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. }
            | NativeSessionEvent::StaticContextIncluded { .. }
            | NativeSessionEvent::EditTransactionPrepared { .. }
            | NativeSessionEvent::EditTransactionFinished { .. } => None,
```

This intentionally keeps edit evidence out of provider transcript replay and
out of the current UI session message/stat projections. A later UI slice can
add an explicit edit history view.

- [ ] **Step 7: Import the new types in tests**

In the `use super::{ ... }` list in `crates/yach-backend/src/lib.rs`, add:

```rust
NativeEditEvidenceOutcome, NativeEditEvidenceSummary, NativeEditOperationEvidence,
NativeEditTransactionId,
```

- [ ] **Step 8: Run the focused test and verify it passes**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_edit_transaction_evidence_jsonl -- --nocapture
```

Expected: the JSONL round-trip test passes.

- [ ] **Step 9: Run session and runner-focused tests**

Run:

```bash
just dev cargo test -p yach-backend native_session_log -- --nocapture
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --nocapture
just dev cargo test -p yach-backend native_runner::tests:: -- --nocapture
```

Expected: session log and native runner projection tests pass.

## Task 2: Edit Error Labels And Evidence Summary Conversion

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`
- Create: `crates/yach-backend/src/edit_harness.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing summary conversion test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_harness_summarizes_preview_without_file_bodies() {
    let root_path = temp_resource_dir("native-edit-harness-preview-summary");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    let Some(root) = root else {
        return;
    };

    let preview = NativeEditEngine::preview(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("secret body\n"),
            }],
        },
        &NativeEditPolicy::test(),
    );

    assert!(preview.is_ok());
    let summary = preview
        .as_ref()
        .map(native_edit_prepared_evidence_summary)
        .ok();

    assert!(matches!(
        summary.as_ref().map(|summary| summary.operations.as_slice()),
        Some([NativeEditOperationEvidence::CreateTextFile {
            relative_path,
            after_bytes: 12,
            bytes_written: None,
            ..
        }]) if relative_path == "src/new.rs"
    ));
    assert!(
        summary
            .as_ref()
            .is_some_and(|summary| !summary.diff_summary.summary.contains("secret body"))
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Add failing error label test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_error_labels_are_categorical() {
    assert_eq!(
        native_edit_error_label(&NativeEditError::TargetExists {
            path: String::from("src/lib.rs")
        }),
        "target_exists"
    );
    assert_eq!(
        native_edit_error_label(&NativeEditError::HashMismatch {
            path: String::from("src/lib.rs"),
            expected_sha256: String::from("expected"),
            actual_sha256: String::from("actual"),
        }),
        "hash_mismatch"
    );
}
```

- [ ] **Step 3: Run focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness_summarizes_preview_without_file_bodies -- --nocapture
just dev cargo test -p yach-backend native_edit_error_labels_are_categorical -- --nocapture
```

Expected: compile failures for missing helper functions and possibly missing module wiring.

- [ ] **Step 4: Add categorical edit error labels**

In `crates/yach-backend/src/edit.rs`, add this helper near the other free functions:

```rust
pub(crate) fn native_edit_error_label(error: &NativeEditError) -> &'static str {
    match error {
        NativeEditError::EmptyTransaction => "empty_transaction",
        NativeEditError::TooManyOperations { .. } => "too_many_operations",
        NativeEditError::TransactionTooLarge { .. } => "transaction_too_large",
        NativeEditError::CreateDisabled => "create_disabled",
        NativeEditError::ModifyDisabled => "modify_disabled",
        NativeEditError::AbsolutePath { .. } => "absolute_path",
        NativeEditError::PathTraversal { .. } => "path_traversal",
        NativeEditError::PathOutsideRoot { .. } => "path_outside_root",
        NativeEditError::ParentMissing { .. } => "parent_missing",
        NativeEditError::TargetMissing { .. } => "target_missing",
        NativeEditError::TargetExists { .. } => "target_exists",
        NativeEditError::DuplicateTarget { .. } => "duplicate_target",
        NativeEditError::SymlinkRejected { .. } => "symlink_rejected",
        NativeEditError::ExpectedFile { .. } => "expected_file",
        NativeEditError::UnsupportedMetadataPath { .. } => "unsupported_metadata_path",
        NativeEditError::UnsupportedFileType { .. } => "unsupported_file_type",
        NativeEditError::NotUtf8 { .. } => "not_utf8",
        NativeEditError::FileTooLarge { .. } => "file_too_large",
        NativeEditError::HunkNotFound { .. } => "hunk_not_found",
        NativeEditError::HunkAmbiguous { .. } => "hunk_ambiguous",
        NativeEditError::EmptyHunks { .. } => "empty_hunks",
        NativeEditError::EmptyFind { .. } => "empty_find",
        NativeEditError::HashMismatch { .. } => "hash_mismatch",
        NativeEditError::Io { .. } => "io",
    }
}
```

- [ ] **Step 5: Create the edit harness module with summary conversion**

Create `crates/yach-backend/src/edit_harness.rs`:

```rust
use crate::{
    NativeEditAppliedOperation, NativeEditApplyResult, NativeEditOperationEvidence,
    NativeEditEvidenceSummary, NativeToolPayloadSummary, PreparedNativeEditOperation,
    PreparedNativeEditTransaction,
};

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
        operations: result.operations.iter().map(applied_operation_evidence).collect(),
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

fn applied_operation_evidence(operation: &NativeEditAppliedOperation) -> NativeEditOperationEvidence {
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
```

- [ ] **Step 6: Wire the module**

In `crates/yach-backend/src/lib.rs`, add:

```rust
mod edit_harness;
```

Add crate-local re-exports for tests and same-crate callers, but do not add a
public re-export:

```rust
pub(crate) use edit::native_edit_error_label;
pub(crate) use edit_harness::{
    native_edit_apply_evidence_summary, native_edit_prepared_evidence_summary,
};
```

- [ ] **Step 7: Import helpers for crate tests**

In the `use super::{ ... }` list in `crates/yach-backend/src/lib.rs`, add:

```rust
native_edit_error_label, native_edit_prepared_evidence_summary, NativeEditEngine,
NativeEditError, NativeEditOperation, NativeEditPolicy, NativeEditTransactionRequest,
```

- [ ] **Step 8: Run focused tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness_summarizes_preview_without_file_bodies -- --nocapture
just dev cargo test -p yach-backend native_edit_error_labels_are_categorical -- --nocapture
```

Expected: both tests pass.

## Task 3: Backend-Local Preview/Apply Harness

**Files:**
- Modify: `crates/yach-backend/src/edit_harness.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing successful harness test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_harness_records_prepare_and_complete_events() {
    let root_path = temp_resource_dir("native-edit-harness-success");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    let Some(root) = root else {
        return;
    };
    let mut log = NativeSessionLog::default();
    let context = NativeEditHarnessContext {
        session_id: NativeSessionId(String::from("session-edit")),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_request_id: None,
    };

    let result = NativeEditHarness::preview_and_apply(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("created\n"),
            }],
        },
        &NativeEditPolicy::test(),
        &mut log,
        &context,
    );

    assert!(result.is_ok());
    assert_eq!(
        std::fs::read_to_string(root_path.join("src/new.rs")).ok(),
        Some(String::from("created\n"))
    );
    assert!(matches!(
        log.events.as_slice(),
        [
            NativeSessionEvent::EditTransactionPrepared { transaction_id, summary, .. },
            NativeSessionEvent::EditTransactionFinished {
                transaction_id: Some(finished_transaction_id),
                outcome: NativeEditEvidenceOutcome::Completed,
                reason: None,
                summary: Some(finished_summary),
                ..
            },
        ] if transaction_id == finished_transaction_id
            && summary.operation_count == 1
            && finished_summary.operation_count == 1
            && matches!(
                finished_summary.operations.as_slice(),
                [NativeEditOperationEvidence::CreateTextFile {
                    relative_path,
                    bytes_written: Some(8),
                    ..
                }] if relative_path == "src/new.rs"
            )
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Add failing preview failure evidence test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_harness_records_validation_failure_without_raw_payload() {
    let root_path = temp_resource_dir("native-edit-harness-validation-failure");
    let root = NativeResourceRoot::project(&root_path).ok();
    let Some(root) = root else {
        return;
    };
    let mut log = NativeSessionLog::default();
    let context = NativeEditHarnessContext {
        session_id: NativeSessionId(String::from("session-edit")),
        turn_id: NativeTurnId(String::from("turn-2")),
        tool_request_id: None,
    };

    let result = NativeEditHarness::preview_and_apply(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("../outside.rs"),
                content: String::from("secret payload\n"),
            }],
        },
        &NativeEditPolicy::test(),
        &mut log,
        &context,
    );

    assert!(matches!(result, Err(NativeEditError::PathTraversal { .. })));
    assert!(matches!(
        log.events.as_slice(),
        [NativeSessionEvent::EditTransactionFinished {
            transaction_id: None,
            outcome: NativeEditEvidenceOutcome::ValidationFailed,
            reason: Some(reason),
            summary: None,
            ..
        }] if reason == "path_traversal"
    ));
    assert!(
        serde_json::to_string(&log.events)
            .is_ok_and(|json| !json.contains("secret payload"))
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 3: Add failing apply failure evidence test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_harness_records_apply_failure_after_prepare() {
    let root_path = temp_resource_dir("native-edit-harness-apply-failure");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    let Some(root) = root else {
        return;
    };
    let mut policy = NativeEditPolicy::test();
    policy.allow_create = true;
    let mut deny_apply_policy = policy;
    deny_apply_policy.allow_create = false;
    let mut log = NativeSessionLog::default();
    let context = NativeEditHarnessContext {
        session_id: NativeSessionId(String::from("session-edit")),
        turn_id: NativeTurnId(String::from("turn-3")),
        tool_request_id: Some(NativeToolRequestId(String::from("tool-request-local-edit"))),
    };

    let result = NativeEditHarness::preview_and_apply_with_apply_policy(
        &root,
        NativeEditTransactionRequest {
            operations: vec![NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("created\n"),
            }],
        },
        &policy,
        &deny_apply_policy,
        &mut log,
        &context,
    );

    assert_eq!(result, Err(NativeEditError::CreateDisabled));
    assert!(!root_path.join("src/new.rs").exists());
    assert!(matches!(
        log.events.as_slice(),
        [
            NativeSessionEvent::EditTransactionPrepared {
                tool_request_id: Some(NativeToolRequestId(tool_request_id)),
                transaction_id,
                ..
            },
            NativeSessionEvent::EditTransactionFinished {
                tool_request_id: Some(NativeToolRequestId(finished_tool_request_id)),
                transaction_id: Some(finished_transaction_id),
                outcome: NativeEditEvidenceOutcome::Failed,
                reason: Some(reason),
                summary: Some(summary),
                ..
            },
        ] if tool_request_id == "tool-request-local-edit"
            && finished_tool_request_id == "tool-request-local-edit"
            && transaction_id == finished_transaction_id
            && reason == "create_disabled"
            && summary.operation_count == 1
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 4: Run focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness_records_ -- --nocapture
```

Expected: compile failures for missing `NativeEditHarness`, `NativeEditHarnessContext`, and harness methods.

- [ ] **Step 5: Implement harness context and methods**

Append to `crates/yach-backend/src/edit_harness.rs`:

```rust
use crate::{
    native_edit_error_label, NativeEditEngine, NativeEditError, NativeEditEvidenceOutcome,
    NativeEditPolicy, NativeEditTransactionRequest, NativeResourceRoot, NativeSessionEvent,
    NativeSessionId, NativeSessionLog, NativeToolRequestId, NativeTurnId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
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
        policy: &NativeEditPolicy,
        log: &mut NativeSessionLog,
        context: &NativeEditHarnessContext,
    ) -> Result<NativeEditApplyResult, NativeEditError> {
        Self::preview_and_apply_with_apply_policy(root, request, policy, policy, log, context)
    }

    pub(crate) fn preview_and_apply_with_apply_policy(
        root: &NativeResourceRoot,
        request: NativeEditTransactionRequest,
        preview_policy: &NativeEditPolicy,
        apply_policy: &NativeEditPolicy,
        log: &mut NativeSessionLog,
        context: &NativeEditHarnessContext,
    ) -> Result<NativeEditApplyResult, NativeEditError> {
        let preview = match NativeEditEngine::preview(root, request, preview_policy) {
            Ok(preview) => preview,
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: context.tool_request_id.clone(),
                    transaction_id: None,
                    outcome: NativeEditEvidenceOutcome::ValidationFailed,
                    reason: Some(native_edit_error_label(&error).to_owned()),
                    summary: None,
                });
                return Err(error);
            }
        };

        let transaction_id = preview.transaction_id.clone();
        let prepared_summary = native_edit_prepared_evidence_summary(&preview);
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: context.tool_request_id.clone(),
            transaction_id: transaction_id.clone(),
            summary: prepared_summary.clone(),
        });

        match NativeEditEngine::apply(root, preview, apply_policy) {
            Ok(result) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: context.tool_request_id.clone(),
                    transaction_id: Some(result.transaction_id.clone()),
                    outcome: NativeEditEvidenceOutcome::Completed,
                    reason: None,
                    summary: Some(native_edit_apply_evidence_summary(&result)),
                });
                Ok(result)
            }
            Err(error) => {
                log.push(NativeSessionEvent::EditTransactionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: context.tool_request_id.clone(),
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
```

If rustfmt or clippy flags duplicate imports, merge the `use crate::{ ... }` groups into one sorted import block.

- [ ] **Step 6: Import harness types in tests**

In `crates/yach-backend/src/lib.rs` test imports, add:

```rust
NativeEditHarness, NativeEditHarnessContext,
```

Also add crate-local re-exports near the Task 2 `edit_harness` re-exports:

```rust
pub(crate) use edit_harness::{NativeEditHarness, NativeEditHarnessContext};
```

- [ ] **Step 7: Run focused harness tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness -- --nocapture
```

Expected: all native edit harness tests pass.

## Task 4: Guard Against Provider/Registry Mutation Exposure

**Files:**
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add provider exposure guard test**

Add this test to `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_edit_harness_does_not_register_or_advertise_mutation_tools() {
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let policy = NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
    let candidates = registry.provider_advertising_candidates(&policy, ["project_path_info"]);

    assert_eq!(registry.get("edit"), None);
    assert_eq!(registry.get("write"), None);
    assert_eq!(registry.get("native_edit"), None);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "project_path_info");
}
```

- [ ] **Step 2: Run the guard test**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness_does_not_register_or_advertise_mutation_tools -- --nocapture
```

Expected: the test passes without production code changes.

## Task 5: Final Verification And Commit

**Files:**
- Modify: `crates/yach-backend/src/edit.rs`
- Create: `crates/yach-backend/src/edit_harness.rs`
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Run formatting**

Run:

```bash
just dev cargo fmt
just dev cargo fmt --check
```

Expected: formatting check passes.

- [ ] **Step 2: Run focused edit/session tests**

Run:

```bash
just dev cargo test -p yach-backend native_edit_harness -- --nocapture
just dev cargo test -p yach-backend native_session_log_preserves_edit_transaction_evidence_jsonl -- --nocapture
just dev cargo test -p yach-backend native_session_log -- --nocapture
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --nocapture
just dev cargo test -p yach-backend native_runner::tests:: -- --nocapture
```

Expected: all focused tests pass.

- [ ] **Step 3: Run backend tests and clippy**

Run:

```bash
just dev cargo test -p yach-backend
just dev cargo clippy -p yach-backend --lib -- -D warnings
git diff --check
```

Expected: backend tests, clippy, and whitespace checks pass.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yach-backend/src/edit.rs crates/yach-backend/src/edit_harness.rs crates/yach-backend/src/session.rs crates/yach-backend/src/native_runner.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Record native edit transaction evidence"
```

Expected: one implementation commit.

## Follow-Up Slices

- Add a local CLI or TUI-accessible harness entry point after deciding the UX.
- Add Criterion benchmarks for preview/apply/evidence timing.
- Design provider-visible edit/write tools and approval UX separately.
- Design extension-owned mutation capability routing separately.
