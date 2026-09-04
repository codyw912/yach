# Production Edit Tracing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement bounded durable production trace records for provider-originated native agent edits.

**Architecture:** Add a distinct `EditTraceRecorded` native session event family, then thread one `NativeEditTraceId` through provider-originated edit validation, normalization, permission/preview, review wait, apply/reject, result shaping, and provider continuation. Keep trace records diagnostic-only: provider transcript projection ignores them, edit evidence remains the authoritative local-effect record, and trace fields are categorical and bounded.

**Tech Stack:** Rust workspace, `yach-backend`, native JSONL session evidence, native provider one-round tool path, existing `just dev cargo test ...` recipes.

---

## Scope Notes

This plan implements `docs/project/specs/2026-05-17-production-edit-tracing-design.md`.

It does not add provider schemas, new edit/write tools, extension mutation,
auto-review runtime, sandboxing, TUI UI changes, or a high-frequency tracing
framework. Trace records must not include raw provider arguments, file bodies,
absolute paths, full diffs, model text, extension stdout, or provider debug
payloads.

## File Structure

- `crates/yach-backend/src/session.rs`: trace ID/type definitions, session event variant, bounded trace normalization helpers, JSONL round-trip, turn indexing, and transcript-ignore behavior.
- `crates/yach-backend/src/edit_access.rs`: explicit prepare diagnostics seam so permission and preview phases can be correlated without scraping side-effected log events.
- `crates/yach-backend/src/agent_edit_tools.rs`: trace ID creation and tool validation, normalization, permission/preview, apply/reject, and result-shaping trace records.
- `crates/yach-backend/src/native_runner.rs`: review-wait timing, provider-continuation timing, trace identity handoff through provider round state, and provider projection tests.
- `crates/yach-backend/src/lib.rs`: backend tests for agent edit tracing, privacy bounds, JSONL compatibility, and helper behavior.
- `docs/project/state.md` and `docs/project/next.md`: update active planning handoff after implementation lands.

---

### Task 1: Add Native Edit Trace Session Records

**Files:**
- Modify: `crates/yach-backend/src/session.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing JSONL and projection tests**

Add tests near `native_session_log_preserves_edit_transaction_evidence_jsonl` and `native_session_permission_evidence_is_not_provider_transcript` in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_session_log_preserves_edit_trace_records_jsonl() {
    let path = temp_resource_dir("native-edit-trace-jsonl").join("session.jsonl");
    let mut log = NativeSessionLog::default();
    log.record_edit_trace(
        NativeSessionId(String::from("default")),
        NativeTurnId(String::from("turn-7")),
        NativeEditTraceRecord {
            trace_id: NativeEditTraceId(String::from("edit-trace-1")),
            phase: NativeEditTracePhase::Preview,
            source: NativeEditTraceSource::ProviderTool,
            tool_name: Some(String::from("edit_text_file")),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            provider_call_id: Some(String::from("call-edit-1")),
            preview_id: Some(NativeEditPreviewId(String::from("edit-preview-1"))),
            permission_decision_id: Some(NativePermissionDecisionId(String::from(
                "permission-decision-1",
            ))),
            transaction_id: Some(NativeEditTransactionId(String::from("edit-1"))),
            outcome: NativeEditTraceOutcome::Completed,
            duration_ms: 3,
            reason_label: None,
            attributes: vec![NativeMetricAttribute {
                key: String::from("operation"),
                value: String::from("edit_text_file"),
            }],
        },
    );

    assert!(log.write_to_file(&path).is_ok());
    let raw = std::fs::read_to_string(&path).ok();
    let loaded = NativeSessionLog::load_from_file(&path).ok();

    assert!(raw.as_deref().is_some_and(|raw| {
        raw.contains("edit_trace_recorded")
            && raw.contains("\"phase\":\"preview\"")
            && raw.contains("\"trace_id\":\"edit-trace-1\"")
    }));
    assert_eq!(loaded.as_ref().map(NativeSessionLog::next_turn_index), Some(8));
    assert_eq!(loaded, Some(log));
    if let Some(parent) = path.parent() {
        assert!(std::fs::remove_dir_all(parent).is_ok());
    }
}

#[test]
fn native_session_edit_trace_is_not_provider_transcript() {
    let mut log = completed_text_exchange(
        NativeSessionId(String::from("default")),
        NativeEntryId(String::from("entry-1-user")),
        NativeEntryId(String::from("entry-1-assistant")),
        NativeTurnId(String::from("turn-1")),
        String::from("hello"),
        String::from("world"),
    );
    log.record_edit_trace(
        NativeSessionId(String::from("default")),
        NativeTurnId(String::from("turn-1")),
        NativeEditTraceRecord::test_record(
            NativeEditTraceId(String::from("edit-trace-1")),
            NativeEditTracePhase::Preview,
        ),
    );

    let messages = log.transcript_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "hello");
    assert_eq!(messages[1].text, "world");
}
```

