# Benchmark Buildout

This directory holds yach performance reports and benchmark-harness notes. Performance is a first-class product requirement for yach, not a nice-to-have: the Rust shell only justifies itself if it proves better responsiveness or scalability on important same-machine workloads.

Active performance status and evidence indexing live in `../project/`. Historical tracking in `../project-os/performance-evidence.md` is reference-only. Use this directory for detailed reports, harness notes, and benchmark artifacts. Same-machine Pi comparisons must follow `pi-comparison-methodology.md` before any product claim is made.

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

## Current harnesses

- `crates/yach-bench/src/latency.rs` — benchmark-only latency summaries for p50/p95/p99/max reporting.
- `crates/yach-bench/src/fixtures.rs` — deterministic protocol-native workload fixtures for transcripts, prompt streams, heavy tool output, paste payloads, and backend-ready state.
- `crates/yach-bench/src/replay.rs` plus `yach_ui::BenchmarkApp` — headless app/event/render replay seam. This is component/proxy evidence only, not live terminal latency evidence.
- `crates/yach-bench/benches/tui_latency.rs` — first headless TUI workloads for idle keypress, active stream replay, heavy tool output, paste, and transcript viewport characterization.
- `crates/yach-bench/benches/startup.rs` — headless backend-ready-to-first-interactive measurement path.
- `cargo run -p yach-bench --release -- headless-report --samples N` — direct headless replay sampler that emits p50/p95/p99/max for report-friendly tail summaries.
- `cargo run -p yach-bench --release -- terminal-report --samples N` — live Crossterm terminal draw/flush sampler for the same startup-ready/key/render path. Requires a real TTY; non-interactive agent shells may return `Device not configured`.
- `cargo run -p yach-bench --release -- terminal-keypress-report --samples N` — live Crossterm terminal draw/flush sampler for repeated idle keypress-to-draw interactions after initial ready render. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-active-stream-report --samples N` — live Crossterm terminal draw/flush sampler for keypress-to-draw interactions while synthetic prompt deltas are being appended. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-stream-backlog-report --samples N` — live Crossterm sampler that includes applying a small synthetic stream-event burst before each keypress/draw sample. This is a first queue/backlog proxy, not true async contention. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-async-backlog-report --samples N` — live Crossterm sampler with an independent producer thread feeding synthetic stream events while the UI loop drains queued events before each keypress/draw sample. This is the first true async queue/backlog harness, but still synthetic and still requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-async-backlog-stress-report --samples N` — higher-rate async-backlog variant that sends 50 prompt-delta events every 100 µs and reports sent/drained counts plus max per-sample drain depth. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-heavy-output-report --samples N` — live Crossterm terminal draw/flush sampler for keypress-to-draw interactions after a 1 MiB synthetic tool result has been summarized into the transcript. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-transcript-scroll-report --samples N` — live Crossterm scroll-to-draw/flush sampler for a 10,000-entry transcript fixture. Requires a real TTY.
- `cargo run -p yach-bench --release -- terminal-transcript-scroll-stress-report --samples N` — live Crossterm scroll-to-draw/flush sampler for a 50,000-entry transcript fixture. Requires a real TTY.
- `cargo run -p yach-bench --release -- pi-transcript-fixture --entries N --output /tmp/pi-fixture.jsonl` — writes a deterministic Pi session v3 JSONL transcript fixture with assistant usage metadata for future clean Pi large-transcript comparison harnesses.
- `cargo run -p yach-bench --release -- pi-clean-startup-report --samples N` — clean Pi PTY first-output sampler with extensions/skills/templates/themes/context files disabled. Methodology prototype only; not an apples-to-apples yach comparison.
- `cargo run -p yach-bench --release -- yach-cli-startup-report --samples N` — yach CLI first-output sampler for process-startup methodology experiments. Asymmetric with Pi PTY startup unless an equivalent boundary is added.
- `cargo run -p yach-bench --release -- yach-tui-startup-report --samples N` — yach full TUI PTY first-output sampler. Approximate counterpart to Pi PTY first-output, but first byte is still not first stable prompt/readiness.
- `cargo run -p yach-bench --release -- yach-tui-ready-startup-report --samples N` — yach synthetic-ready TUI PTY first-output sampler. Splits post-ready TUI first-output from backend spawn/initialize behavior.
- `cargo run -p yach-bench --release -- native-edit-profile-report --samples N` — native edit profile sampler for preview, apply, evidence summary, session append, and end-to-end harness phases. Uses synthetic local fixtures and does not expose edit UX or provider-visible mutation.

## Current reports

- `current-baseline-2026-05-05.md` — current yach-only headless replay, live Crossterm draw/flush proxies, transcript scroll, and synthetic-ready PTY first-output refresh. Narrow synthetic/live-terminal evidence; not a Pi comparison or real-provider latency claim.
- `native-edit-profile-2026-05-15.md` — first local native edit preview/apply/evidence/session-append profiling baseline. Synthetic edit fixtures only; not a Pi comparison or user-facing edit latency claim.
- `baseline-2026-04-23.md` — protocol parsing/dispatch/serialization/transcript internals baseline. Useful for ruling out protocol internals as the obvious bottleneck, but not sufficient for user-perceived TUI latency claims.
- `replay-2026-04-27.md` — first headless TUI app/event/render replay baseline. Component evidence only, not user-perceived terminal latency.
- `startup-2026-04-27.md` — first headless backend-ready-to-first-interactive baseline. Component evidence only, not live startup SLO evidence.
- `terminal-2026-04-27.md` — first live Crossterm terminal draw/flush baseline for synthetic backend-ready-to-interactive. Narrow live terminal evidence; still excludes backend startup and real OS input delivery.
- `keypress-2026-04-27.md` — first live Crossterm idle keypress-to-draw/flush baseline. Narrow live terminal evidence; still excludes OS keyboard event delivery.
- `pi-comparison-2026-04-27.md` — first clean Pi PTY first-output methodology prototype. Not a product comparison claim.
