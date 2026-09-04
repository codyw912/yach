# Native Read-Only Provider Continuation Mapping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add backend-only mapping from validated safe read-only `NativeProviderToolResult` values into adapter-ready continuation input.

**Architecture:** Add a provider-independent continuation submission builder in `tools.rs`, then add a Rig adapter projection that turns that submission into the existing `ProviderRequest` seam as ordered `Tool` messages. Keep execution, policy, session mutation, and live provider calls outside this slice.

**Tech Stack:** Rust, `yach-backend`, existing provider/tool/session types, `serde_json`, `just dev cargo`.

---

## File Structure

- Modify `crates/yach-backend/src/tools.rs`: add normalized continuation submission/result structs, mapping error enum, and builder.
- Modify `crates/yach-backend/src/rig_adapter.rs`: add Rig continuation projection helper.
- Modify `crates/yach-backend/src/lib.rs`: add unit tests and imports for the new mapping path.
- Modify `docs/project/state.md`: record that backend-only continuation mapping exists after implementation.
- Modify `docs/project/next.md`: update next recommended move to explicit native-provider one-round integration after implementation.

## Stop Gates

- Do not call `run_provider_request` from the continuation projection.
- Do not wire this into `--backend native-provider`.
- Do not add provider SDK native tool-result block support.
- Do not add new tools or expose file contents, search results, absolute paths, raw arguments, or command output.
- Do not mutate `NativeSessionLog` from continuation mapping or Rig projection.
- Do not change `yach-proto`, UI, or default backend behavior.

---

### Task 1: Provider-Independent Continuation Submission

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add the failing success test**

Add `build_provider_continuation_submission_preserves_tool_result_metadata` in `crates/yach-backend/src/lib.rs` near the existing `provider_continuation_request_*` tests:

```rust
#[test]
fn build_provider_continuation_submission_preserves_tool_result_metadata() {
    let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
        "tool-request-1",
        Some("provider-call-1"),
        "{\"relative_path\":\"Cargo.toml\",\"kind\":\"file\",\"byte_size\":10,\"provider_visibility\":\"never\"}",
    )]);

    let submission = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    );

    assert!(submission.as_ref().is_ok_and(|submission| {
        submission.turn_id == NativeTurnId(String::from("turn-1"))
            && submission.model.provider == "fixture-provider"
            && submission.prior_messages.len() == 1
            && submission.extensions.len() == 1
            && submission.tool_results.len() == 1
    }));
    let result = submission
        .ok()
        .and_then(|submission| submission.tool_results.into_iter().next());
    assert_eq!(
        result.as_ref().map(|result| result.tool_request_id.as_str()),
        Some("tool-request-1")
    );
    assert_eq!(
        result.as_ref().map(|result| result.provider_call_id.as_str()),
        Some("provider-call-1")
    );
    assert_eq!(
        result.as_ref().map(|result| result.status),
        Some(NativeToolOutcome::Completed)
    );
    assert!(
        result
            .as_ref()
            .is_some_and(|result| result.content.contains("\"provider_visibility\":\"never\""))
    );
}
```

Also add `build_provider_continuation_submission` to the test imports from `super`.

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend build_provider_continuation_submission_preserves_tool_result_metadata
```

Expected: FAIL because `build_provider_continuation_submission` does not exist.

- [ ] **Step 3: Add submission structs, error enum, and builder**

Add these items in `crates/yach-backend/src/tools.rs` near `ProviderContinuationRequest` and the validation types:

```rust
/// Provider-independent adapter submission for a validated continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationSubmission {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<ProviderContinuationToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-bound tool result normalized for adapter continuation mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationToolResult {
    pub tool_request_id: String,
    pub provider_call_id: String,
    pub status: NativeToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Fail-closed errors while preparing adapter continuation input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderContinuationMappingError {
    Validation(ProviderContinuationValidationError),
    EmptyToolResults,
    UnsupportedToolResultStatus {
        tool_request_id: String,
        status: NativeToolOutcome,
    },
}
```

Add this function near `validate_provider_continuation_request`:

```rust
pub fn build_provider_continuation_submission(
    request: &ProviderContinuationRequest,
    policy: ProviderContinuationValidationPolicy,
) -> Result<ProviderContinuationSubmission, ProviderContinuationMappingError> {
    validate_provider_continuation_request(request, policy)
        .map_err(ProviderContinuationMappingError::Validation)?;
    if request.tool_results.is_empty() {
        return Err(ProviderContinuationMappingError::EmptyToolResults);
    }

    let mut tool_results = Vec::with_capacity(request.tool_results.len());
    for result in &request.tool_results {
        if result.status != NativeToolOutcome::Completed {
            return Err(
                ProviderContinuationMappingError::UnsupportedToolResultStatus {
                    tool_request_id: result.tool_request_id.clone(),
                    status: result.status,
                },
            );
        }
        let Some(provider_call_id) = result.provider_call_id.clone() else {
            return Err(ProviderContinuationMappingError::Validation(
                ProviderContinuationValidationError::MissingProviderCallId {
                    tool_request_id: result.tool_request_id.clone(),
                },
            ));
        };
        tool_results.push(ProviderContinuationToolResult {
            tool_request_id: result.tool_request_id.clone(),
            provider_call_id,
            status: result.status,
            content: result.content.clone(),
            byte_count: result.byte_count,
            redacted: result.redacted,
            truncated: result.truncated,
            reason: result.reason.clone(),
        });
    }

    Ok(ProviderContinuationSubmission {
        turn_id: request.turn_id.clone(),
        model: request.model.clone(),
        prior_messages: request.prior_messages.clone(),
        tool_results,
        extensions: request.extensions.clone(),
    })
}
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
just dev cargo test -p yach-backend build_provider_continuation_submission_preserves_tool_result_metadata
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: add provider continuation submission mapping"
```

---

### Task 2: Continuation Submission Rejection Cases

**Files:**
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add rejection tests**

Add these tests near the success test from Task 1:

```rust
#[test]
fn build_provider_continuation_submission_rejects_empty_results() {
    let request = fixture_provider_continuation_request(Vec::new());

    let result = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    );

    assert_eq!(
        result,
        Err(ProviderContinuationMappingError::EmptyToolResults)
    );
}