Add a `#[cfg(test)]` helper on `NativeEditTraceRecord` in `session.rs`:

```rust
#[cfg(test)]
impl NativeEditTraceRecord {
    pub(crate) fn test_record(
        trace_id: NativeEditTraceId,
        phase: NativeEditTracePhase,
    ) -> Self {
        Self {
            trace_id,
            phase,
            source: NativeEditTraceSource::ProviderTool,
            tool_name: Some(String::from("edit_text_file")),
            tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
            provider_call_id: Some(String::from("call-edit-1")),
            preview_id: None,
            permission_decision_id: None,
            transaction_id: None,
            outcome: NativeEditTraceOutcome::Completed,
            duration_ms: 1,
            reason_label: None,
            attributes: Vec::new(),
        }
    }
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_edit_trace_records_jsonl -- --exact
just dev cargo test -p yach-backend native_session_edit_trace_is_not_provider_transcript -- --exact
```

Expected: compile fails because the trace types, session event variant, and `record_edit_trace` helper do not exist.

- [ ] **Step 3: Add trace record types in `session.rs`**

Add imports at the top of `crates/yach-backend/src/session.rs`:

```rust
use crate::{
    NativeEditPreviewId, NativeEditTransactionId, NativePermissionDecisionId,
    NativePermissionDecisionSummary, NativeToolError, NativeToolPermissionState,
};
```

