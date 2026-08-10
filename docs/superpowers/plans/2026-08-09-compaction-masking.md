# Compaction Masking (Slice 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add deterministic tool-result masking to the compaction transaction: old tool-result bodies are superseded by append-only mask events and rendered as elision markers, reclaiming context without a summary call when masking alone suffices.

**Architecture:** A new `SessionEvent::ToolResultMasked` event appended inside the compaction transaction; a masking pass in `run_compaction_with` that runs before cut-point selection; provider-message assembly and summary serialization read through the mask index. The mask-only short-circuit (`CompactionApplication::Masked`) applies only to client-rebuilt context; native (openai-responses) compaction decisions use the pre-mask estimate.

**Spec:** `docs/superpowers/specs/2026-08-09-compaction-masking-design.md` (owner-designed 2026-08-09). Read it first; this plan assumes its decisions.

**Tech Stack:** Rust, serde-tagged `SessionEvent` JSONL log, existing compaction module (`crates/yach-backend/src/compaction.rs`), provider assembly in `crates/yach-backend/src/runner.rs`.

## Global Constraints

- Session log is append-only. NEVER mutate or truncate existing events; masking is expressed only through new `ToolResultMasked` events.
- `compaction.masking` config bool, default `true`. No other new config.
- Protection window: `compaction.keep_recent_tokens` worth of result content, newest-first. Current turn's results are never masked.
- Min-savings floor: `max(5% of usable window tokens, 8192 tokens)`. Below floor → no masks, summarize as today.
- Marker text (exact): `[result masked by compaction: {bytes} bytes; re-read the source if needed]`
- Call/result adjacency invariant: mask the body only; assistant tool-call message and tool-result message always survive with full arguments.
- Native path: compaction dispatch decision uses the PRE-MASK estimate. `Masked` is unreachable when native compaction is selected and applicable.
- Bash is not involved; Rust only. Use `just dev cargo ...` for all cargo commands (project devenv rule). TDD: failing test before implementation, watch it fail, then minimal code.
- Checkpoint after each task: `jj describe -m "<message>" && jj new`.

---

### Task 1: `ToolResultMasked` session event

**Files:**
- Modify: `crates/yach-backend/src/session.rs` (event enum ~line 287, `last_entry_id` ~431, `transcript_messages` ~450, `event_turn_id` ~588)
- Modify: `crates/yach-backend/src/compaction.rs` (`estimate_event_tokens` ~123, `turn_scoped_event_turn_id` ~172, `serialize_events_for_summary` ~380)
- Modify: `crates/yach-backend/src/runner.rs` (`provider_messages_from_event_slice` ~2542 exhaustive match)
- Test: unit tests in `session.rs` and `compaction.rs` test modules

**Interfaces:**
- Produces: `SessionEvent::ToolResultMasked { session_id: SessionId, turn_id: TurnId, masked_turn_id: TurnId, tool_request_id: ToolRequestId, bytes_freed: u64, reason: MaskReason }`
- Produces: `pub enum MaskReason { ThresholdPrePass }` in `session.rs`, serde `snake_case`.
- Produces: `SessionLog::masked_results(&self) -> std::collections::HashSet<(TurnId, ToolRequestId)>` helper (or free fn in `session.rs`) used by assembly in Task 2.

- [ ] **Step 1: Write the failing test**

In `crates/yach-backend/src/session.rs` test module:

```rust
#[test]
fn tool_result_masked_event_round_trips_through_jsonl() {
    let event = SessionEvent::ToolResultMasked {
        session_id: SessionId(String::from("s")),
        turn_id: TurnId(String::from("turn-2")),
        masked_turn_id: TurnId(String::from("turn-1")),
        tool_request_id: ToolRequestId(String::from("req-1")),
        bytes_freed: 12_345,
        reason: MaskReason::ThresholdPrePass,
    };
    let line = serde_json::to_string(&event).unwrap();
    assert!(line.contains("\"type\":\"tool_result_masked\""));
    let parsed: SessionEvent = serde_json::from_str(&line).unwrap();
    assert_eq!(parsed, event);
}
```

Also assert `event_turn_id` returns the masking turn and `last_entry_id` skips the event (mirror existing match arms for `CompactionCheckpoint`).

- [ ] **Step 2: Run test to verify it fails**

