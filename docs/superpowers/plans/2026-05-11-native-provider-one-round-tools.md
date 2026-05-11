# Native Provider One-Round Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire explicit native-provider dogfood to handle exactly one safe read-only `project_path_info` tool continuation round.

**Architecture:** Add a runner-local async requester seam and one-round orchestration helper in `native_runner.rs`, test it with a fake requester, then route the production native-provider path through the helper while keeping Rig tool advertising and provider-native tool-result blocks out of scope.

**Tech Stack:** Rust, `yach-backend`, Tokio, futures `BoxFuture`, existing native session/tool/provider types, `just dev cargo`.

---

## File Structure

- Modify `crates/yach-backend/src/native_runner.rs`: add requester seam, one-round orchestration helper, runtime wiring, failure labeling, and unit tests.
- Modify `crates/yach-backend/src/lib.rs`: add or adjust integration-level backend tests only if runner-private tests cannot cover behavior.
- Modify `docs/project/state.md`: record one-round runtime handling after implementation.
- Modify `docs/project/next.md`: update next work to provider tool advertising for `project_path_info`.

## Stop Gates

- Do not add provider tool advertising or schema registration.
- Do not add provider-native SDK tool-result block support.
- Do not add new tools beyond `project_path_info`.
- Do not send file contents, search results, absolute paths, raw tool arguments, or command output to providers.
- Do not change default backend behavior.
- Do not change `yach-proto`, UI approval surfaces, or advertised backend capabilities.
- Do not allow more than one continuation request.
- Do not continue after second-round tool calls.

---

### Task 1: Provider Requester Seam And No-Tool Round Helper

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add the failing no-tool helper test**

In the `#[cfg(test)] mod tests` in `crates/yach-backend/src/native_runner.rs`, extend the `use super::{...}` list to include the new helper/types once added:

```rust
NativeProviderRoundResult, ProviderRequester, run_native_provider_one_readonly_tool_round,
```

Add this fake requester and test near the existing native-provider message tests:

```rust
#[derive(Debug, Default)]
struct FakeProviderRequester {
    requests: Vec<ProviderRequest>,
    responses: std::collections::VecDeque<Result<Vec<ProviderStreamEvent>, ProviderError>>,
}

impl FakeProviderRequester {
    fn with_responses(
        responses: impl IntoIterator<Item = Result<Vec<ProviderStreamEvent>, ProviderError>>,
    ) -> Self {
        Self {
            requests: Vec::new(),
            responses: responses.into_iter().collect(),
        }
    }
}

impl ProviderRequester for FakeProviderRequester {
    fn request<'a>(
        &'a mut self,
        request: ProviderRequest,
    ) -> futures::future::BoxFuture<'a, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        self.requests.push(request);
        let response = self.responses.pop_front().unwrap_or_else(|| {
            Err(ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                message: String::from("missing fake provider response"),
                redacted_debug: None,
            })
        });
        Box::pin(async move { response })
    }
}

#[tokio::test]
async fn native_provider_one_round_without_tools_preserves_one_shot_response() {
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "inspect cargo",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let mut requester = FakeProviderRequester::with_responses([Ok(vec![
        ProviderStreamEvent::Started {
            turn_id: turn.clone(),
            model: model.clone(),
        },
        ProviderStreamEvent::TextDelta {
            turn_id: turn.clone(),
            delta: String::from("plain answer"),
        },
        ProviderStreamEvent::Completed {
            turn_id: turn.clone(),
            finish_reason: Some(crate::ProviderFinishReason::Stop),
            usage: None,
            provider_response_id: Some(String::from("response-1")),
        },
    ])]);

    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        None,
    )
    .await;

    assert_eq!(
        result,
        Ok(NativeProviderRoundResult {
            text: String::from("plain answer"),
            provider_response_id: Some(String::from("response-1")),
        })
    );
    assert_eq!(requester.requests.len(), 1);
    assert!(pending_events.is_empty());
}
```

