# Performance Measurement Framework Design

**Outcome:** plane:YACH-8

Status: accepted 2026-09-08 (owner decision in session)
Date: 2026-09-08

## Problem

Yach positions on a minimal, fast core with functionality delivered as
extensions. That claim is only defensible if every change can state its
performance delta and every release can restate absolute numbers. Today:

- `crates/yach-bench` has seven Criterion benches and eighteen `*-report`
  samplers, but they emit `key=value` text that is pasted by hand into
  `docs/benchmarks/*.md`. The last baseline is 2026-05-05 on an M2 Max;
  nothing has been measured on Linux and no report exists since.
- No comparison exists between two builds. A change cannot prove it did not
  regress anything.
- Coverage stops at the TUI, startup, edit engine, and session store. The
  core loop (request assembly, provider encoding, tool dispatch, the turn
  itself), binary size, memory, and the cost of extensions on the request
  path are unmeasured.
- CI runs fmt, clippy, and tests only.

Reference point: fx v0.0.8 (2026-09-07) ships per-release absolutes (CLI
startup ~0.5 ms before terminal init, 22 TUI interactions ≤15 ms p95 at 50
samples, binary 6.01 MiB, host-tool round trip 1.56–3.10 ms p95). The
credibility is not the numbers; it is that each release restates them.

## Decision

Extend `yach-bench` with a `perf` subcommand family: a static workload
registry, a JSON result format, a paired same-invocation A/B runner against
`main`, numeric per-workload budgets, and Markdown rendering. Expose the
private core-loop seams under the existing `yach-backend` `bench` feature.
Add a bounded turn-lifecycle trace sink. Gate deterministic classes in CI;
gate wall-clock classes locally through `just perf`.

Alternatives rejected:

- Composing hyperfine, `criterion --baseline`, and shell: cannot reach
  in-process seams, cannot interleave two builds, three output formats.
- A new crate on a paired-benchmark harness (tango): only covers code
  loadable as a shared library; process-level workloads (startup, RSS, PTY,
  extension host) are out of reach.
- A stored per-host baseline as the gate: cannot control load, thermals,
  toolchain drift, or cache state. Stored results are trend evidence only.

## Workload model

A workload is `{ id, class, isolation, requires, run }` registered in a
static table in `crates/yach-bench/src/perf/registry.rs`. `id` is a stable
hierarchical string; existing report labels (for example
`terminal/idle_keypress_to_draw_flush_live`) are kept verbatim so historical
reports still map to new results.

| class | samples | compared on |
|---|---|---|
| `latency` | `Duration` × N | p50/p95/p99/max; p95 is primary |
| `memory` | peak RSS bytes × N | max |
| `size` | one `u64` | value |
| `count` | one `u64` | value |

`isolation` ∈ {`in_process_serial`, `in_process_threaded`, `child_process`}.

- `in_process_serial` workloads run one at a time on the bench binary's main
  thread with no other workload live. Only these record allocation count and
  bytes, via a `#[global_allocator]` counting wrapper in the bench binary.
  Allocation figures are exact for a given build, not build-invariant:
  they are compared only between two builds from the same toolchain in the
  same run, never against a stored number. Every `in_process_serial`
  `latency` workload therefore contributes three verdict rows: its own
  (class `latency`, wall clock) and two derived `count` rows,
  `<id>#alloc_count` and `<id>#alloc_bytes`, measured only around the
  operation under test (fixture construction and result-vector setup are
  outside the counting window; the registry entry marks the window
  explicitly). Derived rows are real registry outputs, not fields, so
  filters, thresholds, `--deterministic`, and CI treat them like any
  other `count` workload.
- `in_process_threaded` workloads (async backlog producers, extension host
  round trips) report `alloc: null`.