Add trace types after `NativeDurationMetric`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeEditTraceId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditTracePhase {
    ToolValidation,
    ArgumentNormalization,
    PermissionDecision,
    Preview,
    ReviewWait,
    Apply,
    Reject,
    ResultShaping,
    ProviderContinuation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditTraceSource {
    ProviderTool,
    LocalUi,
    ExtensionTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEditTraceOutcome {
    Completed,
    Failed,
    Denied,
    Rejected,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEditTraceRecord {
    pub trace_id: NativeEditTraceId,
    pub phase: NativeEditTracePhase,
    pub source: NativeEditTraceSource,
    pub tool_name: Option<String>,
    pub tool_request_id: Option<NativeToolRequestId>,
    pub provider_call_id: Option<String>,
    pub preview_id: Option<NativeEditPreviewId>,
    pub permission_decision_id: Option<NativePermissionDecisionId>,
    pub transaction_id: Option<NativeEditTransactionId>,
    pub outcome: NativeEditTraceOutcome,
    pub duration_ms: u64,
    pub reason_label: Option<String>,
    pub attributes: Vec<NativeMetricAttribute>,
}
```

Do not add a `Started` outcome unless a later implementation task introduces a real write-ahead trace need.

- [ ] **Step 4: Add bounded trace normalization helpers**

Add constants and helpers in `session.rs`:

```rust
const TRACE_TOOL_NAME_MAX_BYTES: usize = 64;
const TRACE_PROVIDER_CALL_ID_MAX_BYTES: usize = 256;
const TRACE_REASON_LABEL_MAX_BYTES: usize = 64;
const TRACE_ATTRIBUTE_LIMIT: usize = 8;
const TRACE_ATTRIBUTE_KEY_MAX_BYTES: usize = 48;
const TRACE_ATTRIBUTE_VALUE_MAX_BYTES: usize = 128;

#[must_use]
pub fn bounded_edit_trace_record(mut record: NativeEditTraceRecord) -> NativeEditTraceRecord {
    record.tool_name = record
        .tool_name
        .map(|value| bounded_trace_string(&value, TRACE_TOOL_NAME_MAX_BYTES));
    record.provider_call_id = record
        .provider_call_id
        .map(|value| bounded_trace_string(&value, TRACE_PROVIDER_CALL_ID_MAX_BYTES));
    record.reason_label = record
        .reason_label
        .map(|value| bounded_trace_reason_label(&value));
    record.attributes = record
        .attributes
        .into_iter()
        .take(TRACE_ATTRIBUTE_LIMIT)
        .map(|attribute| NativeMetricAttribute {
            key: bounded_trace_string(&attribute.key, TRACE_ATTRIBUTE_KEY_MAX_BYTES),
            value: bounded_trace_string(&attribute.value, TRACE_ATTRIBUTE_VALUE_MAX_BYTES),
        })
        .collect();
    record
}

fn bounded_trace_reason_label(value: &str) -> String {
    let bounded = bounded_trace_string(value, TRACE_REASON_LABEL_MAX_BYTES);
    if bounded
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        bounded
    } else {
        String::from("redacted_reason")
    }
}

fn bounded_trace_string(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_owned()
}
```

If the worker chooses a different helper name, update tests and imports consistently.

- [ ] **Step 5: Add the session event variant and projection handling**

Add to `NativeSessionEvent`:

```rust
EditTraceRecorded {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    trace: NativeEditTraceRecord,
},
```

Update all exhaustive matches in `session.rs` and `native_runner.rs` so trace events are ignored by transcript/provider projections and included in `event_turn_id`.

Add to `impl NativeSessionLog`:

```rust
pub fn record_edit_trace(
    &mut self,
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    trace: NativeEditTraceRecord,
) {
    self.push(NativeSessionEvent::EditTraceRecorded {
        session_id,
        turn_id,
        trace: bounded_edit_trace_record(trace),
    });
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_edit_trace_records_jsonl -- --exact
just dev cargo test -p yach-backend native_session_edit_trace_is_not_provider_transcript -- --exact
just dev cargo test -p yach-backend native_provider_messages_ignore_agent_edit_evidence -- --exact
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs crates/yach-backend/src/native_runner.rs
git commit -m "feat: add native edit trace session records"
```

---

### Task 2: Add Edit Access Prepare Diagnostics

**Files:**
- Modify: `crates/yach-backend/src/edit_access.rs`
- Modify: `crates/yach-backend/src/agent_edit_tools.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing tests for explicit prepare diagnostics**

Add tests near existing agent edit tool tests in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn agent_edit_tool_prepare_review_carries_trace_identity() {
    let root_guard = temp_native_edit_root("agent-edit-trace-review");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).expect("project root");
    let store = NativeJsonlSessionStore::new(root_guard.root().join("session.jsonl"));
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let prepared = prepare_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::default_local_edit(),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    );

    let Ok(NativeAgentEditToolPrepared::NeedsUserReview { trace_id, .. }) = prepared else {
        assert!(matches!(
            prepared,
            Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
        ));
        return;
    };
    assert!(trace_id.0.starts_with("edit-trace-"));
}
```

Add a unit test in `crates/yach-backend/src/edit_access.rs` for a new diagnostics-returning prepare method:

```rust
#[test]
fn prepare_with_diagnostics_reports_permission_and_preview_ids() {
    let project = TempProject::new("diagnostics");
    write_file(&project, "file.txt", "hello\n");
    let Some(root) = native_root(&project) else {
        return;
    };
    let mut access = NativeEditAccess::default();
    let mut log = NativeSessionLog::default();

    let outcome = access.prepare_with_diagnostics(
        &root,
        modify_request(),
        context(NativePermissionMode::Ask),
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
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend prepare_with_diagnostics_reports_permission_and_preview_ids -- --exact
just dev cargo test -p yach-backend agent_edit_tool_prepare_review_carries_trace_identity -- --exact
```

Expected: compile fails because diagnostics and trace handoff fields do not exist.

- [ ] **Step 3: Add diagnostics structs in `edit_access.rs`**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditAccessPrepareDiagnostics {
    pub permission_decision_id: NativePermissionDecisionId,
    pub review_state: NativeEditAccessReviewState,
    pub transaction_id: Option<NativeEditTransactionId>,
    pub reason_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditAccessPrepareOutcome {
    pub preview: NativeEditPreview,
    pub diagnostics: NativeEditAccessPrepareDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeEditAccessPrepareError {
    PermissionDenied {
        reason: String,
        diagnostics: NativeEditAccessPrepareDiagnostics,
    },
    Preview {
        error: NativeEditError,
        diagnostics: NativeEditAccessPrepareDiagnostics,
    },
}
```

- [ ] **Step 4: Add `prepare_with_diagnostics` without breaking existing callers**

Refactor the existing `prepare` body into:

```rust
pub fn prepare_with_diagnostics(
    &mut self,
    root: &NativeResourceRoot,
    request: NativeEditTransactionRequest,
    context: NativeEditAccessContext,
    log: &mut NativeSessionLog,
) -> Result<NativeEditAccessPrepareOutcome, NativeEditAccessPrepareError> {
    let permission_request = permission_request_from_edit(&request);
    let decision =
        NativePermissionDecisionEngine::decide(&permission_request, &context.permission_policy);
    let permission_summary = decision.summary(&permission_request, false);
    log.record_permission_decision(
        context.session_id.clone(),
        context.turn_id.clone(),
        permission_summary.clone(),
    );

    let permission_decision_id = decision.decision_id();
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
            let diagnostics = NativeEditAccessPrepareDiagnostics {
                permission_decision_id,
                review_state: NativeEditAccessReviewState::NeedsUserApproval,
                transaction_id: None,
                reason_label: Some(reason.clone()),
            };
            return Err(NativeEditAccessPrepareError::PermissionDenied {
                reason: reason.clone(),
                diagnostics,
            });
        }
    };

    let prepared = NativeEditEngine::preview(root, request, &context.edit_policy).map_err(|error| {
        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: context.session_id.clone(),
            turn_id: context.turn_id.clone(),
            tool_request_id: context.tool_request_id.clone(),
            transaction_id: None,
            outcome: NativeEditEvidenceOutcome::ValidationFailed,
            reason: Some(native_edit_error_label(&error).to_owned()),
            summary: None,
        });
        NativeEditAccessPrepareError::Preview {
            diagnostics: NativeEditAccessPrepareDiagnostics {
                permission_decision_id: permission_decision_id.clone(),
                review_state: review_state.clone(),
                transaction_id: None,
                reason_label: Some(native_edit_error_label(&error).to_owned()),
            },
            error,
        }
    })?;

    let summary = native_edit_prepared_evidence_summary(&prepared);
    log.push(NativeSessionEvent::EditTransactionPrepared {
        session_id: context.session_id.clone(),
        turn_id: context.turn_id.clone(),
        tool_request_id: context.tool_request_id.clone(),
        transaction_id: prepared.transaction_id.clone(),
        summary,
    });

    let preview_id = NativeEditPreviewId(next_edit_preview_id());
    let preview = NativeEditPreview {
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
        PendingNativeEditPreview {
            context,
            root: root.clone(),
            prepared,
            permission_decision_id: preview.permission_decision_id.clone(),
            permission_summary,
        },
    );

    Ok(NativeEditAccessPrepareOutcome {
        diagnostics: NativeEditAccessPrepareDiagnostics {
            permission_decision_id,
            review_state,
            transaction_id: Some(preview.transaction_id.clone()),
            reason_label: None,
        },
        preview,
    })
}
```

Keep the existing `prepare` public API by wrapping the new method and mapping errors back to `NativeEditAccessError`:

```rust
pub fn prepare(
    &mut self,
    root: &NativeResourceRoot,
    request: NativeEditTransactionRequest,
    context: NativeEditAccessContext,
    log: &mut NativeSessionLog,
) -> Result<NativeEditPreview, NativeEditAccessError> {
    self.prepare_with_diagnostics(root, request, context, log)
        .map(|outcome| outcome.preview)
        .map_err(|error| match error {
            NativeEditAccessPrepareError::PermissionDenied { reason, .. } => {
                NativeEditAccessError::PermissionDenied { reason }
            }
            NativeEditAccessPrepareError::Preview { error, .. } => {
                NativeEditAccessError::Preview(error)
            }
        })
}
```

Use existing reason labels from `native_edit_error_label` and existing permission reasons. Do not expose prepared transactions or after-images.

- [ ] **Step 5: Add trace ID fields to agent edit handoff types**

In `crates/yach-backend/src/agent_edit_tools.rs`, add `NativeEditTraceId` imports and add `trace_id` fields:

```rust
pub enum NativeAgentEditToolPrepared {
    Completed {
        trace_id: NativeEditTraceId,
        result: NativeProviderToolResult,
    },
    Denied {
        trace_id: NativeEditTraceId,
        result: NativeProviderToolResult,
    },
    NeedsUserReview {
        trace_id: NativeEditTraceId,
        request_id: String,
        provider_call_id: String,
        preview: NativeEditPreview,
        path: String,
        operation: String,
    },
}

pub struct PendingAgentEditToolReview {
    pub trace_id: NativeEditTraceId,
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub request_id: String,
    pub provider_call_id: String,
    pub preview_id: NativeEditPreviewId,
    pub permission_decision_id: NativePermissionDecisionId,
    pub path: String,
    pub operation: String,
}
```

Update all matches in `agent_edit_tools.rs`, `native_runner.rs`, and tests.

- [ ] **Step 6: Run focused tests**

Run:

```bash
just dev cargo test -p yach-backend prepare_with_diagnostics_reports_permission_and_preview_ids -- --exact
just dev cargo test -p yach-backend agent_edit_tool_prepare_review_carries_trace_identity -- --exact
just dev cargo test -p yach-backend agent_edit -- --nocapture
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/yach-backend/src/edit_access.rs crates/yach-backend/src/agent_edit_tools.rs crates/yach-backend/src/native_runner.rs crates/yach-backend/src/lib.rs
git commit -m "feat: expose edit prepare trace diagnostics"
```

---

### Task 3: Trace Provider-Originated Agent Edit Phases

**Files:**
- Modify: `crates/yach-backend/src/agent_edit_tools.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing tests for successful, rejected, validation-failed, and bounded trace records**

Add these tests near existing agent edit tool tests in `crates/yach-backend/src/lib.rs`:

```rust
fn edit_trace_records(log: &NativeSessionLog) -> Vec<NativeEditTraceRecord> {
    log.events
        .iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EditTraceRecorded { trace, .. } => Some(trace.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn agent_edit_tool_allow_mode_records_correlated_trace_phases() {
    let root_guard = temp_native_edit_root("agent-edit-trace-allow");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).expect("project root");
    let store_path = root_guard.root().join("session.jsonl");
    let store = NativeJsonlSessionStore::new(store_path.clone());
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let result = execute_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(
                NativePermissionMode::Allow,
            ),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    );

    assert!(result.is_ok());
    let log = NativeJsonlSessionStore::new(store_path).load().expect("session log");
    let traces = edit_trace_records(&log);
    let trace_id = traces
        .first()
        .map(|trace| trace.trace_id.clone())
        .expect("at least one trace");
    for phase in [
        NativeEditTracePhase::ToolValidation,
        NativeEditTracePhase::ArgumentNormalization,
        NativeEditTracePhase::PermissionDecision,
        NativeEditTracePhase::Preview,
        NativeEditTracePhase::Apply,
        NativeEditTracePhase::ResultShaping,
    ] {
        assert!(traces.iter().any(|trace| {
            trace.trace_id == trace_id
                && trace.phase == phase
                && trace.outcome == NativeEditTraceOutcome::Completed
                && trace.tool_request_id.as_ref().map(|id| id.0.as_str())
                    == Some("tool-request-1")
                && trace.provider_call_id.as_deref() == Some("call-edit-1")
        }));
    }
}

#[test]
fn agent_edit_tool_reject_review_records_rejected_trace_phase() {
    let root_guard = temp_native_edit_root("agent-edit-trace-reject");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).expect("project root");
    let store_path = root_guard.root().join("session.jsonl");
    let store = NativeJsonlSessionStore::new(store_path.clone());
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };
    let prepared = prepare_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::default_local_edit(),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    );
    let Ok(NativeAgentEditToolPrepared::NeedsUserReview {
        trace_id,
        request_id,
        provider_call_id,
        preview,
        path,
        operation,
    }) = prepared else {
        assert!(matches!(
            prepared,
            Ok(NativeAgentEditToolPrepared::NeedsUserReview { .. })
        ));
        return;
    };

    let result = reject_agent_edit_tool_review(
        &mut access,
        &store,
        PendingAgentEditToolReview {
            trace_id: trace_id.clone(),
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            request_id,
            provider_call_id,
            preview_id: preview.preview_id,
            permission_decision_id: preview.permission_decision_id,
            path,
            operation,
        },
    );

    assert!(result.is_ok());
    let log = NativeJsonlSessionStore::new(store_path).load().expect("session log");
    let traces = edit_trace_records(&log);
    assert!(traces.iter().any(|trace| {
        trace.trace_id == trace_id
            && trace.phase == NativeEditTracePhase::Reject
            && trace.outcome == NativeEditTraceOutcome::Rejected
            && trace.reason_label.as_deref() == Some("user_rejected")
    }));
}

