# Native Provider Multi-Round Tool Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the native-provider one-round tool continuation boundary with a bounded multi-round backend-owned tool loop.

**Architecture:** Keep the first implementation in `crates/yach-backend/src/native_runner.rs`, where the current provider orchestration already lives. Reuse the existing registry, permission policy, read-only executor, edit access facade, review channel, session evidence, and Rig adapter projection. Continuation requests must preserve provider-visible schemas across rounds so the provider can read, then edit, then answer in one turn.

**Tech Stack:** Rust 2024, `tokio`, existing yach backend/provider abstractions, existing fake provider requester tests, `just dev cargo test`, `just lint`.

---

## File Structure

- Modify `crates/yach-backend/src/rig_adapter.rs`: update the provider continuation guard message so it allows additional advertised tool calls.
- Modify `crates/yach-backend/src/native_runner.rs`: add loop policy/budget helpers, extract tool batch execution, replace one-round agent orchestration with a bounded loop, and preserve edit review/evidence behavior.
- Modify `docs/project/state.md`: after implementation lands, record that the multi-round loop is implemented.
- Modify `docs/project/next.md`: after implementation lands, point the next move at dogfooding the loop and then extension runtime design.

Do not create a new runtime module in this slice. Moving provider loop internals into a new module now would require exposing several private edit/review helpers and would increase first-implementation risk.

## Task 1: Update Continuation Guard For Multi-Round Semantics

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Test: existing Rig continuation projection tests in `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write the failing guard-message test**

Add a test named `rig_continuation_guard_allows_more_advertised_tools` near the existing Rig continuation projection tests. Build a fixture continuation submission with one completed tool result, project it through `rig_adapter::project_provider_continuation_request`, find the system guard message, and assert:

```rust
assert!(guard.content.contains("You may call more advertised tools"));
assert!(!guard.content.contains("No additional tools are available"));
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend rig_continuation_guard_allows_more_advertised_tools -- --exact --nocapture
```

Expected: the test fails because the current guard says no additional tools are available.

- [ ] **Step 3: Replace the one-round guard text**

In `crates/yach-backend/src/rig_adapter.rs`, replace `provider_continuation_guard_message` with:

```rust
fn provider_continuation_guard_message() -> ProviderMessage {
    ProviderMessage {
        role: NativeRole::System,
        content: String::from(
            "Yach has executed exactly the tool results included in this continuation. \
You may call more advertised tools if more work is required, or answer only from executed \
evidence. Do not claim local effects unless they are present in the tool results.",
        ),
    }
}
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
just dev cargo test -p yach-backend rig_continuation_guard_allows_more_advertised_tools -- --exact --nocapture
```

Expected: the test passes.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
git commit -m "fix: allow advertised tools in provider continuations"
```

## Task 2: Add Multi-Round Loop Policy And Budget Helpers

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write failing policy tests**

Add tests named `native_provider_tool_loop_policy_matches_design_limits` and `native_provider_tool_loop_budget_rejects_round_call_and_byte_overages` inside the existing `native_runner.rs` test module. Assert these defaults:

```rust
assert_eq!(policy.max_tool_rounds, 4);
assert_eq!(policy.max_tool_calls_per_round, 4);
assert_eq!(policy.max_total_tool_calls, 12);
assert_eq!(policy.max_result_bytes_per_tool, 64 * 1024);
assert_eq!(policy.max_total_result_bytes, 256 * 1024);
```

For budget failures, use a small policy with one round, two calls per round, three total calls, eight bytes per tool, and twelve aggregate bytes. Assert these error labels:

```rust
"tool_round_too_many_calls"
"tool_loop_too_many_rounds"
"tool_loop_too_many_total_calls"
"tool_loop_total_result_too_large"
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_provider_tool_loop_ -- --nocapture
```

Expected: compile failure because `NativeProviderToolLoopPolicy` and `NativeProviderToolLoopBudget` do not exist.

