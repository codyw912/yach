# Benchmark Buildout

This directory holds yach performance reports and benchmark-harness notes. Performance is a first-class product requirement for yach, not a nice-to-have: the Rust shell only justifies itself if it proves better responsiveness or scalability on important same-machine workloads.

Use this directory for detailed reports, harness notes, and benchmark artifacts. Same-machine Pi comparisons must follow `pi-comparison-methodology.md` before any product claim is made; note that the Pi adapter was removed on 2026-07-16, so historical Pi comparisons are no longer reproducible in-repo.

## Performance targets from the PRD

Source: `../../PRD-v0.1.md` §10-11.

| Target | Status | Harness placeholder |
|---|---|---|
| Startup to interactive prompt `<250 ms after backend ready` | `met` (`startup/backend_ready_to_first_interactive_headless` p95 508 µs) | Measure time from backend-ready event to first usable input frame. |
| p95 keypress-to-paint, idle `<16 ms` | `met` (`keypress/idle_keypress_to_paint_headless` p95 532 µs; live `terminal/idle_keypress_to_draw_flush_live` p95 52 µs) | Synthetic key event replay through TUI render loop while backend is idle. |
| p95 keypress-to-paint, active stream `<32 ms` | `met` (`keypress/active_stream_replay_headless/100` p95 22.75 ms; live `terminal/active_stream_keypress_to_draw_flush_live` p95 37 µs) | Replay high-rate token stream while injecting input events. |
| p99 keypress-to-paint, heavy tool output `<50 ms` | `met` (`replay/heavy_tool_output_tail_headless/102400` p99 1.10 ms; live `terminal/heavy_output_keypress_to_draw_flush_live` p99 41 µs) | Replay large tool-call start/finish/output events and measure tail latency. |
| Large paste handling: `0` corruption / `0` accidental submit | `unknown` (`paste/large_multiline_component/102400` is latency only) | Paste burst replay with multiline and slash-prefixed content. |
| Huge transcript viewport changes avoid full-buffer render behavior | `unknown` (`viewport/huge_transcript_scroll_headless/10000` and `terminal/huge_transcript_scroll_to_draw_flush_live` time the scroll; they do not assert bounded dirty-region work) | Large transcript fixture plus scroll/resize replay; verify bounded visible-work behavior. |
| Beats Pi on at least one important tail-latency workload | `unknown` (Pi adapter removed 2026-07-16; no same-machine comparison in this baseline) | Same-machine comparison against current Pi for long transcript, streaming, heavy tool output, paste, or session-tree navigation. |

## Benchmark suite buildout placeholder

### Phase A — Measurement scaffolding

Goal: make latency observable without guessing.

- Add a record/replay-friendly TUI benchmark harness that can run without a real terminal when possible.
- Add stable workload fixtures for transcript entries, prompt input, model/session events, dialogs, and tool output.
- Capture p50/p95/p99, max, sample count, build profile, machine, and command.
- Keep benchmark reports append-only under this directory and index summarized evidence in `../project/`.

### Phase B — Core dogfood latency workloads

Goal: validate the M2 dogfood loop under realistic pressure.

- Startup-to-interactive after backend-ready.
- Idle keypress-to-paint.
- Active-stream keypress-to-paint.
- Heavy tool-output tail latency.
- Large paste correctness and responsiveness.
- Long transcript scroll/resize behavior.

### Phase C — Pi comparison workloads

Goal: prove yach has a measured advantage somewhere that matters.

- Run yach and current Pi on the same machine.
- Use equivalent fixtures or recorded sessions where possible.
- Compare at least one important tail-latency workload before using performance as a product claim.
- Record limitations when workloads are not perfectly equivalent.

## Pi comparison methodology

Use `pi-comparison-methodology.md` before adding or interpreting same-machine Pi comparisons. The short version:

- Prefer methodology that could show either yach or Pi winning.
- Disable user-configured Pi extensions/skills/templates/themes/context files for clean baselines.
- Never compare yach headless internals against Pi live terminal behavior.
- Label exact timing boundaries and excluded phases.
- Use cautious claim wording unless workload equivalence is strong.

## Report naming

Use date-prefixed Markdown reports:

- `baseline-YYYY-MM-DD.md` for broad baselines.
- `startup-YYYY-MM-DD.md` for startup/interactivity measurements.
- `keypress-YYYY-MM-DD.md` for keypress-to-paint measurements.
- `replay-YYYY-MM-DD.md` for transcript/tool/stream replay measurements.
- `pi-comparison-YYYY-MM-DD.md` for same-machine Pi comparisons.

## Minimum report contents

Each report should include:

- Date.
- Commit SHA.
- Machine/environment.
- Command or harness.
- Build/profile mode.
- Workload and fixture size.
- Results: p50/p95/p99/max where latency is involved.
- Comparison target, if any.
- Claim supported.
- Confidence/limitations.
- Follow-up.

## Harness

The measurement harness is `yach-bench perf`. Every former `*-report` command is a workload in the static registry at `crates/yach-bench/src/perf/registry.rs`. List them:

```
just dev cargo run -p yach-bench --release -- perf run --list
```

