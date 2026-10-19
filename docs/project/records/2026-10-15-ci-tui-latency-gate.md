# CI TUI Latency Gate (2026-10-15)

Kata: yach#mpag

Owner request (2026-10-14): CI must catch render-time regressions, not only
allocation counts. `Perf (deterministic)` gates size and count. This change
adds `Perf (latency)`, an opt-in wall-clock gate for the headless TUI rows
whose shared-runner A/A spread is tight enough to budget.

## Probe

Workflow: `.github/workflows/perf-latency-probe.yml` on branch
`perf-probe-tui-rows`, deleted in this change once the result below was
recorded. The 2026-09-14 takeaways record says to keep that workflow only
until the per-row gate is built from it; this change is that gate, so the
workflow goes.

A/A by construction: both sides are the same commit, so every delta is
harness noise. Each row is its own exact-id filter. `glob` 0.3 does not
expand braces, and a first push (run 38735209195, commit `8fb31c71`) filtered
with a brace alternation, matched nothing, and wrote empty verdict tables.
That run is not evidence.

Evidence run: [38737173763](https://github.com/codyw912/yach/actions/runs/38737173763),
commit `9c54bc4e`, job 116429317453, 2026-10-15. Three runs per row, five
rounds, 20 samples. All twelve comparisons wrote a document.

Runner: GitHub-hosted `ubuntu-latest`, 4 cores, Intel Xeon Platinum 8573C,
16 GB, kernel `6.17.0-1022-azure`, Microsoft hypervisor. Steal ticks were 0
on every comparison.

Toolchain: the workflow pins `dtolnay/rust-toolchain@1.94.0`, the pin the
2026-09-14 measurement used. It did not take. `rust-toolchain.toml` overrides
the default, and the runner auto-installed 1.98.1 and compiled with it
(`rustc 1.98.1 (48a229cea 2026-09-01)`, recorded in every result document).
That is the repository toolchain, and the same one `Perf (latency)` installs,
so the spreads below are the noise floor of the gate rather than a different
compiler. The pin was left as written; the override is why the result is
1.98.1.

## Per-row spread

`base_spread_pct` is the p95 spread the latency gate judges. Verdicts are
against the 5% default, which is why two within-delta runs came back
`inconclusive`.

| row | spreads | widest | verdicts | current p95 |
|---|---|---:|---|---|
| `viewport/huge_transcript_scroll_headless/10000` | 4.84 / 1.96 / 6.43% | 6.43% | unchanged x2, inconclusive x1 | 21.0–25.2 ms |
| `keypress/active_stream_on_history_headless/100` | 2.46 / 1.68 / 0.80% | 2.46% | unchanged x3 | 37.9–42.6 ms |
| `replay/heavy_tool_output_tail_headless/102400` | 4.19 / 3.01 / 1.71% | 4.19% | unchanged x3 | 0.85–1.20 ms |
| `startup/backend_ready_to_first_interactive_headless` | 4.01 / 6.34 / 4.77% | 6.34% | unchanged x2, inconclusive x1 | 0.43–0.74 ms |

Budget rule, same as the existing rows in
`crates/yach-bench/perf-thresholds.toml`:
`latency_pct = ceil(widest base_spread_pct across runs) + 3`. A row whose
widest spread is at or above about 40%, or that is `inconclusive` on most
runs, stays manual. None of these four hit either bar. The two inconclusive
verdicts are one run each, against the 5% default, with the median delta
inside that default; they settle under the budgets below.

| row | budget |
|---|---:|
| `viewport/huge_transcript_scroll_headless/10000` | 10 |
| `keypress/active_stream_on_history_headless/100` | 6 |
| `replay/heavy_tool_output_tail_headless/102400` | 8 |
| `startup/backend_ready_to_first_interactive_headless` | 10 |

## Telemetry

CPU pressure (PSI some, mean/max) and steal, sampled at 1 Hz during each
comparison:

| comparison | cpu PSI mean/max | steal ticks |
|---|---|---:|
| viewport 1 | 24.68 / 35.86 | 0 |
| viewport 2 | 5.16 / 9.56 | 0 |
| viewport 3 | 1.20 / 2.35 | 0 |
| keypress 1 | 0.19 / 0.38 | 0 |
| keypress 2 | 0.02 / 0.06 | 0 |
| keypress 3 | 0.00 / 0.00 | 0 |
| replay 1–3 | 0.00 / 0.00 | 0 |
| startup 1–3 | 0.00 / 0.00 | 0 |

Memory pressure was 0.00 on every comparison. The viewport row took the
first window, while the runner was still settling, and still held a 6.43%
worst spread. Later rows ran quiet.

## Rows left manual

Every latency row not named above stays on `just perf`. That is the short
headless rows (`keypress/idle_keypress_to_paint_headless`,
`paste/large_multiline_component/102400`), every live `terminal/*` row (needs
a TTY the runner does not have), the shipping-binary first-output rows,
`startup/phase/*`, `turn/scripted/*`, `review/route/*`, and `extension/*`.
The 2026-09-14 shared-runner probe already left
`extension/execute/one_call` manual: `inconclusive` on two of three runs,
spread 48/62/63% against a 48% budget. No row measured for this gate was
left manual.

## Absolute frame-time guard

Not implemented. The issue asked whether a per-delta streaming render on a
10k history should also be held under a fixed fraction of a 16 ms frame,
independent of A/B.

The probe does not support that guard for the row it would apply to.
`keypress/active_stream_on_history_headless/100` measured p95 37.9–42.6 ms
on this runner, above a 16 ms frame, so a fraction-of-a-frame ceiling would
fail the current code on every run. That absolute is a product question,
not a noise question: the same row's A/A spread was 0.80–2.46%, which is
why it is gated relatively. `viewport/huge_transcript_scroll_headless/10000`
measured p95 21.0–25.2 ms, also over a frame. The two sub-millisecond rows
are far under one, and a ceiling there would not have caught the #290
regression, which was a relative jump on the viewport row.

## What the job does

`Perf (latency)` runs on pull requests only. It materializes the PR base the
same way `Perf (deterministic)` does, restores the same `perf` rust-cache,
builds the controller into `target/release-root`, and runs `perf ab` with
`--rounds 2` and `--build-cmd cargo`, once per gated row. One `--filter`
cannot name four ids: glob 0.3 does not expand braces, which is what made
run 38735209195 empty. Exit 1 (`regressed` or `error`) fails the job. Exit 2
(`inconclusive`) is written to the step summary and does not fail it.

## Superseded 2026-10-19: latency is advisory

Owner decision: wall-clock latency on shared runners no longer gates
merges. In the 30 days to 2026-10-19 this job failed five times, all false
alarms: four were shared-runner noise, including +10.5% and +22.2%
"regressions" on pull requests that changed only AGENTS.md, and one was a
count-row verdict this job should not have judged. On `bd2e496e` its count
values were the deterministic job's per-iteration values times 100
(`keypress/active_stream_on_history_headless/100#alloc_bytes` 6,336,566,400
→ 6,336,576,000 against a 96-byte budget). That scaling was fixed in #309
(2026-10-16), which takes latency runs' `#alloc_*` values from a separate
one-sample measurement; count rows still belong to `Perf (deterministic)`.

The job moved to `.github/workflows/perf-latency.yml`. It runs on pull
requests labeled `perf:latency` and on `workflow_dispatch`, reads each
row's verdict from its `ab-*.json` document, warns on a latency
`regressed` or `inconclusive`, ignores count rows, and fails only on a
harness failure. The budgets above still decide each advisory verdict.
Current behaviour is described in `docs/benchmarks/README.md`, "CI gates".