#[test]
fn agent_edit_tool_missing_provider_call_id_records_validation_trace_without_transaction() {
    let root_guard = temp_native_edit_root("agent-edit-trace-missing-provider-call");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).expect("project root");
    let store_path = root_guard.root().join("session.jsonl");
    let store = NativeJsonlSessionStore::new(store_path.clone());
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: None,
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let result = prepare_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::default_local_edit(),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::Validation(
            NativeToolError::MalformedArguments
        ))
    );
    let log = NativeJsonlSessionStore::new(store_path).load().expect("session log");
    let traces = edit_trace_records(&log);
    assert!(traces.iter().any(|trace| {
        trace.phase == NativeEditTracePhase::ToolValidation
            && trace.outcome == NativeEditTraceOutcome::Failed
            && trace.reason_label.as_deref() == Some("missing_provider_call_id")
            && trace.transaction_id.is_none()
    }));
}

#[test]
fn agent_edit_trace_records_are_bounded_and_do_not_include_raw_arguments() {
    let root_guard = temp_native_edit_root("agent-edit-trace-bounds");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).expect("project root");
    let store_path = root_guard.root().join("session.jsonl");
    let store = NativeJsonlSessionStore::new(store_path.clone());
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let sentinel = "RAW_ARGUMENT_SENTINEL_DO_NOT_PERSIST";
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some("call-".repeat(80)),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": sentinel
        }),
    };

    let result = execute_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(
                NativePermissionMode::Allow,
            ),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    );

    assert!(result.is_ok());
    let raw = std::fs::read_to_string(&store_path).expect("raw session log");
    assert!(raw.contains("edit_trace_recorded"));
    assert!(!raw.contains(sentinel));
    let log = NativeJsonlSessionStore::new(store_path).load().expect("session log");
    let traces = edit_trace_records(&log);
    assert!(traces.iter().all(|trace| {
        trace
            .provider_call_id
            .as_ref()
            .is_none_or(|provider_call_id| provider_call_id.len() <= 256)
    }));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_tool_allow_mode_records_correlated_trace_phases -- --exact