- [ ] **Step 3: Add policy and budget helper types**

Near `native_provider_agent_tool_continuation_policy`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeProviderToolLoopPolicy {
    max_tool_rounds: usize,
    max_tool_calls_per_round: usize,
    max_total_tool_calls: usize,
    max_result_bytes_per_tool: usize,
    max_total_result_bytes: usize,
}

impl NativeProviderToolLoopPolicy {
    const fn agent_default() -> Self {
        Self {
            max_tool_rounds: 4,
            max_tool_calls_per_round: 4,
            max_total_tool_calls: 12,
            max_result_bytes_per_tool: 64 * 1024,
            max_total_result_bytes: 256 * 1024,
        }
    }

    const fn as_continuation_policy(self) -> NativeToolContinuationPolicy {
        NativeToolContinuationPolicy {
            max_tool_calls: self.max_tool_calls_per_round,
            max_result_bytes: self.max_result_bytes_per_tool,
        }
    }
}
```

Add `NativeProviderToolLoopBudget` with:

```rust
struct NativeProviderToolLoopBudget {
    policy: NativeProviderToolLoopPolicy,
    tool_rounds: usize,
    total_tool_calls: usize,
    total_result_bytes: usize,
}
```

Implement `new`, `begin_tool_round(tool_call_count)`, and `record_tool_result(tool_request_id, byte_count)`. `begin_tool_round` must enforce round, per-round call, and total-call limits before incrementing counters. `record_tool_result` must enforce per-tool and aggregate byte limits before incrementing aggregate bytes. Return `NativeProviderRoundError::ToolContinuation(String::from("<label>"))` for each budget failure label from Step 1.

Replace `native_provider_agent_tool_continuation_policy` with:

```rust
fn native_provider_agent_tool_continuation_policy() -> NativeToolContinuationPolicy {
    NativeProviderToolLoopPolicy::agent_default().as_continuation_policy()
}
```

- [ ] **Step 4: Run the focused tests and verify they pass**

Run:

```bash
just dev cargo test -p yach-backend native_provider_tool_loop_ -- --nocapture
```

Expected: the policy and budget tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "feat: add native provider tool loop limits"
```

## Task 3: Extract Agent Tool Batch Execution

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a failing read-batch test**

Add `native_provider_agent_tool_batch_executes_read_tool_results` inside the existing `native_runner.rs` test module. The test should:

- create a temp project with `src/lib.rs` containing `alpha\n`;
- build the same registry and permission policy as `run_native_provider_one_agent_tool_round`;
- create `ProjectReadOnlyToolExecutor`, `NativeEditAccess`, `NativeProviderBufferedEventSink`, review channels, and `NativeProviderToolLoopBudget::new(NativeProviderToolLoopPolicy::agent_default())`;
- call `execute_native_provider_agent_tool_batch` with one `ProviderToolCall` named `read_text_file`;
- assert one `NativeProviderToolResult` exists, the provider call id is `call-read-1`, the content includes `"alpha\n"`, and `pending_events` contains a completed `ToolExecutionFinished` event.

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_tool_batch_executes_read_tool_results -- --exact --nocapture
```

Expected: compile failure because the batch context and helper do not exist.

- [ ] **Step 3: Add the batch context**

In the top-level `crate` import list at the top of `crates/yach-backend/src/native_runner.rs`, add `PendingNativeToolRequest` immediately after `NativeToolRequestId`.

Near `NativeProviderAgentToolRound`, add:

```rust
struct NativeProviderAgentToolBatch<'a> {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    project_root: NativeResourceRoot,
    registry: &'a NativeToolRegistry,
    permission_policy: &'a NativeToolPermissionPolicy,
    read_only_executor: &'a ProjectReadOnlyToolExecutor,
    edit_access: &'a mut NativeEditAccess,
    edit_sink: &'a NativeProviderBufferedEventSink<'a>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: &'a mut AgentEditDecisionReceiver,
    tool_event_store: Option<&'a NativeJsonlSessionStore>,
    budget: &'a mut NativeProviderToolLoopBudget,
    tool_round_index: usize,
    edit_traces: &'a mut Vec<ProviderContinuationEditTrace>,
    log: &'a mut NativeSessionLog,
    pending_events: &'a mut Vec<NativeSessionEvent>,
}
```

- [ ] **Step 4: Extract read-only execution**

Add `execute_native_provider_readonly_tool_request(batch, request)`. Move the current read-only match-branch logic into it without changing behavior:

- call `record_native_tool_validation`;
- call `ProjectReadOnlyToolExecutor::execute`;
- push failed `ToolExecutionFinished` evidence on execution failure;
- call `batch.budget.record_tool_result(&request.request_id, execution.byte_count)`;
- push completed `ToolExecutionFinished` evidence;
- return `NativeProviderToolResult`.

Use these exact failure labels:

```rust
"tool_round_validation_failed"
"tool_round_execution_failed"
"tool_round_result_too_large"
```

- [ ] **Step 5: Extract edit execution**

Add `execute_native_provider_edit_tool_request(batch, request)`. Move the current edit match-branch logic into it without changing behavior:

- persist pending tool events before entering edit access when `tool_event_store` exists;
- call `prepare_agent_edit_tool_request`;
- drain `edit_sink` into `log` and `pending_events`;
- send `ServerEvent::ToolReviewRequested` for `NeedsUserReview`;
- wait with `wait_for_agent_edit_review_decision`;
- apply or reject through existing helpers;
- drain edit sink again after review;
- push `ProviderContinuationEditTrace` for completed and reviewed edit results;
- call `batch.budget.record_tool_result(&result.tool_request_id, result.byte_count)`;
- return `NativeProviderToolResult`.

Do not change edit permission policy, edit policy, review event payloads, or trace fields in this task.

- [ ] **Step 6: Add the batch dispatcher**

Add `execute_native_provider_agent_tool_batch(batch, tool_calls)`. It must:

- call `batch.budget.begin_tool_round(tool_calls.len())`;
- convert each provider tool call with `pending_tool_request_from_provider_call(format!("tool-request-{}-{}", batch.tool_round_index, index + 1), batch.turn_id.clone(), tool_call)`;
- dispatch read/list/search tools to the read-only helper;
- dispatch `edit_text_file` and `create_text_file` to the edit helper;
- record validation evidence and return `tool_round_validation_failed` for unknown tools;
- append pending session events to `tool_event_store` after the batch when a store exists;
- return the ordered `Vec<NativeProviderToolResult>`.

- [ ] **Step 7: Run focused and existing agent tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_tool_batch_executes_read_tool_results -- --exact --nocapture
just dev cargo test -p yach-backend native_provider_agent_ -- --nocapture
```

Expected: the new batch test and existing agent tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "refactor: extract native provider tool batch execution"
```

## Task 4: Preserve Tool Advertising Across Continuation Requests

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a failing continuation advertising test**

Add `native_provider_agent_continuation_preserves_tool_advertising`. The fake provider should return:

1. one `read_text_file` tool call and completion with `ToolCalls`;
2. final completion with `Stop`.

After running `run_native_provider_one_agent_tool_round`, assert:

```rust
assert_eq!(requester.requests.len(), 2);
assert!(parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)?.is_some());
assert!(parse_provider_tool_advertising_extensions(&requester.requests[1].extensions)?.is_some());
```

Use explicit `expect` calls instead of the `?` operator in the actual test.

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_continuation_preserves_tool_advertising -- --exact --nocapture
```

Expected: the test fails because continuation requests currently strip provider tool advertising extensions.

- [ ] **Step 3: Preserve extensions in agent continuations**

In the agent provider continuation request construction, replace:

```rust
extensions: crate::strip_provider_tool_advertising_extensions(initial_request.extensions),
```

with:

```rust
extensions: initial_request.extensions.clone(),
```