This test intentionally passes `None` for project root because no tool call is present.

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_without_tools_preserves_one_shot_response
```

Expected: FAIL because `ProviderRequester`, `NativeProviderRoundResult`, and `run_native_provider_one_readonly_tool_round` do not exist.

- [ ] **Step 3: Add requester seam, result, error, and no-tool helper path**

In `crates/yach-backend/src/native_runner.rs`, add imports:

```rust
use futures::future::BoxFuture;
```

Extend the `crate::{...}` import list with:

```rust
NativeResourceRoot, ProviderToolCall,
```

Add these runner-local items near `handle_native_provider_prompt`:

```rust
pub trait ProviderRequester {
    fn request<'a>(
        &'a mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<Vec<ProviderStreamEvent>, ProviderError>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeProviderRoundResult {
    pub text: String,
    pub provider_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeProviderRoundError {
    Provider(ProviderError),
    Cancelled(String),
    StreamEndedWithoutCompletion,
    ProjectRootUnavailable,
    ToolContinuation(String),
    SecondRoundToolCall,
}

struct NativeProviderFirstRound {
    text: String,
    provider_response_id: Option<String>,
    tool_calls: Vec<ProviderToolCall>,
}

fn collect_native_provider_first_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<NativeProviderFirstRound, NativeProviderRoundError> {
    let mut text = String::new();
    let mut completed = false;
    let mut provider_response_id = None;
    let mut tool_calls = Vec::new();
    for event in events {
        match event {
            ProviderStreamEvent::TextDelta { delta, .. } => text.push_str(&delta),
            ProviderStreamEvent::ToolCallCompleted { tool_call, .. } => tool_calls.push(tool_call),
            ProviderStreamEvent::Completed {
                provider_response_id: response_id,
                ..
            } => {
                completed = true;
                provider_response_id = response_id;
            }
            ProviderStreamEvent::Failed { error, .. } => {
                return Err(NativeProviderRoundError::Provider(error));
            }
            ProviderStreamEvent::Cancelled { reason, .. } => {
                return Err(NativeProviderRoundError::Cancelled(
                    reason.unwrap_or_else(|| String::from("native provider cancelled")),
                ));
            }
            ProviderStreamEvent::Started { .. }
            | ProviderStreamEvent::ToolCallStarted { .. }
            | ProviderStreamEvent::ToolCallDelta { .. } => {}
        }
    }
    if !completed {
        return Err(NativeProviderRoundError::StreamEndedWithoutCompletion);
    }
    Ok(NativeProviderFirstRound {
        text,
        provider_response_id,
        tool_calls,
    })
}

fn collect_native_provider_final_round(
    events: Vec<ProviderStreamEvent>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let first_round = collect_native_provider_first_round(events)?;
    if !first_round.tool_calls.is_empty() {
        return Err(NativeProviderRoundError::SecondRoundToolCall);
    }
    Ok(NativeProviderRoundResult {
        text: first_round.text,
        provider_response_id: first_round.provider_response_id,
    })
}

async fn run_native_provider_one_readonly_tool_round(
    requester: &mut impl ProviderRequester,
    model: ProviderModel,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    turn_id: &NativeTurnId,
    project_root: Option<NativeResourceRoot>,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError> {
    let initial_request = ProviderRequest {
        turn_id: turn_id.clone(),
        model,
        messages: native_provider_messages_from_log(log, turn_id),
        extensions: Vec::new(),
    };
    let first_events = requester
        .request(initial_request.clone())
        .await
        .map_err(NativeProviderRoundError::Provider)?;
    let first_round = collect_native_provider_first_round(first_events)?;
    if first_round.tool_calls.is_empty() {
        return Ok(NativeProviderRoundResult {
            text: first_round.text,
            provider_response_id: first_round.provider_response_id,
        });
    }
    let _ = project_root.ok_or(NativeProviderRoundError::ProjectRootUnavailable)?;
    let _ = pending_events;
    Err(NativeProviderRoundError::ToolContinuation(String::from(
        "native_provider_tool_continuation_unimplemented",
    )))
}
```

Task 1 intentionally stops before implementing the tool-call branch.

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_without_tools_preserves_one_shot_response
```

Expected: PASS.

- [ ] **Step 5: Run formatting**

Run:

```bash
just dev cargo fmt --check
```

Expected: PASS. If it fails, run `just dev cargo fmt`, then re-run the check.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: add native provider one-round requester seam"
```

---

### Task 2: Successful Tool Continuation Round

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add the failing continuation success test**

Add this test near the Task 1 test:

```rust
#[tokio::test]
async fn native_provider_one_round_executes_project_path_info_and_continues() {
    let root_path = temp_native_provider_root("native-provider-one-round-success");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "inspect cargo",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let mut requester = FakeProviderRequester::with_responses([
        Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn.clone(),
                delta: String::from("I will inspect that."),
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("project_path_info"),
                    arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: Some(String::from("response-1")),
            },
        ]),
        Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: turn.clone(),
                model: model.clone(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn.clone(),
                delta: String::from("Cargo.toml is a file."),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: Some(String::from("response-2")),
            },
        ]),
    ]);

    let Some(root) = root else {
        return;
    };
    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        Some(root),
    )
    .await;

    assert_eq!(
        result,
        Ok(NativeProviderRoundResult {
            text: String::from("Cargo.toml is a file."),
            provider_response_id: Some(String::from("response-2")),
        })
    );
    assert_eq!(requester.requests.len(), 2);
    assert_eq!(requester.requests[1].messages.len(), 2);
    assert_eq!(requester.requests[1].messages[1].role, NativeRole::Tool);
    assert!(requester.requests[1].messages[1]
        .content
        .contains("\"provider_call_id\":\"provider-call-1\""));
    assert!(requester.requests[1].messages[1]
        .content
        .contains("\"relative_path\":\"Cargo.toml\""));
    assert!(!requester.requests[1].messages[1]
        .content
        .contains("[package]"));
    assert!(pending_events.iter().any(|event| matches!(
        event,
        NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "project_path_info"
    )));
    assert!(pending_events.iter().any(|event| matches!(
        event,
        NativeSessionEvent::ToolExecutionFinished {
            outcome: crate::NativeToolOutcome::Completed,
            ..
        }
    )));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

