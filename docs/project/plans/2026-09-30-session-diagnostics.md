# Session Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a dogfood session diagnosable from its durable evidence: when
events happened, how long each tool call and provider attempt took, why an
attempt failed, and (opt-in) the exact request sent to the model, with a
`yach sessions` inspector.

**Architecture:**

- Both session JSONL writers (`JsonlSessionStore::write_events` and
  `SessionLog::write_to_file`) stamp every line with `at_ms` through one
  borrowed `#[serde(flatten)]` encoder that skips `Unknown`; `SessionEvent`
  itself is not stamped.
- `ToolExecutionFinished` gains source-measured `started_at_ms`/`duration_ms`
  on every exit after dispatch starts, including cancellation.
- A new `SessionEvent::ProviderAttemptFinished` records every started
  attempt: turn and portable-summary compaction attempts inside
  `provider_request_with_retry_context`, native compaction around
  `compactor.compact`. Attempt events are appended to the store directly as
  they settle, so failure exits cannot drop them. Transport facts (request
  ID, first-event time, capture file) come from a per-attempt
  `AttemptRecorder` filled by a Yach-owned `RecordingHttpClient` that Rig
  clients use through `ClientBuilder::http_client`, and by the native
  compactor. The vendored Rig patch does not change.
- `yach sessions list|show` reads stamped JSONL and renders a per-turn
  timeline, text or `--json`.

**Tech Stack:** Rust 2024 workspace. `yach-backend` (session model, store,
runner, adapter), `yach` CLI crate at `crates/yach-cli`, vendored
`rig-core` 0.41 (`rig::http_client`), serde/serde_json 1.0.228, reqwest 0.13,
tokio.

**Spec:** `docs/project/specs/2026-09-30-session-diagnostics-design.md`

**Source:** plane:YACH-15

## Global Constraints

- Workspace clippy denies `unwrap_used`, `expect_used`, `panic`,
  `print_stdout`, `print_stderr`, `dbg_macro`, `todo`, `await_holding_lock`.
  Tests follow neighbouring tests (`assert!(x.is_ok()); let Ok(x) = x else { return; };`
  or the crate's `test_unwrap()` helper). Write output through `io::Write`.
- Never hold a `std::sync::Mutex` guard across `.await`.
- Session JSONL files stay `0600`, directories `0700`.
- Raw provider bodies, headers, endpoints, and credentials never enter session
  JSONL, protocol frames, `ProviderError`, or status output. The only
  header-derived value persisted is the bounded provider request ID.
- Request ID: from `x-request-id`, else `request-id`; ≤ 128 bytes; ASCII
  alphanumerics and `-_.:` only; otherwise dropped whole, never truncated.
- Capture: `YACH_CAPTURE_REQUESTS=<absolute dir>`; files
  `<dir>/<session-id>/<turn-id>-<purpose>-<attempt_sequence>.json`, opened
  with `create_new`; relative path rejected; failure disables capture for the
  process and warns once.
- Retry policy and error classification outcomes do not change.
- Run commands through the dev shell: `just dev cargo test -p <crate> <filter>`
  (`yach-backend`, `yach`). Final gate: `just lint` and `just test`.
- Commit each task with `jj commit <paths> -m "<message>"` naming the files the
  task touched; never a bare `jj commit`.

---

### Task 1: Stamp session lines and tolerate unknown events

**Files:**
- Modify: `crates/yach-backend/src/session.rs` (enum `SessionEvent`, `SessionLog::load_from_file_with_warnings`, `SessionLog::write_to_file`, fn `event_turn_id`, tests module)
- Modify: `crates/yach-backend/src/session_store.rs` (fn `JsonlSessionStore::write_events`)
- Modify: `crates/yach-backend/src/compaction.rs` (fns `estimate_event_tokens`, `serialize_events_for_summary_with_masks`)
- Modify: `crates/yach-backend/src/runner.rs` (fn `provider_messages_from_event_slice`)
- Modify: `crates/yach-backend/src/runner/session_state.rs` (the three exhaustive matches: event projection loop near the top of the file, `send_native_session_stats_with_estimate`, `session_first_message`)

**Interfaces:**
- Produces: `pub fn unix_ms_now() -> Option<u64>`;
  `pub struct StampedSessionEvent { pub at_ms: Option<u64>, pub event: SessionEvent }`;
  `pub struct StampedLoadResult { pub events: Vec<StampedSessionEvent>, pub warnings: Vec<SessionLoadWarning> }`;
  `SessionLog::load_stamped_from_file(path: &Path) -> io::Result<StampedLoadResult>`;
  `SessionEvent::Unknown` (unit, `#[serde(other)]`, never written).

- [ ] **Step 1: Write the failing tests** in the `session.rs` tests module.

```rust
#[test]
fn store_stamps_each_line_and_plain_loader_still_reads_it() {
    let path = test_jsonl_path("stamp");
    let store = crate::JsonlSessionStore::new(path.clone());
    let event = SessionEvent::TurnFinished {
        session_id: SessionId(String::from("s")),
        turn_id: TurnId(String::from("turn-0")),
        outcome: TurnOutcome::Completed,
        reason: None,
    };
    assert!(store.append_events_without_sync(std::slice::from_ref(&event)).is_ok());

    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(raw.trim()).unwrap_or_default();
    assert!(value.get("at_ms").and_then(serde_json::Value::as_u64).is_some());
    assert_eq!(value.get("type").and_then(serde_json::Value::as_str), Some("turn_finished"));

    let plain = SessionLog::load_from_file_with_warnings(&path);
    assert!(plain.is_ok());
    let Ok(plain) = plain else { return; };
    assert!(plain.warnings.is_empty());
    assert_eq!(plain.log.events, vec![event.clone()]);

    let stamped = SessionLog::load_stamped_from_file(&path);
    assert!(stamped.is_ok());
    let Ok(stamped) = stamped else { return; };
    assert_eq!(stamped.events.len(), 1);
    assert!(stamped.events[0].at_ms.is_some());
    assert_eq!(stamped.events[0].event, event);
}

#[test]
fn unknown_event_type_loads_silently_but_malformed_line_warns() {
    let path = test_jsonl_path("unknown");
    let lines = concat!(
        "{\"type\":\"some_future_event\",\"session_id\":\"s\",\"x\":1}\n",
        "{not json\n",
        "{\"type\":\"turn_finished\",\"session_id\":\"s\",\"turn_id\":\"turn-0\",\"outcome\":\"completed\",\"reason\":null}\n",
    );
    assert!(std::fs::write(&path, lines).is_ok());
    let loaded = SessionLog::load_from_file_with_warnings(&path);
    assert!(loaded.is_ok());
    let Ok(loaded) = loaded else { return; };
    assert_eq!(loaded.warnings.len(), 1);
    assert!(matches!(loaded.log.events.as_slice(), [SessionEvent::TurnFinished { .. }]));

    let stamped = SessionLog::load_stamped_from_file(&path);
    assert!(stamped.is_ok());
    let Ok(stamped) = stamped else { return; };
    assert_eq!(stamped.warnings.len(), 1);
    assert_eq!(stamped.events.len(), 1);
    assert_eq!(stamped.events[0].at_ms, None);
}
```

Both tests use this helper in the tests module, following the existing
`std::env::temp_dir()` convention (`yach-backend` has no `tempfile`
dependency); remove the file at the end of each test as the neighbouring
round-trip tests do:

```rust
fn test_jsonl_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "yach-{label}-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ))
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend session::tests::store_stamps_each_line`
Expected: compile error (`load_stamped_from_file` undefined) or assertion on
missing `at_ms`.

- [ ] **Step 3: Implement.** In `session.rs`:

```rust
/// Wall-clock Unix milliseconds; `None` if the clock is before the epoch.
#[must_use]
pub fn unix_ms_now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

/// One session JSONL line as written by the store: the event plus the wall
/// clock at write time. Pre-stamp logs load with `at_ms = None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StampedSessionEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    #[serde(flatten)]
    pub event: SessionEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampedLoadResult {
    pub events: Vec<StampedSessionEvent>,
    pub warnings: Vec<SessionLoadWarning>,
}
```

Add the last variant of `SessionEvent`:

```rust
    /// A line whose `type` this build does not know. Loaded, never written.
    #[serde(other)]
    Unknown,
```

In `load_from_file_with_warnings`, drop `SessionEvent::Unknown` after parsing
(`Ok(SessionEvent::Unknown) => {}`) so callers never see it. Add
`load_stamped_from_file`, mirroring the same loop but parsing
`StampedSessionEvent` and skipping `event: SessionEvent::Unknown`.

Add the shared encoder in `session.rs` and use it from both writers:

```rust
#[derive(Serialize)]
struct StampedEventRef<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    at_ms: Option<u64>,
    #[serde(flatten)]
    event: &'a SessionEvent,
}