Keep strip behavior in the older `#[cfg(test)] run_native_provider_one_tool_round_with_registry` helper unless tests show it conflicts.

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_continuation_preserves_tool_advertising -- --exact --nocapture
```

Expected: the test passes.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "fix: preserve native provider tools across continuations"
```

## Task 5: Replace One-Round Agent Orchestration With A Loop

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a failing read-then-edit test**

Add `native_provider_agent_loop_reads_then_edits_in_later_round`. The fake provider should return three responses:

1. optional text delta plus `read_text_file` call for `note.txt`;
2. `edit_text_file` call replacing `ok` with `passed`;
3. final text `Updated note.txt.` with `Stop`.

The test should create `note.txt` with `native provider edit dogfood ok`, approve the edit review by sending `AgentEditReviewDecision`, then assert:

```rust
assert_eq!(requester.requests.len(), 3);
assert_eq!(
    std::fs::read_to_string(root_path.join("note.txt")).expect("edited note"),
    "native provider edit dogfood passed"
);
assert_eq!(result.expect("loop result").text, "Updated note.txt.");
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_loop_reads_then_edits_in_later_round -- --exact --nocapture
```

Expected: the test fails with the current second-round tool-call behavior.

- [ ] **Step 3: Add a continuation request helper**

Add `build_native_provider_tool_continuation_request(initial_request, prior_messages, tool_results)`. It should build `ProviderContinuationRequest` with:

```rust
turn_id: initial_request.turn_id.clone()
model: initial_request.model.clone()
prior_messages
tool_results
extensions: initial_request.extensions.clone()
```

Validate with:

```rust
ProviderContinuationValidationPolicy::strict_tool_results(
    NativeProviderToolLoopPolicy::agent_default().max_result_bytes_per_tool,
)
```

Project through `crate::rig_adapter::project_provider_continuation_request`.

- [ ] **Step 4: Replace the one-round request/continue block with a loop**

Inside `run_native_provider_one_agent_tool_round`, keep setup for registry, policy, advertising, static context, `initial_request`, project root, read-only executor, edit access, and edit sink. Replace the first request through final continuation handling with:

```rust
let loop_policy = NativeProviderToolLoopPolicy::agent_default();
let mut loop_budget = NativeProviderToolLoopBudget::new(loop_policy);
let mut next_request = initial_request.clone();
let mut prior_messages = initial_request.messages.clone();

loop {
    let provider_events = requester
        .request(next_request.clone())
        .await
        .map_err(NativeProviderRoundError::Provider)?;
    let round = collect_native_provider_first_round(provider_events)?;
    if round.tool_calls.is_empty() {
        return Ok(NativeProviderRoundResult {
            text: round.text,
            provider_response_id: round.provider_response_id,
        });
    }

    let tool_round_index = loop_budget.tool_rounds + 1;
    let tool_results = execute_native_provider_agent_tool_batch(
        NativeProviderAgentToolBatch {
            session_id: NativeSessionId(String::from("default")),
            turn_id: turn_id.clone(),
            project_root: project_root.clone(),
            registry: &registry,
            permission_policy: &permission_policy,
            read_only_executor: &read_only_executor,
            edit_access: &mut edit_access,
            edit_sink: &edit_sink,
            review_tx: review_tx.clone(),
            review_decisions: &mut review_decisions,
            tool_event_store,
            budget: &mut loop_budget,
            tool_round_index,
            edit_traces: &mut provider_continuation_edit_traces,
            log,
            pending_events,
        },
        round.tool_calls,
    )
    .await?;

    next_request = build_native_provider_tool_continuation_request(
        &initial_request,
        prior_messages,
        tool_results,
    )?;
    prior_messages = next_request.messages.clone();
}
```

After this works, re-add existing provider continuation trace recording around provider failures, mapping failures, and final completion so edit trace coverage remains intact.