Add helper in the test module:

```rust
fn temp_native_provider_root(label: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "yach-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    assert!(std::fs::create_dir_all(&path).is_ok());
    path
}
```

Update test imports from `crate::{...}` for:

```rust
NativeResourceRoot, NativeToolOutcome, ProviderModel, ProviderStreamEvent, ProviderToolCall,
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_executes_project_path_info_and_continues
```

Expected: FAIL with `ToolContinuation("native_provider_tool_continuation_unimplemented")` or equivalent.

- [ ] **Step 3: Implement tool-call branch**

Extend imports in `native_runner.rs`:

```rust
build_project_readonly_provider_tool_results, build_provider_continuation_submission,
NativeToolContinuationContext, NativeToolContinuationPolicy, NativeToolPermissionPolicy,
NativeToolRegistry, ProviderContinuationRequest, ProviderContinuationValidationPolicy,
```

Use `crate::rig_adapter::project_provider_continuation_request` if needed rather than importing it through the `crate::{...}` block.

In `run_native_provider_one_readonly_tool_round`, replace the unimplemented branch with:

```rust
let root = project_root.ok_or(NativeProviderRoundError::ProjectRootUnavailable)?;
let tool_event_start = log.events.len();
let tool_results = match build_project_readonly_provider_tool_results(
    log,
    &NativeToolContinuationContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: turn_id.clone(),
    },
    first_round.tool_calls,
    root,
    &NativeToolRegistry::with_project_read_only_tools(),
    &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
    NativeToolContinuationPolicy::fixture_default(),
)
{
    Ok(results) => results,
    Err(error) => {
        pending_events.extend(log.events[tool_event_start..].iter().cloned());
        return Err(NativeProviderRoundError::ToolContinuation(
            native_tool_round_error_label(&error),
        ));
    }
};
pending_events.extend(log.events[tool_event_start..].iter().cloned());

let continuation_request = ProviderContinuationRequest {
    turn_id: turn_id.clone(),
    model: initial_request.model.clone(),
    prior_messages: initial_request.messages,
    tool_results,
    extensions: initial_request.extensions,
};
let submission = build_provider_continuation_submission(
    &continuation_request,
    ProviderContinuationValidationPolicy::strict_tool_results(
        NativeToolContinuationPolicy::fixture_default().max_result_bytes,
    ),
)
.map_err(|error| NativeProviderRoundError::ToolContinuation(native_provider_mapping_error_label(&error)))?;
let continuation_request = crate::rig_adapter::project_provider_continuation_request(submission);
let continuation_events = requester
    .request(continuation_request)
    .await
    .map_err(NativeProviderRoundError::Provider)?;
collect_native_provider_final_round(continuation_events)
```