/// Appends one stamped JSONL line per event; skips `Unknown`. One clock read
/// per call, so events written together share `at_ms`.
pub(crate) fn encode_stamped_lines<'a>(
    buffer: &mut Vec<u8>,
    events: impl IntoIterator<Item = &'a SessionEvent>,
) -> io::Result<()> {
    let at_ms = unix_ms_now();
    for event in events {
        if matches!(event, SessionEvent::Unknown) {
            continue;
        }
        serde_json::to_writer(&mut *buffer, &StampedEventRef { at_ms, event })
            .map_err(io::Error::other)?;
        buffer.push(b'\n');
    }
    Ok(())
}
```

`JsonlSessionStore::write_events` replaces its per-event loop with
`crate::encode_stamped_lines(&mut buffer, events)?;`. `SessionLog::write_to_file`
encodes `&self.events` into one buffer the same way and writes it with a
single `write_all` before the existing `flush`/`sync_data`.

Add `SessionEvent::Unknown` arms to every exhaustive match (none has a
wildcard): `event_turn_id` → `None`; `estimate_event_tokens` → `0`;
`serialize_events_for_summary_with_masks` → the ignored-event group;
`provider_messages_from_event_slice` → empty vector; the three
`runner/session_state.rs` matches → no effect / `None`. `cargo check` lists any
match missed.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend session::tests`
Expected: PASS, including the existing round-trip tests.

- [ ] **Step 5: Run the backend suite**

Run: `just dev cargo test -p yach-backend`
Expected: PASS. Tests comparing raw JSONL text must now allow `at_ms`; fix
them by parsing the line and comparing `type`/fields, not by removing stamps.
Add one test that `write_to_file` output carries `at_ms` on every line and
that a log holding `SessionEvent::Unknown` writes no line for it.

- [ ] **Step 6: Commit**

```bash
jj commit crates/yach-backend/src -m "Stamp session JSONL lines and tolerate unknown events"
```

### Task 2: Record tool execution timing

**Files:**
- Modify: `crates/yach-backend/src/session.rs` (`ToolExecutionFinished`, new timing types)
- Modify: `crates/yach-backend/src/tools.rs` (generic workflow loop calling `self.executor.execute`; `record_tool_validation_result`)
- Modify: `crates/yach-backend/src/runner.rs` (builtin dispatch calling `batch.read_only_executor.execute`; extension dispatch calling `extension_executor.execute_with_resources`; `record_native_bash_finished_event` and its two callers; `record_missing_provider_tool_batch_events`)
- Modify: `crates/yach-backend/src/agent_edit_tools.rs` (finish-event construction sites, including helper `finished_event`)
- Modify: `crates/yach-backend/src/request_assembly.rs` (`build_fixture_log`)
- Modify: every other struct-literal `SessionEvent::ToolExecutionFinished { … }` without `..` (tests included; `cargo check` finds them)