just dev cargo test -p yach-backend agent_edit_tool_reject_review_records_rejected_trace_phase -- --exact
just dev cargo test -p yach-backend agent_edit_tool_missing_provider_call_id_records_validation_trace_without_transaction -- --exact
just dev cargo test -p yach-backend agent_edit_trace_records_are_bounded_and_do_not_include_raw_arguments -- --exact
```

Expected: tests fail because agent edit tracing is not emitted yet.

- [ ] **Step 3: Add trace helper functions in `agent_edit_tools.rs`**

Add a counter and helpers:

```rust
static AGENT_EDIT_TRACE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_agent_edit_trace_id() -> NativeEditTraceId {
    let next = AGENT_EDIT_TRACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    NativeEditTraceId(format!("edit-trace-{next}"))
}

fn duration_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn trace_attribute(key: &str, value: impl Into<String>) -> NativeMetricAttribute {
    NativeMetricAttribute {
        key: key.to_owned(),
        value: value.into(),
    }
}
```

Use `NativeSessionLog::record_edit_trace` to append trace records into the same local log batches that already get persisted through `append_events`.

- [ ] **Step 4: Emit validation and normalization traces**

At the start of `prepare_agent_edit_tool_request`, mint `trace_id`.

Time and record:

- `NativeEditTracePhase::ToolValidation` for turn mismatch, schema validation, missing provider call ID, and success.
- `NativeEditTracePhase::ArgumentNormalization` for normalization success, permission denial from metadata path, and malformed arguments.

Each record should include `source=ProviderTool`, `tool_name`, `tool_request_id`, `provider_call_id` when available, no preview/permission/transaction IDs yet, bounded `reason_label`, and `operation` attribute where known.

- [ ] **Step 5: Emit permission, preview, apply/reject, and result-shaping traces**

Change `prepare_agent_edit_tool_request` to call `edit_access.prepare_with_diagnostics`.

Emit:

- `permission_decision` using diagnostics permission decision ID, review state, and denial reason where applicable.
- `preview` with transaction and preview IDs on success; `failed` with no transaction ID for preview validation failures.
- `apply` from `apply_agent_edit_tool_review` with `completed` or `failed`.
- `reject` from `reject_agent_edit_tool_review` with `rejected` and `user_rejected`.
- `result_shaping` after provider result creation for applied, rejected, denied, and validation-failed outcomes.

Do not trace raw result content. Use only categorical attributes such as `operation`, `review_state`, `decision`, and `evidence_persisted`.

- [ ] **Step 6: Preserve existing evidence safety**

Keep existing write-ahead behavior in `NativeEditAccess::apply_with_evidence_sink`. If trace append fails after visible apply, do not convert the edit to failed. Use a bounded `reason_label` such as `trace_persist_failed` only where existing result shaping already has a diagnostic place to report it.

- [ ] **Step 7: Run focused tests**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_tool_allow_mode_records_correlated_trace_phases -- --exact
just dev cargo test -p yach-backend agent_edit_tool_reject_review_records_rejected_trace_phase -- --exact
just dev cargo test -p yach-backend agent_edit_tool_missing_provider_call_id_records_validation_trace_without_transaction -- --exact
just dev cargo test -p yach-backend agent_edit_trace_records_are_bounded_and_do_not_include_raw_arguments -- --exact
just dev cargo test -p yach-backend agent_edit -- --nocapture
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/yach-backend/src/agent_edit_tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: trace agent edit tool phases"
```