Add private label helpers:

```rust
fn native_tool_round_error_label(error: &crate::NativeToolContinuationError) -> String {
    match error {
        crate::NativeToolContinuationError::TooManyToolCalls { .. } => {
            String::from("tool_round_too_many_calls")
        }
        crate::NativeToolContinuationError::Validation(_) => {
            String::from("tool_round_validation_failed")
        }
        crate::NativeToolContinuationError::Execution(_) => {
            String::from("tool_round_execution_failed")
        }
        crate::NativeToolContinuationError::ResultTooLarge { .. } => {
            String::from("tool_round_result_too_large")
        }
    }
}

fn native_provider_mapping_error_label(error: &crate::ProviderContinuationMappingError) -> String {
    match error {
        crate::ProviderContinuationMappingError::Validation(_) => {
            String::from("tool_continuation_validation_failed")
        }
        crate::ProviderContinuationMappingError::EmptyToolResults => {
            String::from("tool_continuation_empty_results")
        }
        crate::ProviderContinuationMappingError::UnsupportedToolResultStatus { .. } => {
            String::from("tool_continuation_unsupported_status")
        }
    }
}
```

- [ ] **Step 4: Run success test**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_executes_project_path_info_and_continues
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: continue native provider after read-only tool"
```

---

### Task 3: Fail-Closed Tool Continuation Cases

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add failure tests**

Add these tests near the Task 2 success test:

```rust
#[tokio::test]
async fn native_provider_one_round_rejects_second_round_tool_calls() {
    let root_path = temp_native_provider_root("native-provider-second-tool");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "inspect cargo",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let tool_call = ProviderToolCall {
        call_id: String::from("provider-call-1"),
        name: String::from("project_path_info"),
        arguments_json: serde_json::json!({"path":"Cargo.toml"}),
    };
    let mut requester = FakeProviderRequester::with_responses([
        Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: tool_call.clone(),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ]),
        Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call,
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ]),
    ]);

    let Some(root) = root else {
        return;
    };
    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        Some(root),
    )
    .await;

    assert_eq!(result, Err(NativeProviderRoundError::SecondRoundToolCall));
    assert_eq!(requester.requests.len(), 2);
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[tokio::test]
async fn native_provider_one_round_rejects_unknown_tool_before_second_request() {
    let root_path = temp_native_provider_root("native-provider-unknown-tool");
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "inspect cargo",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let mut requester = FakeProviderRequester::with_responses([Ok(vec![
        ProviderStreamEvent::ToolCallCompleted {
            turn_id: turn.clone(),
            tool_call: ProviderToolCall {
                call_id: String::from("provider-call-1"),
                name: String::from("read"),
                arguments_json: serde_json::json!({"path":"Cargo.toml"}),
            },
        },
        ProviderStreamEvent::Completed {
            turn_id: turn.clone(),
            finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
            usage: None,
            provider_response_id: None,
        },
    ])]);

    let Some(root) = root else {
        return;
    };
    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        Some(root),
    )
    .await;

    assert_eq!(
        result,
        Err(NativeProviderRoundError::ToolContinuation(String::from(
            "tool_round_validation_failed"
        )))
    );
    assert_eq!(requester.requests.len(), 1);
    assert!(pending_events.iter().any(|event| matches!(
        event,
        NativeSessionEvent::ToolExecutionFinished {
            outcome: crate::NativeToolOutcome::ValidationFailed,
            ..
        }
    )));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[tokio::test]