**Interfaces:**
- Consumes: `unix_ms_now()` (Task 1).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolTiming {
    pub started_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolTimer {
    started_at_ms: Option<u64>,
    started: std::time::Instant,
}

impl ToolTimer {
    #[must_use]
    pub fn start() -> Self {
        Self { started_at_ms: unix_ms_now(), started: std::time::Instant::now() }
    }

    #[must_use]
    pub fn stop(&self) -> ToolTiming {
        ToolTiming {
            started_at_ms: self.started_at_ms,
            duration_ms: Some(
                u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            ),
        }
    }
}
```

`ToolExecutionFinished` gains, after `result_content`:

```rust
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_at_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
```

- [ ] **Step 1: Write the failing tests.** In the `lib.rs` tests that drive
  the generic workflow (the ones asserting `Some(SessionEvent::ToolExecutionFinished { … })`
  for success and error), add a success case asserting timing is present:

```rust
let finished = log.events.iter().find_map(|event| match event {
    SessionEvent::ToolExecutionFinished { started_at_ms, duration_ms, .. } => {
        Some((*started_at_ms, *duration_ms))
    }
    _ => None,
});
assert!(matches!(finished, Some((Some(_), Some(_)))));
```

Add the same assertion to one builtin runner test and one extension runner
test (the extension invoker-fake tests in `runner.rs` that assert output and
evidence), plus a validation-failure test asserting `(None, None)`. Add a bash
test in `runner.rs` that runs a slow command (e.g. `sleep 5`) through
`execute_native_provider_bash_tool_request`, cancels the batch's
cancellation token after the spawn, and asserts the session log holds one
`ToolExecutionFinished` for that request with `outcome = Cancelled` and
`(Some(_), Some(_))` timing, not a timing-less fallback event. Where the
review flow is scriptable, add a bash test with a scripted approval delay
asserting `duration_ms` is below that delay.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend tool_`
Expected: compile errors on the new fields, then assertion failures.

- [ ] **Step 3: Implement.** At each dispatch point take the timer
  immediately before the call and stop it immediately after it returns,
  before matching the result:

```rust
let timer = crate::ToolTimer::start();
let raw_execution = self.executor.execute(self.registry, &request, &validation);
let timing = timer.stop();
let execution = match raw_execution {
    Ok(execution) => execution,
    Err(error) => {
        log.push(SessionEvent::ToolExecutionFinished {
            // existing fields …
            started_at_ms: timing.started_at_ms,
            duration_ms: timing.duration_ms,
        });
        return Err(ToolContinuationError::Execution(error));
    }
};
```

Apply the same shape to the builtin dispatch (`batch.read_only_executor.execute`)
and the extension dispatch (between the existing `extension_invoke_start` and
`extension_invoke_end` marks: start before `execute_with_resources`, stop right
after). Every finish event after a dispatch in that function, including
result-budget, sensitive-denied, and edit-proposal-mismatch failures, uses the
same `timing`.

`record_native_bash_finished_event` gains a `timing: ToolTiming` parameter.
Both calls live in `execute_native_provider_bash_tool_request`: the
`finish_failed` closure (spawn and setup failures) and the final record after
the command finishes. Start one `ToolTimer` immediately before
`crate::CommandExecutor::run(…)`, after the approval or review decision has
resolved, so approval waits never count. Failures before that point pass
`ToolTiming::default()`; failures after it pass `timer.stop()`. In the
cancellation arm of the `tokio::select!` around `run_and_forward`, record a
`ToolOutcome::Cancelled` finish event with `timer.stop()` and a cancelled
`ProviderToolResult` before returning `ProviderRoundError::Cancelled`, so the
batch fallback finds the existing event and keeps it.

`record_missing_provider_tool_batch_events` keeps its current behaviour: it
only creates a `ToolExecutionFinished` when none exists, and that fallback
event has `None` timing because its tool never started (or its evidence was
already lost). Other sites that never dispatched (validation failure in
`record_tool_validation_result`, missing extension executor, review-rejected
and human-performs paths in `agent_edit_tools.rs`, `build_fixture_log`) set
both fields to `None`. In `agent_edit_tools.rs`, pass a `ToolTiming` into
`finished_event` from each caller; time the edit application at the call that
applies it, not the review wait. A result rejected as too large after
execution keeps the real timing; only rejections before dispatch get `None`.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src -m "Record tool execution timing in session evidence"
```

### Task 3: Record provider attempts

**Files:**
- Modify: `crates/yach-backend/src/session.rs` (new variant and types; `event_turn_id`)
- Modify: `crates/yach-backend/src/provider.rs` (`ProviderErrorMetadata`)
- Modify: `crates/yach-backend/src/error_dialect.rs` (set the variant label; expose `generic_status_kind` as `pub(crate)`)
- Modify: `crates/yach-backend/src/runner.rs` (`ProviderRequester`, `ProviderRetryContext`, `provider_request_with_retry_context`, its call sites: turn loop in `run_native_provider_one_agent_tool_round`, portable summary in `run_compaction_with_sequence`, test wrapper `provider_request_with_retry`, and the direct test calls; the native `compactor.compact` call in `run_compaction_with_sequence`; `provider_messages_from_event_slice`)
- Modify: `crates/yach-backend/src/compaction.rs` (`estimate_event_tokens`, `serialize_events_for_summary_with_masks`), `crates/yach-backend/src/runner/session_state.rs` (exhaustive match arms)

**Interfaces:**
- Consumes: `unix_ms_now()` (Task 1).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptPurpose { Turn, CompactionSummary, CompactionNative }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttemptOutcome { Succeeded, Partial, Failed, Cancelled }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttemptSummary {
    pub purpose: ProviderAttemptPurpose,
    pub attempt_sequence: u64,
    pub retry_index: u8,
    pub outcome: ProviderAttemptOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<crate::ProviderErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification_source: Option<crate::ClassificationSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_phase: Option<crate::TimeoutPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_delay_ms: Option<u64>,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_event_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<String>,
}

// SessionEvent variant (flat JSON shape per spec):
ProviderAttemptFinished {
    session_id: SessionId,
    turn_id: TurnId,
    #[serde(flatten)]
    attempt: ProviderAttemptSummary,
},

// runner.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttemptDiagnostics {
    pub provider_request_id: Option<String>,
    pub first_event_ms: Option<u64>,
    pub capture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptLabel {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub purpose: ProviderAttemptPurpose,
    pub attempt_sequence: u64,
}

/// Where settled attempts go. With a store, each event is appended directly
/// (never through the pending batch, so failure exits cannot drop it);
/// without one it joins log + pending like any other event.
pub(crate) struct AttemptSink<'a> {
    pub session_id: &'a SessionId,
    pub purpose: ProviderAttemptPurpose,
    pub log: &'a mut SessionLog,
    pub pending_events: &'a mut Vec<SessionEvent>,
    pub store: Option<&'a JsonlSessionStore>,
}

impl AttemptSink<'_> {
    pub(crate) fn record(&mut self, turn_id: TurnId, attempt: ProviderAttemptSummary) {
        let event = SessionEvent::ProviderAttemptFinished {
            session_id: self.session_id.clone(),
            turn_id,
            attempt,
        };
        self.log.push(event.clone());
        if let Some(store) = self.store {
            let _ = store.append_event(&event);
        } else {
            self.pending_events.push(event);
        }
    }
}

// New ProviderRequester default methods (existing fakes keep compiling):
fn begin_attempt(&mut self, label: AttemptLabel) { let _ = label; }
fn take_attempt_diagnostics(&mut self) -> AttemptDiagnostics { AttemptDiagnostics::default() }

// ProviderRetryContext gains:
attempts: Option<AttemptSink<'a>>,
```

`ProviderErrorMetadata` gains `#[serde(skip)] pub error_variant: Option<&'static str>`
(skipped: protocol output unchanged; include it in the all-empty check next to
`classification_source_is_variant`). `error_dialect.rs` sets it to the
`completion_variant` label where it builds the metadata. There are five
`ProviderErrorMetadata { … }` literals (`rig_adapter.rs` ×2, `provider.rs` ×2,
`error_dialect.rs` ×1); the type derives `Default`.

- [ ] **Step 1: Write the failing tests** in the `runner.rs` tests module,
  modelled on `provider_retry_merges_typed_partial_output_without_duplicate_terminal_events`
  and its local `AttemptRequester` (scripted `VecDeque<ProviderStreamAttempt>`).
  Add a helper that runs `provider_request_with_retry_context` with
  `attempts: Some(AttemptSink { session_id: &id, purpose: Turn, log: &mut log, pending_events: &mut pending, store: None })`
  and returns `(result, attempt events from log)`. Cases:

  1. `Complete` with a `Completed` event → one `Succeeded`, `retry_index = 0`,
     `attempt_sequence = 1`, no error fields.
  2. `Partial` with a `Network` error (no text), then `Complete` → `Failed`
     with `error_kind = Some(Network)` and `next_delay_ms = Some(1_000)`, then
     `Succeeded` with `retry_index = 1` and `attempt_sequence = 2`.
  3. Three `ProviderInternal` failures → three `Failed`; the last has
     `next_delay_ms = None`; the result is `Err`.
  4. `Complete` whose events contain `ProviderStreamEvent::Failed` → one
     `Failed` event with that error's kind, not `Succeeded`.
  5. `Complete` with text but no `Completed` event → `Failed`,
     `error_kind = Some(MalformedStream)`.
  5a. `Partial { tool_round_complete: true, .. }` with a transient error,
     where the complete tool round is used rather than retried (reuse the
     setup of the existing test that terminalizes a completed tool round) →
     one `Partial` event carrying the error's kind.
  6. The requester returns `Err(ProviderError)` directly (make the fake's
     `request_attempt_streaming` return an error) → one `Failed` event before
     the function returns or waits.
  7. Cancellation during an attempt (a fake whose attempt awaits
     `std::future::pending()`; cancel the token) → one `Cancelled` event.
  8. Cancellation during the retry delay (reuse
     `cancellation_during_retry_delay_starts_no_replacement_attempt` setup) →
     exactly one `Failed` event, none for the attempt that never started.
  9. Two calls sharing one `attempt_sequence` counter (a two-round tool turn)
     → sequences strictly increase across calls.
  10. No event text: serialize every recorded event and assert none contains
      the scripted text delta.
  11. With a real `JsonlSessionStore` in the sink: after a failing call the
      attempt events are already in the file although `pending_events` was
      never flushed.

  Compaction tests (drive `run_compaction_with`, like
  `mask_then_still_over_summarizes_with_masked_input`; use the test
  `impl Compactor` fake near those tests):
  - a portable-summary run records one `CompactionSummary` event;
  - native success, native `CompactionError::HttpStatus { status: 503 }`, and
    cancellation during `compact` each record exactly one `CompactionNative`
    event: `Succeeded`; `Failed` with `status_code = Some(503)` and
    `error_kind = Some(ProviderInternal)`; `Cancelled`;
  - with a store, a summary request that fails: after `run_compaction_with`
    returns `Ok(CompactionApplication::NotApplied)`, the attempt event is in
    the file, and `SessionLog::load_from_file` of that file has a
    `next_turn_index` past the compaction turn.

  In `session.rs` tests, add a JSONL round trip for `ProviderAttemptFinished`
  asserting the line has top-level `attempt_sequence` and `purpose` keys
  (flattened) and parses back equal, and that `event_turn_id` returns its
  turn.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend provider_attempt`
Expected: compile errors, then assertion failures.

- [ ] **Step 3: Implement.** In `provider_request_with_retry_context`:

  - The attempt starts immediately before `request_attempt_streaming`, after
    the pre-attempt cancellation check and after a successful
    `PromptAttemptReset` delivery. Keep the reset delivery inside the
    existing `select!` future so a cancellation racing it still skips the
    frame, as today. Declare `let mut start: Option<AttemptStart> = None;`
    before the `select!`. Inside the future, after delivery succeeds, set
    `start = Some(AttemptStart { instant: Instant::now(), at_ms: unix_ms_now().unwrap_or(0) })`,
    call `requester.begin_attempt(AttemptLabel { … })` (current
    `*context.attempt_sequence`, the sink's purpose) when `context.attempts`
    is present, then invoke the requester. The future borrows `start` and
    `requester` mutably; both borrows end when the `select!` completes, so
    `start` is read afterwards without atomics. Settle only when `start` is
    `Some`; a delivery failure or a cancellation before delivery leaves it
    `None` and records nothing.
  - One helper builds and records the summary:
    `fn settle_attempt(context: &mut ProviderRetryContext<'_>, requester: &mut impl ProviderRequester, start: AttemptStart, settle: AttemptSettle)`.
    It calls `requester.take_attempt_diagnostics()` exactly once and
    `sink.record(request.turn_id.clone(), summary)`. No sink → no-op.
  - Derive the outcome from stream facts, never from the `Complete`/`Partial`
    wrapper alone: a `Failed` event → `Failed` with that error; a `Cancelled`
    event → `Cancelled`; `Completed` present → `Succeeded`; otherwise
    `Failed` with `ProviderErrorKind::MalformedStream`. For `Partial`, the
    partial error is the error; a complete tool round that is used is
    `ProviderAttemptOutcome::Partial` and keeps the partial error's fields.
  - Settle every started attempt after the `select!` completes and before
    any return or retry wait. The cancellation arm does not settle inside
    its body (the future's `&mut requester` borrow); it yields
    `Err(ProviderError::cancelled(…))` as the `select!` value, and the code
    after the `select!` settles it as `Cancelled` when `start` is `Some`,
    then returns as today. The same post-`select!` point settles a direct
    `Err` from the requester, `Complete`, each `Partial` branch that returns,
    and the retry branch, before `wait_for_provider_retry`, with
    `next_delay_ms = provider_retry_delay_ms(&error, retry_index, delay_spent_ms)`.
    Settle before the post-delay `attempt_sequence` increment so the event
    names the attempt that failed.
  - Error fields come from `ProviderError.kind` and `.metadata`
    (`classification_source` is stored explicitly even when it is the
    `variant` default). `retry_index` saturates into `u8`. `model` is
    `request.model.model`.

  Call sites:
  - Turn loop (`run_native_provider_one_agent_tool_round`): pass
    `attempts: Some(AttemptSink { session_id, purpose: Turn, log: &mut *log, pending_events: &mut *pending_events, store: tool_event_store })`.
    The reborrows end when the call returns.
  - Portable summary (`run_compaction_with_sequence`): pass
    `AttemptSink { session_id: run.session_id, purpose: CompactionSummary, log: &mut *run.log, pending_events: &mut *run.pending_events, store: run.tool_event_store }`;
    keep `session_id: "compaction"` for status frames.
  - Native compaction: around `compactor.compact(preparation.clone())` in
    `run_compaction_with_sequence`, time the call and record one
    `CompactionNative` event through a sink built the same way on every
    branch: success; unreplayable output → `MalformedStream`; `Err`; and the
    cancellation arm of its `select!` → `Cancelled` before returning. Map
    `CompactionError::Timeout` → `Timeout`, `Transport` → `Network`,
    `HttpStatus { status }` → `generic_status_kind(Some(status))` with
    `status_code`, `Decode`/`InvalidOutput` → `MalformedStream`.
    `UnsupportedProvider` and `MissingNativeRequest` never reach HTTP and
    record nothing.
  - The test wrapper and existing direct test calls pass `attempts: None`
    (behaviour unchanged).

  Add `ProviderAttemptFinished` arms: `event_turn_id` → `Some(turn_id)` (so a
  persisted failed manual compaction advances `next_turn_index` after
  reload; `Unknown` goes in the `None` arm); `turn_scoped_event_turn_id`
  keeps its `_ => None` arm; `estimate_event_tokens` → `0`; summary
  serialization → ignored group; `provider_messages_from_event_slice` →
  empty; `session_state.rs` matches → no transcript effect.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend/src -m "Record provider attempts in session evidence"
```

### Task 4: Recording transport, request IDs, and request capture

**Files:**
- Create: `crates/yach-backend/src/recording_http.rs`
- Modify: `crates/yach-backend/src/lib.rs` (`mod recording_http;`)
- Modify: `crates/yach-backend/Cargo.toml` (add `bytes = "1"`; add `"stream"` to the `reqwest` features)
- Modify: `crates/yach-backend/src/rig_adapter.rs` (`run_provider_request_attempt_with_approved_tools`, `PreparedCompletion`, the OpenAI, OpenAI-compatible, and ChatGPT client construction branches, `collect_rig_completion_stream`)
- Modify: `crates/yach-backend/src/compaction.rs` (`CompactionPreparation` gains `recorder`; `OpenAiResponsesCompactor::compact` records request ID and capture)
- Modify: `crates/yach-backend/src/runner.rs` (`RigProviderRequester`: implement `begin_attempt`/`take_attempt_diagnostics`, pass the recorder; native compaction call site builds a recorder; every `CompactionPreparation { … }` literal, 4 in runner.rs and 7 in compaction.rs tests, plus `crates/yach-cli/src/main.rs` compaction smoke)

**Interfaces:**
- Consumes: `AttemptLabel`, `AttemptDiagnostics`, `ProviderRequester` hooks (Task 3).
- Produces (`recording_http.rs`; everything crate-private except the opaque `AttemptRecorder` type):

```rust
/// Opaque per-attempt recorder. `pub` (re-exported from the crate root)
/// because `CompactionPreparation` is a public struct that `yach-cli`
/// constructs; its fields and methods stay `pub(crate)`.
#[derive(Debug, Clone, Default)]
pub struct AttemptRecorder { state: Arc<Mutex<RecorderState>> }

impl AttemptRecorder {
    pub(crate) fn new(capture_path: Option<PathBuf>) -> Self;
    pub(crate) fn record_response_headers(&self, headers: &http::HeaderMap);
    pub(crate) fn capture_body(&self, body: &[u8]);
    pub(crate) fn mark_first_event(&self);
    pub(crate) fn diagnostics(&self) -> AttemptDiagnostics;
}

/// `Clone + Default + Debug` as Rig's completion models require; the default
/// holds an empty recorder that records nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecordingHttpClient { client: reqwest::Client, recorder: AttemptRecorder }

impl RecordingHttpClient {
    pub(crate) fn new(recorder: AttemptRecorder) -> Self;
}

impl rig::http_client::HttpClientExt for RecordingHttpClient { /* send, send_multipart, send_streaming */ }

pub(crate) fn capture_dir_from_env() -> Option<PathBuf>;
/// `<dir>/<session-id>/<turn-id>-<purpose>-<attempt_sequence>.json`
pub(crate) fn capture_path(dir: &Path, label: &AttemptLabel) -> PathBuf;
pub(crate) fn bounded_request_id(headers: &http::HeaderMap) -> Option<String>;
```

- [ ] **Step 1: Write the failing tests** in `recording_http.rs`, using a local
  `TcpListener` fixture in the style of `local_sse_fixture` in `rig_adapter.rs`
  (accept on a thread, read to `\r\n\r\n` plus content-length, reply with a
  canned response).

```rust
#[test]
fn request_id_is_bounded_and_charset_checked() {
    let mut headers = rig::http_client::HeaderMap::new();
    headers.insert("x-request-id", rig::http_client::HeaderValue::from_static("req_01-ab.c:9"));
    assert_eq!(bounded_request_id(&headers).as_deref(), Some("req_01-ab.c:9"));

    let mut fallback = rig::http_client::HeaderMap::new();
    fallback.insert("request-id", rig::http_client::HeaderValue::from_static("abc"));
    assert_eq!(bounded_request_id(&fallback).as_deref(), Some("abc"));

    let mut bad = rig::http_client::HeaderMap::new();
    bad.insert("x-request-id", rig::http_client::HeaderValue::from_static("a b"));
    assert_eq!(bounded_request_id(&bad), None);

    let long = "a".repeat(129);
    let mut too_long = rig::http_client::HeaderMap::new();
    too_long.insert("x-request-id", rig::http_client::HeaderValue::from_str(&long).unwrap_or_else(|_| rig::http_client::HeaderValue::from_static("")));
    assert_eq!(bounded_request_id(&too_long), None);
}
```

Async tests (follow the async test attribute the `rig_adapter.rs` fixture
tests use):

  1. Success streaming response with `x-request-id: gw-1` → body streams
     unchanged; `recorder.diagnostics().provider_request_id == Some("gw-1")`.
  2. `503` with `request-id: gw-2`, `Retry-After: 2`, JSON body → returns
     `rig::http_client::Error::InvalidStatusCodeWithMessage(503, body, Some(_))`
     with the body text identical to what `reqwest::Client`'s Rig
     implementation returns for the same fixture (run both against the fixture
     and compare), and the recorder holds `gw-2`.
  3. Error body over `rig::http_client::ERROR_BODY_MAX_BYTES` → message equals
     Rig's truncation sentinel and matches Rig's reqwest path on the same
     fixture. Make the fixture send the oversized prefix and then keep the
     connection open without finishing the body; the call must return within
     a short timeout (bounded read drops the response instead of waiting).
  4. With a capture path, the exact request body bytes land in the file with
     mode `0600` under a `0700` directory, and `diagnostics().capture` is the
     file name; without a capture path, no file is written.
  5. A capture path that already exists → the file is not overwritten
     (`create_new`), `diagnostics().capture` is `None`, and the request still
     succeeds; a following attempt with a new path still captures (a
     collision does not disable capture).
  6. Capture into an unwritable directory → request still succeeds, no
     capture recorded, and a second request does not attempt capture
     (process-wide disable flag).
  7. `capture_path` for purposes `Turn` and `CompactionSummary` with the same
     turn and sequence yields different file names.

In `rig_adapter.rs` tests, extend one OpenAI-compatible streaming test that
uses `local_sse_fixture` so the fixture sends `x-request-id`, and assert the
recorder passed to the attempt reports it and a `first_event_ms`. In
`compaction.rs` tests, run `OpenAiResponsesCompactor` against a local fixture
returning `503` with `x-request-id` and assert the recorder in the
preparation holds the request ID and, with a capture path, the body.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend recording_http`
Expected: compile error (module missing).

- [ ] **Step 3: Implement `recording_http.rs`.**

  - `send`/`send_streaming`: split the request (`req.into_parts()`), convert
    the body with `Into<Bytes>`, call `recorder.capture_body(&bytes)`, then
    build and execute a `reqwest` request with the same method, URI, headers,
    and body. Call `recorder.record_response_headers(response.headers())` on
    every response. On a non-success status, read the error body
    incrementally, mirroring Rig's private `read_reqwest_error_body`:

```rust
let status = response.status();
let retry_after = rig::http_client::retry_after_from_headers(response.headers());
let mut chunks: Vec<bytes::Bytes> = Vec::new();
let mut buffered = 0_usize;
let body = loop {
    match response.chunk().await {
        Ok(Some(chunk)) => {
            buffered = buffered.saturating_add(chunk.len());
            chunks.push(chunk);
            // Over the bound: stop reading and drop the response now.
            // `from_chunks` sees the overflowing chunk and yields Rig's
            // truncation sentinel.
            if buffered > ERROR_BODY_MAX_BYTES {
                drop(response);
                break BoundedErrorBody::from_chunks(&chunks);
            }
        }
        Ok(None) => break BoundedErrorBody::from_chunks(&chunks),
        Err(_) => break BoundedErrorBody::from_slice(b""),
    }
};
let message = body.into_string();
return Err(Error::InvalidStatusCodeWithMessage(status, message, retry_after));
```

    Never call `response.bytes()` on an error response, and do not route it
    through `error_from_response` (that awaits the whole body before
    bounding). `BoundedErrorBody::from_chunks` is public and produces Rig's
    exact sentinel on overflow, so no private detail is copied. On success,
    copy status, version,
    and headers into the
    `http::Response`, and map the body (`bytes()` for `send`,
    `bytes_stream()` mapped to `Error::Instance(Box::new(e))` for
    `send_streaming`), as Rig's reqwest implementation in
    `vendor/rig-core/src/http_client/mod.rs` does.
  - `send_multipart`: delegate to `self.client` (not used by completion paths).
  - `capture_body` opens the file with `OpenOptions::new().write(true).create_new(true)`
    and mode `0o600`, after `create_dir_all` plus `set_permissions(0o700)` on
    the session directory. `AlreadyExists` leaves `capture = None` without
    disabling capture. Any other error sets a static `AtomicBool` that
    disables capture for the process and writes one warning with
    `writeln!(std::io::stderr(), …)`.
  - `capture_dir_from_env()` reads `YACH_CAPTURE_REQUESTS`; a relative path
    warns once and returns `None`.
  - `mark_first_event` stores milliseconds since the recorder was created.
  - Never hold the recorder's `Mutex` across `.await`.

  Wire it:

  - `RigProviderRequester` gains `capture_dir: Option<PathBuf>` (from
    `capture_dir_from_env()` at construction) and `recorder: AttemptRecorder`.
    `begin_attempt` replaces `recorder` with
    `AttemptRecorder::new(capture_dir.as_deref().map(|dir| capture_path(dir, &label)))`.
    `take_attempt_diagnostics` returns `recorder.diagnostics()` and resets it.
    `request_attempt_streaming` passes `Some(recorder.clone())` to
    `run_provider_request_attempt_with_approved_tools`.
  - `run_provider_request_attempt_with_approved_tools` gains
    `recorder: Option<AttemptRecorder>`, stored on `PreparedCompletion`; the
    other entry points pass `None`.
  - In the `OpenAi`, `OpenAiCompatible`, and `ChatGptSubscription` branches,
    call `.http_client(RecordingHttpClient::new(recorder))` on the builder
    before `.build()`, using `AttemptRecorder::default()` when none was
    passed. `ChatGPTBuilder`'s `oauth`/`allow_device_flow`/`auth_file` are
    generic over the transport, so ordering does not matter. The `Anthropic`
    branch is unchanged.
  - In `collect_rig_completion_stream`, where `marked_first_event` becomes
    true, also call `recorder.mark_first_event()` when a recorder is present.
  - Native compaction: `CompactionPreparation` gains
    `pub recorder: Option<AttemptRecorder>` (`None` at every existing
    literal, including the `yach-cli` compaction smoke).
    The runner's native call site builds one with the
    `CompactionNative` label and passes it; `OpenAiResponsesCompactor::compact`
    serializes the body with `serde_json::to_vec`, calls `capture_body`,
    sends it with `.body(bytes)` and a JSON content type, and calls
    `record_response_headers` before checking status. The Task 3 native
    attempt event reads `recorder.diagnostics()`. The compactor's own
    `reqwest::Client` stays; only these recorder calls are added.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-backend`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-backend -m "Record provider request IDs and opt-in request capture"
```

### Task 5: `yach sessions` inspector

**Files:**
- Create: `crates/yach-cli/src/sessions.rs`
- Modify: `crates/yach-cli/src/main.rs` (`CliArgs::from_args`, `enum Command`, `Command::run`, `enum CommandResult` + `render_lines` + `exit_code`, `usage_lines`)
- Create: `crates/yach-cli/tests/sessions.rs`

**Interfaces:**
- Consumes: `SessionLog::load_stamped_from_file`, `StampedSessionEvent`,
  `ProviderAttemptSummary`, `ToolExecutionFinished` timing fields,
  `project_session_log_dir` (all `pub` via `yach_backend`).
- Produces (`sessions.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionsCommand {
    List { json: bool },
    Show { id: SessionSelector, json: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionSelector { Latest, Id(String) }

pub(crate) fn sessions_command_from_args(args: &[String]) -> Result<SessionsCommand, String>;
pub(crate) fn run_sessions(command: &SessionsCommand, project_root: &Path) -> Result<Vec<String>, String>;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionListing {
    pub id: String,
    pub started_at_ms: Option<u64>,
    pub modified_at_ms: Option<u64>,
    pub turns: usize,
    pub last_outcome: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionTimeline {
    pub id: String,
    pub warnings: Vec<String>,
    pub turns: Vec<TurnTimeline>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TurnTimeline {
    pub turn_id: String,
    pub started_at_ms: Option<u64>,
    pub prompt: Option<String>,        // first line, at most 120 chars
    pub items: Vec<TimelineItem>,
    pub outcome: Option<String>,
    pub reason: Option<String>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TimelineItem {
    ProviderAttempt { offset_ms: Option<u64>, attempt: yach_backend::ProviderAttemptSummary },
    Tool { offset_ms: Option<u64>, tool_request_id: String, tool_name: Option<String>,
           outcome: String, duration_ms: Option<u64>, arguments: Option<String> },
    Permission { offset_ms: Option<u64>, summary: String },
    Review { offset_ms: Option<u64>, tool_request_id: String, decision: String },
    Compaction { offset_ms: Option<u64>, tokens_before: u64, tokens_after_estimate: u64, reason: String },
}
```

`CommandResult` gains `Sessions { lines: Vec<String>, failed: bool }`;
`render_lines` returns `lines`; `exit_code` returns `1` when `failed`.

- [ ] **Step 1: Write the failing tests** in `crates/yach-cli/tests/sessions.rs`.
  Copy the `TempDir` helper and the local `test_unwrap` trait from
  `crates/yach-cli/tests/presets.rs` (integration tests do not share
  helpers) for the home, project, and session directories. Build a fixture
  directory with two sessions written as stamped JSONL lines
  (hand-written strings: user `entry_appended`, `provider_attempt_finished`
  failed then succeeded with `provider_request_id`, `tool_request_recorded` +
  `tool_execution_finished` with `duration_ms`, assistant `entry_appended`
  with provider usage, `turn_finished`), plus one pre-change session without
  `at_ms`. Invoke the binary as the existing tests do. Like the
  `presets.rs` helper, strip every inherited `YACH_*` variable first, then
  set the fixture's, so the test never reads the developer's
  `~/.yach/sessions`:

```rust
fn sessions_command(home: &Path, sessions: &Path, project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yach"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("YACH_") {
            command.env_remove(&key);
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("YACH_SESSION_DIR", sessions)
        .current_dir(project);
    command
}
```

  Cases:
  1. `yach sessions list --json` → one JSON array, newest first by mtime,
     each with `turns` and `last_outcome`.
  2. `yach sessions show latest --json` → the turn has two
     `provider_attempt` items in order, the failed one carrying the request
     ID, and a `tool` item with `duration_ms`.
  3. `yach sessions show <id>` (text) contains the tool name, `attempt 1`,
     the request ID, and the turn outcome.
  4. The pre-change session renders without offsets and exits `0`.
  5. `yach sessions show missing-id` exits `1` with an `error=` line.
  6. `yach sessions bogus` exits `2` with usage.
  7. A session whose file order differs from measured start order (an
     attempt line written before an earlier-started tool's finish line)
     renders items sorted by timing.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach --test sessions`
Expected: FAIL (unknown command).

- [ ] **Step 3: Implement.**

  - Parser: `Some("sessions") => match sessions::sessions_command_from_args(&positional[1..]) { Ok(command) => Command::Sessions(command), Err(message) => Command::UsageError { message } }`
    (use the existing usage-error variant name in `Command`).
  - `Command::run`: resolve the project root with `std::env::current_dir()`,
    call `sessions::run_sessions`, map `Ok(lines)` / `Err(message)` (as
    `error=<message>`) into `CommandResult::Sessions`.
  - `run_sessions`: `project_session_log_dir(project_root)` → list `*.jsonl`
    entries sorted by mtime descending (ties by path).
    `List` loads each file with `load_stamped_from_file` and summarizes.
    `Show` resolves `Latest` to the newest file or `Id` to
    `session_log_path_in(dir, id)`, loads it, and builds the timeline by
    walking events in order: start a `TurnTimeline` at the first event of a
    new `turn_id`; take the prompt from the user `EntryAppended`; map
    `ProviderAttemptFinished`, `ToolRequestRecorded` (remember name and
    `argument_summary` by `tool_request_id`), `ToolExecutionFinished`,
    `PermissionDecisionRecorded`, `ToolReviewDecisionRecorded`,
    `CompactionCheckpoint`, `TurnFinished`, and assistant provider usage.
    Group by turn ID in first-appearance order. Within a turn, sort items by
    timing (attempt and tool `started_at_ms`, else the line's `at_ms`),
    keeping file order for equal times; an untimed item keeps its file
    position after the preceding timed item. Attempt events are appended
    directly and can precede the batch they belong to in the file, so file
    order alone is wrong. Offsets: that timing minus the turn's earliest
    known timing; `None` when either side is missing or `0`.
  - All output, including `--json`, goes through `CommandResult::Sessions`
    lines and the existing `emit_lines` writer (`print_stdout` is denied).
    JSON is one compact line, `serde_json::to_string(&value)`, matching
    `yach run`'s stdout convention.
  - Text output, one line per item, e.g.:

```text
session s-123  turns=2  warnings=0
turn turn-0  "fix the failing test"
  +0ms      attempt 1 turn failed provider_internal source=variant variant=provider status=503 request=gw-2 next_delay=1000ms 812ms
  +1830ms   attempt 2 turn succeeded first_event=410ms 2210ms request=gw-3
  +4050ms   tool read_file completed 3ms  path=src/lib.rs
  outcome completed  usage input=1234 output=210
```

  An absent `classification_source` renders as `variant`.
  - `usage_lines`: add `yach sessions list [--json]` and
    `yach sessions show <session-id|latest> [--json]`, and `sessions` to the
    commands list.

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach --test sessions`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
jj commit crates/yach-cli -m "Add yach sessions list and show"
```

### Task 6: Documentation, gateway smoke, and dogfood evidence record

**Files:**
- Modify: `README.md` (Quickstart command list, Environment table, "Session logs and privacy")
- Modify: `secretspec.toml`
- Create: `docs/project/records/<run-date>-session-diagnostics-gateway-smoke.md` (the date the smoke runs, `YYYY-MM-DD`)

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Document.** README:
  - Quickstart: `yach sessions list` / `yach sessions show latest [--json]`.
  - Environment table: `YACH_CAPTURE_REQUESTS` — absolute directory; writes
    each provider request body per attempt; private files; contains prompts
    and code; off by default.
  - Session logs and privacy: lines carry `at_ms`; tool and provider-attempt
    timing; provider request IDs from response headers; the inspector; capture
    files are separate from session logs and never contain credentials.

- [ ] **Step 2: Admit the gateway credential.** Add to `secretspec.toml`:

```toml
[profiles.default]
TYPESAFE_API_KEY = { description = "TypeSafe Jev reviewer credential (Iron replace-header placeholder)" }
YACH_RIG_OPENAI_COMPAT_API_KEY = { description = "OpenAI-compatible gateway credential (host subscription gateway)" }

[scopes.gateway]
secrets = ["YACH_RIG_OPENAI_COMPAT_API_KEY"]
```

- [ ] **Step 3: Full gate**

Run: `just lint` then `just test`
Expected: both PASS.

- [ ] **Step 4: Gateway smoke.** Build and install from this checkout
  (`just dev cargo install --locked --path crates/yach-cli --root "$PWD/.devenv/state/smoke"`),
  create a scratch project (`mktemp -d`, `git init`, a small `README.md` and
  one source file), then run from it:

```bash
YACH_RIG_PROVIDER=openai-compatible \
YACH_RIG_OPENAI_COMPAT_BASE_URL=http://omp-subscriptions.home.lan:4000/v1 \
YACH_RIG_OPENAI_COMPAT_MODEL=openai-codex/gpt-6.1-sol \
YACH_CAPTURE_REQUESTS="$SCRATCH/.capture" \
YACH_SESSION_DIR="$SCRATCH/.sessions" \
secretspec run --provider omp-subscriptions --scope gateway \
  --reason "yach session diagnostics gateway smoke" -- \
  "$PWD/.devenv/state/smoke/bin/yach" run --prompt "List the files here, read README.md, and summarize it in one sentence."
```

  Confirm the run used the gateway model (the outcome document names the
  model). `[model.default]` in `~/.yach/config.toml` (currently
  `provider = "openai-codex"`, `model = "gpt-5.6-terra"`) resolves only
  against a Ready connection of the same provider; the env variables above
  create the `openai-compatible` env connection. Prefer
  `yach run --model <YACH_RIG_OPENAI_COMPAT_MODEL value>` if the default
  still wins. If that is not enough, change `[model.default]` to
  `provider = "openai-compatible"` and `model` set to the same value as
  `YACH_RIG_OPENAI_COMPAT_MODEL` (the owner approved this), note the previous
  value in the record, and rerun. Record which mechanism the run actually
  used. Then run `yach sessions show latest` and `--json` with
  the same `YACH_SESSION_DIR`.

  Expected: at least one `provider_attempt` with `provider_request_id`,
  `first_event_ms`, and `capture`; tool items with `duration_ms`; every line
  of the session file carries `at_ms`; one capture file per attempt; no
  credential in any capture file (`grep -i -E 'authorization|bearer|api[-_]key'`
  returns nothing).

- [ ] **Step 5: Record the evidence.** Write the record: date, commit,
  command, model, the `yach sessions show` text output, attempt and tool
  timings observed, capture file count and sizes (not contents), anything
  that did not work, and follow-ups. Keep capture contents and credentials
  out of the record.

- [ ] **Step 6: Commit**

```bash
jj commit README.md secretspec.toml docs/project/records -m "Document session diagnostics and record gateway smoke"
```