#[test]
fn build_provider_continuation_submission_rejects_non_completed_results() {
    let mut failed_result = fixture_provider_tool_result(
        "tool-request-1",
        Some("provider-call-1"),
        "tool failed",
    );
    failed_result.status = NativeToolOutcome::Failed;
    failed_result.reason = Some(String::from("resource_path_missing"));
    let request = fixture_provider_continuation_request(vec![failed_result]);

    let result = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    );

    assert_eq!(
        result,
        Err(ProviderContinuationMappingError::UnsupportedToolResultStatus {
            tool_request_id: String::from("tool-request-1"),
            status: NativeToolOutcome::Failed,
        })
    );
}

#[test]
fn build_provider_continuation_submission_wraps_validation_errors() {
    let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
        "tool-request-1",
        None,
        "redacted result",
    )]);

    let result = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    );

    assert_eq!(
        result,
        Err(ProviderContinuationMappingError::Validation(
            ProviderContinuationValidationError::MissingProviderCallId {
                tool_request_id: String::from("tool-request-1"),
            },
        ))
    );
}
```

Also add `ProviderContinuationMappingError` to the test imports from `super`.

- [ ] **Step 2: Run tests**

Run:

```bash
just dev cargo test -p yach-backend build_provider_continuation_submission
```

Expected: PASS for all four `build_provider_continuation_submission_*` tests.

- [ ] **Step 3: Run existing validation tests**

Run:

```bash
just dev cargo test -p yach-backend provider_continuation_request
```

Expected: PASS for the existing continuation validation tests.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "test: cover continuation submission rejections"
```

---

### Task 3: Rig ProviderRequest Projection

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing projection tests**

Add these tests in `crates/yach-backend/src/lib.rs` near the continuation submission tests:

```rust
#[test]
fn rig_continuation_projection_appends_ordered_tool_messages() {
    let request = fixture_provider_continuation_request(vec![
        fixture_provider_tool_result("tool-request-1", Some("provider-call-1"), "{\"one\":true}"),
        fixture_provider_tool_result("tool-request-2", Some("provider-call-2"), "{\"two\":true}"),
    ]);
    let submission = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    )
    .ok();
    assert!(submission.is_some());

    let Some(submission) = submission else {
        return;
    };
    let projected = rig_adapter::project_provider_continuation_request(submission);

    assert_eq!(projected.turn_id, NativeTurnId(String::from("turn-1")));
    assert_eq!(projected.model.provider, "fixture-provider");
    assert_eq!(projected.extensions.len(), 1);
    assert_eq!(projected.messages.len(), 3);
    assert_eq!(projected.messages[0].role, NativeRole::User);
    assert_eq!(projected.messages[1].role, NativeRole::Tool);
    assert_eq!(projected.messages[2].role, NativeRole::Tool);
    assert!(projected.messages[1].content.contains("\"provider_call_id\":\"provider-call-1\""));
    assert!(projected.messages[2].content.contains("\"provider_call_id\":\"provider-call-2\""));
}

#[test]
fn rig_continuation_projection_excludes_raw_arguments() {
    let request = fixture_provider_continuation_request(vec![fixture_provider_tool_result(
        "tool-request-1",
        Some("provider-call-1"),
        "{\"relative_path\":\"Cargo.toml\",\"provider_visibility\":\"never\"}",
    )]);
    let submission = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(256),
    )
    .ok();
    assert!(submission.is_some());

    let Some(submission) = submission else {
        return;
    };
    let projected = rig_adapter::project_provider_continuation_request(submission);
    let tool_message = projected
        .messages
        .iter()
        .find(|message| message.role == NativeRole::Tool);

    assert!(
        tool_message
            .is_some_and(|message| message.content.contains("\"content\":\"{\\\"relative_path\\\":\\\"Cargo.toml\\\",\\\"provider_visibility\\\":\\\"never\\\"}\""))
    );
    assert!(
        tool_message.is_some_and(|message| !message.content.contains("arguments_json"))
    );
    assert!(tool_message.is_some_and(|message| !message.content.contains("\"path\"")));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend rig_continuation_projection
```