- `child_process` workloads spawn the built `yach` binary. Peak RSS is
  `VmHWM` from `/proc/<pid>/status`, sampled while the child is alive
  (post-exec mm). Linux `ru_maxrss` from `wait4` returns
  max(child peak, parent peak at fork), so a worker bloated by earlier
  in-process workloads inflated every later child (measured 6.4 MiB
  alone vs 17.9 MiB in a full run, producing a false `regressed`
  verdict). `wait4` is used only to reap. `memory` workloads remain
  Linux-only in v1: the sampler reports `status: "skipped"`,
  `reason: "unsupported_os"` on every other target. Long-lived targets
  (the TUI) never exit on their own, so each `memory` workload declares
  a stop boundary: the sampler waits for that boundary (first output
  byte for `tui_ready`; `turn_completed` trace record for scripted
  turns), then sends SIGKILL and reaps with `wait4`. The reported
  figure is therefore peak RSS in bytes up to the boundary, and the
  boundary is part of the workload id's contract.

`requires` is a set drawn from {`binary`, `tty`, `linux`}. `tty` and
`linux` may be unsatisfiable: without a controlling terminal, or on a
non-Linux target, those workloads yield `status: "skipped"` with reason
`requires tty` / `unsupported_os`. `memory` workloads require
`{binary, linux}`. `binary` is always satisfiable because `perf run` and
`perf ab` build (or are given) the `yach` binary before measuring; a
missing binary or a missing `jj` for base materialization is a hard error.

Workload `status` is one of `ok`, `skipped`, or `error`. `error` is a
runtime failure inside a workload that was expected to run (child exit
before the boundary, PTY read timeout, fixture I/O failure, fewer than
`N` samples collected); the row carries `reason` and any collected
samples are discarded. `error` is never a verdict input: in `perf run`
it makes the exit code 1 after all other workloads have run; in
`perf ab` an `error` on either side yields verdict `error` for that
workload and exit code 1, ahead of `regressed`. Today's
`report_lines_indicate_failure` heuristics (`_error=`, `errors=`,
`count=0`) are replaced by this field.

Every existing `*-report` command becomes a workload. The `key=value`
commands and `usage_lines` are removed. All callers are documentation:
`docs/benchmarks/README.md` is rewritten; dated historical reports, plans,
and records keep their original command text as history.

## Result format

`yach-bench perf run --out <file> [--filter <glob>] [--samples N]
[--yach-bin <path>] [--deterministic] [--raw]` writes one JSON document:

```json
{
  "schema": 2,
  "host": { "fingerprint": "…", "cpu": "…", "cores": 24, "os": "…", "kernel": "…" },
  "build": { "source_sha256": "…", "commit": "<git HEAD or null>", "dirty": true,
             "profile": "release", "rustc": "…", "cargo_lock_sha256": "…",
             "yach_bin_sha256": "…" },
  "started_at": "2026-09-08T20:00:00Z",
  "workloads": [
    { "id": "request/assemble/100_turns", "class": "latency",
      "isolation": "in_process_serial", "status": "ok", "count": 100,
      "p50_ns": 0, "p95_ns": 0, "p99_ns": 0, "max_ns": 0,
      "alloc_count": 0, "alloc_bytes": 0 },
    { "id": "binary/size_bytes", "class": "size", "isolation": "child_process",
      "status": "ok", "value": 0 },
    { "id": "terminal/idle_keypress_to_draw_flush_live", "class": "latency",
      "isolation": "in_process_serial", "status": "skipped", "reason": "requires tty" }
  ]
}
```

`build` identifies what was measured, and must work for a jj working
copy, a plain Git worktree (`--base-dir`), and CI alike:

- `source_sha256` is the primary identity: the digest produced by
  `evals/scripts/source-digest.sh` (crate sources plus workspace
  manifests), reused rather than defining a second digest convention. The
  worker captures it from the checkout immediately before invoking the
  build, and the controller recomputes it after the build and refuses
  the result if it changed, so the digest always describes the artifact
  measured. Two results with equal `source_sha256` measured the same code
  regardless of `commit`.
- `commit` is `git rev-parse HEAD` of the checkout, or `null` when the
  checkout has no Git metadata. In a jj colocated checkout HEAD is the
  parent of `@`, so `commit` alone never identifies working-copy state.
- `dirty` is true when the tree differs from `commit`
  (`git status --porcelain` non-empty, or `commit` is `null`), so a
  working-copy measurement is never mistaken for the committed state.
- jj change ids are mutable and jj-specific; they are not in the schema.
  `perf report` may print `jj log -r @` context beside the table when
  run from a jj checkout, but that is presentation, not provenance.