async fn native_provider_one_round_maps_second_provider_failure() {
    let root_path = temp_native_provider_root("native-provider-second-failure");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "inspect cargo",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let mut requester = FakeProviderRequester::with_responses([
        Ok(vec![
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn.clone(),
                tool_call: ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("project_path_info"),
                    arguments_json: serde_json::json!({"path":"Cargo.toml"}),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: turn.clone(),
                finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ]),
        Err(ProviderError::malformed_stream("second provider request failed")),
    ]);

    let Some(root) = root else {
        return;
    };
    let result = run_native_provider_one_readonly_tool_round(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        Some(root),
    )
    .await;

    assert_eq!(
        result,
        Err(NativeProviderRoundError::Provider(
            ProviderError::malformed_stream("second provider request failed")
        ))
    );
    assert_eq!(requester.requests.len(), 2);
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run failure tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_rejects
just dev cargo test -p yach-backend native_provider_one_round_maps_second_provider_failure
```

Expected: PASS.

- [ ] **Step 3: Run all one-round tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round
```

Expected: PASS.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "test: cover native provider tool continuation failures"
```

---

### Task 4: Production Native-Provider Wiring

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add production requester wrapper**

Add near the requester trait:

```rust
struct RigProviderRequester {
    adapter: RigProviderAdapterConfig,
}

impl ProviderRequester for RigProviderRequester {
    fn request<'a>(
        &'a mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'a, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        let adapter = self.adapter.clone();
        Box::pin(async move { run_provider_request(adapter, request).await })
    }
}
```

- [ ] **Step 2: Refactor `handle_native_provider_prompt` through helper**

Replace direct `ProviderRequest` construction and `run_provider_request(provider.adapter, request).await` handling in `handle_native_provider_prompt` with:

```rust
let project_root = NativeResourceRoot::project(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))).ok();
let mut requester = RigProviderRequester {
    adapter: provider.adapter,
};
let result = run_native_provider_one_readonly_tool_round(
    &mut requester,
    ProviderModel {
        provider: provider_name.to_owned(),
        model: model_id.clone(),
    },
    log,
    pending_events,
    &ids.turn,
    project_root,
)
.await;
```

Then handle:

```rust
match result {
    Ok(round) => {
        for delta in native_response_chunks(&round.text) {
            if tx
                .send(BackendEvent::Server(ServerEvent::PromptDelta {
                    session_id: String::from("default"),
                    delta,
                }))
                .is_err()
            {
                push_native_prompt_total_metric(
                    log,
                    pending_events,
                    &ids.turn,
                    ids.prompt_started,
                );
                push_native_session_event(
                    log,
                    pending_events,
                    NativeSessionEvent::TurnFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id: ids.turn,
                        outcome: NativeTurnOutcome::Cancelled,
                        reason: Some(String::from("ui receiver dropped")),
                    },
                );
                let _ = append_pending_native_session_events(store, pending_events);
                return;
            }
        }
        push_native_prompt_total_metric(log, pending_events, &ids.turn, ids.prompt_started);
        push_native_session_event(
            log,
            pending_events,
            NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("default")),
                entry_id: ids.assistant_entry,
                parent_entry_id: Some(ids.user_entry),
                turn_id: ids.turn.clone(),
                role: NativeRole::Assistant,
                text: round.text,
                provider: Some(ProviderMetadata {
                    provider: provider_name.to_owned(),
                    model: model_id,
                    response_id: round.provider_response_id,
                }),
            },
        );
        push_native_session_event(
            log,
            pending_events,
            NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("default")),
                turn_id: ids.turn,
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            },
        );
        finish_native_prompt(
            tx,
            store,
            pending_events,
            "turn_end native provider",
            PromptOutcome::Completed,
        );
    }
    Err(error) => {
        let provider_error = native_provider_round_error_to_provider_error(&error);
        let outcome = if matches!(error, NativeProviderRoundError::Cancelled(_)) {
            NativeTurnOutcome::Cancelled
        } else {
            NativeTurnOutcome::Failed
        };
        let prompt_outcome = if matches!(error, NativeProviderRoundError::Cancelled(_)) {
            PromptOutcome::Cancelled
        } else {
            PromptOutcome::Failed
        };
        let status = if matches!(error, NativeProviderRoundError::Cancelled(_)) {
            "turn_end native provider cancelled"
        } else {
            "turn_end native provider failed"
        };
        push_native_prompt_total_metric(log, pending_events, &ids.turn, ids.prompt_started);
        persist_native_fixture_error(
            tx,
            log,
            pending_events,
            ids.turn,
            outcome,
            &provider_error,
        );
        finish_native_prompt(tx, store, pending_events, status, prompt_outcome);
    }
}
```

Add:

```rust
fn native_provider_round_error_to_provider_error(error: &NativeProviderRoundError) -> ProviderError {
    match error {
        NativeProviderRoundError::Provider(error) => error.clone(),
        NativeProviderRoundError::Cancelled(reason) => ProviderError::cancelled(reason.clone()),
        NativeProviderRoundError::StreamEndedWithoutCompletion => ProviderError {
            kind: ProviderErrorKind::MalformedStream,
            message: String::from("Native provider stream ended without completion"),
            redacted_debug: None,
        },
        NativeProviderRoundError::ProjectRootUnavailable => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider project root unavailable"),
            redacted_debug: None,
        },
        NativeProviderRoundError::ToolContinuation(reason) => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider tool continuation failed"),
            redacted_debug: Some(reason.clone()),
        },
        NativeProviderRoundError::SecondRoundToolCall => ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Native provider requested another tool round"),
            redacted_debug: Some(String::from("second_round_tool_call")),
        },
    }
}
```

Keep UI delta emission after helper success, not inside the helper. This preserves the spec rule that first-round text is not emitted when tools are used.

- [ ] **Step 3: Run targeted existing runtime tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round
just dev cargo test -p yach-cli native_dogfood_loop_provider_cancel
```

