# Native Read-Only Tool Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a backend-only autonomous tool-loop helper that validates, authorizes, executes, records, and result-shapes safe read-only `project_path_info` provider tool calls without contacting a provider.

**Architecture:** Keep provider-loop semantics registry/executor-oriented so future extension-registered tools can enter the same path. Reuse `NativeToolContinuationWorkflow` for loop limits, request conversion, validation, execution, result shaping, and session records; add a project read-only helper that supplies `ProjectReadOnlyToolExecutor` and a caller-provided registry/policy. Do not wire the helper into `--backend native-provider` in this slice.

**Tech Stack:** Rust 2024, existing `yach-backend` native tool/resource/session/provider types, Serde JSON, `just` recipes.

---

## Scope

In scope:

- backend-only helper for project read-only provider tool calls;
- `project_path_info` execution through yach-owned validation, permission, execution, result shaping, and session evidence;
- failure evidence for execution errors such as resource path failures;
- tests for success, denial, unknown tool, resource path failure, result size limit, tool-call count limit, and provider call id preservation;
- planning docs update after implementation.

Out of scope:

- live `--backend native-provider` integration;
- provider continuation network calls;
- text read/search provider exposure;
- file contents sent to a provider;
- file edit/write/delete/rename tools;
- process, shell, or network tools;
- approval UI or `yach-proto` changes;
- extension runtime implementation.

## Files

- Modify `crates/yach-backend/src/tools.rs`: execution failure evidence, project read-only helper, execution-error labels.
- Modify `crates/yach-backend/src/lib.rs`: backend tests for the read-only tool loop.
- Modify `docs/project/state.md`: record completed safe read-only tool-loop backend helper.
- Modify `docs/project/next.md`: point next work to provider continuation mapping if implementation completes.

## Task 1: Project Read-Only Tool Loop Success Path

**Files:**

- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing success-path test**

Add `build_project_readonly_provider_tool_results` to the test imports in `crates/yach-backend/src/lib.rs`.

Add this test near the existing provider tool-result tests:

```rust
#[test]
fn project_readonly_provider_tool_results_execute_metadata_and_record_success() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-success");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("project_path_info"),
        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
    }];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let results = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy::fixture_default(),
    );

    assert!(results.as_ref().is_ok_and(|results| results.len() == 1));
    let result = results.ok().and_then(|mut results| results.pop());
    assert_eq!(
        result.as_ref().and_then(|result| result.provider_call_id.as_deref()),
        Some("provider-call-1")
    );
    assert_eq!(
        result.as_ref().map(|result| result.status),
        Some(NativeToolOutcome::Completed)
    );
    assert!(result
        .as_ref()
        .is_some_and(|result| result.content.contains("\"relative_path\":\"Cargo.toml\"")));
    assert!(result
        .as_ref()
        .is_some_and(|result| result.content.contains("\"provider_visibility\":\"never\"")));
    assert!(result
        .as_ref()
        .is_some_and(|result| !result.content.contains("[package]")));
    assert_eq!(log.events.len(), 2);
    assert!(matches!(
        log.events.first(),
        Some(NativeSessionEvent::ToolRequestRecorded {
            tool_name,
            permission: NativeToolPermissionState::Allowed,
            ..
        }) if tool_name == "project_path_info"
    ));
    assert!(matches!(
        log.events.last(),
        Some(NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::Completed,
            result_summary: Some(summary),
            ..
        }) if summary.summary.contains("\"relative_path\":\"Cargo.toml\"")
            && !summary.summary.contains("[package]")
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_execute_metadata_and_record_success
```

Expected: FAIL because `build_project_readonly_provider_tool_results` does not exist.

- [ ] **Step 3: Implement project read-only helper**

Add this function to `crates/yach-backend/src/tools.rs` near `build_fixture_provider_tool_results`:

```rust
pub fn build_project_readonly_provider_tool_results(
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    project_root: NativeResourceRoot,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    let executor = ProjectReadOnlyToolExecutor::new(project_root);
    NativeToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor: &executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}
```

This helper intentionally accepts a registry and permission policy rather than creating hidden globals. That keeps the loop compatible with later extension-registered tools.

