# Benchmark Buildout

This directory holds yach performance reports and benchmark-harness notes. Performance is a first-class product requirement for yach, not a nice-to-have: the Rust shell only justifies itself if it proves better responsiveness or scalability on important same-machine workloads.

Canonical tracking lives in `../project-os/performance-evidence.md`. Use that file for status and evidence indexing; use this directory for detailed reports, harness notes, and benchmark artifacts.

## Performance targets from the PRD

Source: `../../PRD-v0.1.md` §10-11.

| Target | Status | Harness placeholder |
|---|---|---|
| Startup to interactive prompt `<250 ms after backend ready` | `unknown` | Measure time from backend-ready event to first usable input frame. |
| p95 keypress-to-paint, idle `<16 ms` | `unknown` | Synthetic key event replay through TUI render loop while backend is idle. |
| p95 keypress-to-paint, active stream `<32 ms` | `unknown` | Replay high-rate token stream while injecting input events. |
| p99 keypress-to-paint, heavy tool output `<50 ms` | `unknown` | Replay large tool-call start/finish/output events and measure tail latency. |
| Large paste handling: `0` corruption / `0` accidental submit | `unknown` | Paste burst replay with multiline and slash-prefixed content. |
| Huge transcript viewport changes avoid full-buffer render behavior | `unknown` | Large transcript fixture plus scroll/resize replay; verify bounded visible-work behavior. |
| Beats Pi on at least one important tail-latency workload | `unknown` | Same-machine comparison against current Pi for long transcript, streaming, heavy tool output, paste, or session-tree navigation. |

## Benchmark suite buildout placeholder

### Phase A — Measurement scaffolding

Goal: make latency observable without guessing.

- Add a record/replay-friendly TUI benchmark harness that can run without a real terminal when possible.
- Add stable workload fixtures for transcript entries, prompt input, model/session events, dialogs, and tool output.
- Capture p50/p95/p99, max, sample count, build profile, machine, and command.
- Keep benchmark reports append-only under this directory and index summarized evidence in `../project-os/performance-evidence.md`.

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

## Current reports

- `baseline-2026-04-23.md` — protocol parsing/dispatch/serialization/transcript internals baseline. Useful for ruling out protocol internals as the obvious bottleneck, but not sufficient for user-perceived TUI latency claims.