Expected: PASS.

- [ ] **Step 4: Run clippy**

Run:

```bash
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: wire native provider through one-round tool helper"
```

---

### Task 5: Verification And Planning Docs

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run full verification**

Run:

```bash
just dev cargo fmt --all
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
just dev cargo test -p yach-cli native_dogfood_loop_provider_cancel
git diff --check
```

Expected: all pass.

- [ ] **Step 2: Update state doc**

In `docs/project/state.md`, update the native read-only posture bullet to say:

```markdown
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, backend-only continuation mapping into adapter-ready provider request input, and explicit native-provider one-round handling for completed safe read-only tool calls.
```

Update Plan Sufficiency to say:

```markdown
The plan is sufficient to plan provider tool advertising for `project_path_info` behind explicit native-provider opt-in. It is not sufficient for file writes, process execution, network tools, extension runtime, provider-native tool-result block support, or default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.
```

Add these records:

```markdown
- `docs/superpowers/specs/2026-05-11-native-provider-one-round-tools-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-one-round-tools.md`
```

- [ ] **Step 3: Update next doc**

In `docs/project/next.md`, update Recommended Next Move:

```markdown
Recommended next move: continue Native MVP implementation with provider tool advertising for `project_path_info` behind explicit native-provider opt-in.
```

Use this Why text:

```markdown
Why: the native-provider runner can now handle a completed safe read-only tool call and one continuation round, but real provider dogfooding still needs the initial provider request to advertise only the metadata-safe `project_path_info` schema so models can request it intentionally.
```

Add the spec and plan paths to Relevant sources.

- [ ] **Step 4: Commit docs**

Run:

```bash
git add docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "docs: update native provider one-round tool status"
```

---

## Final Branch Verification

After all tasks:

```bash
just dev cargo fmt --all -- --check
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
just dev cargo test -p yach-cli native_dogfood_loop_provider_cancel
git diff --check origin/main...HEAD
```

Expected: all pass.

Dispatch a final whole-branch review before creating the PR.