---

### Task 4: Trace Review Wait And Provider Continuation

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Test: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write failing runner tests**

Extend `native_provider_agent_edit_tool_pauses_for_user_review_and_continues` so after loading the session log it asserts:

```rust
let trace_records: Vec<_> = log
    .events
    .iter()
    .filter_map(|event| match event {
        NativeSessionEvent::EditTraceRecorded { trace, .. } => Some(trace),
        _ => None,
    })
    .collect();
assert!(trace_records.iter().any(|trace| {
    trace.phase == NativeEditTracePhase::ReviewWait
        && trace.outcome == NativeEditTraceOutcome::Completed
}));
assert!(trace_records.iter().any(|trace| {
    trace.phase == NativeEditTracePhase::ProviderContinuation
        && trace.outcome == NativeEditTraceOutcome::Completed
}));
```

Add this helper-level test in `crates/yach-backend/src/native_runner.rs` near the provider one-round tests:

```rust
#[test]
fn native_provider_agent_edit_continuation_records_each_edit_trace() {
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    let trace_ids = vec![
        NativeEditTraceId(String::from("edit-trace-1")),
        NativeEditTraceId(String::from("edit-trace-2")),
    ];

    record_provider_continuation_trace_records(
        &mut log,
        &mut pending_events,
        NativeSessionId(String::from("default")),
        NativeTurnId(String::from("turn-1")),
        &trace_ids,
        Duration::from_millis(12),
        NativeEditTraceOutcome::Completed,
        None,
        2,
        "sent",
    );

    let continuation_traces: Vec<_> = log
        .events
        .iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EditTraceRecorded { trace, .. }
                if trace.phase == NativeEditTracePhase::ProviderContinuation =>
            {
                Some(trace)
            }
            _ => None,
        })
        .collect();
    assert_eq!(continuation_traces.len(), 2);
    for trace_id in trace_ids {
        assert!(continuation_traces.iter().any(|trace| {
            trace.trace_id == trace_id
                && trace.outcome == NativeEditTraceOutcome::Completed
                && trace
                    .attributes
                    .iter()
                    .any(|attribute| attribute.key == "tool_result_count"
                        && attribute.value == "2")
                && trace
                    .attributes
                    .iter()
                    .any(|attribute| attribute.key == "continuation"
                        && attribute.value == "sent")
        }));
    }
    assert_eq!(pending_events, log.events);
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_edit_tool_pauses_for_user_review_and_continues -- --exact
just dev cargo test -p yach-backend native_provider_agent_edit_continuation_records_each_edit_trace -- --exact
```

