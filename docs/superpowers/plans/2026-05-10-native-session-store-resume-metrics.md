# Native Session Store Resume Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make native sessions resumable across restarts through an append-only JSONL store seam and record the first low-frequency native-session metrics baseline.

**Architecture:** `NativeSessionLog` remains the in-memory projection over canonical session events. Runtime persistence moves behind `NativeJsonlSessionStore`, which appends events and can later be swapped for an indexed, snapshot, SQLite, or binary-backed store without changing runner semantics. Metrics start as summarized session evidence only; high-frequency telemetry should not be written into the transcript log in this slice.

**Tech Stack:** Rust 2024, Tokio, Serde JSONL, Criterion, `just` recipes.

---

## Scope

This replaces `docs/superpowers/plans/2026-05-10-native-session-resume-metrics-baseline.md` for execution. It incorporates the storage feedback that full-file JSONL rewrites are likely to fail before JSON parsing itself becomes a bottleneck.

This plan does not add read/search tools, file edits, verification actions, branch/fork sessions, MCP, or extensions.

## Files

- Modify `crates/yach-backend/src/session.rs`: projection helpers plus low-frequency metric event types.
- Create `crates/yach-backend/src/session_store.rs`: append-only JSONL session store and event sink trait.
- Modify `crates/yach-backend/src/lib.rs`: export the store and add backend tests.
- Modify `crates/yach-backend/src/native_runner.rs`: load through the store, batch newly-created prompt events, append only those events, and build provider requests from resumed transcript context.
- Modify `crates/yach-cli/src/main.rs`: runner integration tests for restart/resume and runtime metric evidence.
- Modify `crates/yach-bench/Cargo.toml`: add backend dependency and native-session benchmark target.
- Create `crates/yach-bench/benches/native_session.rs`: benchmark append, load, and projection.
- Modify `docs/project/state.md` and `docs/project/next.md`: update active planning state after implementation.

## Task 1: Session Projections and Metric Event Types

**Files:**
- Modify `crates/yach-backend/src/session.rs`
- Modify `crates/yach-backend/src/lib.rs`

- [ ] Add tests in `crates/yach-backend/src/lib.rs` proving:
  - `NativeSessionLog::next_turn_index()` returns one greater than the highest `turn-N` seen in transcript, tool, turn-finished, or metric events.
  - `NativeSessionLog::last_entry_id()` returns the most recent transcript entry ID.
  - `NativeSessionLog::transcript_messages()` returns only transcript entries in order.
  - `NativeSessionLog::record_duration_metric(...)` writes and reloads a `metric_recorded` JSONL event without raw high-frequency samples.

Use concrete test names:

```rust
native_session_resume_projection_derives_next_ids_and_transcript
native_session_log_preserves_metric_records_jsonl
```

- [ ] Run failing tests:

```bash
just dev cargo test -p yach-backend native_session_resume_projection_derives_next_ids_and_transcript -- --exact
just dev cargo test -p yach-backend native_session_log_preserves_metric_records_jsonl -- --exact
```

- [ ] Implement these public types in `session.rs`:

```rust
pub struct NativeTranscriptMessage {
    pub role: NativeRole,
    pub text: String,
}

pub struct NativeMetricAttribute {
    pub key: String,
    pub value: String,
}

pub struct NativeDurationMetric {
    pub name: String,
    pub duration_ms: u64,
    pub attributes: Vec<NativeMetricAttribute>,
}
```

Derive `Debug, Clone, PartialEq, Eq, Serialize, Deserialize` for each.

- [ ] Add `NativeSessionEvent::MetricRecorded { session_id, turn_id, metric }`.

- [ ] Add `NativeSessionLog` methods:

```rust
pub fn next_turn_index(&self) -> u64
pub fn last_entry_id(&self) -> Option<NativeEntryId>
pub fn transcript_messages(&self) -> Vec<NativeTranscriptMessage>
pub fn record_duration_metric(
    &mut self,
    session_id: NativeSessionId,
    turn_id: Option<NativeTurnId>,
    name: impl Into<String>,
    duration: std::time::Duration,
    attributes: Vec<NativeMetricAttribute>,
)
```

- [ ] Update all exhaustive `NativeSessionEvent` matches to handle `MetricRecorded`; transcript, message, stats, and first-message projections must ignore metrics.

- [ ] Run and commit:

```bash
just dev cargo test -p yach-backend native_session
git add crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add native session projections and metrics"
```

## Task 2: Append-Only JSONL Store

**Files:**
- Create `crates/yach-backend/src/session_store.rs`
- Modify `crates/yach-backend/src/lib.rs`

- [ ] Add a backend test named `native_jsonl_session_store_appends_events_without_rewriting_log` proving a seeded log can be appended to, reloaded, and projected to the next turn index.

- [ ] Run the failing test:

```bash
just dev cargo test -p yach-backend native_jsonl_session_store_appends_events_without_rewriting_log -- --exact
```

- [ ] Create `session_store.rs` with:

```rust
pub trait NativeSessionEventSink {
    fn append_event(&self, event: &NativeSessionEvent) -> std::io::Result<()>;
    fn append_events(&self, events: &[NativeSessionEvent]) -> std::io::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeJsonlSessionStore {
    path: std::path::PathBuf,
}

impl NativeJsonlSessionStore {
    pub fn new(path: std::path::PathBuf) -> Self;
    pub fn path(&self) -> &std::path::Path;
    pub fn load(&self) -> std::io::Result<NativeSessionLog>;
}
```

`append_event` must use `OpenOptions::new().create(true).append(true)` and write one serialized JSON line. It must not truncate.

- [ ] Export the module from `crates/yach-backend/src/lib.rs`:

```rust
mod session_store;
pub use session_store::*;
```