Run: `just dev cargo test -p yach-backend tool_result_masked`
Expected: FAIL — `no variant ToolResultMasked` compile error.

- [ ] **Step 3: Write minimal implementation**

Add the `MaskReason` enum and the `ToolResultMasked` variant to `SessionEvent` in `session.rs`. Update every exhaustive match over `SessionEvent` in `session.rs`, `compaction.rs`, and `runner.rs`:

- `event_turn_id` → `Some(turn_id)` (the masking turn).
- `last_entry_id`, `transcript_messages` → `None` arm (like `TurnFinished`).
- `estimate_event_tokens` → 0 (the mask event itself contributes no content; the body it supersedes is subtracted by the masking accounting, Task 3).
- `turn_scoped_event_turn_id` → the masking `turn_id`.
- `serialize_events_for_summary` → emit a marker line: `[Tool result masked: {bytes_freed} bytes reclaimed]`.
- `provider_messages_from_event_slice` → `Vec::new()` arm for now; masking render lands in Task 2.

Any other exhaustive `SessionEvent` matches the compiler flags (UI/session_state): mask events render as a dim "[result masked]" row in the TUI event rendering if such a match exists there; check `crates/yach-ui` for exhaustive matches and give them the minimal marker treatment.

- [ ] **Step 4: Run test to verify it passes**

Run: `just dev cargo test -p yach-backend tool_result_masked`
Expected: PASS. Also `just dev cargo check -p yach-backend` clean (all exhaustive matches updated).

- [ ] **Step 5: Commit**

```bash
jj describe -m "feat: add ToolResultMasked session event"
jj new
```

---

### Task 2: Mask-aware provider assembly

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` (`provider_messages_from_event_slice` ~2542, `ToolExecutionFinished` arm ~2600)
- Test: runner test module (existing provider-assembly tests nearby)

**Interfaces:**
- Consumes: `SessionLog::masked_results()` from Task 1.
- Produces: masked results render body as `[result masked by compaction: {bytes} bytes; re-read the source if needed]`; the tool-call pair survives intact.

- [ ] **Step 1: Write the failing test**

In `runner.rs` test module, build a log: turn-1 tool request + finished (result_content "BIG BODY"), turn-1 TurnFinished Completed, then a `ToolResultMasked` event for that request, then current turn-2. Call `provider_messages_from_log(&log, &turn2)` and assert:

```rust
#[test]
fn masked_tool_result_renders_elision_marker_with_call_pair_intact() {
    // ... construct log per above ...
    let messages = provider_messages_from_log(&log, &TurnId(String::from("turn-2")));
    let tool_result = messages.iter().find(|m| /* is tool result block */).expect("pair survives");
    let text = /* tool result content */;
    assert!(text.contains("[result masked by compaction: 8 bytes; re-read the source if needed]"));
    // assistant tool-call message with full arguments immediately precedes it
}
```

Also a negative test: without a mask event the body renders verbatim (existing behavior preserved).

- [ ] **Step 2: Run test to verify it fails**

Run: `just dev cargo test -p yach-backend masked_tool_result`
Expected: FAIL — body renders verbatim, no marker.

- [ ] **Step 3: Write minimal implementation**

In `provider_messages_from_event_slice`:
- Build the masked set from `complete_log` (all events, not just the slice — masks can be appended after the sliced range's events).
- In the `ToolExecutionFinished` arm, if `(turn_id, tool_request_id)` is in the masked set, substitute `crate::mask_marker(bytes_freed)` for `result` — never a hand-written marker string, so render and savings math can never drift apart.

The marker carries `bytes_freed` from the mask event; keep a `HashMap<(TurnId, ToolRequestId), u64>` instead of a set. Duplicate mask events for the same request: first one wins (idempotent).

- [ ] **Step 4: Run test to verify it passes**

Run: `just dev cargo test -p yach-backend masked_tool_result`
Expected: PASS, plus existing provider-assembly tests still green:
`just dev cargo test -p yach-backend provider_messages`

- [ ] **Step 5: Commit**

```bash
jj describe -m "feat: render masked tool results as elision markers"
jj new
```

---

### Task 3: Masking pass in the compaction transaction

**Files:**
- Modify: `crates/yach-backend/src/compaction.rs` (`CompactionConfig` ~35 — add `masking: bool`; new `select_mask_candidates` fn; `estimate` helpers)
- Modify: `crates/yach-backend/src/runner.rs` (`CompactionApplication` ~4574 — add `Masked` variant; `run_compaction_with` ~4619 — masking pass before cut selection; refill accounting ~3685, ~4175)
- Test: `compaction.rs` test module

**Interfaces:**
- Produces: `pub fn select_mask_candidates(log: &SessionLog, current_turn_id: &TurnId, protect_tokens: u64) -> Vec<MaskCandidate>` where `MaskCandidate { masked_turn_id: TurnId, tool_request_id: ToolRequestId, bytes: u64, net_tokens: u64 }`. `bytes` is the original body length (drives the marker text); `net_tokens` is `estimate_text_tokens(body).saturating_sub(estimate_text_tokens(&mask_marker(bytes)))` — the marker stays provider-visible, so net, never gross, drives every decision.
- Produces: `pub const MASK_MIN_SAVINGS_TOKENS: u64 = 8_192;` and `pub fn mask_savings_floor(usable_tokens: u64) -> u64` = `max(usable/20, MASK_MIN_SAVINGS_TOKENS)`.
- Produces: `pub fn mask_marker(bytes_freed: u64) -> String` returning exactly `[result masked by compaction: {bytes_freed} bytes; re-read the source if needed]` — one definition shared by candidate math and assembly render.
- Produces: `CompactionApplication::Masked` variant.
- Consumes: Task 1 event, Task 2 assembly.

- [ ] **Step 1: Write the failing tests**

`compaction.rs` tests:

```rust
#[test]
fn mask_candidates_respect_protection_budget_newest_first() {
    // log: turn-1 result 10_000 chars, turn-2 result 10_000 chars, current turn-3
    // protect_tokens = 3000 (~12_000 chars) -> turn-2 protected, turn-1 candidate
    let candidates = select_mask_candidates(&log, &turn3, 3_000);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].masked_turn_id.0, "turn-1");
}

