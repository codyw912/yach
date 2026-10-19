# Callgrind Instruction Counts for Headless TUI Rows (2026-10-19)

Kata: yach#kt6q

Owner direction (2026-10-17): measure CPU instruction counts with callgrind
for the in-process headless rows, stable on shared runners, and keep a
separate lower-noise wall-clock path for cache and I/O effects that
instruction counts miss. On 2026-10-19 wall-clock latency on shared runners
became advisory (yach#2mre). This record asks one question: are instruction
counts repeatable enough on GitHub's shared runners to report, and later to
gate?

## Method

Rows: the four headless rows `Perf (latency)` measures, as they run in
`yach-bench perf worker --ids <row> --samples 1` today. No harness change.

```
valgrind --tool=callgrind --collect-atstart=no \
  '--toggle-collect=*yach_bench::perf::workloads::tui::HEADLESS::{closure#*' \
  yach-bench perf worker --ids <row> --samples 1 --out w.json
```

`--toggle-collect` limits collection to the registry closure that runs the
workload (`crates/yach-bench/src/perf/workloads/tui.rs`), so worker start-up,
provenance hashing (`capture_build` hashes the checkout and was 42% of an
unbounded startup-row count) and JSON output are not counted. Workload
warm-up and per-sample setup inside the closure are. The unbounded counts
("whole") are recorded alongside for comparison.

## Results

Probe workflow `.github/workflows/callgrind-probe.yml` on branch
`callgrind-probe`, deleted once this record was written (commit `c7109e3c`, base `main` `e7f17843`), run
[39188791127](https://github.com/codyw912/yach/actions/runs/39188791127),
three parallel `ubuntu-latest` jobs, three runs per row each. Ubuntu's
Valgrind 3.22.0, rustc 1.98.1. Slots a and b ran on AMD EPYC 7763; slot c on
AMD EPYC 9V74 (AVX-512).

| row | bounded Ir, all 9 runs | whole Ir range |
|---|---:|---|
| `startup/backend_ready_to_first_interactive_headless` | 6,532,521 (first run) / 6,532,755 (runs 2–3) | 12,914,301 – 12,914,982 |
| `keypress/active_stream_on_history_headless/100` | 748,788,472 | 755,701,929 – 755,702,294 |
| `replay/heavy_tool_output_tail_headless/102400` | 16,940,452 | 23,322,559 – 23,322,971 |
| `viewport/huge_transcript_scroll_headless/10000` | 431,127,676 | 438,836,776 – 438,837,172 |

- Bounded counts are identical across both CPU models and all three jobs.
  The only variation is the startup row's first run in each job, +234
  instructions (0.004%) on runs 2–3, and the same on every job. That is
  [INFERENCE] a first-run effect outside the binary's control (for example
  a file-system cache the startup path reads); a fresh `$HOME` locally did
  not reproduce it. The CI report discards one run per row before
  measuring base and current, so the effect is never on a reported side.
- Whole counts vary by up to ~400 instructions across CPU models and between
  runs on one job. Provenance and process start-up are why the bounded form
  is the one to use.
- Wall time on the runner: the Measure step was 47–60 s for 24 callgrind
  runs, about 2–2.5 s each. The CI report runs eight (four rows, two
  sides), about 20 s. Valgrind installs in 10–16 s.

Local, NixOS on Ryzen 9 3900X with Valgrind 3.27.1 and a devenv build:
bounded counts also repeated exactly within the host, and differ from the
runner's by 0.1–0.9% per row. Counts are comparable only within one build
environment and Valgrind version, which a base/current comparison on one
runner satisfies.

## Invocation must match on both sides

The same binary measured from two paths (`.perf/base/target/release-root/...`
and `./target/release-root/...`) gave deltas of +0.03%, −0.05%, −0.00% and
−0.01% on the four rows. Environment size changes it too: `env -i` moved the
startup row from 6,476,962 to 6,488,044. Path and argv length change
allocation sizes and string work. A comparison must therefore run both sides
from one fixed path with one environment; the CI report copies each binary
to the same temporary path before measuring, after which the same binary
reports +0.00% on all four rows.

## Decision

Add the counts to `Perf (deterministic)` as a report-only step summary
table (`.github/scripts/instruction_counts.sh`), next to the existing count
rows. It never fails the job. Gating, a schema `count` row, and moving the
bounds into the harness itself (client requests around the timed regions,
excluding warm-up) are follow-ups once several pull requests have shown
what a real change moves.

The wall-clock half of the owner direction (a quiet machine or `main`
trend) is unchanged by this record.
