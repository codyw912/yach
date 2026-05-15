# Native Edit Benchmark And Profiling Design

Date: 2026-05-15
Status: proposed

## Context

Native edit transactions now have a preview engine, guarded crate-local apply,
redacted session evidence, and a backend-local harness that records prepared
and finished edit events. The next question is performance and traceability:
before edits become CLI, TUI, hidden-tool, provider-visible, or extension-owned
surfaces, yach should be able to explain where edit time goes.

Existing benchmark infrastructure already has two useful patterns:

- Criterion microbenchmarks in `crates/yach-bench/benches/` for stable,
  optimizer-aware primitive measurements.
- `yach-bench` report commands that collect repeated samples and print
  report-friendly `p50`, `p95`, `p99`, and `max` summaries for granular
  profiling.

The native edit harness gives a good profiling boundary without exposing
mutation to providers. This slice should measure that boundary and its internal
phases, not add new edit UX.

## Goal

Design benchmark-only edit profiling that can measure native edit preview,
apply, evidence conversion, session append, and end-to-end harness cost with
enough granularity to identify bottlenecks before file mutation is exposed to
users, providers, or extensions.

## Non-Goals

- No CLI or TUI edit command.
- No provider-advertised edit, write, or mutation tool.
- No hidden built-in edit tool.
- No extension-owned mutation tool.
- No approval UI.
- No production tracing API.
- No new runtime session events beyond the existing edit evidence records.
- No performance claims against Pi or other harnesses.
- No optimization work unless a benchmark implementation bug requires it.

## Approach Options

### Option A: Criterion microbenchmarks only

Add `native_edit` Criterion benches for preview, apply, evidence summary, and
session append.

This is repeatable and fits the existing bench crate, but it is less useful for
quick exploratory profiling because it does not emit the same phase-by-phase
human-readable report shape used by startup profiling.

### Option B: Report mode only

Add `yach-bench native-edit-profile-report --samples N` that repeatedly runs
edit scenarios and prints p50/p95/p99/max for each phase.

This gives the most actionable local profiling output, but it loses the
Criterion baseline that is useful for tracking primitive changes over time.

### Option C: Criterion plus report mode

Add both a Criterion bench target and a `yach-bench` report command over the
same deterministic edit scenarios.

This is the recommended option. Criterion gives stable microbenchmark coverage,
while the report mode gives granular phase timings that are easy to paste into
benchmark reports and compare between branches.

## Recommended Shape

Add a new benchmark module, likely `crates/yach-bench/benches/native_edit.rs`,
and register it in `crates/yach-bench/Cargo.toml`.

Add a report command to `crates/yach-bench/src/main.rs`:

```text
yach-bench native-edit-profile-report --samples N
```

The report command should use the existing `LatencySummary` and output format:

```text
workload=native_edit/<scenario>/<phase> count=100 p50=... p95=... p99=... max=...
```

The Criterion and report paths should share scenario-building helpers where
that does not complicate the code. If sharing would require awkward public API
or generic abstractions, duplicate small fixture setup locally in the bench
crate.

## Scenarios

The first benchmark set should cover a small matrix that matches the current
edit engine:

- `create_small_text_file`: creates a new UTF-8 file under `src/`.
- `modify_single_hunk_small_file`: replaces one exact-match hunk in a small
  UTF-8 file.
- `modify_multi_hunk_medium_file`: replaces multiple exact-match hunks in a
  medium UTF-8 file.
- `validation_failure_path_traversal`: fails during preview before a
  transaction exists.
- `apply_failure_hash_changed`: succeeds at preview, mutates the target between
  preview and apply, then fails apply without writing the requested change.

All five scenarios are in scope for the first implementation. The
apply-failure fixture should avoid timing-sensitive races by making a
deterministic file change between preview and apply.

## Phases

The report mode should time these phases separately where applicable:

- `preview`
- `prepared_evidence_summary`
- `apply`
- `finished_evidence_summary`
- `session_append_events`
- `end_to_end_harness_success`
- `end_to_end_harness_validation_failure`
- `end_to_end_harness_apply_failure`

For direct phase timing, the report should call the underlying edit engine and
summary helpers in sequence. For end-to-end timing, it should call
`NativeEditHarness::preview_and_apply` or
`NativeEditHarness::preview_and_apply_with_apply_policy`.

Because `NativeEditEngine::apply`, the harness, and the evidence summary
helpers are intentionally crate-local, the implementation should add a narrow
benchmark-facing seam rather than making those primitives public.

The recommended seam is a `bench` feature on `yach-backend` that exposes a
small profiling helper, such as `NativeEditProfileRunner`, only when
`yach-bench` opts into that feature. The helper should run predefined scenarios
and return phase durations plus categorical sample outcomes. It must not return
prepared transactions, after-images, full file bodies, raw edit request JSON, or
make `NativeEditEngine::apply` public.

`crates/yach-bench/Cargo.toml` should depend on `yach-backend` with the bench
feature enabled. Normal yach binaries should not enable the feature.

## Fixtures

Fixtures should be deterministic, local-only, and disposable:

- create temporary project roots under `std::env::temp_dir()`;
- use unique directory names with process ID and an atomic counter;
- remove fixture directories in `Drop`;
- avoid reading repository files;
- avoid network access and subprocesses;
- keep file sizes small/medium enough for stable local runs.

For apply benchmarks, each sample should create a fresh fixture or restore the
target file before applying, because apply mutates local state. Criterion should
use `iter_batched` or `iter_batched_ref` with an appropriate `BatchSize`.

## Report Semantics

The report command should collect each sample independently and continue
collecting when an individual sample fails, mirroring startup profile behavior.
Output should include:

- `samples_requested=N`
- `samples_collected=M`
- optional `errors=K`
- optional `first_error=...`
- one summary line per workload/phase with non-empty samples

Validation-failure and apply-failure scenarios are successful benchmark samples
when they fail with the expected `NativeEditError`. Unexpected success or the
wrong error variant should count as a sample error.

## Evidence And Privacy

Benchmarks must preserve the existing redaction boundary:

- do not print file bodies;
- do not print raw edit request JSON;
- do not persist absolute temp paths in report output;
- do not add provider-visible tool schemas;
- do not modify `NativeToolRegistry`.

It is acceptable for temporary fixture files to contain simple synthetic text.
The report should identify scenarios and phases, not local paths.

## Relationship To Production Tracing

Do not add production edit tracing hooks in this slice.

Bench-only profiling is enough to answer the immediate question: whether edit
preview, apply, evidence conversion, or session append is the expensive part.
Production tracing can be designed later when there is a real user-facing edit
entry point and a clear need for runtime diagnostics.

## Testing

The implementation plan should include focused tests for report parsing and
failure behavior:

- `native-edit-profile-report --samples 1` emits expected workload labels;
- validation-failure samples are counted as successful samples when the
  expected error occurs;
- report output does not include synthetic file bodies;
- Criterion target compiles.

Final verification should include:

```bash
just dev cargo fmt --check
just dev cargo test -p yach-bench
just dev cargo test -p yach-backend
just dev cargo bench -p yach-bench --bench native_edit -- --test
just dev cargo run -p yach-bench --release -- native-edit-profile-report --samples 5
```

If release report runtime is too slow for routine CI-like verification, the
implementation plan may use a smaller debug-mode smoke command for local
correctness and leave the release command as the evidence-collection path.

## Follow-Up Work

- Record a benchmark report under `docs/benchmarks/` after implementation.
- Use the profile to decide whether any edit optimization work is warranted.
- Design CLI/TUI edit access only after the edit cost is understood.
- Add production tracing only when a user-facing edit path needs diagnostics.