#[test]
fn mask_candidates_exclude_current_turn_and_already_masked() {
    // current-turn result and an already-masked old result never appear
}

#[test]
fn mask_savings_floor_is_max_of_five_percent_and_8k() {
    assert_eq!(mask_savings_floor(100_000), 8_192);   // 5% = 5_000 < floor
    assert_eq!(mask_savings_floor(200_000), 10_000);  // 5% wins
}

#[test]
fn mask_candidates_use_net_savings_and_drop_non_positive() {
    // A 40-char body nets negative once the ~70-char marker replaces it:
    // it must not appear in candidates even when eligible by age.
    // A 40_000-char body nets ~10_000 - ~18 tokens.
    let candidates = select_mask_candidates(&log, &current_turn, 0);
    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.bytes, 40_000);
    assert_eq!(
        c.net_tokens,
        estimate_text_tokens(&"x".repeat(40_000))
            - estimate_text_tokens(&mask_marker(40_000))
    );
}
```

Config test: `CompactionConfig::default().masking == true`; serde default when key absent.

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -p yach-backend mask_`
Expected: FAIL — functions don't exist.

- [ ] **Step 3: Write minimal implementation**

**Transaction discipline (hard constraint, verified against
`runner.rs:2443`):** `push_native_session_event` eagerly mutates BOTH
`log` and `pending_events`, and any later `append_pending_native_session_events`
flushes pending to disk. Masks MUST NOT be pushed before the summary/native
work — an early `NotApplied`/error would leave them live and later
persisted, violating the spec's no-orphan invariant. Stage proposed masks
in a local `Vec<SessionEvent>` overlay; commit them only at the successful
transaction boundary.

`CompactionConfig`: add `pub masking: bool` with `#[serde(default = ...)]` defaulting true (serde field default fn returning true; `Default` impl sets true).

`select_mask_candidates` (pure read, no mutation):
1. Build the already-masked set from the log.
2. Collect terminal-turn `ToolExecutionFinished` events with `Some(result_content)`, excluding `current_turn_id` and already-masked. "Terminal" = the owning turn has a `TurnFinished` event (reuse the `successful_entry_turns`/`terminal_tool_turns` pattern from `provider_messages_from_event_slice` — extract a small shared helper in `session.rs` if duplication offends).
3. Walk newest-first (reverse event order), accumulate `estimate_text_tokens(result_content)` until `protect_tokens` reached; everything after is a candidate, oldest-to-newest order in the output.