Expected: fail because runner-level review and continuation traces are not emitted.

- [ ] **Step 3: Carry trace IDs through the runner**

Update matches in `run_native_provider_one_agent_tool_round`:

```rust
NativeAgentEditToolPrepared::Completed { trace_id, result } => {
    edit_trace_ids.push(trace_id);
    result
}
NativeAgentEditToolPrepared::Denied { trace_id, result } => {
    return Err(NativeProviderRoundError::ToolExecutionDenied {
        tool_request_id: result.tool_request_id,
        tool_name,
        reason: result.reason.unwrap_or_else(|| String::from("denied")),
    });
}
NativeAgentEditToolPrepared::NeedsUserReview {
    trace_id,
    request_id,
    provider_call_id,
    preview,
    path,
    operation,
} => {
    let pending = PendingAgentEditToolReview {
        trace_id,
        session_id: NativeSessionId(String::from("default")),
        turn_id: turn_id.clone(),
        request_id: request_id.clone(),
        provider_call_id,
        preview_id: preview.preview_id.clone(),
        permission_decision_id: preview.permission_decision_id.clone(),
        path: path.clone(),
        operation: operation.clone(),
    };
}
```

Maintain a `Vec<NativeEditTraceId>` for edit traces that contribute provider tool results. Do not include denied edits that abort before continuation.

- [ ] **Step 4: Emit `review_wait`**

Time only the wait:

```rust
let review_started = Instant::now();
let decision = wait_for_agent_edit_review_decision(&mut review_decisions, &pending).await;
let review_duration = duration_ms(review_started);
let review_outcome = if decision.is_ok() {
    NativeEditTraceOutcome::Completed
} else {
    NativeEditTraceOutcome::Failed
};
push_edit_trace_event(
    log,
    pending_events,
    NativeSessionId(String::from("default")),
    turn_id.clone(),
    NativeEditTraceRecord {
        trace_id: pending.trace_id.clone(),
        phase: NativeEditTracePhase::ReviewWait,
        source: NativeEditTraceSource::ProviderTool,
        tool_name: Some(tool_name.clone()),
        tool_request_id: Some(NativeToolRequestId(pending.request_id.clone())),
        provider_call_id: Some(pending.provider_call_id.clone()),
        preview_id: Some(pending.preview_id.clone()),
        permission_decision_id: Some(pending.permission_decision_id.clone()),
        transaction_id: None,
        outcome: review_outcome,
        duration_ms: review_duration,
        reason_label: decision.as_ref().err().map(|_| String::from("review_wait_failed")),
        attributes: vec![NativeMetricAttribute {
            key: String::from("review_state"),
            value: String::from("needs_user_approval"),
        }],
    },
);
let decision = decision?;
```

If review channel closes or a stale decision arrives, emit `cancelled` or `failed` where possible before returning the existing provider round error. Do not leak the stale IDs beyond existing categorical labels.