Expected: FAIL because `rig_adapter::project_provider_continuation_request` does not exist.

- [ ] **Step 3: Add Rig projection helper**

In `crates/yach-backend/src/rig_adapter.rs`, add imports for `NativeProviderToolResult` replacement types by extending the `crate::{...}` list:

```rust
NativeRole, NativeTurnId, ProviderContinuationSubmission, ProviderContinuationToolResult,
ProviderError, ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderRequest,
ProviderStreamEvent, ProviderToolCall,
```

Then add this helper near `run_provider_request` or before `prompt_from_request`:

```rust
#[must_use]
pub fn project_provider_continuation_request(
    submission: ProviderContinuationSubmission,
) -> ProviderRequest {
    let mut messages = submission.prior_messages;
    messages.extend(
        submission
            .tool_results
            .into_iter()
            .map(provider_tool_result_message),
    );
    ProviderRequest {
        turn_id: submission.turn_id,
        model: submission.model,
        messages,
        extensions: submission.extensions,
    }
}

fn provider_tool_result_message(result: ProviderContinuationToolResult) -> ProviderMessage {
    ProviderMessage {
        role: NativeRole::Tool,
        content: serde_json::json!({
            "provider_call_id": result.provider_call_id,
            "status": native_tool_outcome_label(result.status),
            "content": result.content,
            "byte_count": result.byte_count,
            "redacted": result.redacted,
            "truncated": result.truncated,
            "reason": result.reason,
        })
        .to_string(),
    }
}

const fn native_tool_outcome_label(status: crate::NativeToolOutcome) -> &'static str {
    match status {
        crate::NativeToolOutcome::Completed => "completed",
        crate::NativeToolOutcome::Failed => "failed",
        crate::NativeToolOutcome::Denied => "denied",
        crate::NativeToolOutcome::Cancelled => "cancelled",
        crate::NativeToolOutcome::ValidationFailed => "validation_failed",
    }
}
```

- [ ] **Step 4: Run projection tests**

Run:

```bash
just dev cargo test -p yach-backend rig_continuation_projection
```

Expected: PASS.

- [ ] **Step 5: Run Rig adapter prompt tests**

Run:

```bash
just dev cargo test -p yach-backend rig_provider_prompt
```

Expected: PASS to confirm existing prompt behavior is preserved.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: project continuation results for rig requests"
```

---

### Task 4: Full Verification And Planning Docs

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
just dev cargo fmt --all
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

Expected: all pass.

- [ ] **Step 2: Update state doc**

In `docs/project/state.md`, update the read-only/native posture bullet to say:

```markdown
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, and backend-only continuation mapping into adapter-ready provider request input.
```

Update Plan Sufficiency to say:

```markdown
The plan is sufficient to plan explicit native-provider one-round integration for safe read-only tool results. It is not sufficient for file writes, process execution, network tools, extension runtime, provider-native tool-result block support, or default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.
```

Add these records under Currently Relevant Records:

```markdown
- `docs/project/specs/2026-05-11-native-readonly-provider-continuation-design.md`
- `docs/project/plans/2026-05-11-native-readonly-provider-continuation.md`
```

- [ ] **Step 3: Update next doc**

In `docs/project/next.md`, update Recommended Next Move:

```markdown
Recommended next move: continue Native MVP implementation with explicit native-provider one-round integration for safe read-only tool results.
```

Use this Why text:

```markdown
Why: yach can now execute safe read-only metadata tools locally and map their results into adapter-ready continuation input, but native-provider dogfooding still needs a guarded runtime loop that collects model tool calls, executes the local read-only tool loop, submits one continuation round, and records the final turn outcome.
```

Add the spec and plan paths to Relevant sources.

- [ ] **Step 4: Commit docs**

Run:

```bash
git add docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "docs: update provider continuation mapping status"
```

---

## Final Branch Verification

After all tasks:

```bash
just dev cargo fmt --all -- --check
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check origin/main...HEAD
```

Expected: all pass.

Dispatch a final whole-branch review before creating the PR.
