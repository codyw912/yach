# SDD ledger — plan: docs/project/plans/2026-09-08-performance-measurement-framework.md

Spec: docs/project/specs/2026-09-08-performance-measurement-framework-design.md
Providers: roadmap plane:Yach (YACH-8); execution kata:yach; completion pull_request
Kata parent: yach#te2k
Baseline: 1285 passed, 0 failed (`just test`), trunk main@origin

Task 1: Kata yach#6yz1 — change wswnlvklrvqynwpqxpuzrnrytwsxwpnv (reused)
Task 2: Kata yach#6pr0
Task 3: Kata yach#bvtw
Task 4: Kata yach#hz4v
Task 5: Kata yach#m23p
Task 6: Kata yach#zxrs
Task 7: Kata yach#jach
Task 8: Kata yach#fw48
Task 9: Kata yach#djma
Task 10: Kata yach#pgz0
Task 11: Kata yach#81f7
Task 12: Kata yach#0njw
Task 13: Kata yach#a16t
Task 14: Kata yach#f07f
Task 15: Kata yach#7z6c

## Pre-flight scan

| Pair / task | Produces vs consumes | Finding |
|---|---|---|
| T1 → T2,T3,T8,T10,T12 | `yach_trace::{TraceSink, TraceScope, TraceRecord, parse_records}` | consistent |
| T2 → T3 | `RunnerConfig.trace: Option<TraceSink>` | consistent |
| T3 ↔ T5 | T3 says `ScriptedProvider` emits `provider_first_event`/`provider_stream_end` itself; T5's `ScriptedProvider` code has no trace field | **gap** — Ruling below |
| T3 ↔ Task 3 Files | Files list names `rig_adapter.rs:1184-1227`; step text now threads `trace` into `RigProviderRequester` | consistent |
| T4 → T9,T12 | `request_assembly::{fixture_log, assemble}`, `tools::advertised_roster_bytes` | consistent |
| T5 → T6,T12 | `bench_loop::{Script, ScriptedProvider, ScriptedTurnConfig, run_scripted_turn}`; `run_native_loop_with_provider_requester` `pub(crate)` under `any(test, bench)` | consistent |
| T5 ↔ T3 | T5 `ScriptedTurnConfig.trace` passes into `RunnerConfig.trace` | consistent |
| T6 → T12 | `YACH_RIG_PROVIDER=scripted`, `YACH_BENCH_SCRIPT`, `target/bench/release/yach` | consistent |
| T7 → T8,T9,T12,T13 | schema/registry types; `RunCtx` has `yach_bench_yach_bin` per T7 interface block (updated) | consistent |
| T7 registry vs T9 `measure` | `Measured::Latency { samples, alloc }` (T7 interfaces) vs T7 Step 5 registry snippet using `Measured::Latency(sample_startup(..))` tuple form | **inconsistency** — Ruling below |
| T8 ↔ T7 | T8 deletes `render_duration` from main.rs; T9 keeps a private copy; T14 moves to report.rs | consistent |
| T9 ↔ T12 | `request/assemble/*` registered in T9 (core_loop.rs created there), T12 extends | consistent |
| T9 `perf run` builds via `build_side` from T13 | T9 says "local `build_current()` until T13 lands" | consistent |
| T10 → T12 | `rss::{peak_rss_bytes, Spawn, StopBoundary}`; T10 interface block lists `peak_rss_bytes(command, boundary, timeout)` (3 args) but Step 3 adds `Spawn` (4 args); T12 calls 3-arg form | **inconsistency** — Ruling below |
| T11 → T13 | `Thresholds::{load, for_id, unmatched_rows, defaults}`, `judge_latency`, `judge_value`, `Verdict` incl. `NoBaseWorker` | consistent |
| T13 → T14 | `AbDoc`, `EXTERNAL_IDS`, `BuildStage` | consistent |
| T14 CI | Nix actions pinned "at implementation time" | acceptable (implementer chooses; reviewer checks pin present) |
| T15 | acceptance exit 0 only | consistent with spec |
| Each task's tests vs code | T1, T4, T5, T7, T9, T10, T11, T12, T13, T14 tests reference only names their own task or an earlier one defines | consistent |

Rulings:
- Ruling: T5 `ScriptedProvider` gains `trace: Option<TraceSink>` + `turn_id` set by `run_scripted_turn`/CLI wiring, and emits `provider_first_event`/`provider_stream_end` around producing its vector — the spec requires those labels for every turn and T3 already says so; T5's code block is amended by the implementer. Cost if wrong: two extra fields on a bench-only type.
- Ruling: `Measured::Latency` is the struct form `{ samples, alloc }` everywhere (T7 interface block wins over the T7 Step 5 snippet). Cost if wrong: none; purely mechanical.
- Ruling: `peak_rss_bytes` takes four args `(command, spawn: Spawn, boundary, timeout)`; T12's `memory/peak_rss/turn_scripted_tools_4` uses `Spawn::Piped`. Cost if wrong: none.
- Ruling: in-process serial `turn/scripted/*` rows: `ScriptedTurnConfig` needs no trace for latency, but `ScriptedProvider` only marks when `trace` is `Some`; leave `trace: None` there. Cost if wrong: none.

## Execution