- [ ] Run and commit:

```bash
just dev cargo test -p yach-backend native_jsonl_session_store_appends_events_without_rewriting_log -- --exact
git add crates/yach-backend/src/session_store.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add append-only native session store"
```

## Task 3: Runner Resume and Append-Only Persistence

**Files:**
- Modify `crates/yach-backend/src/native_runner.rs`
- Modify `crates/yach-cli/src/main.rs`

- [ ] Add runner tests in `crates/yach-cli/src/main.rs`:
  - `native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids`
  - `native_dogfood_loop_persists_prompt_runtime_metrics`

The first seeds `turn-0`, restarts the native runner, submits another prompt, reloads the log, and asserts user turns are `["turn-0", "turn-1"]`. The second uses the fixture prompt helper and asserts the persisted file contains `metric_recorded`, `session_log_load`, and `native_prompt_total`.

- [ ] Run failing tests:

```bash
just dev cargo test -p yach-cli native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids -- --exact
just dev cargo test -p yach-cli native_dogfood_loop_persists_prompt_runtime_metrics -- --exact
```

- [ ] In `run_native_dogfood_loop`, create `NativeJsonlSessionStore` from `config.session_path`, load the existing log through the store, and initialize `turn_index` from `next_turn_index()`.

- [ ] In prompt handling, keep two structures:

```rust
let mut log = store.load().unwrap_or_default();
let mut pending_events: Vec<NativeSessionEvent> = Vec::new();
```

Add a helper:

```rust
fn push_native_session_event(
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    event: NativeSessionEvent,
)
```

Every event created for the current prompt must go through this helper so the resumed projection and append batch stay consistent.

- [ ] Replace runtime persistence with `NativeJsonlSessionStore::append_events(&pending_events)`. Do not call `NativeSessionLog::write_to_file` in `native_runner.rs` runtime paths.

- [ ] Record exactly these low-frequency metric events for each prompt:
  - `session_log_load`, immediately after loading the existing log.
  - `native_prompt_total`, immediately before appending terminal prompt events.

- [ ] Verify no runtime rewrite remains:

```bash
rg -n "write_to_file\\(&session_path|write_to_file\\(session_path" crates/yach-backend/src/native_runner.rs
```

Expected: no matches.

- [ ] Run and commit:

```bash
just dev cargo test -p yach-cli native_dogfood_loop
git add crates/yach-backend/src/native_runner.rs crates/yach-cli/src/main.rs
git commit -m "feat: append native runner session events"
```

## Task 4: Provider Resume Context

**Files:**
- Modify `crates/yach-backend/src/native_runner.rs`

- [ ] Add a backend test named `native_provider_messages_include_resumed_transcript`.

- [ ] Run the failing test:

```bash
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --exact
```

- [ ] Add:

```rust
fn native_provider_messages_from_log(log: &NativeSessionLog) -> Vec<ProviderMessage>
```

It should map `log.transcript_messages()` to provider messages in order.

- [ ] Change `handle_native_provider_prompt` so `ProviderRequest.messages` uses `native_provider_messages_from_log(log)` after the current user entry has been pushed into the projection.

- [ ] Run and commit:

```bash
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --exact
git add crates/yach-backend/src/native_runner.rs
git commit -m "feat: resume native provider transcript context"
```

## Task 5: Native Session Benchmark Baseline

**Files:**
- Modify `crates/yach-bench/Cargo.toml`
- Create `crates/yach-bench/benches/native_session.rs`

- [ ] Add `yach-backend = { path = "../yach-backend" }` and a `native_session` bench target.

- [ ] Create a Criterion benchmark with functions:
  - `native_session_append_event`
  - `native_session_load_10_turns`, `native_session_load_100_turns`, `native_session_load_1000_turns`
  - `native_session_projection_10_turns`, `native_session_projection_100_turns`, `native_session_projection_1000_turns`

Do not add a rewrite benchmark as the primary baseline; runtime rewrite is no longer the intended path.

- [ ] Run and commit:

```bash
just dev cargo test -p yach-bench --bench native_session --no-run
git add crates/yach-bench/Cargo.toml crates/yach-bench/benches/native_session.rs
git commit -m "bench: add native session append baseline"
```

## Task 6: Project Docs and Verification

**Files:**
- Modify `docs/project/state.md`
- Modify `docs/project/next.md`

- [ ] Update active project state to mention the accepted Native MVP definition and append-only native session baseline.

- [ ] Update next work to point to read/search/context tools as the next Native MVP slice.

- [ ] Run final verification:

```bash
git status --short --branch
just fmt
just test
just lint
just dev cargo test -p yach-bench --bench native_session --no-run
just dev cargo bench -p yach-bench --bench native_session -- --sample-size 10
```

- [ ] Commit docs:

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update native mvp next work"
```

## Final Verification

Before completion, use `superpowers:verification-before-completion` and confirm:

```bash
git status --short --branch
just fmt
just test
just lint
rg -n "write_to_file\\(&session_path|write_to_file\\(session_path" crates/yach-backend/src/native_runner.rs
just dev cargo test -p yach-bench --bench native_session --no-run
just dev cargo bench -p yach-bench --bench native_session -- --sample-size 10
```

Expected:

- Runtime native runner paths have no full-session rewrite calls.
- Tests, lint, benchmark compile, and short benchmark sample pass.
- The benchmark output includes append, load, and projection measurements.

## Self-Review

- Spec coverage: covers session resume and first metrics/benchmarking baseline only; read/search, autonomous tools, edits, verification actions, and extensions remain separate Native MVP slices.
- Storage feedback coverage: runtime persistence is append-only JSONL behind a store seam; no runtime rewrite benchmark is positioned as the desired path.
- Metrics scope: only low-frequency summary events are stored in the session log.
