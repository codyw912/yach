# Performance Measurement Framework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give yach a paired same-machine regression gate (`just perf`) over a JSON workload registry, with new core-loop workloads, a bounded turn trace sink, deterministic CI checks, and profiling recipes.

**Architecture:** `yach-bench` gains a `perf` subcommand family: a static workload registry that produces typed rows, a `worker` that measures one side, and a controller (`ab`) that builds `main` and `@` through the declared dev shell, runs interleaved ABBA rounds, applies numeric budgets, and renders Markdown. Private core-loop seams are exposed under the existing `yach-backend` `bench` feature; a new `yach-trace` crate replaces the truncate-rewrite `StartupTrace` with an append-only JSONL sink that both UI and backend write to through a CLI-owned handle.

**Tech Stack:** Rust 2024, `yach-backend` (`bench` feature), `yach-bench`, `yach-trace` (new), `serde`/`serde_json`/`toml`, `libc` (`wait4`), `jj`, `just dev cargo …`, GitHub Actions.

**Spec:** `docs/project/specs/2026-09-08-performance-measurement-framework-design.md`

**Source:** plane:YACH-8

## Global Constraints

- Builds run through the declared environment: `just dev cargo …` from the checkout root. The controller never invokes bare `cargo`.
- Both A/B sides are built `--release --locked`; same `rustc` required, mismatch is a hard error.
- Shipping artifact is exactly `just dev cargo build --release --locked -p yach`; the bench-feature binary goes to `CARGO_TARGET_DIR=target/bench`.
- Workspace clippy lints (`Cargo.toml:15-41`) apply: no `unwrap`/`expect`/`panic`/`print_stdout`/`print_stderr`/`exit` outside `#[cfg(test)]`; `yach-bench` main is the one binary that writes stdout, via `io::stdout().lock()` as `emit_lines` does today (`crates/yach-bench/src/main.rs:115-123`).
- Workload ids are stable strings; existing labels are preserved verbatim (`terminal/idle_keypress_to_draw_flush_live`, `yach/cli_startup_first_output`, etc.).
- `YACH_STARTUP_TRACE` is renamed `YACH_TRACE`; no alias. `*-report` commands are removed; no alias.
- `memory` workloads are Linux-only (`ru_maxrss` × 1024); elsewhere `skipped`/`unsupported_os`.
- Allocation counting: `in_process_serial` only, window scoped to the operation under test; rows `<id>#alloc_count` and `<id>#alloc_bytes` are `count` class.
- `perf ab` exit: 1 on any `error`/`regressed`, 2 on any `inconclusive`, else 0.
- Skip project-wide `just lint`/`just test`/`just fmt` inside tasks; run the focused commands each task names. The final task runs the full suite once.

---

## File Structure

New:

- `crates/yach-trace/{Cargo.toml,src/lib.rs}` — `TraceSink`, `TraceRecord`, `TraceScope`; append-only JSONL writer; parser.
- `crates/yach-bench/src/perf/mod.rs` — `perf` subcommand dispatch (`run`, `worker`, `ab`, `report`).
- `crates/yach-bench/src/perf/schema.rs` — result document types (`ResultDoc`, `WorkloadRow`, `Class`, `Isolation`, `Status`, `BuildInfo`, `HostInfo`), `SCHEMA: u32 = 1`.
- `crates/yach-bench/src/perf/registry.rs` — `Workload` descriptor + static table + derived alloc rows.
- `crates/yach-bench/src/perf/alloc.rs` — counting `#[global_allocator]` and `AllocWindow`.
- `crates/yach-bench/src/perf/rss.rs` — `wait4` peak-RSS sampler with stop boundary.
- `crates/yach-bench/src/perf/worker.rs` — runs the registry for one side, writes `ResultDoc`.
- `crates/yach-bench/src/perf/provenance.rs` — `BuildInfo`/`HostInfo` capture (git HEAD, dirty, source digest, lock hash, binary hash, rustc).
- `crates/yach-bench/src/perf/thresholds.rs` — `perf-thresholds.toml` loader + glob matching.
- `crates/yach-bench/src/perf/verdict.rs` — round aggregation and verdict rules.
- `crates/yach-bench/src/perf/ab.rs` — controller: base materialization, builds, ABBA rounds, external mode, exit codes.
- `crates/yach-bench/src/perf/report.rs` — Markdown rendering.
- `crates/yach-bench/src/perf/workloads/{tui.rs,startup.rs,edit.rs,extension.rs,core_loop.rs,binary.rs}` — registry entries grouped by subsystem; existing sampler bodies move here from `main.rs`.
- `crates/yach-bench/perf-thresholds.toml`.
- `crates/yach-backend/src/bench_loop.rs` — `ScriptedProvider`, `run_scripted_turn`.
- `crates/yach-backend/src/request_assembly.rs` — fixture log builder + `assemble`.
- `docs/benchmarks/baseline-<date>.md` — first Linux baseline (final task).

Modified:

- `Cargo.toml` (workspace members), `justfile`, `.github/workflows/ci.yml`, `.gitignore`.
- `crates/yach-bench/{Cargo.toml,src/main.rs,src/lib.rs}`; `src/startup_trace.rs` deleted.
- `crates/yach-backend/{Cargo.toml,src/lib.rs,src/runner.rs,src/runner/extension_state.rs,src/tools.rs}`.
- `crates/yach-cli/{Cargo.toml,src/main.rs,src/headless.rs}`.
- `crates/yach-ui/{Cargo.toml,src/app.rs,src/lib.rs}`.
- `docs/benchmarks/README.md`.

Task order is dependency order: the trace crate and backend seams come first because the CLI and workloads consume them; the registry and worker come before the controller; the controller before recipes/CI; the baseline last.

---

### Task 1: `yach-trace` crate

**Files:**
- Create: `crates/yach-trace/Cargo.toml`, `crates/yach-trace/src/lib.rs`
- Modify: `Cargo.toml:2-11` (members), `justfile:4` (`publish_crates`)

**Interfaces:**
- Produces: `yach_trace::{TraceSink, TraceRecord, TraceScope, parse_records}`.
  - `TraceSink::from_env(name: &str) -> Result<Option<TraceSink>, TraceOpenError>` — `Ok(None)` when unset; `Err` when set but unopenable (carries the path).
  - `TraceSink::mark(&self, scope: TraceScope<'_>, label: &str)` and `mark_n(&self, scope: TraceScope<'_>, label: &str, n: u32)`; per mark: timestamp, serialize a borrowed view, buffered write — no allocations for scope/label/turn id, no syscalls beyond the buffered write; a no-op once disabled.
  - `TraceSink::flush(&self)`; final flush and the once-only diagnostic happen in the shared `Inner`'s `Drop` (runs when the last clone is released).
  - `TraceScope<'a>::Startup` | `TraceScope<'a>::Turn(&'a str)` — callers borrow the turn id.
  - Writer and diagnostic targets are `Box<dyn Write + Send>` behind one mutex; `#[cfg(test)] TraceSink::with_writer_and_diag(writer, diag)` injects both so the failure test drives a real flush error and counts exactly one diagnostic line.
  - `TraceRecord { t_us: u64, scope: String, turn_id: Option<String>, label: String, n: Option<u32> }`, `Serialize + Deserialize`.
  - `parse_records(contents: &str) -> Result<Vec<TraceRecord>, TraceParseError>`; a truncated final line is `TraceParseError::TruncatedLine { line_no }`.

- [ ] **Step 1: Create the crate manifest and register it**

`crates/yach-trace/Cargo.toml`:

```toml
[package]
name = "yach-trace"
version = "0.1.0"
edition = "2024"
license = "MIT"
repository = "https://github.com/codyw912/yach"
description = "Append-only JSONL lifecycle trace sink for yach"

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

[lints]
workspace = true
```

In `Cargo.toml` add `"crates/yach-trace",` after `"crates/yach-proto",`. In `justfile` line 4 change `publish_crates` to `"yach-proto yach-trace yach-catalog yach-connections yach-hashline-extension yach-ui yach-backend yach"` (trace has no workspace deps, so it publishes before anything that depends on it).

- [x] **Steps 2–6: implemented (Task 1 is complete on this stack)**

The crate as landed differs from the first draft of this plan in four ways found during review; the interface block above is authoritative. Test module: `unset_env_is_none_and_opens_nothing`, `marks_append_jsonl_records_in_order`, `drop_flushes`, `unknown_keys_are_ignored_and_truncated_line_is_error`, `flush_failure_disables_sink_once` — each asserts `is_ok()` on setup/parse results before destructuring so failures cannot early-return as passes. Commits: `Add yach-trace append-only JSONL trace sink`, `Fix yach-trace hot path, drop flush, and failure tests`, `Route yach-trace diagnostics through an injectable sink; borrow turn ids`.

---

### Task 2: Replace `StartupTrace` and `StartupTraceMarker` with `TraceSink`

**Files:**
- Modify: `crates/yach-ui/Cargo.toml:9-22`, `crates/yach-ui/src/app.rs:37-91,3998-4070,4083-4087,4156-4159,4291-4294`, `crates/yach-ui/src/lib.rs:21-24`
- Modify: `crates/yach-backend/Cargo.toml:13-30`, `crates/yach-backend/src/runner/extension_state.rs:12-37,80-215,512-515`, `crates/yach-backend/src/runner.rs:79,119-146,1081,1650-1652,11444-11455` and every `startup_trace: None` in tests (unchanged text, type changes)
- Modify: `crates/yach-cli/Cargo.toml:9-21`, `crates/yach-cli/src/main.rs:51-69,331-335,3527-3596,3628-3660,3974-4005`, `crates/yach-cli/src/headless.rs:328-347`
- Modify: `crates/yach-bench/Cargo.toml` (add `yach-trace`), `crates/yach-bench/src/main.rs` (`YACH_STARTUP_TRACE` → `YACH_TRACE` in child envs; pollers propagate parse errors), `crates/yach-bench/src/startup_trace.rs` (adapter over `yach_trace::parse_records`, see Step 7)

**Interfaces:**
- Consumes: `yach_trace::{TraceSink, TraceScope}`.
- Produces: `RunnerConfig.trace: Option<TraceSink>` (field renamed from `startup_trace`); `yach_ui::run_tui_with_trace(…, trace: Option<TraceSink>)` and `run_tui_with_trace_and_options(…)` (renamed from `*_startup_trace*`); CLI reads `YACH_TRACE`.

- [ ] **Step 1: Add the dependency to the three crates**

Add `yach-trace = { version = "0.1.0", path = "../yach-trace" }` under `[dependencies]` in `crates/yach-ui/Cargo.toml`, `crates/yach-backend/Cargo.toml`, and `crates/yach-cli/Cargo.toml`.

- [ ] **Step 2: Write the failing backend test**

In `crates/yach-backend/src/runner.rs` tests, find the test around line 11440 that constructs `super::StartupTraceMarker::new(move |label| …)` and collects `trace_labels`. Rewrite its marker construction to a real sink:

```rust
            let trace_path = std::env::temp_dir().join(format!(
                "yach-runner-trace-{}-{}.jsonl",
                std::process::id(),
                TEMP_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let trace = yach_trace::TraceSink::open(&trace_path).ok();
            assert!(trace.is_some());
```

and pass `trace: trace.clone()` in the `RunnerConfig` (field renamed). Where the test previously asserted on `trace_labels`, read the file instead:

```rust
            drop(trace);
            let contents = std::fs::read_to_string(&trace_path).unwrap_or_default();
            let _ = std::fs::remove_file(&trace_path);
            let labels: Vec<String> = yach_trace::parse_records(&contents)
                .unwrap_or_default()
                .into_iter()
                .map(|record| record.label)
                .collect();
```

Keep the test's existing assertions on the label sequence (`extension_manifest_scan_scheduled`, `…_started`, `…_finished`).

- [ ] **Step 3: Run it to see it fail**

Run: `just dev cargo test -p yach-backend extension_manifest_scan -- --nocapture`
Expected: compile error, no field `trace` on `RunnerConfig`.

- [ ] **Step 4: Backend cutover**

In `crates/yach-backend/src/runner/extension_state.rs`:
- Delete `StartupTraceMarker`, `StartupTraceMarkFn`, and the `Debug` impl (lines 12-37).
- Change every `startup_trace: Option<StartupTraceMarker>` parameter to `trace: Option<yach_trace::TraceSink>`.
- Replace `mark_extension_scan` (512-515) with:

```rust
fn mark_extension_scan(trace: Option<&yach_trace::TraceSink>, label: &str) {
    if let Some(trace) = trace {
        trace.mark(yach_trace::TraceScope::Startup, label);
        trace.flush();
    }
}
```

In `crates/yach-backend/src/runner.rs`:
- Line 79: remove `StartupTraceMarker` from the `pub use`.
- `RunnerConfig` (119-146): rename `pub startup_trace: Option<StartupTraceMarker>` to `pub trace: Option<yach_trace::TraceSink>`; update the `Debug` impl field at 163.
- Rename the destructured binding at 1081 and the argument at 1651.
- Every test `startup_trace: None,` becomes `trace: None,` (mechanical; `grep -n 'startup_trace: None' crates/yach-backend/src/runner.rs` lists them).

In `crates/yach-backend/src/lib.rs`, if `StartupTraceMarker` is re-exported, remove it.

- [ ] **Step 5: UI cutover**

In `crates/yach-ui/src/app.rs`:
- Delete `StartupTrace`, `StartupTraceMark`, and their impls (lines 37-91) and the now-unused `OpenOptions`/`Write` imports if nothing else uses them.
- Rename `run_tui_with_startup_trace` → `run_tui_with_trace`, `run_tui_with_startup_trace_and_options` → `run_tui_with_trace_and_options`; parameter type `Option<yach_trace::TraceSink>`.
- Every `trace.mark("label")` in those functions becomes `trace.mark(yach_trace::TraceScope::Startup, "label")`; the final `trace.flush()` after `tui_first_render_end` stays.

In `crates/yach-ui/src/lib.rs:21-24` export `run_tui_with_trace, run_tui_with_trace_and_options` instead of the old names and drop `StartupTrace`.

- [ ] **Step 6: CLI cutover**

In `crates/yach-cli/src/main.rs`:
- Lines 51-55:

```rust
fn main() -> ExitCode {
    let trace = match yach_trace::TraceSink::from_env("YACH_TRACE") {
        Ok(trace) => trace,
        Err(error) => {
            let _ = emit_lines(&[format!("error: {error}")]);
            return ExitCode::from(2);
        }
    };
    if let Some(trace) = trace.as_ref() {
        trace.mark(yach_trace::TraceScope::Startup, "process_main_start");
    }
```