- [ ] **Step 5: Emit `provider_continuation` for each contributing edit trace**

Time `build_provider_continuation_submission`, projection, provider request, and final round collection as one `ProviderContinuation` phase after all tool results are ready.

For each edit trace ID in the round, append a trace record with:

- same duration;
- `outcome=completed` on success;
- `outcome=failed` with categorical reason on validation/provider errors;
- `attribute tool_result_count=<tool_results.len()>`;
- `attribute continuation=sent` or `continuation=skipped`.

Drain the buffered sink or push to `log`/`pending_events` consistently with nearby event handling.

Add a small runner helper used by both production code and the helper-level test:

```rust
fn record_provider_continuation_trace_records(
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    trace_ids: &[NativeEditTraceId],
    duration: Duration,
    outcome: NativeEditTraceOutcome,
    reason_label: Option<String>,
    tool_result_count: usize,
    continuation: &str,
) {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    for trace_id in trace_ids {
        push_edit_trace_event(
            log,
            pending_events,
            session_id.clone(),
            turn_id.clone(),
            NativeEditTraceRecord {
                trace_id: trace_id.clone(),
                phase: NativeEditTracePhase::ProviderContinuation,
                source: NativeEditTraceSource::ProviderTool,
                tool_name: None,
                tool_request_id: None,
                provider_call_id: None,
                preview_id: None,
                permission_decision_id: None,
                transaction_id: None,
                outcome,
                duration_ms,
                reason_label: reason_label.clone(),
                attributes: vec![
                    NativeMetricAttribute {
                        key: String::from("tool_result_count"),
                        value: tool_result_count.to_string(),
                    },
                    NativeMetricAttribute {
                        key: String::from("continuation"),
                        value: continuation.to_owned(),
                    },
                ],
            },
        );
    }
}
```

- [ ] **Step 6: Run focused runner tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_edit_tool_pauses_for_user_review_and_continues -- --exact
just dev cargo test -p yach-backend native_provider_agent_edit_tool_mismatched_review_decision_finishes_failed -- --exact
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-backend/src/agent_edit_tools.rs
git commit -m "feat: trace agent edit review and continuation"
```

---

### Task 5: Update Project Handoff And Final Verification

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update project state**

In `docs/project/state.md`, add a concise paragraph after the native agent edit tool surface paragraph:

```markdown
The production edit tracing implementation now records bounded durable
`EditTraceRecorded` session events for provider-originated agent edits. Trace
records correlate validation, normalization, permission, preview, review wait,
apply/reject, result shaping, and provider continuation phases through
`NativeEditTraceId` plus existing tool request, provider call, permission,
preview, and transaction IDs. Trace records are ignored by provider transcript
projection and remain diagnostic-only; edit evidence remains the authoritative
local-effect record. This is not sufficient for broader mutation tools,
extension-owned mutation, auto-review runtime, sandboxing, or read/search
content tools.
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, set the recommended next move to provider-visible read/search content unless implementation findings point elsewhere:

```markdown
Recommended next move: design provider-visible read/search content tools for
agent edit usefulness.

Why: exact/create edit tools and production edit tracing now cover mutation
execution and diagnostics, but practical edit usefulness still depends on the
provider having target file context. Read/search content exposure is the next
bounded step before broad write/patch/delete/rename tools or extension-owned
mutation expand the surface.
```

Keep production edit tracing records in the relevant sources list.

- [ ] **Step 3: Run focused verification**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_edit_trace_records_jsonl -- --exact
just dev cargo test -p yach-backend native_session_edit_trace_is_not_provider_transcript -- --exact
just dev cargo test -p yach-backend agent_edit -- --nocapture
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
```

Expected: all pass.

- [ ] **Step 4: Run workspace gates**

Run:

```bash
just test
just lint
```

Expected: both pass. If sandboxing blocks the shared devenv target lock, rerun the same command with the required escalation rather than changing commands.

- [ ] **Step 5: Commit docs**

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update edit tracing handoff"
```

If final verification forces code fixes, make targeted fix commits before the docs handoff commit.

---

## Self-Review

- Spec coverage: covers distinct trace records, bounded/categorical fields, provider transcript exclusion, trace ID handoff, edit-access diagnostics, review wait, provider continuation, privacy, and startup boundaries.
- Scope: excludes new tools, schema changes, extension mutation, auto-review, sandboxing, and UI changes.
- Type consistency: plan uses `NativeEditTraceId`, `NativeEditTraceRecord`, `NativeEditTracePhase`, `NativeEditTraceSource`, and `NativeEditTraceOutcome` consistently with the accepted design.
- Verification: focused backend tests plus `just test` and `just lint`.