- [ ] **Step 5: Run focused and broad provider tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_loop_reads_then_edits_in_later_round -- --exact --nocapture
just dev cargo test -p yach-backend native_provider_agent_ -- --nocapture
```

Expected: the new read-then-edit test passes. Existing tests that assert the old one-round boundary may still fail until Task 6.

- [ ] **Step 6: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "feat: run native provider tools across multiple rounds"
```

## Task 6: Convert One-Round Boundary Tests To Limit Tests

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Find old one-round expectations**

Run:

```bash
rg -n "SecondRoundToolCall|second_round_tool_call|requested another tool round|one_round" crates/yach-backend/src/native_runner.rs
```

Expected: old tests or labels identify the former one-round boundary.

- [ ] **Step 2: Replace boundary expectations with limit expectations**

For tests that intentionally expected a second tool round to fail, convert them to exceed `max_tool_rounds`. Use five fake provider responses with one cheap `project_path_info` or `read_text_file` call each. Assert:

```rust
assert_eq!(
    result,
    Err(NativeProviderRoundError::ToolContinuation(String::from(
        "tool_loop_too_many_rounds"
    )))
);
assert_eq!(requester.requests.len(), 4);
```

The fifth response exists only to prove yach does not request it.

- [ ] **Step 3: Update stale labels**

In `native_provider_round_error_label`, keep `SecondRoundToolCall` only for `collect_native_provider_final_round` test helpers if still needed. Map it to `unexpected_tool_call` instead of `second_round_tool_call`.

- [ ] **Step 4: Run converted and broad native provider tests**

Run:

```bash
just dev cargo test -p yach-backend tool_loop_too_many_rounds -- --nocapture
just dev cargo test -p yach-backend native_provider -- --nocapture
```

Expected: all native provider tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "test: replace one-round boundary with loop limit coverage"
```

## Task 7: Add Deterministic Stop Outcome Mapping

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a failing stop mapping test**

Add `native_provider_agent_loop_limit_maps_to_redacted_provider_error`. Construct:

```rust
let error = NativeProviderRoundError::ToolContinuation(String::from(
    "tool_loop_too_many_rounds",
));
let provider_error = native_provider_round_error_to_provider_error(&error);
```

Assert:

```rust
assert_eq!(provider_error.kind, ProviderErrorKind::InvalidRequest);
assert_eq!(
    provider_error.message,
    "Native provider tool loop stopped before completion"
);
assert_eq!(
    provider_error.redacted_debug,
    Some(String::from("tool_loop_too_many_rounds"))
);
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_loop_limit_maps_to_redacted_provider_error -- --exact --nocapture
```

Expected: the current generic continuation failure message does not match.

- [ ] **Step 3: Add loop stop message mapping**

Add `native_provider_tool_loop_stop_message(reason)`. Return `"Native provider tool loop stopped before completion"` for:

```rust
"tool_loop_too_many_rounds"
"tool_loop_too_many_total_calls"
"tool_loop_total_result_too_large"
```

Return `"Native provider tool continuation failed"` for existing validation, execution, mapping, and per-round failures. Update the `ToolContinuation(reason)` arm in `native_provider_round_error_to_provider_error` to use this helper.

- [ ] **Step 4: Run mapping tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_loop_limit_maps_to_redacted_provider_error -- --exact --nocapture
just dev cargo test -p yach-backend native_provider_round_error -- --nocapture
```