- Every subsequent `trace.mark("…")` in main.rs (lines 66-68, 332-334, 3549-3551, and any others `grep -n '\.mark("' crates/yach-cli/src/main.rs` finds) gains the `TraceScope::Startup` first argument.
- Rename all `startup_trace` parameters/bindings to `trace` with type `Option<&yach_trace::TraceSink>` / `Option<yach_trace::TraceSink>`.
- Delete `startup_trace_marker` (4000-4005). In `runner_config` (3974-3996) set `trace: trace.cloned(),`.
- Callers of `run_tui_with_startup_trace_and_options` use the new name.

In `crates/yach-cli/src/headless.rs:328-347` rename the `startup_trace: None` field to `trace: None`.

- [ ] **Step 7: Keep `yach-bench` working on the new format**

`yach-bench` still compiles after the rename (it only parses the trace at runtime), so it must be adapted here rather than left silently broken until Task 8 moves the samplers. Add `yach-trace = { path = "../yach-trace" }` to `crates/yach-bench/Cargo.toml`. In `main.rs`, every `.env("YACH_STARTUP_TRACE", …)` becomes `.env("YACH_TRACE", …)`. `startup_trace.rs` keeps `StartupTraceMark { label, elapsed }` but its parser becomes:

```rust
pub fn parse_startup_trace_marks(contents: &str) -> Result<Vec<StartupTraceMark>, String>
```