Each line is `id | class | isolation | requires`. In-process serial latency workloads also emit derived `#alloc_count` and `#alloc_bytes` rows (class `count`), marked `derived`. Filters, thresholds, `--deterministic`, and CI treat those like any other `count` workload.

`cargo bench` remains the Criterion path for exploratory microbenchmarks and is not part of the gate.

### Recipes

- `just perf [args]` — paired A/B against `main` (ABBA rounds, budgets in `crates/yach-bench/perf-thresholds.toml`). Extra args pass through to `perf ab` (`just perf --filter 'request/*'`). Exit 1 on `error`/`regressed`, 2 on `inconclusive`. On this machine the first landing exits 2: `inconclusive` on `yach/cli_startup_first_output` (and sometimes `yach/tui_startup_first_output_pty`) because first-output round spread exceeds the evidence-derived budgets while median deltas stay inside them — not a regression; see `baseline-2026-09-10.md`.
- `just perf-record` — `perf run` for `@` only, saved under `~/.cache/yach/perf/<fingerprint>/<date>-<commit>.json`. Trend evidence, never a gate input.
- `just perf-report <results.json> [<ab.json>]` — Markdown on stdout.
- `just perf-profile <id> [samples]` — flamegraph the **worker** (`perf worker --schema <SCHEMA>`), not the controller. Linux: `perf` + `inferno`; macOS: `samply`.

### JSON schema

Result documents use **schema 2**:

- `host`: `{ fingerprint, cpu, cores, os, kernel }`
- `build`: `{ source_sha256, commit, dirty, profile, rustc, cargo_lock_sha256, yach_bin_sha256 }`
- `started_at`: RFC3339 UTC
- `workloads[]`: `{ id, class, isolation, status, reason, count, p50_ns, p95_ns, p99_ns, max_ns, value }` plus optional raw samples

`#alloc_*` rows exist only in result documents (derived from in-process serial latency). A/B documents add `base_mode`, `base[]`, `current[]`, and `verdicts[]` (`VerdictRow` includes `budget`).

### Verdicts

Latency/memory: median round delta vs budget, with sign agreement ≥ 0.8. Size/count: exact compare against the numeric budget. Exit 1 if any `error` or `regressed`, else 2 if any `inconclusive`, else 0.

| verdict | meaning |
|---|---|
| `regressed` / `improved` / `unchanged` | judged against the workload budget |
| `inconclusive` | delta beyond budget with weak sign agreement, or within budget with large base spread; extra ABBA rounds ran and it still did not settle |
| `skipped` | skipped on either side |
| `error` | runtime failure on either side |
| `added` | present only on current (informational, never fails) |
| `removed` | present only on base (informational, never fails) |
| `no_base_worker` | base has no `perf worker` and the row is not one of the five shipping-binary external ids (informational) |

### Thresholds

`crates/yach-bench/perf-thresholds.toml`. Rows match by glob; most specific (longest literal prefix) wins. Only numeric fields have effect. An intentional regression lands by raising the affected workload's budget in the same change. A thresholds row whose glob matches no workload on either side is a hard error.

### External-mode caveat

`--base-mode` defaults to `auto`: `worker` when the base `yach-bench` answers `perf worker --schema-probe` with this schema, `external` otherwise. In `external` mode the controller measures only the five shipping-binary rows (`binary/size_bytes`, the three first-output startups, `memory/peak_rss/tui_ready`) through the base's ordinary `yach` binary. Every other workload is `no_base_worker`. The gate is therefore partial exactly once — on the change that lands this harness — and complete for every change after it.

## Current reports

- `baseline-2026-09-10.md` — first Linux `yach-bench perf` baseline (Ryzen 9 3900X). Absolute numbers only; live-terminal rows from a `script` PTY re-record. Not a Pi comparison.
- `current-baseline-2026-05-05.md` — current yach-only headless replay, live Crossterm draw/flush proxies, transcript scroll, and synthetic-ready PTY first-output refresh. Narrow synthetic/live-terminal evidence; not a Pi comparison or real-provider latency claim.
- `native-edit-profile-2026-05-15.md` — first local native edit preview/apply/evidence/session-append profiling baseline. Synthetic edit fixtures only; not a Pi comparison or user-facing edit latency claim.
- `baseline-2026-04-23.md` — protocol parsing/dispatch/serialization/transcript internals baseline. Useful for ruling out protocol internals as the obvious bottleneck, but not sufficient for user-perceived TUI latency claims.
- `replay-2026-04-27.md` — first headless TUI app/event/render replay baseline. Component evidence only, not user-perceived terminal latency.
- `startup-2026-04-27.md` — first headless backend-ready-to-first-interactive baseline. Component evidence only, not live startup SLO evidence.
- `terminal-2026-04-27.md` — first live Crossterm terminal draw/flush baseline for synthetic backend-ready-to-interactive. Narrow live terminal evidence; still excludes backend startup and real OS input delivery.
- `keypress-2026-04-27.md` — first live Crossterm idle keypress-to-draw/flush baseline. Narrow live terminal evidence; still excludes OS keyboard event delivery.
- `pi-comparison-2026-04-27.md` — first clean Pi PTY first-output methodology prototype. Not a product comparison claim.