- [ ] **Step 4: Run focused test**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_execute_metadata_and_record_success
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: add project read-only tool loop helper"
```

## Task 2: Denial And Validation Failures

**Files:**

- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing permission and unknown-tool tests**

Add these tests near the Task 1 success test:

```rust
#[test]
fn project_readonly_provider_tool_results_deny_without_execution() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-denied");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("project_path_info"),
        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
    }];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::deny_all(),
        NativeToolContinuationPolicy::fixture_default(),
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::Validation(
            NativeToolError::PermissionDenied
        ))
    );
    assert_eq!(log.events.len(), 2);
    assert!(matches!(
        log.events.last(),
        Some(NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::Denied,
            result_summary: None,
            ..
        })
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn project_readonly_provider_tool_results_reject_unknown_tool_without_execution() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-unknown");
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("read"),
        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
    }];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy::fixture_default(),
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::Validation(
            NativeToolError::UnknownTool
        ))
    );
    assert_eq!(log.events.len(), 2);
    assert!(matches!(
        log.events.last(),
        Some(NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::ValidationFailed,
            result_summary: None,
            ..
        })
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run tests**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_deny_without_execution
just dev cargo test -p yach-backend project_readonly_provider_tool_results_reject_unknown_tool_without_execution
```

Expected: PASS if Task 1 helper correctly reuses existing validation/session evidence behavior.

- [ ] **Step 3: Commit**

Run:

```bash
git add crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "test: cover read-only tool loop validation failures"
```

## Task 3: Execution Failure Evidence

**Files:**

- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing resource path error test**

Add this test near the other project read-only tool-loop tests:

```rust
#[test]
fn project_readonly_provider_tool_results_record_resource_path_failure() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-missing");
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("project_path_info"),
        arguments_json: serde_json::json!({"path":"missing.txt"}),
    }];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy::fixture_default(),
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::Execution(
            NativeToolExecutionError::ResourcePath {
                error: NativeResourcePathError::Missing,
            }
        ))
    );
    assert_eq!(log.events.len(), 2);
    assert!(matches!(
        log.events.last(),
        Some(NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            ..
        }) if reason == "resource_path_missing"
            && !reason.contains(std::path::MAIN_SEPARATOR)
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_record_resource_path_failure
```

Expected: FAIL because execution errors currently return before recording `ToolExecutionFinished`.

- [ ] **Step 3: Record execution failures in `NativeToolContinuationWorkflow`**

In `NativeToolContinuationWorkflow::build_provider_tool_results`, replace the direct `?` on executor execution with explicit error handling:

```rust
            let execution = match self.executor.execute(self.registry, &request, &validation) {
                Ok(execution) => execution,
                Err(error) => {
                    log.push(NativeSessionEvent::ToolExecutionFinished {
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        tool_request_id: NativeToolRequestId(request.request_id.clone()),
                        outcome: NativeToolOutcome::Failed,
                        reason: Some(native_tool_execution_error_label(&error)),
                        result_summary: None,
                    });
                    return Err(NativeToolContinuationError::Execution(error));
                }
            };
```

Add this helper near `native_tool_error_label`:

```rust
fn native_tool_execution_error_label(error: &NativeToolExecutionError) -> String {
    match error {
        NativeToolExecutionError::UnknownTool => String::from("unknown_tool"),
        NativeToolExecutionError::PermissionDenied => String::from("permission_denied"),
        NativeToolExecutionError::UnsupportedTool => String::from("unsupported_tool"),
        NativeToolExecutionError::ResourcePath { error } => {
            format!("resource_path_{}", native_resource_path_error_label(*error))
        }
    }
}

const fn native_resource_path_error_label(error: NativeResourcePathError) -> &'static str {
    match error {
        NativeResourcePathError::RootUnavailable => "root_unavailable",
        NativeResourcePathError::Missing => "missing",
        NativeResourcePathError::EscapesRoot => "escapes_root",
        NativeResourcePathError::ExpectedFile => "expected_file",
        NativeResourcePathError::ExpectedDirectory => "expected_directory",
    }
}
```

- [ ] **Step 4: Run focused test**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_record_resource_path_failure
```

Expected: PASS.

- [ ] **Step 5: Run existing fixture continuation tests**

Run:

```bash
just dev cargo test -p yach-backend fixture_provider_tool_results
```