Overlay plumbing:

- Reuse the mask index from Task 2 as a free function over any event
  slice: `masked_result_map(events: &[SessionEvent]) -> HashMap<(TurnId, ToolRequestId), u64>`.
  The in-transaction view is `masked_result_map(log.events ++ staged_masks)`
  — compute by building the map from the log and extending it with the
  staged masks, not by touching the log.
- Post-mask estimate: `pre_mask_estimate - savings_tokens`. Exact under
  the chars/4 estimator; do not re-walk the log.
- Summary serialization: `serialize_events_for_summary` gains a mask-map
  parameter (or a `serialize_events_for_summary_with_masks` sibling;
  keep the existing signature as a delegating wrapper so slice-1 callers
  and tests are untouched). Staged masks make masked bodies serialize as
  the marker line without any log mutation.

`runner.rs` `run_compaction_with`, after the enabled/unknown-compactor checks and BEFORE `select_compaction_cut`:

```rust
let pre_mask_estimate = run.tokens_before;
let mut staged_masks: Vec<SessionEvent> = Vec::new();
let mut reclaimed_tokens: u64 = 0;
if run.config.masking {
    let candidates =
        crate::select_mask_candidates(run.log, run.turn_id, run.config.keep_recent_tokens);
    let net_savings: u64 = candidates.iter().map(|c| c.net_tokens).sum();
    if net_savings >= crate::mask_savings_floor(usable_tokens) {
        reclaimed_tokens = net_savings;
        staged_masks = candidates.into_iter().map(|c| {
            SessionEvent::ToolResultMasked { /* bytes_freed: c.bytes, ... */ }
        }).collect();
        // staged only — NOTHING pushed to log or pending_events here
    }
}
let post_mask_estimate = pre_mask_estimate.saturating_sub(reclaimed_tokens);
```

Then the short-circuit, summary-path only:

```rust
if !native_selected && post_mask_estimate fits under threshold {
    // mask-only SUCCESS: commit staged masks explicitly
    for event in staged_masks { push_native_session_event(run.log, run.pending_events, event); }
    append_pending_native_session_events(run.store, run.pending_events)
        .map_err(|_| ProviderRoundError::ToolContinuation("compaction_persist_failed".into()))?;
    return Ok(CompactionApplication::Masked { reclaimed_tokens });
}
```

On the continuing path (summary or native), staged masks ride along as a
local overlay: the summary request's serialized input is built with the
overlay map; the checkpoint and native work proceed unchanged. Only after
the summary (and native artifact, when selected) has succeeded — at the
same point the checkpoint is pushed today (~4812) — push staged masks,
then the checkpoint, then let the existing single persist call flush
both. On every `NotApplied`/error return, `staged_masks` drops with the
frame: nothing in the log, nothing pending, invariant holds.

`native_selected` is already computed in the function (dispatch logic
~4652). The native dispatch decision and trigger use `pre_mask_estimate`
— never the post-mask estimate. Add `Masked` to `CompactionApplication`
with its status string ("context masked (N tokens reclaimed)") and refill
accounting arms (post-mask estimate; no checkpoint).

- [ ] **Step 4: Run tests to verify they pass**

Run: `just dev cargo test -p yach-backend mask_` and `just dev cargo test -p yach-backend compaction`
Expected: PASS, no slice-1 regressions.

- [ ] **Step 5: Commit**

```bash
jj describe -m "feat: mask old tool results inside the compaction transaction"
jj new
```

---