Expected: mapping tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "fix: report native provider loop stops deterministically"
```

## Task 8: Add Evidence And Advertising Regression Coverage

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a multi-round evidence test**

Add `native_provider_agent_loop_records_read_and_edit_evidence_before_final_answer`. Use the same three-response fake provider pattern from Task 5. Assert:

```rust
assert!(result.is_ok());
assert!(log.events.iter().any(|event| {
    matches!(
        event,
        NativeSessionEvent::ToolExecutionFinished {
            outcome: NativeToolOutcome::Completed,
            ..
        }
    )
}));
assert!(edit_trace_records(&log).iter().any(|trace| {
    trace.phase == NativeEditTracePhase::ProviderContinuation
        && trace.tool_name.as_deref() == Some("edit_text_file")
        && trace.outcome == NativeEditTraceOutcome::Completed
}));
for request in &requester.requests {
    let advertising = parse_provider_tool_advertising_extensions(&request.extensions)
        .expect("extensions parse")
        .expect("advertising exists");
    assert!(advertising.tools.iter().any(|tool| tool.name == "read_text_file"));
    assert!(advertising.tools.iter().any(|tool| tool.name == "edit_text_file"));
}
```

- [ ] **Step 2: Run the focused test**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_loop_records_read_and_edit_evidence_before_final_answer -- --exact --nocapture
```

Expected: the test passes if previous tasks preserved evidence correctly. If it fails, fix the missing event or trace path before proceeding.

- [ ] **Step 3: Run native provider tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider -- --nocapture
```

Expected: all native provider tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "test: cover multi-round provider loop evidence"
```

## Task 9: Update Active Project Docs

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update project state**

In `docs/project/state.md`, replace the multi-round design paragraph with:

```markdown
Native-provider dogfooding showed the one-round continuation boundary was the
main blocker for practical agent edits. The native-provider path now has a
bounded backend-owned multi-round tool loop for provider-visible read/search/list
and exact/create edit tools. The loop preserves yach-owned validation,
permissions, review, execution, redacted evidence, provider continuation, and
provider-visible tool schemas across rounds. It remains registry-oriented so
future extension-owned tools and explicit built-in replacement can participate
without changing provider-loop semantics.
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, replace the recommended next move with:

```markdown
Recommended next move: dogfood the native-provider multi-round read/search/edit
loop against real provider sessions, then design the extension runtime and
tool replacement UX.

Why: the core loop should now support normal read-then-edit workflows. The next
useful evidence is real-session behavior: whether loop limits are right, whether
review/cancellation feels responsive, whether provider-visible result summaries
are sufficient, and whether extension runtime design should happen before
broader tools.
```

- [ ] **Step 3: Check docs diff**

Run:

```bash
git diff -- docs/project/state.md docs/project/next.md
```

Expected: only current status and next-work sections changed.

- [ ] **Step 4: Commit**

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update native provider loop status"
```

## Task 10: Final Verification And PR

**Files:**
- No source edits expected after this task begins.

- [ ] **Step 1: Run focused backend tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider -- --nocapture
```

Expected: all native provider tests pass.

- [ ] **Step 2: Run focused UI review tests**

Run:

```bash
just dev cargo test -p yach-ui tool_review -- --nocapture
```

Expected: all tool review tests pass. This guards the edit approval path used by provider-originated edits.

- [ ] **Step 3: Run full formatting and lint**

Run:

```bash
just fmt
just lint
```

Expected: both commands pass.

- [ ] **Step 4: Inspect final status**

Run:

```bash
git status --short --branch
```

Expected: the branch is clean after the final commit.

- [ ] **Step 5: Create PR**

Run:

```bash
git push -u origin native-provider-multi-round-loop
gh pr create --base main --head native-provider-multi-round-loop --title "Implement native provider multi-round tool loop" --body "## Summary
- replace native-provider one-round continuation with a bounded multi-round tool loop
- preserve provider-visible tool schemas across rounds
- keep read/search/edit execution under yach-owned validation, review, evidence, and limits

## Verification
- just dev cargo test -p yach-backend native_provider -- --nocapture
- just dev cargo test -p yach-ui tool_review -- --nocapture
- just fmt
- just lint"
```

Expected: GitHub returns the PR URL.

## Notes

- Keep extension replacement implementation out of this slice. The loop should remain registry-oriented, but no new override config is required here.
- Keep shell/process/network tools out of this slice.
- Keep provider-owned tool execution out of this slice.
- Keep existing user review semantics for edit tools.
- Prefer helper extraction over adding another match-heavy loop body.