Expected: PASS; fixture behavior is preserved except execution failures now get explicit session evidence when they occur.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: record read-only tool execution failures"
```

## Task 4: Loop Limit And Result Size Failures

**Files:**

- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing/confirming limit tests**

Add these tests near the other project read-only tool-loop tests:

```rust
#[test]
fn project_readonly_provider_tool_results_enforce_tool_call_limit_before_execution() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-count-limit");
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![
        ProviderToolCall {
            call_id: String::from("provider-call-1"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"one.txt"}),
        },
        ProviderToolCall {
            call_id: String::from("provider-call-2"),
            name: String::from("project_path_info"),
            arguments_json: serde_json::json!({"path":"two.txt"}),
        },
    ];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy {
            max_tool_calls: 1,
            max_result_bytes: 256,
        },
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::TooManyToolCalls { max: 1, actual: 2 })
    );
    assert!(log.events.is_empty());
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn project_readonly_provider_tool_results_enforce_result_size_limit() {
    let root_path = temp_resource_dir("native-readonly-tool-loop-result-limit");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let calls = vec![ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("project_path_info"),
        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
    }];
    let mut log = NativeSessionLog::default();

    let Some(root) = root else {
        return;
    };
    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &fixture_continuation_context(),
        calls,
        root,
        &NativeToolRegistry::with_project_read_only_tools(),
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        NativeToolContinuationPolicy {
            max_tool_calls: 1,
            max_result_bytes: 1,
        },
    );

    assert!(matches!(
        result,
        Err(NativeToolContinuationError::ResultTooLarge {
            max_bytes: 1,
            actual_bytes
        }) if actual_bytes > 1
    ));
    assert_eq!(log.events.len(), 2);
    assert!(matches!(
        log.events.last(),
        Some(NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::Failed,
            reason: Some(reason),
            result_summary: None,
            ..
        }) if reason == "result_too_large"
    ));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run tests**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_enforce
```

Expected: PASS, because the generic workflow already handles these limits.

- [ ] **Step 3: Commit**

Run:

```bash
git add crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "test: cover read-only tool loop limits"
```

## Task 5: Full Backend Verification And Planning Docs

**Files:**

- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run formatting**

Run:

```bash
just dev cargo fmt --all
```

Expected: PASS.

- [ ] **Step 2: Run backend lint**

Run:

```bash
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 3: Run backend tests**

Run:

```bash
just dev cargo test -p yach-backend
```

Expected: PASS.

- [ ] **Step 4: Run whitespace check**

Run:

```bash
git diff --check
```

Expected: no output.

- [ ] **Step 5: Update active project docs**

In `docs/project/state.md`, update `Current Posture` to mention that backend-only autonomous safe read-only tool-loop semantics now exist for metadata-only project path info, with session evidence and provider-bound result shaping but no live provider continuation.

Update `Plan Sufficiency` to say the current planning surface is sufficient to plan provider continuation mapping for safe read-only tool results. It is still not sufficient for file writes, process execution, network tools, extension runtime, or default-backend changes.

Add these new artifacts to `Currently Relevant Records`:

- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-tool-loop.md`

In `docs/project/next.md`, update the recommended next move to provider continuation mapping for safe read-only tool results. Use this wording unless implementation reveals a different blocker:

```markdown
Recommended next move: continue Native MVP implementation with provider continuation mapping for safe read-only tool results.

Why: yach can now execute and shape safe read-only tool results locally, but native-provider dogfooding still needs adapter-level continuation mapping before model-requested tools can complete a live turn.
```

Keep the `Not Ready Without a New Spec` section unchanged unless implementation changes those boundaries.

- [ ] **Step 6: Commit docs**

Run:

```bash
git add docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "docs: update native read-only tool loop status"
```

## Final Verification

Run:

```bash
just dev cargo fmt --all
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

Expected:

- formatting completes successfully;
- clippy reports no warnings;
- backend tests pass;
- whitespace check reports no issues.

## Stop Gates

Stop and ask before:

- wiring this helper into `--backend native-provider`;
- making a provider continuation network call;
- sending file contents, text reads, search results, absolute paths, or command output to a provider;
- adding read, grep, find, ls, edit, write, bash, process, shell, or network tools;
- adding extension runtime behavior;
- adding approval UI or `yach-proto` tool events;
- changing the default backend.