`samples_ns` / `samples_bytes` arrays are present only with `--raw`.
`--deterministic` selects `size` and `count` workloads, including the
derived `#alloc_count` / `#alloc_bytes` rows; the serial scenarios they derive from are
executed with one sample and their wall-clock rows are omitted from the
document. `fingerprint` is a hash of CPU model, core count, OS, and
kernel; it labels trend files and never gates.

`yach-bench perf report <results.json> [<ab.json>]` renders the Markdown
shape `docs/benchmarks/README.md` mandates (date, commit, host, build,
command, per-workload table, verdicts when an A/B file is given).

## Paired A/B runner

`yach-bench perf ab [--base main | --base-dir <path>] [--filter <glob>]
[--rounds R] [--samples N] [--deterministic] --out <file>`.

Base materialization: `--base` resolves through `jj` into a persistent,
gitignored workspace `.perf/base/` (`jj workspace add` once, then
`jj workspace update-stale` and `jj new <base>` on later runs).
`--base-dir` skips materialization and uses whatever checkout is at that
path; CI uses it with a git worktree. Both sides are built through the
repository's declared environment (`just dev cargo build --release
--locked …`, run from each checkout's root so the base uses its own
`justfile` and dev shell) and must use the same `rustc`; a mismatch is a
hard error. The controller never invokes bare `cargo`. The
`Cargo.lock` hash is recorded per side so a dependency bump shows up in
the report alongside a delta rather than being mistaken for a code change.

Each side produces two `yach` artifacts and one `yach-bench`, all release:

- `target/release/yach` — the normal release-build artifact, exactly
  `just dev cargo build --release --locked -p yach`, no features, no
  post-processing. `Cargo.toml` defines no release profile overrides and
  no `strip`; if a release profile is ever added, this workload measures
  it automatically. Used by `binary/size_bytes`, `startup/*`,
  `memory/peak_rss/tui_ready`, and the existing startup first-output
  workloads.
- `target/bench/release/yach` — built with `--features bench` into a
  separate `CARGO_TARGET_DIR` so the feature never leaks into the shipping
  artifact's fingerprint. Used only by workloads that need the scripted
  provider (`turn/*` child-process rows, `memory/peak_rss/turn_*`).
- `target/release/yach-bench` — the runner. It receives both paths
  (`--yach-bin`, `--yach-bench-bin`); each `child_process` workload
  declares which one it needs.

The base side uses the same layout under `.perf/base/`.

Registry membership may differ between sides. A workload present only on
the current side is reported `added` and its row is informational (no
verdict, never fails); present only on the base side → `removed`, also
informational. Renames therefore surface as one `removed` + one `added`
row rather than a silent gap; a thresholds row whose glob matches no
workload on either side is a hard error so stale budgets are noticed.

Controller and workers: the current revision's `yach-bench` is the sole
controller. It never measures in its own process and never lets the base
revision orchestrate. Each side's `yach-bench` is invoked only as a
measurement worker through a versioned protocol:
`yach-bench perf worker --schema 2 --filter <glob> [--ids <csv>]
[--classes <csv>] --samples <N> --yach-bin <path> --yach-bench-bin
<path> --out <file>`. `--ids` and `--classes` scope retry and
class-restricted rounds. The worker inherits stdio so live-terminal
samplers own the real terminal; the controller reads and validates the
`--out` document instead of a stdout handshake. `--schema-probe` and
the mismatch error still print JSON. Registry ids, budgets, verdicts,
and rendering belong to the controller. The controller reads the
worker's `schema` and refuses to proceed (hard error naming both
versions) when it differs from its own. Schema bumps are therefore
deliberate: a change that alters the worker contract cannot compare
against a base older than itself, and the error says so instead of
producing a skewed table.

Bootstrap: a base whose `yach-bench` has no `perf worker` subcommand (every
revision before this spec lands, detected by probing
`yach-bench perf worker --schema-probe` and treating a non-zero exit or
non-JSON reply as "absent") is handled by an explicit
`--base-mode external` path, never by silently guessing. In that mode the
controller measures the base side itself, and only through boundaries the
base's ordinary shipping `yach` binary already exposes with no bench
feature, scripted provider, or trace sink:

- `binary/size_bytes` — file size of the base `target/release/yach`.
- `yach/tui_startup_first_output_pty` and
  `yach/tui_ready_startup_first_output_pty` — the existing first-byte PTY
  boundaries over `yach tui` and `yach tui-bench-ready`.
- `yach/cli_startup_first_output` — the existing first-byte boundary over
  `yach --quiet` with piped stdout (not a PTY; its class and timing
  boundary are preserved exactly as today).
- `memory/peak_rss/tui_ready` — `yach tui-bench-ready` to first output
  byte, then SIGKILL; peak RSS is sampled `VmHWM`, `wait4` only reaps.

Every other workload is reported `no_base_worker` on the base side:
in-process rows because base code is not linked, and `turn/*`,
`memory/peak_rss/turn_*`, `startup/phase/*`, and `turn/phase/*` because
they need the scripted provider or `YACH_TRACE`, which the base lacks.
`no_base_worker` is informational, never a verdict, and printed with its
reason. The controller's own `perf run` result for those rows is still
recorded so the first landing produces a complete trend file for `@`.

`--base-mode` defaults to `auto`: `worker` when the probe succeeds,
`external` otherwise, always printing which mode was chosen. The gate
is therefore partial exactly once — on the change that lands this spec —
and complete for every change after it. Validation is two explicit
stages: pre-merge, `just perf` against current `main` resolves to
`external`, compares exactly the five rows above, and reports every
other row `no_base_worker`; that run is the acceptance smoke for the
implementation. The full worker-mode self-comparison (`@ == main`, all
`unchanged`) is only possible once the framework is on `main` and is
tracked as a post-merge check on the Plane outcome, not as a pre-handoff
gate. Until it runs, "every change proves its delta" holds only for the
five rows above.

Rounds: a round is one ABBA block of four side-measurement slots
(`base`, `current`, `current`, `base`); a slot is a worker invocation in
`worker` mode or the controller's own external sampler for the base side
in `external` mode, with the same filter and `N` samples either way.
Within a round, a side's two result sets are concatenated into
one `2N`-sample set and summarized. Defaults `N=100`, `R=5`; total
per-side samples `2·N·R`.

Per-workload verdict from the `R` round summaries:

- `latency` / `memory`: round delta `d_r = current_stat / base_stat − 1`
  on the primary statistic. Sign agreement `a` = fraction of rounds whose
  delta sign matches the median delta's sign. Base spread
  `s = (max_r − min_r) / median_r` over base's round statistics.
  - median `d` beyond `+threshold` and `a ≥ 0.8` → `regressed`.
  - median `d` beyond `−threshold` and `a ≥ 0.8` → `improved`.
  - median `d` beyond threshold with `a < 0.8`, or median `d` within
    threshold with `s > threshold` → `inconclusive`. The orchestrator runs
    up to two additional ABBA rounds for that workload only and
    re-evaluates over all rounds. Still inconclusive → stays
    `inconclusive`.
  - otherwise `unchanged`.
- `size` / `count`: exact compare of the single value against threshold;
  no rounds beyond the first are needed and none are run.
- `skipped` on either side → `skipped`; `error` on either side → `error`.

Exit code: 1 if any `error` or `regressed`, else 2 if any `inconclusive`,
else 0. Neither `error` nor `inconclusive` ever passes silently.

Output `<file>` contains both sides' per-round result documents plus the
verdict table; `perf report` renders it.

## Budgets

`crates/yach-bench/perf-thresholds.toml`:

```toml
[defaults]
latency_pct = 5.0
memory_pct = 10.0
size_pct = 0.5
count = 0

[[workload]]
id = "turn/scripted/*"
latency_pct = 8.0
comment = "scripted turns include tokio scheduling; wider budget"
```

Rows are matched by glob, most specific (longest literal prefix) wins. Only
numeric fields have effect; `comment` is documentation. An intentional
regression lands by raising the affected workload's budget in the same
change so the reviewer sees a numeric diff against a named workload. There
is no reason-only waiver.

## Recipes

- `just perf [filter]` — build both sides' three artifacts, `perf ab`,
  print the table, propagate the exit code.
- `just perf-record` — `perf run` for `@` only, saved to
  `~/.cache/yach/perf/<fingerprint>/<date>-<commit>.json`. Trend evidence.
- `just perf-report <json> [<ab.json>]` — render Markdown to stdout.
- `just perf-profile <workload-id> [samples]` — `perf record -g` (Linux)
  or `samply` (macOS) around `yach-bench perf run --filter <id> --samples
  <n>` built with `CARGO_PROFILE_RELEASE_DEBUG=1`; prints the flamegraph
  or profile path.
- `cargo bench` remains the Criterion path for exploratory statistical
  microbenchmarks. Criterion benches are not part of the gate.

## CI

A `perf-deterministic` job in `.github/workflows/ci.yml` on `pull_request`:
checkout with `fetch-depth: 0`, `git worktree add .perf/base
<base-sha>`, install `dtolnay/rust-toolchain`, build both sides with
bare `cargo` (the job passes `--build-cmd cargo`), run
`yach-bench perf ab --base-dir .perf/base --deterministic --rounds 1
--build-cmd cargo --out ab.json`, fail on `regressed`, and write the
rendered table to the job summary. The job cannot enter the declared
dev shell: `flake.nix` locks the `nix-config` input to a private
`git+ssh://` URL that a GitHub runner cannot fetch. Deterministic
metrics only require both sides built identically inside the same job.
Local `just perf` is unchanged and still builds through `just dev
cargo`. This gates `binary/size_bytes`, `request/roster_bytes/*`, and
every `#alloc_count` and `#alloc_bytes` row. Wall-clock classes never
run in CI.

## Core-loop seams (`yach-backend`, `bench` feature)

1. `bench_loop` module.
   - `ScriptedProvider` implements the private `ProviderRequester`
     (`runner.rs:3896-3939`) from `Vec<Vec<ProviderStreamEvent>>`, one
     response per request. Responses may request builtin tool calls.
   - `run_scripted_turn(ScriptedTurnConfig) -> ScriptedTurnProfile` drives
     the runner's private per-turn path
     (`run_native_provider_one_agent_tool_round`, orchestration at
     `runner.rs:9037-9067`) against a temp project and temp JSONL session
     store, returning wall time and the trace phases below. The
     `cfg(test)` requester-loop helpers at `runner.rs:1007-1059` merge into
     this seam rather than surviving as a third copy.
2. `request_assembly::assemble(log: &SessionLog, turn: &TurnId,
   checkpoint: Option<&str>)` wrapping `provider_messages_from_event_slice`
   (`runner.rs:3673`) for fixture logs of N turns
   (`SessionLog { events }`, `session.rs:452-457`).
3. `tools::advertised_roster_bytes(&ResolvedToolCatalog) -> usize`, the
   `serde_json::to_vec` length of the value produced by
   `build_provider_tool_advertising_extension` (`tools.rs:630-651`). Not
   bench-gated: it is pure and small.
4. `yach` CLI `bench` feature: `crates/yach-cli/Cargo.toml` gains
   `[features] bench = ["yach-backend/bench"]`. Under it, the provider
   selection at `main.rs:1017-1060` accepts `YACH_RIG_PROVIDER=scripted`
   with `YACH_BENCH_SCRIPT=<path>` (JSON of the same
   `Vec<Vec<ProviderStreamEvent>>`). Child-process turn workloads and the
   hashline extension-host workload need this. The feature binary is built
   into a separate target dir as described under the A/B runner; the
   shipping binary never carries it.

No provider-specific wire encoder exists as a seam.

## Turn trace sink

`StartupTrace` (`yach-ui/src/app.rs:55-91`) accumulates marks in a
mutex-guarded `Vec` and rewrites the whole file on each `flush`. That is
acceptable for a dozen startup marks and unusable for per-turn tracing.
The backend today receives marks through a CLI-injected callback
(`StartupTraceMarker`, `runner/extension_state.rs:13-28`) because
`yach-ui` depends only on `yach-proto` and must not depend on the backend.

Replace both with `yach_trace::TraceSink`, a new tiny crate
`crates/yach-trace` (dependencies: `serde`, `serde_json`), owned by the CLI:

- `yach-proto` stays a transport DTO contract; a sink with a file handle
  and `BufWriter` does not belong there. `yach-trace` is depended on by
  `yach-ui`, `yach-backend`, `yach-cli`, and `yach-bench`; it depends on
  no workspace crate, so the UI→proto-only boundary is preserved and the
  UI never depends on the backend. `StartupTrace` in the UI and
  `StartupTraceMarker` in the backend are removed; `RunnerConfig` and
  `run_tui_with_startup_trace*` take `Option<TraceSink>` (a cheap
  `Arc` handle) instead.
- Constructed by the CLI from `YACH_TRACE` (the `YACH_STARTUP_TRACE` name
  is not kept). Absent → `None`; `mark` on `None` is a no-op, as today.
- Records are appended through a `BufWriter` behind a mutex, flushed at
  turn boundaries and on drop; nothing is retained in memory.
- Each record is one JSON line:
  `{"t_us":<elapsed_micros>,"scope":"startup","label":"cli_args_parsed"}`
  or `{"t_us":…,"scope":"turn","turn_id":"…","label":"tool_dispatched","n":2}`.
  `scope`, `label`, and `t_us` are required; other keys are per-scope
  attributes. Unknown keys are ignored by readers. The bench parser
  (`yach-bench/src/startup_trace.rs`) is replaced by `trace.rs` reading
  this format.
- Startup marks keep their current labels. Turn labels emitted by the
  runner: `prompt_received`, `request_assembled`, `provider_request_sent`,
  `provider_first_event`, `provider_stream_end`, `tool_dispatched` (`n`),
  `tool_result_appended` (`n`), `session_persisted`, `turn_completed`.

Zero cost when unset is a contract: a test asserts no file is opened and
`mark` is a no-op. When set, failure is loud, not silent: if the file
cannot be opened at construction the CLI exits with an error naming the
path, and a write failure after construction is reported once on stderr
and disables the sink for the rest of the process (subsequent `mark`
calls become no-ops) so a full disk cannot fail a turn. The bench
parser treats a truncated final line as `error` for every workload that
depends on that trace file.

## Workloads added

`[s]` = `in_process_serial`, `[t]` = `in_process_threaded`,
`[c]` = `child_process`.

| id | class | notes |
|---|---|---|
| `turn/scripted/text_only` [s] | latency | one request, no tools |
| `turn/scripted/tools_4/builtin` [s] | latency | four sequential `read_text_file` calls on a fixture project |
| `turn/scripted/tools_4/hashline_ext` [c] | latency | same script through the hashline extension host; first extension-vs-builtin comparison |
| `turn/scripted/tools_4/inactive_ext_8` [c] | latency | eight inactive extension roots installed; request-path cost of merely having extensions |
| `turn/phase/{request_assembled,provider_request_sent,provider_first_event,provider_stream_end}` [c] | latency | derived from a `text_only` child run (one round); no `turn/phase/prompt_received` row |
| `turn/phase/{tool_dispatched,tool_result_appended,session_persisted,turn_completed}` [c] | latency | derived from the `tools_4/builtin` child run |
| `request/assemble/{10,100,1000}_turns` [s] | latency | the fold |
| `request/roster_bytes/builtin` [s] | count | |
| `request/roster_bytes/hashline_ext` [s] | count | with the replacement bundle |
| `provider/encode/{rig_messages,rig_tools}/100_turns` [s] | latency | adapter request encoding, no network |
| `startup/phase/<label>` [c] | latency | existing startup marks as separate workloads |
| `binary/size_bytes` [c] | size | file size of `target/release/yach` as built |
| `memory/peak_rss/tui_ready` [c] | memory | synthetic-ready TUI to first output, then kill |
| `memory/peak_rss/turn_scripted_tools_4` [c] | memory | scripted turn via child `yach` to `turn_completed`, then kill |

Existing workloads carried over unchanged: headless replay (6), live
terminal (9, `tty`), startup first-output (2 PTY + 1 piped stdout),
startup profile (3 scenarios), extension runtime profile, native edit
profile (5 scenarios × phases).

## Files

- `crates/yach-bench/src/perf/{mod,registry,runner,ab,verdict,thresholds,report,alloc,rss}.rs` — new.
- `crates/yach-bench/src/main.rs` — dispatch becomes `perf run|ab|report`;
  report-line functions move into registry entries.
- `crates/yach-bench/perf-thresholds.toml` — new.
- `crates/yach-bench/Cargo.toml` — add `libc`, `serde`, `toml`.
- `crates/yach-trace/` — new crate: `TraceSink` and record type; added to
  the workspace members and the `publish_crates` list in `justfile`.
  Publishing is forced, not chosen: `yach-ui` and `yach-backend` are
  published crates and a published crate cannot depend on an unpublished
  one. The public surface is deliberately two items (`TraceSink`,
  `TraceRecord`) so the release burden stays nominal.
- `crates/yach-backend/src/{bench_loop,request_assembly}.rs` — new;
  `runner.rs` emits turn marks through `TraceSink` and takes an injected
  requester in the bench path; `runner/extension_state.rs` drops
  `StartupTraceMarker`; `tools.rs` gains `advertised_roster_bytes`.
- `crates/yach-cli/Cargo.toml`, `src/main.rs` — `bench` feature, scripted
  provider, `YACH_TRACE` sink construction.
- `crates/yach-ui/src/app.rs` — `StartupTrace` removed; `run_tui_*` take
  `Option<TraceSink>`.
- `justfile` — `perf`, `perf-record`, `perf-report`, `perf-profile`.
- `.github/workflows/ci.yml` — `perf-deterministic` job.
- `.gitignore` — `.perf/` (`target/bench/` is already covered by `/target/`).
- `docs/benchmarks/README.md` — rewritten around the registry and
  recipes; PRD target table populated from the first Linux run.
- `docs/benchmarks/baseline-<date>.md` — first Linux baseline rendered by
  `perf report` on the day the implementation lands.

## Testing

- Verdict logic: table-driven unit tests over synthetic round summaries
  covering each branch (regressed, improved, unchanged, inconclusive with
  sign disagreement, inconclusive with base spread, skipped propagation,
  count exact compare, one-sided `added`/`removed` rows never failing,
  derived `#alloc_count` / `#alloc_bytes` rows gated as `count`).
- Threshold matching: most-specific glob wins; defaults apply; unknown
  fields rejected; a row matching no workload on either side is an error.
- RSS sampler: Linux KiB→bytes conversion is a pure function with a unit
  test; non-Linux targets yield `skipped` / `unsupported_os` (tested by
  compiling the skip path unconditionally and asserting on it); one
  Linux test spawns a child that allocates a known amount, stops at its
  first output byte, and asserts the byte figure is within that bound.
- Trace sink: no-op when unset; appended lines parse; bounded memory
  (marks written, not retained).
- Scripted provider: one integration test drives `run_scripted_turn` with
  a two-tool script and asserts the session log contains the expected
  event sequence — this also proves the bench seam matches production
  behavior.
- A/B orchestration: one test with a stub worker binary (shell script)
  proving ABBA order, per-round concatenation, exit codes, a hard error
  on schema mismatch, and `auto` resolving to `external` — with exactly
  the five shipping-binary rows compared and the rest `no_base_worker` —
  when the stub base lacks `perf worker`.
- Pre-merge smoke: `just perf` on this machine against current `main`
  prints `base-mode: external`, gives verdicts for exactly the five
  shipping-binary rows, and reports the rest `no_base_worker`; recorded in
  the baseline report.
- Post-merge check (Plane YACH-8, not pre-handoff): `just perf --filter
  'request/*'` with `@ == main` reports every workload `unchanged`.

## Non-goals

- Token estimates per request (needs a tokenizer; roster bytes is the
  proxy).
- Cross-harness comparisons against fx or Pi.
- Benchmarking real providers or network paths.
- Heap profiling (`dhat`); the allocation counter plus flamegraphs cover
  the first optimization passes.
- Wall-clock gating in CI.
- Keeping the `*-report` commands or `YACH_STARTUP_TRACE` as aliases.