### Task 4: Loop-level masking tests (fake provider)

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` test module (existing compaction loop tests ~3641 area)

**Interfaces:**
- Consumes: Tasks 1–3.

- [ ] **Step 1: Write the failing tests**

Four tests, following the existing fake-provider compaction test patterns in `runner.rs`:

1. `threshold_mask_alone_reclaims_enough_skips_summary_call`: drive estimate over threshold with large old results and small recent tail; assert `Masked`, no summary request observed by the fake provider, mask events persisted, meter estimate dropped.
2. `mask_then_still_over_summarizes_with_masked_input`: candidates exist but post-mask still over; assert summary ran and its serialized prompt contains the mask marker line, not the original bodies.
3. `masking_disabled_preserves_slice1_behavior`: `masking: false` → byte-identical checkpoint behavior to current slice-1 tests.
4. `native_selected_runs_compaction_despite_sufficient_masking`: native-supported provider config, pre-mask estimate over threshold, masking would suffice locally → native compaction still runs (`Native`, not `Masked`).
5. `failed_summary_after_staged_masks_leaves_no_trace`: fake provider fails the summary request after candidates were staged; assert `log.events == original_events`, `pending_events.is_empty()`, and no `tool_result_masked` line on disk — the no-orphan invariant (mirror the existing test at runner.rs:24735 which asserts exactly this shape for the checkpoint path).

- [ ] **Step 2: Run tests to verify they fail**

Run: `just dev cargo test -p yach-backend threshold_mask` (and the other three names)
Expected: FAIL (no `Masked` application, no mask events).

- [ ] **Step 3: Minimal implementation fixes**

Only if the loop wiring (status events, meter update, pending-event persistence on the `Masked` path) is incomplete. Keep to what the tests demand.

- [ ] **Step 4: Run tests to verify they pass**

Run: `just dev cargo test -p yach-backend compaction` (full compaction suite)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj describe -m "test: loop coverage for masking short-circuit and native pre-mask rule"
jj new
```

---

### Task 5: Eval coverage + outcome accounting

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` (outcome document assembly: add `masked_results`/`masked_bytes` to the compaction/checkpoint accounting where `CompactionCheckpoint` details are recorded ~4815)
- Modify: `evals/tasks/compaction-continuation/tests/test.sh` (assert masking evidence when the fixture is masking-eligible) and possibly `evals/tasks/compaction-continuation/fixture/` sizing so masking actually triggers
- Test: `just eval-validate` + targeted gate run

- [ ] **Step 1: Write the failing assertion**

Extend `compaction-continuation/tests/test.sh` to assert the final outcome document (or session log) shows mask evidence when the run masked: e.g. `jq -e` over the session log for a `tool_result_masked` event when `masked_bytes > 0`, and that `answer.txt` still verifies. First check what the current task fixture guarantees (does the current fixture even cross the masking floor? measure; if not, this step is a verifier-only assertion that masking metadata is present when produced, not a forced trigger).

- [ ] **Step 2: Run to verify it fails**

Run: `just eval-validate` (oracle path must still pass — the oracle's synthetic outcome needs the new fields if the verifier requires them) and a live `eval-gate` run of `compaction-continuation` if credentials are available; otherwise rely on unit coverage and mark live confirmation as owner-run.
Expected: the new assertion fails until outcome accounting lands.

- [ ] **Step 3: Write minimal implementation**

Record `masked_results` (count) and `masked_bytes` into the `CompactionCheckpoint` `details` object when a transaction masks (alongside existing details) and surface them in the outcome document's per-turn compaction accounting if such a field path exists; otherwise the session-log assertion suffices.

- [ ] **Step 4: Run verification**

`just eval-validate` green; `just test -p yach-backend` green; `just lint` clean.

- [ ] **Step 5: Commit**

```bash
jj describe -m "feat: record masking evidence in compaction accounting"
jj new
```

---

### Task 6: Final review, measurement, and board update

**Files:**
- Modify: `docs/project/board.md` (slice 2 item → MEASURED), `docs/project/next.md` (next move)
- Create: `docs/project/records/2026-08-XX-masking-slice2-measurement.md`

- [ ] **Step 1: Full verification**

`just fmt-check && just lint && just test && just eval-validate`. Dispatch final reviewer over the whole stack (`jj log -r 'main..@'`).

- [ ] **Step 2: Measurement**

With masking on, run a masking-eligible session (large early reads + long tail) and record: pre-mask estimate, reclaimed tokens, whether summary was skipped, continuation quality. Compare against the same session with `compaction.masking = false`. Write the record.

- [ ] **Step 3: Board/next update**

Mark slice 2 MEASURED with the record link; move next.md to the next queue item.

- [ ] **Step 4: Final checkpoint**

```bash
jj describe -m "docs: masking slice 2 measured"
jj new
```