implemented over `yach_trace::parse_records`, keeping only `scope == "startup"` records. Semantics: `Ok` on a clean parse; `TraceParseError::TruncatedLine` → re-parse through the last `'\n'` and return `Ok` of that (both callers poll a file a live child is still appending to, so a partial final line is normal there — this tolerance is *only* for live polling; Task 12's `turn/phase/*` reads a finished trace and must call `parse_records` directly, strict); `TraceParseError::Malformed { line_no, message }` → `Err(format!("trace line {line_no}: {message}"))`. The pollers `wait_for_trace_label` and `wait_for_startup_profile_terminal_marks` propagate `Err` as `io::Error::other(..)` immediately. Tests: three JSONL startup records parse to three marks; a `"scope":"turn"` record is ignored and a truncated trailing line is tolerated; a malformed line yields `Err` naming the line number.

Do not change TUI viewport behavior to make samplers pass: on Linux, `yach tui` under util-linux `script` times out on crossterm's cursor-position query, so `yach-tui-startup-profile-report` collects zero samples on this host. That is a sampler limitation Task 10's `spawn_on_pty` resolves (the bench owns a real pty); record it in the report, do not work around it in `yach-ui`.

Run: `just dev cargo test --workspace` and `just dev cargo clippy --all-targets --all-features -- -D warnings`.
Expected: both clean, including `yach-bench`.

- [ ] **Step 8: Smoke the real trace file**

Run: `just dev cargo build -p yach && YACH_TRACE=/tmp/yach-trace.jsonl timeout 3 target/debug/yach --quiet; head -c 400 /tmp/yach-trace.jsonl`
Expected: JSON lines with `"scope":"startup"` and labels starting `process_main_start`, `cli_args_parsed`, `command_run_start`.

- [ ] **Step 9: Commit**

```bash
jj commit -m "Replace StartupTrace and StartupTraceMarker with yach-trace TraceSink"
```

As landed on this stack: `Replace StartupTrace and StartupTraceMarker with yach-trace TraceSink`, `Point yach-bench at YACH_TRACE and parse JSONL trace records`, `Propagate malformed trace parse errors; keep TUI viewport behavior unchanged`, `Fail the extension scan trace test on read or parse errors`.

---

### Task 3: Turn trace marks in the runner

**Files:**
- Modify: `crates/yach-backend/src/runner.rs:1965-2016,4969-4977,5735-5737,5290-5348,7015-7038,7222-7226,7321-7325,9141-9159,9227-9239`
- Modify: `crates/yach-backend/src/rig_adapter.rs:1184-1227`

**Interfaces:**
- Consumes: `RunnerConfig.trace` from Task 2.
- Produces: turn-scoped records with labels `prompt_received`, `request_assembled`, `provider_request_sent`, `provider_first_event`, `provider_stream_end`, `tool_dispatched` (n), `tool_result_appended` (n), `session_persisted`, `turn_completed`. Consumed by Task 12 (`turn/phase/*`) and Task 10 (RSS stop boundary).

- [ ] **Step 1: Write the failing test**

In the `runner.rs` test module, next to the test edited in Task 2, add a test that drives one prompt through `run_native_loop_with_provider_requester` with a `FakeProviderRequester` returning `Started`, `TextDelta("ok")`, `Completed`, with `trace: Some(sink)`, waits for `ServerEvent::PromptFinished`, drops the sink, and asserts on the parsed labels:

```rust
    #[test]
    fn prompt_emits_turn_trace_marks_in_order() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let Ok(runtime) = runtime else { return };
        runtime.block_on(async {
            let root = TempProject::new("native-turn-trace");
            let session_path = root.root().join("session.jsonl");
            let trace_path = root.root().join("trace.jsonl");
            let trace = yach_trace::TraceSink::open(&trace_path).ok();
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let turn_id = TurnId(String::from("turn-1"));
            let model = ProviderModel { provider: String::from("fixture"), id: String::from("fixture-model") };
            let provider = FakeProviderRequester::with_responses([Ok(vec![
                ProviderStreamEvent::Started { turn_id: turn_id.clone(), model: model.clone() },
                ProviderStreamEvent::TextDelta { turn_id: turn_id.clone(), delta: String::from("ok") },
                ProviderStreamEvent::Completed { turn_id: turn_id.clone(), finish_reason: None, usage: None, provider_response_id: None },
            ])]);
            let handle = tokio::spawn(super::run_native_loop_with_provider_requester(
                client_rx,
                backend_tx,
                super::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.root().to_path_buf()),
                    provider: Some(provider_test_config()),
                    startup_model_override: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    trace: trace.clone(),
                    catalog_refresh: None,
                    model_discovery: None,
                    provider_connections: None,
                },
                provider,
            ));
            let _ = client_tx.send(ClientEvent::Initialize(native_ready_handshake(true)));
            let _ = client_tx.send(ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: String::from("hello"),
            });
            loop {
                match backend_rx.recv().await {
                    Some(BackendEvent::Server(ServerEvent::PromptFinished { .. })) | None => break,
                    Some(_) => {}
                }
            }
            drop(client_tx);
            assert!(handle.await.is_ok());
            drop(trace);
            let contents = std::fs::read_to_string(&trace_path).unwrap_or_default();
            let labels: Vec<(String, Option<u32>)> = yach_trace::parse_records(&contents)
                .unwrap_or_default()
                .into_iter()
                .filter(|record| record.scope == "turn")
                .map(|record| (record.label, record.n))
                .collect();
            let expected = [
                "prompt_received", "request_assembled", "provider_request_sent",
                "provider_first_event", "provider_stream_end", "session_persisted", "turn_completed",
            ];
            let names: Vec<&str> = labels.iter().map(|(label, _)| label.as_str()).collect();
            assert_eq!(names, expected);
        });
    }
```

Check `ProviderModel`'s exact field names at `crates/yach-backend/src/provider.rs` (grep `pub struct ProviderModel`) and adjust the literal.

- [ ] **Step 2: Run it to see it fail**

Run: `just dev cargo test -p yach-backend prompt_emits_turn_trace_marks -- --nocapture`
Expected: FAIL — `names` is empty.

- [ ] **Step 3: Thread the sink into the turn path and emit marks**

The runner's turn state must reach the nine sites. Add a small helper near `mark_extension_scan`'s sibling location in `runner.rs`:

```rust
fn mark_turn(trace: Option<&yach_trace::TraceSink>, turn_id: &TurnId, label: &str) {
    if let Some(trace) = trace {
        trace.mark(yach_trace::TraceScope::Turn(&turn_id.0), label);
    }
}

fn mark_turn_n(trace: Option<&yach_trace::TraceSink>, turn_id: &TurnId, label: &str, n: u32) {
    if let Some(trace) = trace {
        trace.mark_n(yach_trace::TraceScope::Turn(&turn_id.0), label, n);
    }
}
```

Then, following the path the scout mapped, pass `trace.clone()` (the `Option<TraceSink>` destructured at ~1081) into the prompt-handling state so it is available in `handle_started_native_provider_prompt` (~1965) and downstream, and call:

- `mark_turn(…, "prompt_received")` right after the prompt is accepted and the turn id exists (~1965-2016 / `start_native_prompt` 3533-3555).
- `mark_turn(…, "request_assembled")` immediately after `let initial_request = ProviderRequest { … }` (~4969-4977).
- `mark_turn(…, "provider_request_sent")` immediately before `requester.request_attempt_streaming(…)` (~5735).
- `mark_turn(…, "provider_first_event")` and `"provider_stream_end"` are emitted inside the adapter's stream loop, not after the runner's await (two marks after one await would measure nothing). `rig_adapter.rs:1184-1227` collects events from `stream.next()`; add an `Option<TraceSink>` + `TurnId` to the struct that owns that loop (`PreparedCompletion` or the attempt context — whichever holds `LiveDeltaSink`; the requester passes `trace.clone()` in alongside `live`). Emit `provider_first_event` on the first item received from `stream.next()` (`~1191-1201`) and `provider_stream_end` at the `break` before `ProviderStreamAttempt::Complete(events)` (`~1220`) and on the timeout/error exits (`~1188-1199`). `ScriptedProvider` (Task 5) has no stream, so it emits both marks itself around producing its canned vector; the in-process turn workloads therefore measure them as adjacent, which is correct for a provider with zero stream latency. Thread `trace` into `RigProviderRequester` (`runner.rs:3940-3951`) from `RunnerConfig`.
- `mark_turn_n(…, "tool_dispatched", index)` per tool call at the start of `execute_native_provider_agent_tool_batch` dispatch (~5290-5348), `index` 1-based within the batch.
- `mark_turn_n(…, "tool_result_appended", index)` where each `SessionEvent::ToolExecutionFinished` is pushed (~7015-7038, 7222-7226, 7321-7325).
- `mark_turn(…, "session_persisted")` right after `append_pending_native_session_events` succeeds in `finish_native_prompt` (~9227-9231).
- `mark_turn(…, "turn_completed")` right after `PromptFinished` is sent (~9234-9239); then `trace.flush()` on the sink if present.

Pass the sink by `Option<&TraceSink>` through function parameters; do not add a global.

- [ ] **Step 4: Run the test**

Run: `just dev cargo test -p yach-backend prompt_emits_turn_trace_marks`
Expected: PASS.

- [ ] **Step 5: Add a two-tool variant**

Copy the test as `prompt_with_tools_emits_dispatch_and_result_marks`, with the fake provider returning first `[Started, ToolCallCompleted{read_text_file src/lib.rs}, ToolCallCompleted{read_text_file src/lib.rs}, Completed]` and second `[Started, TextDelta("done"), Completed]`; seed `src/lib.rs` in the temp project. Use `ProviderToolCall { call_id, name, arguments_json: serde_json::json!({"path": "src/lib.rs"}) }` as tests at `runner.rs:10169-10173` do. Assert the label sequence contains `("tool_dispatched", Some(1))`, `("tool_dispatched", Some(2))`, `("tool_result_appended", Some(1))`, `("tool_result_appended", Some(2))`, and ends with `session_persisted`, `turn_completed`.

Run: `just dev cargo test -p yach-backend prompt_with_tools_emits`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
jj commit -m "Emit turn lifecycle trace marks from the native runner"
```

---

### Task 4: `request_assembly` and `advertised_roster_bytes` bench seams

**Files:**
- Create: `crates/yach-backend/src/request_assembly.rs`
- Modify: `crates/yach-backend/src/lib.rs:14-20` (add `#[cfg(feature = "bench")] pub mod request_assembly;`), `crates/yach-backend/src/runner.rs:3673` (`pub(crate)` on `provider_messages_from_event_slice`), `crates/yach-backend/src/tools.rs:630-651`

**Interfaces:**
- Produces:
  - `yach_backend::request_assembly::fixture_log(turns: usize, tool_calls_per_turn: usize) -> SessionLog`
  - `yach_backend::request_assembly::assemble(log: &SessionLog, current_turn: &TurnId, checkpoint: Option<&str>) -> Vec<ProviderMessage>`
  - `yach_backend::tools::advertised_roster_bytes(definitions: &[ToolDefinition]) -> Result<usize, ProviderToolAdvertisingError>` (not feature-gated).

- [ ] **Step 1: Write the failing tests**

`crates/yach-backend/src/request_assembly.rs`:

```rust
use crate::session::{SessionEvent, SessionLog, TurnId};
use crate::ProviderMessage;

#[must_use]
pub fn fixture_log(turns: usize, tool_calls_per_turn: usize) -> SessionLog {
    build_fixture_log(turns, tool_calls_per_turn)
}

#[must_use]
pub fn assemble(log: &SessionLog, current_turn: &TurnId, checkpoint: Option<&str>) -> Vec<ProviderMessage> {
    crate::runner::provider_messages_from_event_slice(log, &log.events, current_turn, checkpoint)
}

#[cfg(test)]
mod tests {
    use super::{assemble, fixture_log};
    use crate::session::TurnId;

    #[test]
    fn fixture_log_has_three_events_per_text_turn() {
        let log = fixture_log(10, 0);
        assert_eq!(log.events.len(), 30);
    }

    #[test]
    fn fixture_log_adds_two_events_per_tool_call() {
        let log = fixture_log(2, 3);
        assert_eq!(log.events.len(), 2 * (3 + 2 * 3));
    }

    #[test]
    fn assemble_yields_one_message_per_entry_plus_tool_pairs() {
        let log = fixture_log(5, 1);
        let messages = assemble(&log, &TurnId(String::from("turn-6")), None);
        assert!(messages.len() >= 10, "got {}", messages.len());
        assert!(messages.iter().any(|m| m.content.contains("turn 4")));
    }
}
```

`build_fixture_log` is implemented in Step 3; write only its signature now (`fn build_fixture_log(turns: usize, tool_calls_per_turn: usize) -> SessionLog { SessionLog { events: Vec::new() } }`) so the tests compile and fail on their assertions.

Add to `crates/yach-backend/src/tools.rs` tests:

```rust
    #[test]
    fn advertised_roster_bytes_matches_serialized_extension() {
        let registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        let policy = ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            ["project_path_info"],
            ["read_text_file", "search_project", "list_project_paths"],
            ["edit_text_file", "create_text_file"],
        );
        let catalog = registry.resolve_provider_turn_catalog(
            &policy,
            ["project_path_info", "read_text_file", "search_project", "list_project_paths", "edit_text_file", "create_text_file"],
        );
        let definitions = catalog.provider_definitions();
        let bytes = advertised_roster_bytes(&definitions);
        let extension = build_provider_tool_advertising_extension(&definitions);
        let expected = extension.ok().and_then(|e| serde_json::to_vec(&e.value).ok()).map(|v| v.len());
        assert_eq!(bytes.ok(), expected);
        assert!(expected.unwrap_or(0) > 1000);
    }
```

Check `ProviderExtension`'s payload field name (`value` assumed; grep `pub struct ProviderExtension` in `provider.rs`) and adjust.

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-backend --features bench request_assembly advertised_roster_bytes`
Expected: `advertised_roster_bytes` missing (compile error) and, once that stub is added, the `request_assembly` assertions fail (`30 != 0`).

- [ ] **Step 3: Implement**

In `runner.rs:3673` change `fn provider_messages_from_event_slice(` to `pub(crate) fn provider_messages_from_event_slice(`.

In `request_assembly.rs`, implement `build_fixture_log`. Use `crate::session::completed_text_exchange` (`session.rs:724-760`) for the three text events per turn, and push the two tool events per call by constructing `SessionEvent::ToolRequestRecorded { … }` and `SessionEvent::ToolExecutionFinished { … }` with the field sets at `session.rs:330-357`. Copy the exact field list from a passing test (`runner.rs:14993-15080`) so validation/permission/outcome enums are the "successful read" values. Prompt text is `format!("user turn {i}")`, assistant text `format!("assistant turn {i}")`, so `assemble_yields_…` can find `turn 4`. Turn ids `turn-{i}` 1-based; `current_turn` in benches is `turn-{turns+1}`.

In `tools.rs` after `build_provider_tool_advertising_extension`:

```rust
/// Bytes of the advertised roster exactly as it is serialized toward a
/// provider. Provider-independent; used by the perf registry.
pub fn advertised_roster_bytes(
    tools: &[ToolDefinition],
) -> Result<usize, ProviderToolAdvertisingError> {
    let extension = build_provider_tool_advertising_extension(tools)?;
    Ok(serde_json::to_vec(&extension.value).map_or(0, |bytes| bytes.len()))
}
```

Export via `lib.rs` alongside `build_provider_tool_advertising_extension` if that is re-exported; otherwise leave it at `yach_backend::tools::advertised_roster_bytes` (check `lib.rs:41-75`).

- [ ] **Step 4: Run the tests**

Run: `just dev cargo test -p yach-backend --features bench request_assembly advertised_roster_bytes`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add request_assembly and advertised_roster_bytes bench seams"
```

---

### Task 5: `bench_loop`: scripted provider and in-process scripted turn

**Files:**
- Create: `crates/yach-backend/src/bench_loop.rs`
- Modify: `crates/yach-backend/src/lib.rs` (`#[cfg(feature = "bench")] pub mod bench_loop;`), `crates/yach-backend/src/runner.rs:1007-1059,3896`

**Interfaces:**
- Produces:
  - `yach_backend::bench_loop::Script` = `Vec<Vec<ProviderStreamEvent>>` newtype with `Serialize + Deserialize` and constructors `Script::text_only(reply: &str)` and `Script::read_tool_calls(paths: &[&str], final_reply: &str)`.
  - `yach_backend::bench_loop::ScriptedTurnConfig { project_root: PathBuf, session_path: PathBuf, script: Script, prompt: String, trace: Option<TraceSink> }`.
  - `yach_backend::bench_loop::ScriptedTurnProfile { wall: Duration, requests: usize, events_appended: usize }`.
  - `yach_backend::bench_loop::run_scripted_turn(config: ScriptedTurnConfig) -> Result<ScriptedTurnProfile, String>` — synchronous; builds a current-thread Tokio runtime internally.
  - `pub(crate) trait ProviderRequester` (visibility widened from private) and `pub(crate) async fn run_native_loop_with_provider_requester` under `#[cfg(any(test, feature = "bench"))]`.
  - Consumed by Task 6 (CLI scripted provider) and Task 12 (turn workloads).

- [ ] **Step 1: Write the failing test**

`crates/yach-backend/src/bench_loop.rs` test module:

```rust
#[cfg(test)]
mod tests {
    use super::{Script, ScriptedTurnConfig, run_scripted_turn};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("yach-bench-loop-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(root.join("src"));
        let _ = std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n");
        root
    }

    #[test]
    fn text_only_turn_appends_user_assistant_and_finished() {
        let root = temp_root("text");
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: root.clone(),
            session_path: root.join("session.jsonl"),
            script: Script::text_only("ok"),
            prompt: String::from("hello"),
            trace: None,
        });
        let contents = std::fs::read_to_string(root.join("session.jsonl")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&root);
        let Ok(profile) = profile else { return };
        assert_eq!(profile.requests, 1);
        assert!(contents.contains("\"assistant\""));
        assert!(contents.contains("turn_finished"));
    }

    #[test]
    fn four_read_calls_produce_four_tool_results_and_two_requests() {
        let root = temp_root("tools");
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: root.clone(),
            session_path: root.join("session.jsonl"),
            script: Script::read_tool_calls(&["src/lib.rs"; 4], "done"),
            prompt: String::from("read it"),
            trace: None,
        });
        let contents = std::fs::read_to_string(root.join("session.jsonl")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&root);
        let Ok(profile) = profile else { return };
        assert_eq!(profile.requests, 2);
        assert_eq!(contents.matches("tool_execution_finished").count(), 4);
    }

    #[test]
    fn script_round_trips_through_json() {
        let script = Script::read_tool_calls(&["a.rs"], "x");
        let json = serde_json::to_string(&script).unwrap_or_default();
        let back: Result<Script, _> = serde_json::from_str(&json);
        assert_eq!(back.ok(), Some(script));
    }
}
```

Adjust the serialized tag strings (`turn_finished`, `tool_execution_finished`) to whatever `SessionEvent`'s serde `rename_all` produces (check `session.rs:291-310`).

- [ ] **Step 2: Run to see it fail**

Run: `just dev cargo test -p yach-backend --features bench bench_loop`
Expected: compile error.

- [ ] **Step 3: Widen the runner seams**

In `runner.rs`:
- `trait ProviderRequester: Send {` → `pub(crate) trait ProviderRequester: Send {`.
- Change `#[cfg(test)]` on `run_native_loop_with_provider_requester` (1008) to `#[cfg(any(test, feature = "bench"))]` and make it `pub(crate)`.

- [ ] **Step 4: Implement `bench_loop`**

```rust
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

use crate::provider::{ProviderError, ProviderErrorKind, ProviderModel, ProviderRequest, ProviderStreamEvent, ProviderToolCall};
use crate::runner::{ProviderRequester, RunnerConfig};
use crate::session::TurnId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script(pub Vec<Vec<ProviderStreamEvent>>);

fn model() -> ProviderModel {
    ProviderModel { provider: String::from("scripted"), id: String::from("scripted-model") }
}

fn turn() -> TurnId {
    TurnId(String::from("turn-1"))
}

impl Script {
    #[must_use]
    pub fn text_only(reply: &str) -> Self {
        Self(vec![vec![
            ProviderStreamEvent::Started { turn_id: turn(), model: model() },
            ProviderStreamEvent::TextDelta { turn_id: turn(), delta: reply.to_owned() },
            ProviderStreamEvent::Completed { turn_id: turn(), finish_reason: None, usage: None, provider_response_id: None },
        ]])
    }

    #[must_use]
    pub fn read_tool_calls(paths: &[&str], final_reply: &str) -> Self {
        let mut first = vec![ProviderStreamEvent::Started { turn_id: turn(), model: model() }];
        for (index, path) in paths.iter().enumerate() {
            first.push(ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn(),
                tool_call: ProviderToolCall {
                    call_id: format!("call-{}", index + 1),
                    name: String::from("read_text_file"),
                    arguments_json: serde_json::json!({ "path": path }),
                },
            });
        }
        first.push(ProviderStreamEvent::Completed { turn_id: turn(), finish_reason: None, usage: None, provider_response_id: None });
        let Self(mut rounds) = Self::text_only(final_reply);
        rounds.insert(0, first);
        Self(rounds)
    }
}

pub struct ScriptedProvider {
    responses: VecDeque<Vec<ProviderStreamEvent>>,
    requests: usize,
}

impl ScriptedProvider {
    #[must_use]
    pub fn new(script: Script) -> Self {
        Self { responses: script.0.into(), requests: 0 }
    }
}

impl ProviderRequester for ScriptedProvider {
    fn request(&mut self, _request: ProviderRequest) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        self.requests += 1;
        let response = self.responses.pop_front().ok_or_else(|| ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("scripted provider exhausted"),
            redacted_debug: None,
            metadata: crate::ProviderErrorMetadata::default(),
        });
        Box::pin(async move { response })
    }
}

pub struct ScriptedTurnConfig {
    pub project_root: PathBuf,
    pub session_path: PathBuf,
    pub script: Script,
    pub prompt: String,
    pub trace: Option<yach_trace::TraceSink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedTurnProfile {
    pub wall: Duration,
    pub requests: usize,
    pub events_appended: usize,
}

pub fn run_scripted_turn(config: ScriptedTurnConfig) -> Result<ScriptedTurnProfile, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let expected_requests = config.script.0.len();
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let provider = ScriptedProvider::new(config.script);
        let start = Instant::now();
        let handle = tokio::spawn(crate::runner::run_native_loop_with_provider_requester(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: config.session_path.clone(),
                project_root: Some(config.project_root),
                provider: Some(scripted_provider_config()),
                startup_model_override: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                trace: config.trace,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
            },
            provider,
        ));
        client_tx
            .send(ClientEvent::Initialize(crate::runner::native_ready_handshake(true)))
            .map_err(|_| String::from("runner closed before initialize"))?;
        client_tx
            .send(ClientEvent::PromptSubmitted { session_id: String::from("default"), prompt: config.prompt })
            .map_err(|_| String::from("runner closed before prompt"))?;
        loop {
            match backend_rx.recv().await {
                Some(BackendEvent::Server(ServerEvent::PromptFinished { .. })) => break,
                Some(_) => {}
                None => return Err(String::from("runner exited before prompt finished")),
            }
        }
        let wall = start.elapsed();
        drop(client_tx);
        handle.await.map_err(|error| error.to_string())?;
        let contents = std::fs::read_to_string(&config.session_path).map_err(|error| error.to_string())?;
        Ok(ScriptedTurnProfile { wall, requests: expected_requests, events_appended: contents.lines().count() })
    })
}

pub(crate) fn scripted_provider_config() -> crate::ProviderConfig {
    // Mirrors the test fixture at runner.rs `provider_test_config`; the
    // adapter is never contacted because the requester is scripted.
    crate::ProviderConfig {
        adapter: std::sync::Arc::new(crate::rig_adapter::RigProviderAdapterConfig {
            provider: crate::rig_adapter::RigProviderConfig::Anthropic {
                api_key: crate::ProviderSecret::new(String::from("scripted")),
                base_url: None,
            },
            timeout: Duration::from_secs(30),
            max_tokens: 1000,
            context_window: 200_000,
            max_tokens_param: crate::rig_adapter::MaxTokensParam::default(),
            error_dialect: crate::DialectSelection::Missing,
        }),
        model: String::from("scripted-model"),
        connection_id: None,
        connection_key: None,
        connection_display: None,
        test_delay_ms: None,
        catalog_models: Vec::new().into(),
        responses_compact: Some(true),
    }
}
```

`native_ready_handshake` (`runner.rs:2853`) needs `pub(crate)`. `ProviderStreamEvent` must derive `Serialize`/`Deserialize` — check `provider.rs:313`; it is serde-annotated per the scout (fields have `#[serde]` attributes) — confirm and add derives if missing. `requests` reports the script length because the scripted provider is consumed by the runner; if the runner made fewer requests the second test's `tool_execution_finished` count would be wrong anyway.

- [ ] **Step 5: Run the tests**

Run: `just dev cargo test -p yach-backend --features bench bench_loop`
Expected: 3 passed.

- [ ] **Step 6: Merge the `cfg(test)` helper**

Delete `run_native_loop_with_unnegotiated_provider_requester` only if no test uses it (`grep -c run_native_loop_with_unnegotiated_provider_requester crates/yach-backend/src/runner.rs`); if tests use it, leave it — it is a different negotiation profile, not a duplicate.

- [ ] **Step 7: Commit**

```bash
jj commit -m "Add bench_loop scripted provider and in-process scripted turn"
```

---

### Task 6: CLI `bench` feature with scripted provider

**Files:**
- Modify: `crates/yach-cli/Cargo.toml` (add `[features] bench = ["yach-backend/bench"]`), `crates/yach-cli/src/main.rs:1017-1060` and the requester construction path (`run_native_loop_with_negotiated_capabilities` callers at `crates/yach-backend/src/runner.rs:978-1005`)

**Interfaces:**
- Consumes: `yach_backend::bench_loop::{Script, ScriptedProvider}`.
- Produces: with `--features bench`, `YACH_RIG_PROVIDER=scripted` + `YACH_BENCH_SCRIPT=<path to Script JSON>` makes `yach tui`/`yach run` use `ScriptedProvider`. Without the feature, `scripted` is rejected by the existing `InvalidValue` arm.
- Also produces `yach_backend::run_native_loop_with_scripted_provider(rx, tx, config, script: Script)` under `bench`, public.

- [ ] **Step 1: Backend entry point**

In `runner.rs` near `run_native_loop` (958), add:

```rust
#[cfg(feature = "bench")]
pub async fn run_native_loop_with_scripted_provider(
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    config: RunnerConfig,
    script: crate::bench_loop::Script,
) {
    run_native_loop_with_provider_requester(rx, tx, config, crate::bench_loop::ScriptedProvider::new(script)).await;
}
```

Export from `lib.rs` under `#[cfg(feature = "bench")]`.

- [ ] **Step 2: CLI wiring**

In `crates/yach-cli/Cargo.toml` add:

```toml
[features]
default = []
bench = ["yach-backend/bench"]
```

In `main.rs` provider match (1019), add before the `_ =>` arm:

```rust
        #[cfg(feature = "bench")]
        "scripted" => {
            let _ = required_env("YACH_BENCH_SCRIPT")?;
            RigProviderConfig::Anthropic {
                api_key: ProviderSecret::new(String::from("scripted")),
                base_url: None,
            }
        }
```

and where the CLI spawns the backend loop for `yach tui` (the call that ends up in `run_native_loop`/`run_native_loop_with_negotiated_capabilities`, around `main.rs:3908-3926`), branch:

```rust
    #[cfg(feature = "bench")]
    if std::env::var("YACH_RIG_PROVIDER").as_deref() == Ok("scripted") {
        let script_path = std::env::var("YACH_BENCH_SCRIPT").unwrap_or_default();
        let script = std::fs::read_to_string(&script_path)
            .ok()
            .and_then(|json| serde_json::from_str::<yach_backend::bench_loop::Script>(&json).ok());
        if let Some(script) = script {
            runtime.spawn(yach_backend::run_native_loop_with_scripted_provider(client_rx, backend_tx, config, script));
            // fall through to the same post-spawn code as the normal path
        } else {
            return CommandResult::Error(format!("YACH_BENCH_SCRIPT unreadable or invalid: {script_path}"));
        }
    }
```

Fit this into the existing structure (the normal path spawns `run_native_loop_with_negotiated_capabilities`); use whatever `CommandResult` error variant exists for setup failures. Apply the same branch in `headless.rs` for `yach run`.

- [ ] **Step 3: Smoke it**

```bash
cat > /tmp/script.json <<'EOF'
[[{"type":"started","turn_id":"turn-1","model":{"provider":"scripted","id":"scripted-model"}},{"type":"text_delta","turn_id":"turn-1","delta":"ok"},{"type":"completed","turn_id":"turn-1","finish_reason":null,"usage":null,"provider_response_id":null}]]
EOF
just dev cargo build --release --locked -p yach --features bench --target-dir target/bench
cd /tmp && mkdir -p scripted-proj && cd scripted-proj && YACH_RIG_PROVIDER=scripted YACH_BENCH_SCRIPT=/tmp/script.json YACH_TRACE=/tmp/turn-trace.jsonl /srv/agent-dev/projects/yach/target/bench/release/yach run --prompt "hello"; tail -3 /tmp/turn-trace.jsonl
```

Adjust the JSON to match `ProviderStreamEvent`'s serde tag style (check `provider.rs:311-313`). Expected: the run finishes with `ok` and the trace tail shows `session_persisted`, `turn_completed`.

- [ ] **Step 4: Confirm the shipping build rejects `scripted`**

Run: `just dev cargo build --release --locked -p yach && YACH_RIG_PROVIDER=scripted target/release/yach run --prompt hi`
Expected: error mentioning `must be anthropic, openai-codex, openai, or openai-compatible`.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add bench-feature scripted provider to the yach CLI"
```

---

### Task 7: `yach-bench` perf schema, registry, allocator, provenance

**Files:**
- Create: `crates/yach-bench/src/perf/{mod.rs,schema.rs,registry.rs,alloc.rs,provenance.rs}`
- Modify: `crates/yach-bench/Cargo.toml` (add `serde = { version = "1.0", features = ["derive"] }`, `toml = "0.8"`, `libc = "0.2"`, `sha2 = "0.10"`), `crates/yach-bench/src/lib.rs` (`pub mod perf;`)

**Interfaces:**
- Produces (`perf::schema`): `SCHEMA: u32 = 1`; `ResultDoc { schema, host: HostInfo, build: BuildInfo, started_at: String, workloads: Vec<WorkloadRow> }`; `WorkloadRow { id, class: Class, isolation: Isolation, status: Status, reason: Option<String>, count: usize, p50_ns/p95_ns/p99_ns/max_ns: Option<u64>, value: Option<u64>, samples_ns: Option<Vec<u64>>, samples_bytes: Option<Vec<u64>> }`; enums `Class { Latency, Memory, Size, Count }`, `Isolation { InProcessSerial, InProcessThreaded, ChildProcess }`, `Status { Ok, Skipped, Error }`; `BuildInfo { source_sha256, commit: Option<String>, dirty: bool, profile, rustc, cargo_lock_sha256, yach_bin_sha256: Option<String> }`; `HostInfo { fingerprint, cpu, cores, os, kernel }`.
- Produces (`perf::registry`): `Requirement { Binary, Tty, Linux }`; `Bin { Shipping, Bench }`; `Workload { id: &'static str, class, isolation, requires: &'static [Requirement], bin: Option<Bin>, run: fn(&RunCtx) -> Result<Measured, String> }`; `Measured { Latency { samples: Vec<Duration>, alloc: Option<AllocCounts> }, Memory(Vec<u64>), Value(u64) }` — a serial workload that wants a tight allocation window opens its own `AllocWindow` around the operation under test and returns the counts in `alloc`; `None` means "use the worker's outer window"; `RunCtx { samples: usize, yach_bin: Option<PathBuf>, yach_bench_yach_bin: Option<PathBuf>, yach_bench_bin: Option<PathBuf>, filter: Option<glob::Pattern> }` (`Clone`); `pub fn all() -> &'static [Workload]`; `pub fn derived_alloc_rows(base: &WorkloadRow, counts: AllocCounts) -> [WorkloadRow; 2]`.
- Produces (`perf::alloc`): `pub struct Counting;` `#[global_allocator]` installed in `main.rs`; `AllocWindow::begin() -> AllocWindow`; `AllocWindow::end(self) -> AllocCounts { count: u64, bytes: u64 }`.
- Produces (`perf::provenance`): `capture_build(checkout: &Path, yach_bin: Option<&Path>) -> Result<BuildInfo, String>`; `capture_host() -> HostInfo`; `source_digest(checkout: &Path) -> Result<String, String>` (runs `evals/scripts/source-digest.sh`).

- [ ] **Step 1: Write the failing tests**

`crates/yach-bench/src/perf/alloc.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::AllocWindow;

    #[test]
    fn window_counts_only_allocations_inside_it() {
        let _outside = vec![0u8; 4096];
        let window = AllocWindow::begin();
        let inside = vec![0u8; 8192];
        let counts = window.end();
        drop(inside);
        assert!(counts.count >= 1);
        assert!(counts.bytes >= 8192, "bytes={}", counts.bytes);
        assert!(counts.bytes < 8192 + 1024, "bytes={}", counts.bytes);
    }
}
```

`crates/yach-bench/src/perf/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{all, derived_alloc_rows, Isolation};
    use crate::perf::alloc::AllocCounts;
    use crate::perf::schema::{Class, Status, WorkloadRow};
    use std::collections::BTreeSet;

    #[test]
    fn ids_are_unique_and_preserve_legacy_labels() {
        let ids: Vec<&str> = all().iter().map(|w| w.id).collect();
        let set: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), set.len());
        for legacy in [
            "startup/backend_ready_to_first_interactive_headless",
            "terminal/idle_keypress_to_draw_flush_live",
            "yach/tui_ready_startup_first_output_pty",
            "yach/cli_startup_first_output",
            "extension_runtime/metadata_tool_invocation_round_trip",
        ] {
            assert!(set.contains(legacy), "missing {legacy}");
        }
    }

    #[test]
    fn only_serial_latency_workloads_get_alloc_rows() {
        let serial = all().iter().filter(|w| w.isolation == Isolation::InProcessSerial && w.class == Class::Latency).count();
        assert!(serial > 0);
        let base = WorkloadRow::latency("request/assemble/10_turns", Isolation::InProcessSerial, &[]);
        let rows = derived_alloc_rows(&base, AllocCounts { count: 3, bytes: 300 });
        assert_eq!(rows[0].id, "request/assemble/10_turns#alloc_count");
        assert_eq!(rows[0].class, Class::Count);
        assert_eq!(rows[0].value, Some(3));
        assert_eq!(rows[1].id, "request/assemble/10_turns#alloc_bytes");
        assert_eq!(rows[1].value, Some(300));
        assert_eq!(rows[1].status, Status::Ok);
    }
}
```

`crates/yach-bench/src/perf/provenance.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{capture_build, capture_host};

    #[test]
    fn build_info_has_digest_and_lock_hash() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Ok(build) = capture_build(&root, None) else { return };
        assert_eq!(build.source_sha256.len(), 64);
        assert_eq!(build.cargo_lock_sha256.len(), 64);
        assert!(build.rustc.contains("rustc"));
        assert!(build.commit.as_ref().is_none_or(|c| c.len() == 40));
    }

    #[test]
    fn host_fingerprint_is_stable_within_process() {
        assert_eq!(capture_host().fingerprint, capture_host().fingerprint);
        assert!(capture_host().cores > 0);
    }
}
```

- [ ] **Step 2: Run to see them fail**

Run: `just dev cargo test -p yach-bench perf::`
Expected: compile errors.

- [ ] **Step 3: Implement `schema.rs`**

Plain serde structs per the interface list; derive `Debug, Clone, PartialEq, Serialize, Deserialize` (enums additionally `Copy, Eq`, `#[serde(rename_all = "snake_case")]`). Add constructors on `WorkloadRow`:

```rust
impl WorkloadRow {
    pub fn latency(id: &str, isolation: Isolation, samples: &[Duration]) -> Self { /* fills p50/p95/p99/max via LatencySummary::from_samples; count = samples.len(); status Ok if count>0 else Error("no samples") */ }
    pub fn memory(id: &str, samples_bytes: &[u64]) -> Self { /* max_ns unused; value = max; count = len; isolation ChildProcess */ }
    pub fn value(id: &str, class: Class, isolation: Isolation, value: u64) -> Self { /* count 1 */ }
    pub fn skipped(id: &str, class: Class, isolation: Isolation, reason: &str) -> Self
    pub fn error(id: &str, class: Class, isolation: Isolation, reason: &str) -> Self
}
```

Percentiles as `u64` nanoseconds: `duration.as_nanos()` clamped with `u64::try_from(..).unwrap_or(u64::MAX)`.

- [ ] **Step 4: Implement `alloc.rs`**

```rust
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub struct Counting;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: delegates every call to `System`; only adds relaxed counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ACTIVE.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size.saturating_sub(layout.size()) as u64, Ordering::Relaxed);
        }
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocCounts { pub count: u64, pub bytes: u64 }

pub struct AllocWindow { count0: u64, bytes0: u64 }

impl AllocWindow {
    #[must_use]
    pub fn begin() -> Self {
        let window = Self { count0: COUNT.load(Ordering::Relaxed), bytes0: BYTES.load(Ordering::Relaxed) };
        ACTIVE.store(true, Ordering::SeqCst);
        window
    }
    #[must_use]
    pub fn end(self) -> AllocCounts {
        ACTIVE.store(false, Ordering::SeqCst);
        AllocCounts {
            count: COUNT.load(Ordering::Relaxed).saturating_sub(self.count0),
            bytes: BYTES.load(Ordering::Relaxed).saturating_sub(self.bytes0),
        }
    }
}
```

`layout.size() as u64`: usize→u64 is lossless on all supported targets; if clippy `cast_possible_truncation` (pedantic) fires, use `u64::try_from(..).unwrap_or(u64::MAX)`. Install in `main.rs`: `#[global_allocator] static ALLOC: yach_bench::perf::alloc::Counting = yach_bench::perf::alloc::Counting;`. The worker (Task 9) guarantees serial workloads run alone on the main thread, so the process-global counter is exact for the window.

- [ ] **Step 5: Implement `registry.rs`**

Descriptor types per the interface list. `all()` returns a `static` slice; workload entries are populated by Tasks 8, 10, 11, 12 — for this task register the six headless workloads only, by moving `headless_report_lines` bodies (`main.rs:125-159`, `sample_replay`, `sample_startup` 1357-1381, and the `*_steps` helpers 1383-1430) into `perf/workloads/tui.rs` as `run: fn(&RunCtx) -> Result<Measured, String>` functions:

```rust
pub static HEADLESS: [Workload; 6] = [
    Workload { id: "startup/backend_ready_to_first_interactive_headless", class: Class::Latency, isolation: Isolation::InProcessSerial, requires: &[], bin: None, run: |ctx| Ok(Measured::Latency(sample_startup(ctx.samples))) },
    // … five more mirroring main.rs:133-152
];
```

`fn` pointers cannot capture, so keep the closure literal free of captures (as above). `all()` concatenates the per-subsystem statics into one `Vec` behind a `std::sync::OnceLock<Vec<Workload>>` — simpler than a giant literal. `derived_alloc_rows` builds two `WorkloadRow::value(..)` rows with ids `format!("{id}#alloc_count")` / `#alloc_bytes`, class `Count`, isolation copied.

- [ ] **Step 6: Implement `provenance.rs`**

- `source_digest`: `Command::new("bash").arg(checkout.join("evals/scripts/source-digest.sh"))`, trim stdout.
- `cargo_lock_sha256`: `sha2::Sha256` over `checkout/Cargo.lock` bytes, hex.
- `commit`: `git -C <checkout> rev-parse HEAD` → `Some(trimmed)` on success else `None`.
- `dirty`: `git -C <checkout> status --porcelain` non-empty, or `commit.is_none()`.
- `rustc`: `rustc --version` via `just dev` is unnecessary — the worker runs inside the dev shell already; call `Command::new("rustc").arg("--version")`.
- `yach_bin_sha256`: sha256 of the file when `Some`.
- `capture_host`: `/proc/cpuinfo` first `model name` (Linux) else `uname -p`; `std::thread::available_parallelism()`; `uname -s` / `uname -r`; fingerprint = sha256 of `cpu|cores|os|kernel` first 16 hex chars.

- [ ] **Step 7: Wire `perf/mod.rs` and `lib.rs`**

`perf/mod.rs`: `pub mod alloc; pub mod provenance; pub mod registry; pub mod schema; pub mod workloads;` plus `pub fn dispatch(args: &[String]) -> Result<Vec<String>, String>` returning usage for now (subcommands come in Tasks 9, 13, 14). `workloads/mod.rs` declares `pub mod tui;`. `lib.rs` adds `pub mod perf;`.

- [ ] **Step 8: Run the tests**

Run: `just dev cargo test -p yach-bench perf::`
Expected: 5 passed.

- [ ] **Step 9: Commit**

```bash
jj commit -m "Add yach-bench perf schema, registry, counting allocator, provenance"
```

---

### Task 8: Migrate remaining samplers into the registry; delete `*-report` commands

**Files:**
- Create: `crates/yach-bench/src/perf/workloads/{startup.rs,edit.rs,extension.rs,binary.rs}`; extend `tui.rs`
- Modify: `crates/yach-bench/src/main.rs` (dispatch becomes `perf` only; remove all `*_report_lines`, `usage_lines`, `report_lines_indicate_failure`, `render_summary`, `render_duration`, `sample_count`), delete `crates/yach-bench/src/startup_trace.rs`, `crates/yach-bench/src/lib.rs`

**Interfaces:**
- Consumes: `yach_trace::parse_records` (replaces `parse_startup_trace_marks`); registry types from Task 7.
- Produces: full registry coverage for existing workloads with the ids listed below; `perf::workloads::startup::trace_labels_since_main(records) -> BTreeMap<String, Duration>`.

- [ ] **Step 1: Move live-terminal samplers**

Into `workloads/tui.rs`, move `sample_live_terminal*` (455-782), `restore_terminal`, `AsyncBacklogProfile`, `AsyncBacklogResult` unchanged, and register nine `Workload`s with `requires: &[Requirement::Tty]`, `isolation: InProcessSerial` except the two async-backlog entries (`InProcessThreaded`), ids exactly as today's labels (`terminal/startup_ready_keypress_draw_flush_live`, `terminal/idle_keypress_to_draw_flush_live`, `terminal/active_stream_keypress_to_draw_flush_live`, `terminal/stream_backlog_keypress_to_draw_flush_live`, `terminal/async_backlog_keypress_to_draw_flush_live`, `terminal/async_backlog_stress_keypress_to_draw_flush_live`, `terminal/heavy_output_keypress_to_draw_flush_live`, `terminal/large_transcript_scroll_to_draw_flush_live`, `terminal/huge_transcript_scroll_to_draw_flush_live`). The extra `async_backlog_profile=…` metadata line is dropped; `events_sent`/`drained` become a `reason`-less `WorkloadRow` — no, keep it simple: return only the latency samples.

- [ ] **Step 2: Move startup samplers**

Into `workloads/startup.rs`, move `sample_yach_tui_first_output`, `sample_yach_cli_first_output`, `resolve_yach_cli_bin` (rewritten to take `ctx.yach_bin` first, then the existing fallbacks), `read_first_byte_with_timeout`, `sample_yach_tui_startup_profile`, `StartupProfileScenario`, `ExtensionManifestPackageRoot`, `wait_for_trace_label`/`wait_for_startup_profile_terminal_marks` (rewritten over `yach_trace::parse_records`, filtering `scope == "startup"`), and the env var name `YACH_STARTUP_TRACE` → `YACH_TRACE`.

Register: `yach/tui_startup_first_output_pty`, `yach/tui_ready_startup_first_output_pty`, `yach/cli_startup_first_output` (all `ChildProcess`, `requires: &[Requirement::Binary]`, `bin: Some(Bin::Shipping)`); and the startup-profile family as one workload per emitted label: `startup/phase/<label>` for each startup mark label the shipping binary emits (`process_main_start`… `tui_first_render_end`, `extension_manifest_scan_*`), plus `yach/tui_startup_profile/observed_process_to_first_render_pty`, and the inactive-extension / many-extensions variants under their existing prefixes. Since a profile run yields all labels at once, implement one sampler that caches per-`RunCtx` results in a `OnceLock<Mutex<HashMap<StartupProfileScenario, Vec<TraceRecordSet>>>>` keyed by scenario so the N `startup/phase/*` workloads share one set of child launches.

- [ ] **Step 3: Move edit and extension samplers**

`workloads/edit.rs`: `native_edit/<scenario>/<phase>` — one workload per (scenario, phase) pair from `EditProfileScenario::all()` × `EditProfilePhase` labels, `InProcessSerial`, sampling via `EditProfileRunner::sample_scenario` with the same shared-cache trick (one scenario run yields all its phases).

`workloads/extension.rs`: `extension_runtime/metadata_host_activation`, `extension_runtime/metadata_tool_invocation_round_trip` from `sample_extension_runtime_profile` + `BenchExtensionHostTransport` (moved verbatim), `InProcessThreaded`.

- [ ] **Step 4: Add `binary/size_bytes`**

`workloads/binary.rs`:

```rust
pub static BINARY: [Workload; 1] = [Workload {
    id: "binary/size_bytes",
    class: Class::Size,
    isolation: Isolation::ChildProcess,
    requires: &[Requirement::Binary],
    bin: Some(Bin::Shipping),
    run: |ctx| {
        let path = ctx.yach_bin.as_ref().ok_or("yach binary path missing")?;
        let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
        Ok(Measured::Value(len))
    },
}];
```

- [ ] **Step 5: Rewrite `main.rs`**

```rust
#[global_allocator]
static ALLOC: yach_bench::perf::alloc::Counting = yach_bench::perf::alloc::Counting;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (lines, code) = match args.first().map(String::as_str) {
        Some("perf") => match yach_bench::perf::dispatch(&args[1..]) {
            Ok(outcome) => (outcome.lines, outcome.exit_code),
            Err(message) => (vec![format!("error: {message}")], 1),
        },
        _ => (vec![String::from("usage: yach-bench perf run|worker|ab|report …")], 2),
    };
    let _ = emit_lines(&lines);
    std::process::exit(code);
}
```

`clippy::exit` denies `std::process::exit`; `main` therefore returns `std::process::ExitCode` (`ExitCode::from(code)`) as it does today. `dispatch` returns `Outcome { lines: Vec<String>, exit_code: u8 }`. Delete everything else in `main.rs` except `emit_lines` and the tests that still apply (move label tests to the modules that own them; delete tests of deleted `*_report_lines`).

- [ ] **Step 6: Delete `startup_trace.rs` and update `lib.rs`**

`lib.rs` exports `fixtures`, `latency`, `perf`, `replay`.

- [ ] **Step 7: Verify**

Run: `just dev cargo test -p yach-bench` and `just dev cargo clippy -p yach-bench --all-targets -- -D warnings`
Expected: pass; no references to `*_report_lines` remain (`grep -rn report_lines crates/yach-bench` is empty).

- [ ] **Step 8: Commit**

```bash
jj commit -m "Move yach-bench samplers into the perf registry and remove report commands"
```

---

### Task 9: `perf worker` and `perf run`

**Files:**
- Create: `crates/yach-bench/src/perf/worker.rs`
- Modify: `crates/yach-bench/src/perf/mod.rs`

**Interfaces:**
- Produces:
  - `perf worker --schema <n> --filter <glob> --samples <N> --yach-bin <path> --yach-bench-yach-bin <path> [--deterministic] [--raw] --out <file>` → measures in this process and writes `ResultDoc` JSON; stdout is `{"schema":1,"workloads":<count>}`; exit 0 unless the document could not be written. `--schema-probe` prints `{"schema":1}` and exits 0. This is the only subcommand that measures in-process; it exists to be spawned.
  - `perf external-sampler --filter <glob> --samples <N> --yach-bin <base shipping yach> --out <file>` → same as `worker` but the registry is restricted to `EXTERNAL_IDS` (Task 13) and the binary is another checkout's shipping `yach`. Spawned by the controller in external mode; never run by hand.
  - `perf run [--filter] [--samples] [--deterministic] [--raw] --out <file>` → a controller: builds the three artifacts for the current checkout through `just dev cargo build …` (Task 13's `build_side`), spawns `<current yach-bench> perf worker …` as a subprocess, reads the document, prints a text table; exit 1 if any row is `error`. It never measures in its own process, so the allocation counter and TTY state of the controller never leak into results.
  - `worker::measure(ctx: &RunCtx, deterministic: bool, raw: bool) -> Vec<WorkloadRow>` — the shared body used by `worker` and `external-sampler`.
  - `worker::spawn(bench_bin: &Path, args: &WorkerArgs) -> Result<ResultDoc, String>` — spawns a worker/sampler subprocess and parses `--out`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::measure;
    use crate::perf::registry::RunCtx;
    use crate::perf::schema::{Class, Status};

    #[test]
    fn deterministic_measure_emits_only_size_and_count_rows_with_alloc_derivatives() {
        let ctx = RunCtx { samples: 1, yach_bin: None, yach_bench_bin: None, filter: glob::Pattern::new("request/assemble/10_turns").ok() };
        let rows = measure(&ctx, true, false);
        assert!(rows.iter().all(|r| matches!(r.class, Class::Size | Class::Count)));
        assert!(rows.iter().any(|r| r.id == "request/assemble/10_turns#alloc_bytes" && r.status == Status::Ok));
        assert!(!rows.iter().any(|r| r.id == "request/assemble/10_turns"));
    }

    #[test]
    fn tty_workloads_skip_without_terminal() {
        let ctx = RunCtx { samples: 1, yach_bin: None, yach_bench_bin: None, filter: glob::Pattern::new("terminal/idle_*").ok() };
        let rows = measure(&ctx, false, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Status::Skipped);
        assert_eq!(rows[0].reason.as_deref(), Some("requires tty"));
    }
}
```

The first test needs `request/assemble/10_turns` in the registry. Register the three `request/assemble/{10,100,1000}_turns` workloads in this task (create `crates/yach-bench/src/perf/workloads/core_loop.rs` with just those entries, using the `assemble_workload` function shown in Task 12 Step 1); Task 12 extends the same file.

- [ ] **Step 2: Implement `measure`**

```rust
pub fn measure(ctx: &RunCtx, deterministic: bool, raw: bool) -> Vec<WorkloadRow> {
    let has_tty = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let mut rows = Vec::new();
    for workload in registry::all() {
        if let Some(filter) = &ctx.filter && !filter.matches(workload.id) { continue; }
        let wants_row = !deterministic || matches!(workload.class, Class::Size | Class::Count);
        let wants_alloc = workload.isolation == Isolation::InProcessSerial && workload.class == Class::Latency;
        if !wants_row && !wants_alloc { continue; }
        if let Some(reason) = unmet(workload, has_tty) {
            if wants_row { rows.push(WorkloadRow::skipped(workload.id, workload.class, workload.isolation, reason)); }
            if wants_alloc { for suffix in ["#alloc_count", "#alloc_bytes"] { rows.push(WorkloadRow::skipped(&format!("{}{suffix}", workload.id), Class::Count, workload.isolation, reason)); } }
            continue;
        }
        let samples = if deterministic && !wants_row { 1 } else { ctx.samples };
        let local = RunCtx { samples, ..ctx.clone() };
        let (result, counts) = if wants_alloc {
            let window = AllocWindow::begin();
            let result = (workload.run)(&local);
            (result, Some(window.end()))
        } else {
            ((workload.run)(&local), None)
        };
        match result {
            Ok(measured) => {
                let row = row_from(workload, measured, raw);
                if let Some(counts) = counts { rows.extend(registry::derived_alloc_rows(&row, counts)); }
                if wants_row { rows.push(row); }
            }
            Err(message) => {
                if wants_row { rows.push(WorkloadRow::error(workload.id, workload.class, workload.isolation, &message)); }
                if wants_alloc { for suffix in ["#alloc_count", "#alloc_bytes"] { rows.push(WorkloadRow::error(&format!("{}{suffix}", workload.id), Class::Count, workload.isolation, &message)); } }
            }
        }
    }
    rows
}
```

`unmet` returns `Some("requires tty")` / `Some("unsupported_os")` / `Some("yach binary missing")` per `requires` and `ctx`. `row_from` converts `Measured` into a `WorkloadRow`; when `Measured::Latency { alloc: Some(counts), .. }` is returned, `measure` uses those counts for the derived rows instead of the outer window (the outer window includes the workload's sample loop and any setup, which is only acceptable for the legacy TUI samplers whose setup is trivial relative to the sampled work).

- [ ] **Step 3: Implement `worker` and `run` subcommands**

Argument parsing: hand-rolled `--key value` loop (no clap in the crate). `worker` writes `ResultDoc { schema: SCHEMA, host: capture_host(), build: capture_build(checkout, yach_bin)?, started_at: rfc3339 now, workloads }` to `--out`; `--schema` must equal `SCHEMA` else error exit 3 with `{"error":"schema mismatch","have":1,"want":n}`. `external-sampler` calls `measure` with `filter` intersected with `EXTERNAL_IDS` and `RunCtx { yach_bin: <given>, yach_bench_yach_bin: None, .. }`; provenance's `commit`/`source_sha256`/`cargo_lock_sha256` come from `--checkout <base dir>`. `run` builds (Task 13 `build_side`, current checkout only), then `worker::spawn(target/release/yach-bench, …)`, then renders a table (`id | status | p50 | p95 | p99 | max | value | alloc`) with the `render_duration` formatting (a private copy here; Task 14 moves it to `report.rs`). Until Task 13 lands, `run` uses a local `build_current()` that runs the three `just dev cargo build` commands; Task 13 replaces it with `build_side`.

`checkout` for provenance: `--checkout <dir>` flag, default: walk up from the executable's directory until `Cargo.toml` + `evals/` exist.

`worker::spawn` inherits the controller's TTY (`Stdio::inherit()` for stdin/stdout would corrupt the JSON handshake — so: stdin inherited, stdout piped for the one-line handshake, stderr inherited), which is what lets `terminal/*` workloads run under `script -q /dev/null just perf-record`.

- [ ] **Step 4: Run tests and smoke**

Run: `just dev cargo test -p yach-bench perf::worker` then
`just dev cargo run -p yach-bench --release -- perf run --filter 'request/*' --samples 20 --out /tmp/perf-run.json && jq '.workloads | length' /tmp/perf-run.json`
Expected: tests pass; a table and a positive count.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add yach-bench perf worker and run subcommands"
```

---

### Task 10: Peak RSS sampler and `memory/*` workloads

**Files:**
- Create: `crates/yach-bench/src/perf/rss.rs`
- Modify: `crates/yach-bench/src/perf/workloads/startup.rs` (register `memory/peak_rss/tui_ready`), later `core_loop.rs` (Task 12 registers `memory/peak_rss/turn_scripted_tools_4`)

**Interfaces:**
- Produces: `rss::StopBoundary { FirstOutputByte, TraceLabel { path: PathBuf, label: &'static str } }`; `rss::peak_rss_bytes(command: &mut Command, boundary: StopBoundary, timeout: Duration) -> Result<u64, String>` (Linux); on other targets a `cfg`-gated version returning `Err("unsupported_os")`; `rss::kib_to_bytes(kib: i64) -> u64`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::kib_to_bytes;

    #[test]
    fn linux_maxrss_is_kib() {
        assert_eq!(kib_to_bytes(1), 1024);
        assert_eq!(kib_to_bytes(0), 0);
        assert_eq!(kib_to_bytes(-1), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_peak_rss_is_at_least_its_allocation() {
        use super::{StopBoundary, peak_rss_bytes};
        use std::process::Command;
        // 32 MiB in the child, then print, then sleep so the kill path is exercised.
        let mut cmd = Command::new("python3");
        cmd.args(["-c", "b=bytearray(32*1024*1024); import sys; sys.stdout.write('x'); sys.stdout.flush(); import time; time.sleep(30)"]);
        let bytes = peak_rss_bytes(&mut cmd, StopBoundary::FirstOutputByte, std::time::Duration::from_secs(10));
        let Ok(bytes) = bytes else { return };
        assert!(bytes >= 32 * 1024 * 1024, "bytes={bytes}");
        assert!(bytes < 256 * 1024 * 1024, "bytes={bytes}");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_is_unsupported() {
        use super::{StopBoundary, peak_rss_bytes};
        let mut cmd = std::process::Command::new("true");
        assert_eq!(peak_rss_bytes(&mut cmd, StopBoundary::FirstOutputByte, std::time::Duration::from_secs(1)), Err(String::from("unsupported_os")));
    }
}
```

If `python3` is not on the dev shell PATH, use `just dev-shell 'which python3'` to confirm; otherwise substitute a tiny Rust helper binary under `crates/yach-bench/src/bin/` — prefer `python3` since the devenv is Nix and it is present on this host.

- [ ] **Step 2: Implement**

```rust
#[must_use]
pub fn kib_to_bytes(kib: i64) -> u64 {
    u64::try_from(kib).unwrap_or(0).saturating_mul(1024)
}

#[cfg(target_os = "linux")]
pub fn peak_rss_bytes(command: &mut Command, boundary: StopBoundary, timeout: Duration) -> Result<u64, String> {
    use std::io::Read as _;
    command.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("missing stdout")?;
    let reached = match boundary {
        StopBoundary::FirstOutputByte => {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || { let mut b = [0u8; 1]; let _ = tx.send(stdout.read(&mut b).map(|n| n > 0)); });
            rx.recv_timeout(timeout).map_err(|_| "timeout waiting for first output byte".to_owned())?.map_err(|e| e.to_string())?
        }
        StopBoundary::TraceLabel { path, label } => wait_for_label(&path, label, timeout)?,
    };
    if !reached { return Err(String::from("child exited before boundary")); }
    let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
    // SAFETY: pid is our own child; SIGKILL then reap with wait4 for rusage.
    let (status, usage) = unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status: libc::c_int = 0;
        let mut usage: libc::rusage = std::mem::zeroed();
        let rc = libc::wait4(pid, &mut status, 0, &mut usage);
        (rc, usage)
    };
    if status < 0 { return Err(String::from("wait4 failed")); }
    Ok(kib_to_bytes(usage.ru_maxrss))
}

#[cfg(not(target_os = "linux"))]
pub fn peak_rss_bytes(_command: &mut Command, _boundary: StopBoundary, _timeout: Duration) -> Result<u64, String> {
    Err(String::from("unsupported_os"))
}
```

`wait_for_label` polls the trace file every 5 ms with `yach_trace::parse_records`, tolerating `TruncatedLine` while polling, until a record with the label appears or the timeout elapses. `wait4` reaps the child; do not call `child.wait()` afterwards (it would fail on the reaped pid). `Child`'s `Drop` does not wait, so letting `child` drop is correct.

- [ ] **Step 3: PTY spawn helper**

`ru_maxrss` from `wait4` is the reaped child's own peak, not its descendants'. Spawning through `script` would measure `script`. Add to `rss.rs` (Linux only):

```rust
#[cfg(target_os = "linux")]
pub fn spawn_on_pty(command: &mut Command) -> Result<(Child, std::fs::File), String> {
    use std::os::unix::io::FromRawFd as _;
    use std::os::unix::process::CommandExt as _;
    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;
    // SAFETY: openpty writes two fds; null for name/termios/winsize is documented.
    let rc = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null()) };
    if rc != 0 { return Err(String::from("openpty failed")); }
    // SAFETY: pre_exec runs in the child before exec; the fds are valid.
    unsafe {
        command.pre_exec(move || {
            libc::setsid();
            libc::ioctl(slave, libc::TIOCSCTTY, 0);
            libc::dup2(slave, 0);
            libc::dup2(slave, 1);
            libc::dup2(slave, 2);
            libc::close(slave);
            libc::close(master);
            Ok(())
        });
    }
    let child = command.spawn().map_err(|e| e.to_string())?;
    // SAFETY: master is an open fd we own; slave is closed in the parent below.
    let master_file = unsafe { std::fs::File::from_raw_fd(master) };
    // SAFETY: slave fd is ours and no longer needed in the parent.
    unsafe { libc::close(slave) };
    Ok((child, master_file))
}
```

Extend `peak_rss_bytes` with a `Spawn { Piped, Pty }` parameter: `Pty` uses `spawn_on_pty` and reads the first byte from the master file; `Piped` is the existing path. Register in `workloads/startup.rs`:

```rust
Workload {
    id: "memory/peak_rss/tui_ready",
    class: Class::Memory,
    isolation: Isolation::ChildProcess,
    requires: &[Requirement::Binary, Requirement::Linux],
    bin: Some(Bin::Shipping),
    run: |ctx| {
        let bin = ctx.yach_bin.as_ref().ok_or("yach binary path missing")?;
        let mut samples = Vec::with_capacity(ctx.samples);
        for _ in 0..ctx.samples {
            let mut cmd = std::process::Command::new(bin);
            cmd.arg("tui-bench-ready");
            samples.push(crate::perf::rss::peak_rss_bytes(
                &mut cmd,
                crate::perf::rss::Spawn::Pty,
                crate::perf::rss::StopBoundary::FirstOutputByte,
                std::time::Duration::from_secs(5),
            )?);
        }
        Ok(Measured::Memory(samples))
    },
},
```

The existing first-output PTY startup samplers keep using `script`; they measure latency, and `script` is part of the boundary those ids have always described.

- [ ] **Step 4: Run tests and smoke**

Run: `just dev cargo test -p yach-bench perf::rss` then `just dev cargo run -p yach-bench --release -- perf run --filter 'memory/peak_rss/tui_ready' --samples 5 --out /tmp/rss.json && jq '.workloads[0].value' /tmp/rss.json`
Expected: tests pass; value is tens of MB (a plausible TUI RSS), not a few hundred KB (which would indicate measuring the wrong process).

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add wait4 peak RSS sampler and memory/peak_rss/tui_ready workload"
```

---

### Task 11: Thresholds and verdicts

**Files:**
- Create: `crates/yach-bench/src/perf/thresholds.rs`, `crates/yach-bench/src/perf/verdict.rs`, `crates/yach-bench/perf-thresholds.toml`

**Interfaces:**
- Produces:
  - `thresholds::Thresholds::load(path: &Path) -> Result<Thresholds, String>`; `Thresholds::for_id(&self, id: &str) -> Budget { latency_pct: f64, memory_pct: f64, size_pct: f64, count: u64 }`; `Thresholds::unmatched_rows(&self, ids: &[&str]) -> Vec<String>`.
  - `verdict::Verdict { Regressed, Improved, Unchanged, Inconclusive, Skipped, Error, Added, Removed, NoBaseWorker }`.
  - `verdict::RoundStat { base: f64, current: f64 }`; `verdict::judge_latency(rounds: &[RoundStat], threshold_pct: f64) -> (Verdict, Detail)`; `verdict::judge_value(base: u64, current: u64, budget_pct: Option<f64>, budget_abs: Option<u64>) -> (Verdict, Detail)`; `Detail { median_delta_pct: f64, sign_agreement: f64, base_spread_pct: f64 }`.

- [ ] **Step 1: Write the failing tests**

`verdict.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{RoundStat, Verdict, judge_latency, judge_value};

    fn rounds(pairs: &[(f64, f64)]) -> Vec<RoundStat> {
        pairs.iter().map(|&(base, current)| RoundStat { base, current }).collect()
    }

    #[test]
    fn regressed_when_median_over_threshold_and_signs_agree() {
        let (v, d) = judge_latency(&rounds(&[(100.0, 108.0), (100.0, 107.0), (100.0, 109.0), (100.0, 106.0), (100.0, 110.0)]), 5.0);
        assert_eq!(v, Verdict::Regressed);
        assert!(d.median_delta_pct > 5.0);
    }

    #[test]
    fn improved_symmetric() {
        let (v, _) = judge_latency(&rounds(&[(100.0, 92.0), (100.0, 93.0), (100.0, 91.0), (100.0, 94.0), (100.0, 92.0)]), 5.0);
        assert_eq!(v, Verdict::Improved);
    }

    #[test]
    fn inconclusive_when_signs_disagree() {
        let (v, _) = judge_latency(&rounds(&[(100.0, 110.0), (100.0, 90.0), (100.0, 112.0), (100.0, 88.0), (100.0, 111.0)]), 5.0);
        assert_eq!(v, Verdict::Inconclusive);
    }

    #[test]
    fn inconclusive_when_base_spread_exceeds_threshold_and_delta_within() {
        let (v, d) = judge_latency(&rounds(&[(100.0, 101.0), (120.0, 121.0), (90.0, 91.0), (100.0, 100.0), (110.0, 111.0)]), 5.0);
        assert_eq!(v, Verdict::Inconclusive);
        assert!(d.base_spread_pct > 5.0);
    }

    #[test]
    fn unchanged_within_threshold_and_quiet_base() {
        let (v, _) = judge_latency(&rounds(&[(100.0, 101.0), (101.0, 100.0), (100.0, 102.0), (100.0, 99.0), (101.0, 101.0)]), 5.0);
        assert_eq!(v, Verdict::Unchanged);
    }

    #[test]
    fn value_compare_uses_pct_or_abs() {
        assert_eq!(judge_value(1000, 1004, Some(0.5), None).0, Verdict::Unchanged);
        assert_eq!(judge_value(1000, 1006, Some(0.5), None).0, Verdict::Regressed);
        assert_eq!(judge_value(10, 11, None, Some(0)).0, Verdict::Regressed);
        assert_eq!(judge_value(10, 9, None, Some(0)).0, Verdict::Improved);
        assert_eq!(judge_value(10, 10, None, Some(0)).0, Verdict::Unchanged);
    }
}
```

`thresholds.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::Thresholds;

    const TOML: &str = r#"
[defaults]
latency_pct = 5.0
memory_pct = 10.0
size_pct = 0.5
count = 0

[[workload]]
id = "turn/scripted/*"
latency_pct = 8.0
comment = "tokio scheduling"

[[workload]]
id = "turn/scripted/tools_4/*"
latency_pct = 12.0
"#;

    #[test]
    fn most_specific_glob_wins_and_defaults_fill() {
        let t = Thresholds::parse(TOML);
        let Ok(t) = t else { return };
        assert_eq!(t.for_id("request/assemble/10_turns").latency_pct, 5.0);
        assert_eq!(t.for_id("turn/scripted/text_only").latency_pct, 8.0);
        assert_eq!(t.for_id("turn/scripted/tools_4/builtin").latency_pct, 12.0);
        assert_eq!(t.for_id("turn/scripted/tools_4/builtin").memory_pct, 10.0);
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(Thresholds::parse("[defaults]\nlatency_pct = 5.0\nmemory_pct = 1.0\nsize_pct = 1.0\ncount = 0\n[[workload]]\nid = \"x\"\naccept = \"nope\"\n").is_err());
    }

    #[test]
    fn unmatched_rows_are_reported() {
        let Ok(t) = Thresholds::parse(TOML) else { return };
        let unmatched = t.unmatched_rows(&["request/assemble/10_turns", "turn/scripted/text_only"]);
        assert_eq!(unmatched, vec![String::from("turn/scripted/tools_4/*")]);
    }
}
```

- [ ] **Step 2: Implement `thresholds.rs`**

serde structs with `#[serde(deny_unknown_fields)]`; `comment: Option<String>`. Specificity = length of the glob's literal prefix before the first `*`/`?`/`[`; ties → later row wins. `for_id` starts from defaults and overlays the winning row's `Some` fields. `unmatched_rows` returns rows whose glob matches no id. Use the `glob` crate (add `glob = "0.3"` to `Cargo.toml`).

- [ ] **Step 3: Implement `verdict.rs`**

```rust
pub fn judge_latency(rounds: &[RoundStat], threshold_pct: f64) -> (Verdict, Detail) {
    if rounds.is_empty() { return (Verdict::Error, Detail::default()); }
    let mut deltas: Vec<f64> = rounds.iter().map(|r| (r.current / r.base - 1.0) * 100.0).collect();
    deltas.sort_by(f64::total_cmp);
    let median = deltas[deltas.len() / 2];
    let sign = median.signum();
    let agree = deltas.iter().filter(|d| d.signum() == sign).count() as f64 / deltas.len() as f64;
    let mut bases: Vec<f64> = rounds.iter().map(|r| r.base).collect();
    bases.sort_by(f64::total_cmp);
    let base_median = bases[bases.len() / 2];
    let spread = (bases[bases.len() - 1] - bases[0]) / base_median * 100.0;
    let detail = Detail { median_delta_pct: median, sign_agreement: agree, base_spread_pct: spread };
    let over = median.abs() > threshold_pct;
    let verdict = match (over, agree >= 0.8, spread > threshold_pct) {
        (true, true, _) if median > 0.0 => Verdict::Regressed,
        (true, true, _) => Verdict::Improved,
        (true, false, _) => Verdict::Inconclusive,
        (false, _, true) => Verdict::Inconclusive,
        (false, _, false) => Verdict::Unchanged,
    };
    (verdict, detail)
}
```

`judge_value`: pct budget → `|current-base|/base*100 > pct`; abs budget → `|current-base| > abs`; direction by sign; `base == 0` with pct → compare abs difference to 0.

- [ ] **Step 4: Write `perf-thresholds.toml`**

```toml
# Numeric budgets per workload id glob. Most specific (longest literal
# prefix) row wins; fields not set fall back to [defaults]. There is no
# reason-only waiver: an intentional regression raises the budget here,
# in the same change, so the reviewer sees it against a named workload.
# `comment` is documentation only.

[defaults]
latency_pct = 5.0
memory_pct = 10.0
size_pct = 0.5
count = 0

[[workload]]
id = "turn/scripted/*"
latency_pct = 8.0
comment = "scripted turns include tokio scheduling; wider budget"

[[workload]]
id = "yach/*_pty"
latency_pct = 10.0
comment = "PTY first-output includes `script` and terminal setup"

[[workload]]
id = "startup/phase/*"
latency_pct = 10.0

[[workload]]
id = "*#alloc_count"
count = 0

[[workload]]
id = "*#alloc_bytes"
count = 0
```

- [ ] **Step 5: Run tests**

Run: `just dev cargo test -p yach-bench perf::thresholds perf::verdict`
Expected: 9 passed.

- [ ] **Step 6: Commit**

```bash
jj commit -m "Add perf thresholds file and verdict rules"
```

---

### Task 12: Core-loop workloads

**Files:**
- Create: `crates/yach-bench/src/perf/workloads/core_loop.rs`
- Modify: `crates/yach-bench/src/perf/workloads/mod.rs`, `crates/yach-bench/src/perf/registry.rs` (`all()` includes `core_loop::CORE_LOOP`), `crates/yach-bench/src/perf/worker.rs` (move the `request/assemble/*` entries registered there into this module if Task 9 placed them)

**Interfaces:**
- Consumes: `yach_backend::request_assembly::{fixture_log, assemble}`, `yach_backend::tools::advertised_roster_bytes`, `yach_backend::bench_loop::{Script, ScriptedTurnConfig, run_scripted_turn}`, `rss::{peak_rss_bytes, StopBoundary}`, `yach_trace::parse_records`, `RunCtx.yach_bench_bin` (Bench binary).
- Produces workloads: `request/assemble/{10,100,1000}_turns`, `request/roster_bytes/builtin`, `request/roster_bytes/hashline_ext`, `provider/encode/{anthropic,openai_responses}/100_turns`, `turn/scripted/text_only`, `turn/scripted/tools_4/builtin`, `turn/scripted/tools_4/hashline_ext`, `turn/scripted/tools_4/inactive_ext_8`, `turn/phase/<label>`, `memory/peak_rss/turn_scripted_tools_4`.

- [ ] **Step 1: In-process serial workloads**

```rust
fn assemble_workload(turns: usize, ctx: &RunCtx) -> Result<Measured, String> {
    let log = yach_backend::request_assembly::fixture_log(turns, 1);
    let current = yach_backend::TurnId(format!("turn-{}", turns + 1));
    let mut samples = Vec::with_capacity(ctx.samples);
    let window = AllocWindow::begin();
    for _ in 0..ctx.samples {
        let start = Instant::now();
        let messages = yach_backend::request_assembly::assemble(&log, &current, None);
        samples.push(start.elapsed());
        std::hint::black_box(messages);
    }
    let alloc = window.end();
    Ok(Measured::Latency { samples, alloc: Some(alloc) })
}
```

Register three entries with ids `request/assemble/10_turns` etc. (each `run` is a capture-free `fn` calling `assemble_workload(10, ctx)`).

`request/roster_bytes/builtin`: build the catalog as in Task 4's test and return `Measured::Value(advertised_roster_bytes(&definitions)? as u64)`. `request/roster_bytes/hashline_ext`: same registry, then apply the hashline replacement bundle — read `crates/yach-hashline-extension/yach.extension.json`'s provider-visible tools (`hashline_read`, `hashline_edit`) and replaced names (`read_text_file`, `edit_text_file`); construct `ToolDefinition`s for the hashline tools the same way the extension resolver does (`extension.rs:656-738`); if that requires private types, expose a `bench`-gated `yach_backend::extension::hashline_bundle_definitions() -> Vec<ToolDefinition>` in this task and use it.

`provider/encode/*/100_turns`: the scout found no pure wire-body encoder. Benchmark the adapter-side conversion that exists without a client: expose `pub fn rig_messages_from_request(request: &ProviderRequest) -> …` (currently private at `rig_adapter.rs:755-758`) under `#[cfg(feature = "bench")] pub` and time it plus `rig_tool_definitions_from_request` for a 100-turn assembled request; ids `provider/encode/rig_messages/100_turns` and `provider/encode/rig_tools/100_turns`. Update the spec's workload table to these two ids in the same commit (the spec named provider-specific encoders that do not exist as seams; the record of why is this task).

`turn/scripted/text_only` and `turn/scripted/tools_4/builtin`: per sample, create a temp project with `src/lib.rs`, call `run_scripted_turn` with `Script::text_only("ok")` / `Script::read_tool_calls(&["src/lib.rs"; 4], "done")`, push `profile.wall`, remove the temp dir. Wrap only the `run_scripted_turn` call in the `AllocWindow` (window per sample, sum the counts).

- [ ] **Step 2: Child-process turn workloads**

Helper `scripted_child(ctx: &RunCtx, script: &Script, extra_env: &[(&str, String)], trace_path: &Path) -> Result<Duration, String>`: writes the script JSON to a temp file, spawns the Bench `yach` binary (`ctx.yach_bench_yach_bin`, i.e. `target/bench/release/yach`; `Bin::Bench`) with `run --prompt "<prompt>"` in a temp project, env `YACH_RIG_PROVIDER=scripted`, `YACH_BENCH_SCRIPT=<file>`, `YACH_TRACE=<trace_path>`, waits for exit with a 30 s timeout, returns wall time.

- `turn/scripted/tools_4/hashline_ext`: additionally `YACH_EXTENSION_PACKAGE_ROOTS=<materialized hashline package root>`; the CLI auto-materializes the bundled hashline manifest under `$HOME/.yach/bundled/...` (`main.rs:4048-4114`), so set `HOME` to a temp dir and let the CLI materialize it; the script's tool names remain `read_text_file` (the replacement bundle maps them).
- `turn/scripted/tools_4/inactive_ext_8`: `YACH_EXTENSION_PACKAGE_ROOTS` pointing at 8 generated toy packages (reuse `ExtensionManifestPackageRoot` from `workloads/startup.rs`).
- `turn/phase/<label>`: from the `tools_4/builtin` child run's trace file, for each turn label compute `t_us(label) - t_us(prompt_received)`; register one workload per label in the spec's list (`request_assembled`, `provider_request_sent`, `provider_first_event`, `provider_stream_end`, `tool_dispatched` (use `n == 4`), `tool_result_appended` (`n == 4`), `session_persisted`, `turn_completed`), sharing one child run per sample via a `OnceLock<Mutex<…>>` cache keyed by `ctx.samples`, as the startup profile family does.
- `memory/peak_rss/turn_scripted_tools_4`: `rss::peak_rss_bytes(cmd, StopBoundary::TraceLabel { path, label: "turn_completed" }, 30s)` over the same Bench-binary command; `requires: &[Binary, Linux]`.

- [ ] **Step 3: Tests**

```rust
#[cfg(test)]
mod tests {
    use crate::perf::registry::{RunCtx, all};
    use crate::perf::schema::Class;

    #[test]
    fn core_loop_ids_registered() {
        let ids: Vec<&str> = all().iter().map(|w| w.id).collect();
        for id in ["request/assemble/10_turns", "request/assemble/1000_turns", "request/roster_bytes/builtin", "turn/scripted/text_only", "turn/scripted/tools_4/builtin", "turn/phase/turn_completed", "memory/peak_rss/turn_scripted_tools_4"] {
            assert!(ids.contains(&id), "missing {id}");
        }
    }

    #[test]
    fn roster_bytes_builtin_is_positive() {
        let w = all().iter().find(|w| w.id == "request/roster_bytes/builtin");
        let Some(w) = w else { return };
        let ctx = RunCtx { samples: 1, yach_bin: None, yach_bench_yach_bin: None, yach_bench_bin: None, filter: None };
        let measured = (w.run)(&ctx);
        assert!(matches!(measured, Ok(crate::perf::registry::Measured::Value(v)) if v > 1000));
        assert_eq!(w.class, Class::Count);
    }

    #[test]
    fn scripted_text_turn_measures_in_process() {
        let w = all().iter().find(|w| w.id == "turn/scripted/text_only");
        let Some(w) = w else { return };
        let ctx = RunCtx { samples: 2, yach_bin: None, yach_bench_yach_bin: None, yach_bench_bin: None, filter: None };
        let Ok(crate::perf::registry::Measured::Latency { samples, alloc }) = (w.run)(&ctx) else { return };
        assert_eq!(samples.len(), 2);
        assert!(alloc.is_some_and(|a| a.count > 0));
    }
}
```

- [ ] **Step 4: Run tests and smoke the child workloads**

Run: `just dev cargo test -p yach-bench perf::workloads::core_loop`, then
`just dev cargo run -p yach-bench --release -- perf run --filter 'turn/*' --samples 5 --out /tmp/turn.json && jq -r '.workloads[] | "\(.id) \(.status) \(.p95_ns)"' /tmp/turn.json`
Expected: every row `ok`; `turn/phase/turn_completed` p95 within a few ms of `turn/scripted/tools_4/builtin` measured in-process plus process startup.

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add core-loop perf workloads: request assembly, roster bytes, scripted turns, turn phases"
```

---

### Task 13: `perf ab` controller

**Files:**
- Create: `crates/yach-bench/src/perf/ab.rs`
- Modify: `crates/yach-bench/src/perf/mod.rs`, `crates/yach-bench/src/perf/schema.rs` (add `AbDoc { schema, base_mode, base: Vec<ResultDoc>, current: Vec<ResultDoc>, verdicts: Vec<VerdictRow> }`, `VerdictRow { id, class, verdict, detail: Option<Detail>, base_summary: Option<f64>, current_summary: Option<f64>, budget: Budget }`), `.gitignore` (`/.perf/`)

**Interfaces:**
- Produces: `perf ab [--base <rev> | --base-dir <path>] [--base-mode auto|worker|external] [--filter] [--rounds R] [--samples N] [--deterministic] [--thresholds <path>] --out <file>`; `ab::Side { checkout: PathBuf, target_dir: PathBuf, yach_bin, yach_bench_yach_bin, yach_bench_bin }`; `ab::materialize_base(rev: &str) -> Result<PathBuf, String>`; `ab::build_side(checkout: &Path, target: &Path) -> Result<Side, String>`; `ab::probe_worker(bench_bin: &Path) -> Option<u32>`; `ab::run(opts) -> Result<(AbDoc, u8 /*exit*/), String>`.
- `ab::EXTERNAL_IDS: [&str; 5]` = `binary/size_bytes`, `yach/tui_startup_first_output_pty`, `yach/tui_ready_startup_first_output_pty`, `yach/cli_startup_first_output`, `memory/peak_rss/tui_ready`. In external mode the controller spawns `<current yach-bench> perf external-sampler --yach-bin <base shipping yach> --checkout <base dir> …` for each base slot; the controller process itself never measures.
- `ab::build_side(checkout, target, stage: BuildStage)` with `BuildStage { Shipping, BenchAndWorker }`: `Shipping` builds `-p yach` (no features) and `-p yach-bench`; `BenchAndWorker` additionally builds `-p yach --features bench` into `<target>/bench`. The controller builds base with `Shipping` first, probes, and only builds `BenchAndWorker` for base when the probe succeeds — a pre-framework base has neither `perf worker` nor a CLI `bench` feature, and its bench-feature build would fail for a reason that is not a compiler regression. Current always gets both stages. Build failures are always hard errors; nothing is `|| true`.

- [ ] **Step 1: Write the failing orchestration test**

Uses a stub worker: a shell script written to a temp dir that echoes a canned `ResultDoc` and records its invocation order.

```rust
#[cfg(test)]
mod tests {
    use super::{AbOptions, BaseMode, Side, run_with_sides};
    use crate::perf::schema::{Class, Isolation};
    use crate::perf::verdict::Verdict;
    use std::path::PathBuf;

    fn stub_worker(dir: &std::path::Path, name: &str, schema: u32, p95_ns: u64, log: &std::path::Path) -> PathBuf {
        let path = dir.join(name);
        let body = format!(
            "#!/bin/bash\necho \"{name}\" >> {log}\nif [[ \"$1\" == \"perf\" && \"$2\" == \"worker\" && \"$3\" == \"--schema-probe\" ]]; then echo '{{\"schema\":{schema}}}'; exit 0; fi\n\
             out=; while [[ $# -gt 0 ]]; do if [[ $1 == --out ]]; then out=$2; fi; shift; done\n\
             cat > \"$out\" <<EOF\n{{\"schema\":{schema},\"host\":{{\"fingerprint\":\"f\",\"cpu\":\"c\",\"cores\":1,\"os\":\"o\",\"kernel\":\"k\"}},\"build\":{{\"source_sha256\":\"s\",\"commit\":null,\"dirty\":true,\"profile\":\"release\",\"rustc\":\"r\",\"cargo_lock_sha256\":\"l\",\"yach_bin_sha256\":null}},\"started_at\":\"t\",\"workloads\":[{{\"id\":\"request/assemble/10_turns\",\"class\":\"latency\",\"isolation\":\"in_process_serial\",\"status\":\"ok\",\"count\":10,\"p50_ns\":{p95_ns},\"p95_ns\":{p95_ns},\"p99_ns\":{p95_ns},\"max_ns\":{p95_ns}}}]}}\nEOF\necho '{{\"schema\":{schema},\"workloads\":1}}'\n",
            log = log.display()
        );
        let _ = std::fs::write(&path, body);
        let _ = std::process::Command::new("chmod").arg("+x").arg(&path).status();
        path
    }

    fn side(dir: &std::path::Path, bench: PathBuf) -> Side {
        Side { checkout: dir.to_path_buf(), target_dir: dir.join("target"), yach_bin: dir.join("yach"), yach_bench_yach_bin: dir.join("yach-bench-yach"), yach_bench_bin: bench }
    }

    #[test]
    fn abba_order_and_regression_verdict() {
        let dir = std::env::temp_dir().join(format!("yach-ab-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = stub_worker(&dir, "base-worker", 1, 100_000, &log);
        let current = stub_worker(&dir, "current-worker", 1, 120_000, &log);
        let opts = AbOptions { rounds: 2, samples: 5, filter: None, deterministic: false, base_mode: BaseMode::Auto, thresholds: crate::perf::thresholds::Thresholds::defaults(), out: dir.join("ab.json") };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let order = std::fs::read_to_string(&log).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        let Ok((doc, code)) = result else { return };
        let calls: Vec<&str> = order.lines().filter(|l| !l.is_empty()).collect();
        // probe, probe, then 2 rounds × ABBA
        assert_eq!(&calls[2..], &["base-worker", "current-worker", "current-worker", "base-worker", "base-worker", "current-worker", "current-worker", "base-worker"]);
        assert_eq!(doc.verdicts[0].verdict, Verdict::Regressed);
        assert_eq!(code, 1);
    }

    #[test]
    fn schema_mismatch_is_hard_error() {
        let dir = std::env::temp_dir().join(format!("yach-ab-schema-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = stub_worker(&dir, "base-worker", 7, 1, &log);
        let current = stub_worker(&dir, "current-worker", 1, 1, &log);
        let opts = AbOptions { rounds: 1, samples: 1, filter: None, deterministic: false, base_mode: BaseMode::Worker, thresholds: crate::perf::thresholds::Thresholds::defaults(), out: dir.join("ab.json") };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_err_and(|e| e.contains("schema")));
    }

    #[test]
    fn missing_worker_falls_back_to_external_with_no_base_worker_rows() {
        let dir = std::env::temp_dir().join(format!("yach-ab-ext-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("order.log");
        let base = dir.join("old-bench");
        let _ = std::fs::write(&base, "#!/bin/bash\nexit 2\n");
        let _ = std::process::Command::new("chmod").arg("+x").arg(&base).status();
        let current = stub_worker(&dir, "current-worker", 1, 1, &log);
        let opts = AbOptions { rounds: 1, samples: 1, filter: glob::Pattern::new("request/*").ok(), deterministic: false, base_mode: BaseMode::Auto, thresholds: crate::perf::thresholds::Thresholds::defaults(), out: dir.join("ab.json") };
        let result = run_with_sides(&opts, &side(&dir, base), &side(&dir, current));
        let _ = std::fs::remove_dir_all(&dir);
        let Ok((doc, code)) = result else { return };
        assert_eq!(doc.base_mode, "external");
        assert_eq!(doc.verdicts[0].verdict, Verdict::NoBaseWorker);
        assert_eq!(code, 0);
    }
}
```

`Thresholds::defaults()` returns the built-in defaults with no rows (add it in `thresholds.rs`).

- [ ] **Step 2: Implement**

- `materialize_base(rev)`: `jj workspace list`; if `.perf/base` absent → `jj workspace add --name perf-base .perf/base -r <rev>`; else `jj -R .perf/base workspace update-stale` then `jj -R .perf/base new <rev>`. Return path. Error if `jj` missing.
- `build_side(checkout, target, stage)`: run, from `checkout`, `just dev cargo build --release --locked -p yach` with `CARGO_TARGET_DIR=<target>/release-root` and `… -p yach-bench` with the same target dir (`Shipping`); for `BenchAndWorker`, also `… -p yach --features bench` with `CARGO_TARGET_DIR=<target>/bench`. Capture `rustc --version` via `just dev rustc --version` in each checkout; mismatch → error. Return `Side` (with `yach_bench_yach_bin: None` after `Shipping` only).
- `probe_worker(bench)`: run `<bench> perf worker --schema-probe`, parse `{"schema":n}`; non-zero exit or unparsable → `None`.
- `run_with_sides(opts, base, current)`: determine mode (`Auto` → `Worker` if probe returns `Some(SCHEMA)`, hard error if `Some(other)`, `External` if `None`); for `Worker`, each slot = `worker::spawn(<side.yach_bench_bin>, WorkerArgs { schema: 1, filter, samples, yach_bin, yach_bench_yach_bin, deterministic, out: <tmp> })`; for `External`, base slots = `worker::spawn(<current.yach_bench_bin>, WorkerArgs::external_sampler { filter ∩ EXTERNAL_IDS, samples, yach_bin: base.yach_bin, checkout: base.checkout })`, and every current-side id not in `EXTERNAL_IDS` gets verdict `NoBaseWorker`. Rounds: ABBA as specified; `size`/`count` rows run only in round 1. After `R` rounds compute verdicts via Task 11; for `Inconclusive` latency/memory rows run up to two extra ABBA rounds filtered to those ids and re-judge. Ids only on one side → `Added`/`Removed`. `Thresholds::unmatched_rows` over the union of ids → hard error listing them. Exit code per spec. Write `AbDoc` to `opts.out`. Print the table (reuse the Task 9 renderer plus a `verdict` column and `Δ%`).
- `run(opts)`: materialize base (unless `--base-dir`), `build_side(base, Shipping)`, `probe_worker`, then `build_side(base, BenchAndWorker)` only if the probe succeeded, `build_side(current, Shipping)` + `build_side(current, BenchAndWorker)`, rustc check, then `run_with_sides`. `--base-dir` in CI: the same sequence, from the given path.

- [ ] **Step 3: Run tests**

Run: `just dev cargo test -p yach-bench perf::ab`
Expected: 3 passed.

- [ ] **Step 4: Smoke against real main (external mode expected)**

Run: `just dev cargo run -p yach-bench --release -- perf ab --base main --rounds 1 --samples 5 --out /tmp/ab.json`
Expected: prints `base-mode: external`; five rows with verdicts, the rest `no_base_worker`; exit 0 or 2 (a 5-sample single round may be inconclusive; that is acceptable for the smoke, the acceptance run in Task 15 uses full defaults).

- [ ] **Step 5: Commit**

```bash
jj commit -m "Add perf ab controller with worker protocol and external bootstrap mode"
```

---

### Task 14: `perf report`, recipes, CI, docs

**Files:**
- Create: `crates/yach-bench/src/perf/report.rs`
- Modify: `crates/yach-bench/src/perf/mod.rs`, `justfile`, `.github/workflows/ci.yml`, `docs/benchmarks/README.md`

**Interfaces:**
- Produces: `perf report <results.json> [<ab.json>]` → Markdown on stdout, sections: header (date, commit, dirty, source digest, host, rustc, profile), command line, per-class tables; with an `AbDoc`: verdict table with `Δ%`, sign agreement, base spread, budget. `report::render(doc: &ResultDoc, ab: Option<&AbDoc>) -> String`.
- `just perf [filter]`, `just perf-record`, `just perf-report <json> [<ab.json>]`, `just perf-profile <id> [samples]`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::render;
    use crate::perf::schema::{BuildInfo, HostInfo, Isolation, ResultDoc, SCHEMA, WorkloadRow};
    use std::time::Duration;

    #[test]
    fn renders_required_report_sections() {
        let doc = ResultDoc {
            schema: SCHEMA,
            host: HostInfo { fingerprint: String::from("abcd"), cpu: String::from("cpu"), cores: 24, os: String::from("Linux"), kernel: String::from("6.18") },
            build: BuildInfo { source_sha256: String::from("s"), commit: Some(String::from("0123456789abcdef")), dirty: false, profile: String::from("release"), rustc: String::from("rustc 1.90"), cargo_lock_sha256: String::from("l"), yach_bin_sha256: None },
            started_at: String::from("2026-09-08T20:00:00Z"),
            workloads: vec![WorkloadRow::latency("request/assemble/10_turns", Isolation::InProcessSerial, &[Duration::from_micros(10), Duration::from_micros(20)])],
        };
        let md = render(&doc, None);
        for needle in ["# Performance Report", "**Date:** 2026-09-08", "**Commit:** 0123456789abcdef", "**Machine/environment:** cpu", "| request/assemble/10_turns |", "p95"] {
            assert!(md.contains(needle), "missing {needle}\n{md}");
        }
    }
}
```

- [ ] **Step 2: Implement `report.rs`** following `docs/benchmarks/README.md`'s "Minimum report contents" (Date, Commit SHA, Machine/environment, Command or harness, Build/profile mode, Workload, Results, Comparison target, Claim supported, Confidence/limitations, Follow-up — the last three as empty headed sections for the author to fill). Move `render_duration` here and have the worker/ab table renderers call it.

- [ ] **Step 3: justfile recipes**

Append after `lint:`:

```make
# Paired regression gate: build main and @ through the dev shell, run
# interleaved ABBA rounds, judge against crates/yach-bench/perf-thresholds.toml.
# Exit 1 on error/regressed, 2 on inconclusive. See
# docs/project/specs/2026-09-08-performance-measurement-framework-design.md.
perf filter="*":
  mkdir -p .perf/results
  just --justfile "{{justfile()}}" dev cargo run -p yach-bench --release --locked -- perf ab --base main --filter "{{filter}}" --out .perf/results/$(date +%Y%m%dT%H%M%S)-ab.json

# Record @ only (trend evidence, never a gate input).
perf-record:
  #!/usr/bin/env bash
  set -euo pipefail
  # `perf run` builds the current checkout's artifacts itself (Task 9).
  fp="$(just --justfile "{{justfile()}}" dev cargo run -p yach-bench --release --locked -- perf host-fingerprint)"
  dir="${XDG_CACHE_HOME:-$HOME/.cache}/yach/perf/$fp"; mkdir -p "$dir"
  out="$dir/$(date +%Y-%m-%d)-$(git rev-parse --short HEAD 2>/dev/null || echo nogit).json"
  just --justfile "{{justfile()}}" dev cargo run -p yach-bench --release --locked -- perf run --out "$out"
  echo "$out"

perf-report results ab="":
  just --justfile "{{justfile()}}" dev cargo run -p yach-bench --release --locked -- perf report "{{results}}" {{ab}}

# Flamegraph one in-process workload. Linux: perf + inferno; macOS: samply.
perf-profile id samples="1000":
  #!/usr/bin/env bash
  set -euo pipefail
  CARGO_PROFILE_RELEASE_DEBUG=1 just --justfile "{{justfile()}}" dev cargo build --release --locked -p yach-bench
  bin=target/release/yach-bench
  if [[ "$(uname -s)" == "Linux" ]]; then
    just --justfile "{{justfile()}}" dev perf record -g -o .perf/profile.data -- "$bin" perf worker --schema 1 --filter "{{id}}" --samples "{{samples}}" --yach-bin target/release/yach --yach-bench-yach-bin target/bench/release/yach --out .perf/profile-run.json
    just --justfile "{{justfile()}}" dev-shell 'perf script -i .perf/profile.data | inferno-collapse-perf | inferno-flamegraph > .perf/flamegraph.svg'
    echo .perf/flamegraph.svg
  else
    just --justfile "{{justfile()}}" dev samply record -- "$bin" perf worker --schema 1 --filter "{{id}}" --samples "{{samples}}" --yach-bin target/release/yach --yach-bench-yach-bin target/bench/release/yach --out .perf/profile-run.json
  fi
```

Add a `perf host-fingerprint` subcommand (prints `capture_host().fingerprint`). `perf` and `inferno` must be in the dev shell: add `perf-tools`/`linuxPackages.perf` and `inferno` to `devenv.nix` packages if the repo declares one (`glob devenv.nix flake.nix`); if the dev environment is a flake, add them there. Note the addition in the commit.

- [ ] **Step 4: CI job**

The job activates the repository's declared environment (`flake.nix` / `devenv.nix` / `.envrc`) so `just dev cargo …` is the build path in CI exactly as locally; bare `cargo` is not used. Append to `.github/workflows/ci.yml`:

```yaml
  perf-deterministic:
    name: Perf (deterministic)
    runs-on: ubuntu-latest
    if: github.event_name == 'pull_request'
    env:
      CARGO_TERM_COLOR: always
    steps:
      - name: Checkout
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Install Nix
        uses: DeterminateSystems/nix-installer-action@v16

      - name: Cache Nix store
        uses: DeterminateSystems/magic-nix-cache-action@v8

      - name: Install just
        run: nix profile install nixpkgs#just

      - name: Materialize base
        run: git worktree add .perf/base "${{ github.event.pull_request.base.sha }}"

      - name: Compare
        run: just dev cargo run -p yach-bench --release --locked -- perf ab --base-dir .perf/base --deterministic --rounds 1 --out ab.json

      - name: Summarize
        if: always()
        run: just dev cargo run -p yach-bench --release --locked -- perf report ab.json >> "$GITHUB_STEP_SUMMARY"
```

`perf ab` does its own staged builds (base `Shipping`, probe, base `BenchAndWorker` only on a successful probe, current both), each through `just dev cargo build --release --locked …` run from the respective checkout root, so the base uses its own `justfile` and dev shell. Nothing in the job invokes `cargo` directly. Pin the two action versions to the latest releases at implementation time and note them in the commit message. `rustc` mismatch between the two dev shells (a toolchain bump in the PR) is reported by `perf ab` as a hard error; that is the intended signal — a toolchain bump is landed on its own, then the gate resumes.

- [ ] **Step 5: Rewrite `docs/benchmarks/README.md`**

Replace the "Current harnesses" list with the registry (`yach-bench perf run --list` — add this flag: prints ids, class, isolation, requires), the four recipes, the JSON schema summary, the verdict rules, the thresholds file, and the external-mode caveat. Keep "Performance targets from the PRD" and fill its status column from the first baseline (Task 15). Keep "Current reports".

- [ ] **Step 6: Run tests, then the real recipes**

Run: `just dev cargo test -p yach-bench perf::report` then `just perf-record` then `just perf-report <printed path>`
Expected: test passes; a JSON path is printed; Markdown renders.

- [ ] **Step 7: Commit**

```bash
jj commit -m "Add perf report, just recipes, deterministic CI job, and benchmark docs"
```

---

### Task 15: Baseline, full validation, spec alignment

**Files:**
- Create: `docs/benchmarks/baseline-<today>.md`
- Modify: `docs/benchmarks/README.md` (index the report, PRD table statuses), `docs/project/specs/2026-09-08-performance-measurement-framework-design.md` (Task 12 replaced `provider/encode/{anthropic,openai_responses}/100_turns` with `provider/encode/{rig_messages,rig_tools}/100_turns`; update the workload table and add one sentence under "Core-loop seams" saying no provider-specific wire encoder exists as a seam)

- [ ] **Step 1: Full-suite validation once**

Run: `just fmt-check && just lint && just test`
Expected: all pass. Fix anything the earlier focused runs missed.

- [ ] **Step 2: Acceptance smoke (external mode)**

Run: `just perf`
Expected: `base-mode: external`; verdicts for exactly `binary/size_bytes`, `yach/tui_startup_first_output_pty`, `yach/tui_ready_startup_first_output_pty`, `yach/cli_startup_first_output`, `memory/peak_rss/tui_ready`; every other row `no_base_worker`; exit 0, or exit 2 with `inconclusive` confined to the process-startup first-output rows (`yach/cli_startup_first_output`, `yach/tui_startup_first_output_pty`) when their base spread exceeds budget while their median delta stays inside it — record the numbers in the baseline report. Any `regressed`, any `error`, or an `inconclusive` on another row still fails.

- [ ] **Step 3: Record and render the baseline**

Run: `just perf-record` and `just perf-report <path> > docs/benchmarks/baseline-$(date +%F).md`. Then edit the report: fill "Claim supported" (narrow: current Linux numbers on this machine; no comparison), "Confidence/limitations" (external-mode gate partial once; live-terminal rows skipped in a non-TTY shell — run those rows via `script -q /dev/null just perf-record` if a TTY is available and note which run produced which rows), "Follow-up" (post-merge worker-mode self-comparison, tracked on plane:YACH-8).

Update the PRD target table statuses in `docs/benchmarks/README.md` from the numbers (`met`/`not met`/`unknown` with the workload id that answers each row).

- [ ] **Step 4: Commit**

```bash
jj commit -m "Record first Linux performance baseline and index it"
```

- [ ] **Step 5: Stack review**

Run: `jj log -r 'main..@'`
Expected: one spec commit, one plan commit, then Tasks 1–15 as separate commits with the messages above.