Task 1: review round 0 — 5 Important (0 Critical). BASE a871db1179fce77c40cdbef605931efd600e1e7f, head c5b85176b701.
- Ruling: (plan-mandated) allocation-free serialization of marks — plan code allocated two Strings per mark; spec says per-mark work is serialize + buffered write. Fix: serialize a borrowed view. Cost if wrong: a few lines of serde plumbing.
- Ruling: (plan-mandated) final flush belongs on the shared `Inner`'s `Drop`, not `Arc::strong_count` in `TraceSink::drop` — the plan's count check races under concurrent drops. Cost if wrong: none; strictly more correct.
- Ruling: (plan-mandated) tests must fail on parse/setup errors instead of early-returning. No crate in the workspace allows `expect`/`unwrap` in tests (grep finds no `cfg_attr(test, allow(clippy::…))`), so the convention is `assert!(result.is_ok(), "…{result:?}")` followed by `let Ok(x) = result else { return };` — the assert makes the early return unreachable on failure. Cost if wrong: none.
- Ruling: the write-failure test must exercise a real writer failure. Direction: `Inner.writer` becomes `Option<Box<dyn Write + Send>>`; `TraceSink::open` boxes a `BufWriter<File>`; a `#[cfg(test)] TraceSink::with_writer(Box<dyn Write + Send>)` constructor injects a writer that returns `Err` on flush; the pathname metadata checks are removed entirely. No per-mark syscalls; unlink/rename of an open trace stays valid Unix behavior. Cost if wrong: one trait object indirection per mark.
Task 1: fix round 1/5 (4 addressed, 1 open — failure test asserts `warned` bool, not exactly-one diagnostic; changes c5b85176b701..f82a894493bc)
- Ruling: `TraceScope::Turn(String)` → `TraceScope::Turn<'a>(&'a str)`; callers (Tasks 2–3) borrow the turn id per mark instead of cloning. Added to round 2 because it is the same hot-path-allocation defect the review already flagged for scope/label and is cheaper to change before callers exist. Cost if wrong: a lifetime on a bench-facing enum.
- Ruling: keep `Box<dyn Write + Send>` for the writer. The trait object is the injection seam that makes the failure test exercise real writer errors; one indirect call per mark next to a serde serialize is not a measurable cost, and a generic `TraceSink<W>` would leak into every `Option<TraceSink>` signature across four crates. Cost if wrong: one vtable call per mark.
Task 1: fix round 2/5 (2 addressed, 0 open; changes f82a894493bc..fd68f16771ff)
Task 1: complete (changes a871db1179fc..48f7f7e3f495, review clean; plan Task 1/3 text aligned with landed API in lkxmnlxw)
Task 2: Kata yach#6pr0 — change qvvlsoyxqvsknozrqrmksprrqrwzkwms (reused); BASE 48f7f7e3f4957d1b8669503bdcfcad22b5f1c4ec
- Ruling: the plan predicted `yach-bench` would fail to compile after Task 2 and deferred its fix to Task 8; it compiles, so the stack would carry a silent runtime break (samplers set `YACH_STARTUP_TRACE`, parse the old format). Task 2 scope extends to: `yach-bench` sets `YACH_TRACE`, `startup_trace.rs` becomes a thin adapter over `yach_trace::parse_records` filtering `scope == "startup"` (same `StartupTraceMark { label, elapsed }` shape, so main.rs consumers are untouched), and the full workspace suite runs. Task 8 still deletes the adapter when it moves samplers. Cost if wrong: ~30 lines that Task 8 deletes anyway.
- Ruling: the `yach-ui/src/app.rs` Inline→Fullscreen fallback added in pqowmqsn is out of Task 2's scope and changes shipped TUI behavior to make a benchmark pass on util-linux `script`; reverted. The startup-profile sampler's inability to run under util-linux `script` on Linux is a pre-existing sampler limitation to be solved in Task 10's `spawn_on_pty` (a real pty the bench owns, which can answer or ignore the cursor query) — Task 8/10 briefs carry this. Cost if wrong: the startup-profile workloads stay skipped on Linux until Task 10.
- Ruling: `parse_startup_trace_marks` must not map `Malformed` to an empty Vec; it returns `Result<Vec<StartupTraceMark>, String>` and pollers propagate the error, per spec ("bench parser treats a truncated final line as error for every workload that depends on that trace file" — a malformed line is strictly worse). `TruncatedLine` while polling a live child remains tolerated by parsing through the last newline. Cost if wrong: none.
Task 2: review round 0 — reviewer clean; controller adds one Important: backend `extension_manifest_scan` test uses `unwrap_or_default()` on `read_to_string` and `parse_records` (runner.rs:11497,11500), the pass-on-failure pattern rejected in Task 1. Enters fix round 1.
- Ruling: `parse_startup_trace_marks` keeps tolerating a truncated final line. Both callers poll a file a live child is still appending to; a strict error there would race every sample. The spec's "truncated final line is an error" applies to reading a finished trace (Task 12's `turn/phase/*` reads after child exit and must use `parse_records` directly, strict). Task 8/12 briefs carry this distinction. Cost if wrong: one wrapper with two semantics; Task 8 deletes it.
Task 2: fix round 1/5 (1 addressed, 0 open — controller verified the single-hunk test fix directly; changes f8f050ecdf8d..96e4b275; changes yyoptvnq)
Task 2: complete (changes 48f7f7e3f495..3d31bcadd83e, review clean; plan Task 2 aligned with landed stack in knsnxuxn)
Task 3: Kata yach#bvtw — change tnmwywwynllpyotnzxrlyqzuwyvmnywk (reused); BASE 3d31bcadd83ea0a24170a550735f5b3a100a156b
- Ruling: implementer's concern "tool_result_appended only on the read-only path" is a spec gap, not a v1 limitation — the spec defines `tool_result_appended (n)` per tool result and the hashline-extension workload in Task 12 runs through the extension host, whose `ToolExecutionFinished` push would otherwise never mark. Entered as an Important finding for fix round 1 regardless of reviewer outcome: every `SessionEvent::ToolExecutionFinished` push in the agent tool batch (read-only, edit, bash, extension, failure/denied outcomes) emits the mark with its call index. Cost if wrong: extra marks on paths no workload measures yet.
- Ruling: compaction requests, `provider_request_with_retry` (test-only), and `persist_native_cancelled_turn` passing `trace: None` is acceptable in v1: no workload measures compaction or cancellation, and the spec's turn labels describe the prompt path. Ledgered as deferred minor for the final review. Cost if wrong: cancelled turns lack `session_persisted`/`turn_completed` in traces.
- Ruling: smoke required `--model`; `yach run` without a model selection short-circuits at `ConnectionMissing` before a turn starts. Task 6/12 briefs carry `--model scripted-model` (or whatever the scripted provider config names) for child invocations. Cost if wrong: none.
- Ruling: the dev-shell `CARGO_TARGET_DIR` is `.devenv/state/target`, not `target/` — every plan path `target/release/…`, `target/bench/…` means `<CARGO_TARGET_DIR>/…`; Task 7's `RunCtx` resolution and Task 13's `build_side` must read `CARGO_TARGET_DIR` (via `cargo metadata --format-version 1 | jq .target_directory` under `just dev`) rather than assume `target/`. Carried into Task 7/9/13 briefs. Cost if wrong: binaries not found.
Task 3: review round 0 — reviewer output failed its result schema but carried one Critical finding (matches the pre-ruled gap): `tool_result_appended` only after the successful read-only push at runner.rs:7051-7056; missing at read failures (6958–7034), extension results/failures (7133–7246), bash results (7597), edit-tool results drained from the buffered sink, synthesized terminal results (8234). Enters fix round 1.
Task 3: fix round 1/5 (1 addressed, 0 open — every ToolExecutionFinished push site marked exactly once, verified by re-review enumeration; changes 815d2933e400..d378de8ee611)
Task 3: minor (deferred): per-site `tool_result_appended` marks could collapse to one unconditional mark after each `record_missing_provider_tool_batch_events` call (single choke point per request exit) — final review to triage.
Task 3: minor (deferred): `prompt_with_failed_tool_emits_result_mark` filters to `n == Some(1)` before counting, so an extra wrong-index mark would not fail it.
Task 3: minor (deferred): compaction requests, `provider_request_with_retry`, `persist_native_cancelled_turn` pass `trace: None`.
Task 3: complete (changes 3d31bcadd83e..070640a4, review clean)
Task 4: Kata yach#hz4v — change xwtlkwssqvytxuqxmqkzowwkmvuruxxl (reused); BASE 070640a4d6dabd05ab3754360554fafdf65bc407
Task 4: review round 0 — 1 Important (`advertised_roster_bytes` mapped serialization failure to `Ok(0)`, defeating the zero-budget CI gate).
Task 4: fix round 1/5 (1 addressed, 0 open — controller verified the 3-line fix matches the reviewer's prescription; changes d7209c3e1f15..1a849975)
Task 4: complete (changes 070640a4d6da..1a849975, review clean)
Task 5: Kata yach#m23p — change xrwwwzyqsmlonukxxxvslvusmvotnzzu (reused); BASE 1a849975f82aced48d6b63ec5d3ffedcaf50c0e5
Task 5: review round 0 — 1 Important (plan-mandated): `ScriptedTurnProfile.requests` reports script length, not requests made.
- Ruling: plan code was wrong; `requests` must be the observed count (shared `Arc<AtomicUsize>` incremented in `ScriptedProvider::request`, read after the runner exits). Cost if wrong: none.
Task 5: fix round 1/5 (1 addressed, 0 open — controller verified Arc<AtomicUsize> counter + discriminating test; changes a35bc907d43a..9d90f217)
Task 5: complete (changes 1a849975f82a..9d90f217, review clean)
Task 6: Kata yach#zxrs — change knmzwmmstuyrozuytowytxotwwnutlwx (reused); BASE 9d90f217ac983c98a7249639dd68edf8269cd89c
Task 6: review round 0 — 1 Critical: `run_native_loop_with_scripted_provider` uses the test-only one-shot factory (`take()` + `unreachable!` on second call, runner.rs:1018-1020); second prompt of a multi-turn `yach run --script` panics.
- Ruling: SIGTERM losing buffered trace marks is acceptable (Rust Drop does not run; turn boundaries flush explicitly). Deferred minor for the final review: consider `flush()` after each startup mark in the CLI, as the backend already does for extension-scan marks.
- Ruling: scripted adapter labeled `anthropic` is cosmetic on the run path; deferred minor (TUI model picker may trigger Anthropic discovery under the scripted env).
Task 6: fix round 1/5 (1 addressed, 0 open; changes ff921100d72c..26d1ec9e)
Task 6: minor (deferred): CLI startup marks are not flushed per mark, so SIGTERM-killed processes lose buffered startup records.
Task 6: minor (deferred): scripted adapter config labels itself `anthropic`; TUI model picker under the scripted env may trigger Anthropic discovery.
Task 6: complete (changes 9d90f217ac98..26d1ec9e, review clean)
Task 7: Kata yach#jach — change tuztwmmrvrwltuuproxsuzyyknvxxqvw (reused); BASE 26d1ec9e4cda1342a6eabc08b9594c032ac57a62
- Ruling: (plan defect) Task 7's `ids_are_unique_and_preserve_legacy_labels` test listed four ids Task 8 registers, so the implementer landed `not_migrated` stub entries returning `Err`. Stubs are placeholders and are removed in fix round 1; the test's legacy list narrows to the six headless ids this task owns, and Task 8's brief carries the full legacy list into its own test. Cost if wrong: none.
Task 7: review round 0 — clean (after controller-directed placeholder removal in zyvpmsqn).
Task 7: complete (changes 26d1ec9e4cda..0af49b1e, review clean)
Task 8: Kata yach#fw48 — change quxlmnzwmpousyzwzuwuozzmwmurrtvx (reused); BASE 0af49b1ecde761ded6b093474729096f778f6dad
- Ruling: dropping the `AllocWindow` upper-bound assertion (`bytes < 8192 + 1024`) because parallel tests share the process-global counter removes the only proof the window is tight. Correct fix: the test takes a process-wide `static ALLOC_TEST_LOCK: Mutex<()>` (or runs the window measurement inside a spawned thread with `ACTIVE` gated per-thread — no, keep it simple: a mutex the alloc-sensitive tests share) and restores the bound. Entered as an Important finding for fix round 1. Cost if wrong: one mutex in test code.
Task 8: review round 0 — 1 Important (alloc-window upper bound removed).
Task 8: fix round 1/5 (0 addressed, 1 open — round-1 fix used a 32-attempt retry loop, which cannot fail; controller rejected without re-review; changes c870106d2b93..lqywtskv)
- Ruling: root cause is the process-global `ACTIVE` gate; counting must be scoped to the measuring thread via a const-initialized `thread_local! Cell<bool>` read with `try_with` (allocation-free inside `GlobalAlloc`). Matches the spec's `in_process_serial` = main-thread-only contract and is why threaded workloads report `alloc: null`. Cost if wrong: a TLS read per allocation in the bench binary only.
Task 8: fix round 2/5 (1 addressed, 0 open — thread-local gate, original bound restored, cross-thread test; changes lqywtskv..xnyprwpv, re-review clean)
Task 8: complete (changes 0af49b1ecde7..b5b1d80e, review clean)
Task 9: Kata yach#djma — change ynptqsxuzpzrrkzzklxxyrsronqllrmp (reused); BASE b5b1d80ea4c35fbc83c29876587d1e253f9be11d
- Ruling: (plan defect) the brief's `measure` sketch emits `skipped` rows for `#alloc_count`/`#alloc_bytes` when a serial latency workload is unmet, but its tty test asserts `rows.len() == 1`. The sketch is right: derived rows must be present as `skipped` so the A/B controller verdicts them `skipped` rather than `removed`/`added`. Implementer chose the test; fix round 1 restores the derived skip rows and the test asserts 3 rows all `skipped`. Cost if wrong: two extra rows per skipped serial workload.
Task 9: review round 0 — 1 Critical (live TUI samplers render to `io::stdout()`, which the worker now pipes for its one-line JSON handshake: any terminal-enabled run corrupts the handshake and measures pipe writes), 1 Important (skipped serial workloads omit `#alloc_*` skip rows; pre-ruled).
- Ruling: the worker's stdout handshake is removed rather than rerouting ratatui to stderr. The `--out` document already carries `schema` and `workloads`; the controller reads it after exit and validates `schema` there. `perf worker` inherits stdin/stdout/stderr so live samplers own the real terminal exactly as they did before the migration; `--schema-probe` stays on stdout (it never runs workloads). The spec's "worker prints `{schema, workloads}` only" is satisfied by the document. Cost if wrong: the controller cannot distinguish "worker crashed before writing" from "worker wrote nothing" except via exit status — which it already checks.
Task 9: fix round 1/5 (2 addressed, 0 open; changes 07c73bbb9e3c..07a1e2bcc828, re-review clean)
Task 9: complete (changes b5b1d80ea4c3..bd8509a9, review clean)
Task 10: Kata yach#pgz0 — change msqmqrluylyorouxltnmyknkontlkmok (reused); BASE bd8509a9264eac49ebf6a6505006d5867fa0710a
Task 10: review round 0 — 4 Important: `spawn_on_pty` leaves a stale `pre_exec` hook with raw fds on a reusable `Command`; non-Linux profile path `mem::forget`s the stdin pipe (fd leak per sample); Linux-only imports break `-D warnings` on non-Linux; a lib unit test depends on an unbuilt `yach-bench` binary (duplicate of the integration test).
Task 10: fix round 1/5 (4 addressed, 0 open; changes deabfe899594..4a304fd23185, re-review clean)
Task 10: minor (deferred): startup-profile sampler (`startup.rs:315`) propagates a first-render timeout / trace-parse error before kill+wait, leaving the child and the pump thread's pty master alive for that failed sample (pre-existing shape; RSS and first-output paths clean up correctly).
Task 10: complete (changes bd8509a9264e..5868630b, review clean)
Task 11: Kata yach#81f7 — change ukvpqlkomvklnwloxusmowzqwstmvkvp (reused); BASE 5868630bfcc28e8c2799969c6286942d72be1744
Task 11: fix round 1/5 (controller-found clippy `float_cmp` on test target; addressed in nstxsypn)
Task 11: review round 0 — 2 Important (plan-mandated): `signum()` counts zero deltas as positive; zero base yields NaN/inf deltas (`startup/phase/process_main_start` is always 0).
- Ruling: zero deltas are their own sign class (agreement counts only rounds whose sign equals the median's, with 0 ≠ +/−); an exactly-zero median delta is `unchanged` unless base spread exceeds the threshold. Cost if wrong: slightly more `inconclusive` verdicts.
- Ruling: zero-base rounds: if base and current are both 0 the round delta is 0.0 and spread contributes 0; if base is 0 and current > 0 the round delta is `+inf` conceptually — represent it as `Detail.median_delta_pct = f64::MAX`? No: `Detail` must stay finite for JSON. Rule: a round with base 0 and current > 0 is scored as delta = +100.0·threshold (i.e. always over threshold, positive), and spread treats base 0 rounds as 0; the reason string on the row notes "zero baseline". Also `startup/phase/process_main_start` is by construction always 0 — Task 12/13 brief: exclude it from the registry (it is the origin, not a phase). Cost if wrong: one degenerate row removed; finite JSON.
- ⚠️ from review: `perf-thresholds.toml` row `turn/scripted/*` matches nothing until Task 12 registers those ids; Task 13's unmatched-row hard error must run against the union of registered ids INCLUDING worker-derived `#alloc_*` ids (the `*#alloc_count`/`*#alloc_bytes` rows match only derived ids). Carried into Task 12/13 briefs.
Task 11: fix round 2/5 (2 addressed, 0 open; changes 0aefc0b2de5e..2f83f31038df, re-review clean)
Task 11: complete (changes 5868630bfcc2..69f1e3e8, review clean)
Task 12: Kata yach#0njw — change msmzymsmvyxuksmnzwqmmtrumzqotvmn (reused); BASE 69f1e3e8f311f0885da49f6e8fc15c3f5753f8bf
- Ruling: `turn/scripted/tools_4/hashline_ext` errors because headless `yach run` submits its prompt on `ModelActivationFinished` while `postFirstPaint` extension activation (scheduled by headless's own `FirstRenderCompleted`) is still running — a configured extension is silently unused on the first headless prompt. That is a product defect the benchmark surfaced, not a bench limitation: fix in `crates/yach-cli/src/headless.rs` — when any extension package root is configured, defer the first prompt until `extension_background_activation_finished`/`_failed` (bounded wait; failure still submits). Unlike Task 2's rejected TUI change, this alters no measured boundary and makes shipped behavior correct. Entered as an Important finding for fix round 1 with a headless test. Cost if wrong: first headless prompt waits for activation when extensions exist — the honest cost.
Task 12: review round 0 — 1 Critical (`inactive_ext_8` creates 8 manifests with the same extension id → scan fails, row measures a failed scan), 4 Important (temp HOME/trace/project/script leak on error paths; inherited `YACH_EXTENSION_PACKAGE_ROOTS` contaminates children; `rig_messages_from_request` unconditionally `pub` via `cfg_attr(doc)`; scripted-turn test returns on `Err`). Minor deferred: config allocations (`root.clone()`, prompt `String`) fall inside the alloc window at core_loop.rs:384-390. ⚠️ unverified: no-extension `yach run` may now wait for the empty-scan `finished` status — measured in Task 15's baseline (`yach/cli_startup_first_output` is `--quiet`, not `run`; compare `turn/scripted/text_only` in-process vs child wall).
Task 12: fix round 1/5 (headless activation gate; changes d88979674b26 in xpurztwx)
Task 12: fix round 2/5 (4 of 5 addressed; changes d88979674b26..a122263d4cac)
Task 12: fix round 3/5 (1 addressed, 0 open — TempFs guard ordering, controller-verified; changes a122263d4cac..d83dc4df)
Task 12: minor (deferred): config allocations fall inside the in-process turn alloc window (core_loop.rs ~384-390).
Task 12: complete (changes 69f1e3e8f311..d83dc4df, review clean)
Task 13: Kata yach#a16t — change rzyptkwqyrsoklqzuroppvywvqzsztzn (reused); BASE d83dc4df695e8942236065afa80095c5cd41835c
Task 13: controller investigation of the implementer's "peak_rss +175%" concern — NOT a real regression and NOT one-round noise. Same shipping binary: `perf worker --filter 'memory/peak_rss/tui_ready'` alone → 6 385 664 B; inside a full unfiltered worker run → 17 866 752 B (reproduced twice; a `--base-dir .` self-comparison of the identical binary reports 0.00%). The `memory` value therefore depends on which workloads ran earlier in the same worker process, so it is not a property of the measured binary. Most likely cause: the child inherits worker state that scales with prior work (env/rlimits are constant, so the candidates are the inherited pty/terminal size set by earlier live-terminal samplers, or the child inheriting a dirtied page cache/`vm.overcommit` context; `ru_maxrss` is per-child so it cannot be the worker's own growth) — the implementer must find the actual mechanism, not guess.
- Ruling: this is a Critical measurement-validity defect in Task 13's own acceptance evidence (it produced a false `regressed` verdict). Fix in Task 10/12's sampler or Task 9's worker as the mechanism dictates; the workload must yield the same value whether run alone or after every other workload, proven by a test. Cost if wrong: the memory class is untrustworthy and the gate cries wolf.
Task 13: mechanism found — Linux `ru_maxrss` from `wait4` reports max(child peak, PARENT peak at fork), so a worker bloated by earlier in-process workloads inflates every later child's number; the pty winsize was ruled out with `TIOCGWINSZ` (24×80 in both contexts). Fix: sample `/proc/<pid>/status` `VmHWM` while the child is alive (post-exec mm) and use `wait4` only to reap. Controller re-verified: alone 6 586 368 B vs full-run 6 565 888 B (0.3% apart; was 180%).
- Ruling: the spec's "Linux `ru_maxrss` × 1024" prescription is wrong and Task 15 must correct that paragraph: peak RSS comes from `/proc/<pid>/status` `VmHWM` (already KiB), which is Linux-only for the same reason the spec already restricts the class. Cost if wrong: none; `ru_maxrss` was measuring the wrong process.
Task 13: review round 0 — 4 Important; fix round 2 addressed 2, left 2 partially open, and introduced a protocol regression (`--classes`/`--ids` sent to a base worker still advertising schema 1).
- Ruling: bump `SCHEMA` to 2. The worker protocol gained required-to-understand flags, which is precisely what the schema number exists to signal; a schema-1 base then fails the controller's existing mismatch check with a message naming both versions instead of dying on an unknown flag. The framework is not on `main` yet, so no released base advertises schema 1 — only intermediate commits of this stack, which are not comparison targets anyone should use. Cost if wrong: comparisons against this stack's own intermediate commits hard-error until they are rebuilt; external mode is unaffected.
Task 13: fix rounds 1-3 (RSS isolation; scoped retries + slot dirs + canonical paths; class-restricted retries + SCHEMA 2) — re-review clean; changes 79ff8cb81af0..af2c8a97
Task 13: complete (changes d83dc4df695e..af2c8a97, review clean)
Task 14: Kata yach#f07f — change lzkwvzmlryykwrnywlpvrmyxtwklxksn (reused); BASE af2c8a97d143ccdfe09c76a45834fad22b8e24bd
Task 14: review round 0 — 4 Important: CI cannot bootstrap `just dev` on a clean runner (`flake.nix:6` locks `nix-config` to `git+ssh://git@github.com/codyw912/nix-config.git`, private, and `actions/checkout`'s token does not configure SSH); `just perf` loses filter quoting across the two recipe layers (glob expands against the cwd); `perf-profile` never builds the shipping `yach` the worker unconditionally hashes; `perf report` on an A/B document reads the LAST slot, dropping the size/count rows that only round 1 measured.
- BLOCKER for the user (spec-level): the spec requires CI to build through the declared environment and forbids bare `cargo`, but the declared environment is unreachable from GitHub-hosted CI. Three ways out, all of which change an accepted spec paragraph or repo policy — the user decides.
- Ruling (user, asked): the `perf-deterministic` CI job builds with `dtolnay/rust-toolchain` + bare `cargo`, like the existing `rust` job; `perf ab` gains `--build-cmd` so the controller does not invoke `just dev` there. Task 15 amends the spec's CI paragraph: GitHub CI is the one environment where the declared dev shell is unreachable (private SSH flake input) and bare cargo is the declared path. Deterministic metrics only require both sides built identically within the same job, which still holds. Cost if wrong: CI's toolchain can differ from a developer's, so CI numbers are comparable only to each other — which is all the deterministic gate claims.
Task 14: fix rounds 1-4 (quoting/profiling/report merge; CI bare cargo + --build-cmd; probe for any cargo form; drop redundant cargo probe) — re-review clean; changes 5c1c3b1027d1..ae23e6d7 plus ovsmpmyo
Task 14: complete (changes af2c8a97d143..ovsmpmyo head, review clean)
Task 15: Kata yach#7z6c — change rwnykkqxulqqwwpsqxklrslytlyyssmk (reused)
Task 15: controller check of the raised budgets — the justification does not match the accepted run. `perf-thresholds.toml` was widened to `yach/*_pty` 30% (citing 21–25% base spread) and `yach/cli_startup_first_output` 16% (citing 14.3%), but the run that was accepted (`.perf/results/20260910T123936-ab.json`) records base spreads of 12.1% (tui_startup), 6.7% (tui_ready), 2.8% (cli) with median deltas −1.2% / +1.4% / −4.1%. A 30% budget against 12% observed spread is ~2.5× the noise and would hide a real regression of exactly the size this framework exists to catch.
- Ruling: budgets must be derived from the run being accepted, not from earlier runs taken while the harness still had the workload-ordering bug (those pre-fix rounds are contaminated by the same effect that caused the false RSS regression). Set each budget from the accepted run's measured base spread with a modest margin — spread rounded up to the next whole percent, plus 3 points — and cite that run's file and numbers in the `comment`. On this evidence: `yach/*_pty` 16%, `yach/cli_startup_first_output` 6%. If a row then comes back `inconclusive`, that is the honest signal (the spec's `inconclusive` path exists precisely for it) — rerun with more rounds rather than widening further. Cost if wrong: occasional `inconclusive` on the PTY rows until the sampler's variance is reduced.
Task 15: budgets corrected to `yach/*_first_output_pty` 16% and `yach/cli_startup_first_output` 6% (ceil(spread)+3 from the accepted run), glob narrowed so profile rows keep their own budget.
- Ruling (user, asked): land at exit 2. `just perf` against `main` verdicts four rows cleanly and reports `inconclusive` on the two process-startup samplers whose base spread (10.5–20.3%) exceeds their budget; every median delta is inside budget, so this is machine noise being reported honestly rather than a regression being hidden. Task 15's acceptance is amended to "exit 0, or a documented `inconclusive` confined to the process-startup first-output rows"; reducing that variance is follow-up on plane:YACH-8. Cost if wrong: `just perf` exits non-zero on an unchanged tree until the sampler variance is reduced, so the two rows gate nothing today — but they lie about nothing either.

## Final whole-branch review

Final review (most capable model, 48 changes): 0 Critical, 5 Important, 8 Minor. Verdict "ready to merge with fixes". One fix wave (change `qrttnlow`) addressed all five; one scoped re-review closed 4 of 5 and found 1 new. No second fix wave (process rule) — both residuals adjudicated below and surfaced at handoff.
- Task 15: complete (changes c2533ee156c1..02f8f21b, review clean)
- Final fix wave: complete (changes 3991e70213ea..074ffbd3; findings 1, 2, 4, 5 addressed)
- Ruling (residual, load-bearing): `emit_alloc` is honored by the worker's measurement path but not by `worker::list_workloads` (worker.rs:742-752) or `ab::check_unmatched` (ab.rs:584-588), so `perf run --list` advertises `#alloc_*` ids that no longer exist and a thresholds row matching only those ids passes the unmatched-row hard error instead of failing it. That defeats a check I mandated in Task 13. It cannot produce a wrong verdict on a real row — it weakens a guard and misreports the registry. Parked for the user with a recommendation to fix before merge; it is a two-call-site change.
- Ruling (residual, load-bearing for CI): the five replay-based headless workloads open their alloc window around `replay_headless`, which builds `BenchmarkApp` and the sample vector before the first timer and collects `prompt_text` after the last, so their `#alloc_*` rows count untimed construction. Same class as the two contamination bugs already fixed. Deterministic CI gates those rows with budget 0, so a fixture or construction change becomes a false CI failure. Parked with the same recommendation: either count inside the timed steps or set `emit_alloc = false` for those five (the latter is one line each and consistent with what the fix wave already did for the cached families).
- Both residuals are bench-internal: no shipped `yach` behavior depends on them, and the local `just perf` latency/memory verdicts are unaffected.

## Landing

Shaped 51 changes → 20 (one per plan task, fix rounds folded in; one stray empty change abandoned). Suite re-run on the squashed tree: 1347 passed, 0 failed.
PR: https://github.com/codyw912/yach/pull/269 — bookmark `perf-measurement-framework`, pushed head `0cf6bbea` (commits re-signed on push).
Kata: all 15 children closed with `--evidence pr:<url> test:<cmd>`; the `sjujperpowers-plan` parent `yach#te2k` left OPEN with a comment naming the two unresolved acceptance items (Step 6 forbids closing it while they stand).
Plane: roll-up posted on YACH-8; outcome left open (acceptance not fully satisfied).
Checkpoints: pre-push and published-state checkpoint refs recorded (host refs redacted).
Recovery workspace retained: pull-request completion keeps the stack resumable.
Note: `kata close` takes `--evidence pr:<url>` / `test:<cmd>`, not `--pr`/`--test`; the unknown flags were absorbed into the ref and surfaced as a misleading `project_not_initialized`.

## Post-publication corrections

CI `Rust workspace` failed on the Lint step. Cause: CI's `dtolnay/rust-toolchain@stable` is **rustc 1.98.1**, the dev shell pins **1.94.0**, and the newer clippy flags `map(..).unwrap_or(..)` on `Result`. Four instances (`yach-trace/src/lib.rs`, `perf/ab.rs`, `perf/provenance.rs`, `perf/report.rs`) — all clean under 1.94, so `just lint` could never have caught them. Installed 1.98.1 via `nix shell nixpkgs#rustup` and linted against it to catch all four at once instead of round-tripping through CI. Fixed in `mtxuwqns`; both toolchains now clean. Documented the gap so it cannot recur silently (`pzmkovts`).
- Ruling: the advisor was right that "hashline 71 ms vs builtin 48 ms" was not extension overhead. `tools_4/builtin` measures the inner turn in-process; `hashline_ext`/`inactive_ext_8` measure a whole child process. The gap was dominated by process startup. Added `turn/scripted/tools_4/builtin_child` (`stxylzvy`) — same child boundary, builtin tools, with a `confirm_builtin_tools` check that fails if hashline is active or the four tool results are missing. Controller-verified at n=8: `builtin_child` p50 50 ms vs `hashline_ext` p50 60 ms → **extension-host overhead ≈ 10 ms**, with `inactive_ext_8` p50 50 ms (inactive extensions cost nothing, as expected) and in-process `builtin` 30 ms confirming ~20 ms of the original figure was process startup. The bad number never reached a committed doc; it was in my PR body and handoff only. Cost if wrong: one extra child workload per gate run (~0.5 s at default samples).
- Outstanding at handoff: the PR body still carries the wrong comparison and the branch is 4 commits ahead of the pushed head. Both need a fresh publication approval (the 30-minute grant expired); `gh` reads 401 until then.
- Ruling: RETRACT the ~10 ms extension-overhead figure too. `wait_child_timeout` (core_loop.rs:708) sleeps 10 ms between `try_wait` polls and `run.wall` stops only when a poll observes exit, so every child sample is quantized to a 10 ms tick — the p50s 50/50/60 ms are tick multiples and the "10 ms delta" is exactly one tick. At n=8 the p95s even reverse the ordering. No child-boundary delta is publishable until elapsed time stops being polling-limited. Fix: block on the child on a dedicated thread and read elapsed there, with the 30 s deadline enforced by a separate watchdog rather than by the sampling loop. Cost if wrong: none; this strictly improves resolution and removes a systematic quantization floor from every `turn/scripted/*_child` and `memory/peak_rss/turn_*` sample.
- Note: the same 10 ms floor applies to any workload whose wall clock ends at `wait_child_timeout`, so the fix is a measurement correction for the whole child-process class, not just the extension comparison.
- Quantization fixed in `qwrmqrxu`: the child is waited on a dedicated thread with a blocking `wait()`, the 30 s deadline moved to a `recv_timeout` watchdog, and elapsed is read at the exit observation. Controller-verified at n=20: 0 of 20 `builtin_child` samples land on a 10 ms boundary (was: all of them). Corrected p50s — `builtin` 29 ms (in-process, not comparable), `builtin_child` 51 ms, `inactive_ext_8` 47 ms, `hashline_ext` 58 ms.
- Ruling: publish NO extension-overhead figure. At n=20 `hashline_ext − builtin_child` is ~7 ms p50 against ~6 ms within-id spread, and at n=12 the p95 gap inverted relative to p50. The honest statement is that the framework can now pose the question on one boundary but cannot yet answer it at these sample counts; answering needs more samples or a tighter boundary (trace-mark to trace-mark rather than whole process). Recorded as follow-up on plane:YACH-8. Cost if wrong: none — this withholds a claim rather than making one.
- Final state: 1350 tests pass; fmt clean; clippy clean on both 1.94.0 and 1.98.1. Checkpoint recorded.
- Ruling: the first quantization fix introduced a worse bug than the one it fixed. On timeout it shelled out to `kill`, discarded the result, then unconditionally joined a thread blocked in `child.wait()` — if `kill` was missing or ineffective the 30 s deadline became infinite, and signalling by pid raced the waiter's reap (PID reuse). Fixed in `nvwvuzlx`: wait on a pidfd (`SYS_pidfd_open`), caller retains the unreaped `Child` so `libc::kill` cannot target a recycled pid, kill is injectable (`fn(u32) -> bool`) and the post-kill reap is bounded by a 5 s grace so the function always returns. New `wait_child_timeout_returns_when_kill_has_no_effect` forces a no-op kill and asserts return rather than hang.
- Ruling: replaced the non-quantization test's `elapsed < 40ms` upper bound (`lwplxzmo`). An 8 ms margin over a 32 ms sleep flaked once under parallel load; a flaky test guarding the perf gate trains people to re-run until green. Now asserts the elapsed value does not land within 2 ms of a 10 ms tick, with one retry — fails on a re-polled implementation, cannot fail because the machine is briefly busy. Verified 3x at `--test-threads=4`: 68/68 each run.
- Final state after four correction rounds: 1351 tests pass, fmt clean, clippy clean on 1.94.0 and 1.98.1, all `turn/scripted/tools_4/*` rows `ok`. Checkpoint recorded. Branch 8 commits ahead of the pushed head; needs `push` + `pr` + `pr-edit` approval.
- Retraction: my claim that the non-quantization test "cannot fail because the box is briefly busy" was wrong in both directions. A legitimate ~40.5 ms exit fails the modulo check, and a 10 ms polled implementation delayed to 43 ms passes it; one retry does not convert a timing coincidence into a property. Reclassified `#[ignore]` (`wrxpzpsq`) as a manual measurement check. The real quantization evidence is the n=20 controller run recorded above (0 of 20 samples on a tick), not a unit test.
- Portability (`vsnuzmwt`): `libc::SYS_pidfd_open` was unconditional, so `yach-bench` no longer built on macOS — the platform the last published baseline was recorded on. Linux keeps the pidfd path; non-Linux gets a 1 ms `try_wait` loop explicitly documented as build/compat support, not a publication-grade timer. Verified by a temporary cfg flip and `cargo check`; a real `--target x86_64-apple-darwin` check is blocked by `aws-lc-sys` cross-compiling, not by this crate — so non-Linux is type-checked, NOT build-verified on a Mac. Someone should confirm on real hardware before the next macOS baseline.
- Leak-free error paths (`vuqosmpl`): `pidfd_wait_timeout(...)?` returned early while the spawned child was still running. Every wait-setup/poll error now kills and reaps under the same 5 s grace as the timeout, with a test that injects wait failure and asserts `kill(pid, 0) == ESRCH`.
- Controller follow-ups the subagent left: the deadline test probed `/proc/<pid>` (Linux-only, in a test that is not cfg-gated — the same defect class just fixed in the implementation), now uses the portable `child_alive_errno` helper; and the `c_int` conversion branch is documented as deliberately not closing, because truncating an out-of-range value to call `close(2)` could close an unrelated descriptor — worse than leaking one on a path the kernel cannot produce.
- Mutation check on the reap assertion: removing `kill_child(pid)` makes `wait_child_timeout_kills_after_deadline` fail at the ESRCH assertion; restoring it passes. The test defends the reap, not just the exit status.
- State: 1351 pass / 0 fail / 1 ignored; fmt clean; clippy clean on 1.94.0 and 1.98.1; all `turn/scripted/tools_4/*` rows ok. Checkpoint recorded.
- `OwnedFd` refactor (`uwwyvxzm`): the pidfd is wrapped in `OwnedFd` right after the `< 0` check, so both manual `libc::close` calls are gone and every current and future early return is leak-free by construction rather than by review. First attempt used `fd as libc::c_int` and both clippy toolchains correctly rejected it (`cast_possible_truncation`) — the same truncation I had already refused by hand. Kept `try_from` and wrapped the checked value: no cast, no `allow`, no suppression.
- Renamed the non-Linux `pidfd_wait_timeout` to `poll_wait_timeout` — it is a `try_wait` loop, and the old name contradicted its own comment. Dispatch already goes through `platform_wait_child`, so nothing else moved.
- Two items in that review round were already resolved and I did not re-apply them: the `/proc/<pid>` probe was replaced with `child_alive_errno`/ESRCH last round, and no truncating `close` was ever committed. Verified against the file before acting rather than trusting the description.
- Non-Linux branch re-verified by cfg flip with exit status checked (`cargo check --all-targets` exit 0), not a filtered grep; 6 cfgs confirmed restored afterward.
- State: 1351 pass / 0 fail / 1 ignored; fmt clean; clippy exit 0 on both 1.94.0 and 1.98.1; workload rows ok. Checkpoint recorded. Branch 13 commits ahead of the pushed head.
- Stack hygiene: squashed the two fixup commits that corrected the commit before them — `uwwyvxzm` ("Wrap the pidfd without a truncating cast", whose parent never passed clippy) into `tkuuvpsx`, and `ryoulqrr` (the `/proc` → ESRCH portability correction) into `vuqosmpl`, the commit that introduced the probe. A commit whose subject describes undoing the previous one is review noise. Stack is 11 commits, one coherent unit each.
- Verified the squashes are semantically neutral rather than assuming: `jj new` onto each squashed commit and ran `clippy -p yach-bench --all-targets -D warnings` standalone — both exit 0, so the stack is bisect-safe, not merely green at the tip. Tip re-verified at 1351 pass / 0 fail / 1 ignored with zero clippy diagnostic lines on 1.94.0 and 1.98.1 (diagnostics shown, not exit codes alone).
- Two items in this review round were already in the tree: the non-Linux rename to `poll_wait_timeout` (definition and caller, done by hand — rust-analyzer indeed cannot see the inactive cfg) and the `#[cfg(target_os = "linux")]` on the `std::os::fd` import. My first check of the import used a line window that cut above the attribute; re-read the block before concluding. The cfg-flip build already proved there is no unused-import warning off Linux.
- Checkpoint recorded.
- Provenance of `ryoulqrr`, settled: `nvwvuzlx` ADDED the `/proc/{pid}` probe, so at first glance the removal "belongs" there. It does not. `vuqosmpl` is the commit that defines `child_alive_errno` (as part of `wait_child_timeout_reaps_child_when_wait_setup_fails`), and the replacement ESRCH assertion calls it — squashing the fix into `nvwvuzlx` would reference a helper that does not exist yet and make that commit fail to compile, breaking the bisect-safety verified above. Current placement in `vuqosmpl` is the earliest correct one, not merely the cheapest. The only residue is that `nvwvuzlx` briefly carries a Linux-only test assertion, which is harmless: it passes there, and the cfg split that makes it wrong does not land until `vsnuzmwt`. No further reshuffling; stack is final at 11 commits.

## Publication

Pushed on approval: `perf-measurement-framework` head `a1575b32`, 30 commits, PR #269 open. Push re-signed 10 commits, so local ids moved; remote head verified equal to local before reporting.

CI green on the pushed head: `Rust workspace` pass (3m45s) and `Perf (deterministic)` pass (12m24s). The Lint step that failed the previous round now passes, confirming the 1.98.1 clippy fixes.

Corrections published:
- Plane YACH-8 roll-up: comment retracting the ~23 ms extension claim, explaining both defects that produced it, and stating plainly that no overhead figure is resolvable at current sample counts.
- PR body: the live body never carried the bad number (the earlier `gh pr edit` attempt died on 401 before writing), and PR #269 has zero comments, so the wrong figure existed only in the chat handoff and the Plane roll-up. The prepared body update — corrected test counts, dual-toolchain lint note, 30-commit count, and a "Measurement corrections after review" section with the explicit no-claim statement — is staged at `.sjujperpowers/sdd/2026-09-08-performance-measurement-framework/pr-body-pending.md` and NOT applied: the PATCH returned HTTP 403 because the grant covered `push` and `pr` only. `agent-github-publication-plan` prints those two operations by default and does not mention `pr-edit`, which is why the approval lacked it.

Outstanding: apply `.sjujperpowers/sdd/2026-09-08-performance-measurement-framework/pr-body-pending.md` to PR #269 once a `pr-edit`-scoped approval exists. The published body is accurate as written (it makes no extension-overhead claim); the update adds the correction record and fixes the stale commit count and test numbers.

### Final verification of the retraction reach

Checked every surface by body text rather than by assumption, because an empty grep for two number
spellings is weak evidence:

- PR #269 issue comments: 0. Review comments: 0. Reviews: 0. No retracted figure exists anywhere on
  GitHub, in any spelling.
- Plane YACH-8: all three comments read in full. The original roll-up (`df2a6e4f`) lists the
  extension workloads but states no overhead number — it never carried the claim either. The
  correction comment (`1991ee94`) is in place and retracts BOTH figures (23 ms and ~10 ms).
- Conclusion: both retracted numbers existed only in the chat handoff. Nothing published needed a
  correction; the Plane comment is a record, not a repair.

Pending PR body refined per review: "corrections below" → "described above" (the section precedes
"Reviewing this"), and the pidfd bullet now carries the macOS caveat — non-Linux is a 1 ms
`try_wait` fallback, build/compat only, type-checked via cfg flip and NOT built on a Mac, which
matters because the prior baseline was an M2 Max.

`pr-edit` flag spelling is UNVERIFIED: the publication-plan helper prints only the `push` and `pr`
approval forms, and the policy tool could not be run from this session. The spelling below comes
from AGENTS.md prose, not from the tool's own help.

A checkpoint was recorded after the push and all subsequent commits (the pushed head `a1575b32` is
an ancestor of the checkpointed revision).

### Correction: the ledger is not version-controlled

`.sjujperpowers/` is gitignored (`.gitignore:39`) and `jj file list 'glob:.sjujperpowers/**'` returns
nothing. Three "commits" of ledger content therefore recorded zero changes and sat on the stack as
empty description-only revisions; all three abandoned. My earlier statement that the pending PR body
was "committed" was wrong: `pr-body-pending.md` exists on disk in this ledger directory and is
covered by host backups, NOT by Git. Anyone applying the pr-edit later reads it from disk; if the
workspace is lost, it must be reconstructed.

Stack is 30 commits, head `a1575b32`, identical to origin, with no empty revisions.
A final checkpoint was recorded.
