# Performance Evidence

Yach’s performance claims require repeatable evidence. This file indexes measurements and highlights what each result supports. Detailed benchmark reports and harness notes live in `../benchmarks/`; start with `../benchmarks/README.md` for the benchmark buildout placeholder.

Last updated: 2026-04-27

## Evidence standard

Before a performance claim counts as evidence, record:

- Date
- Machine/environment
- Command or harness
- Build/profile mode
- Workload
- Result
- Comparison target, if any
- Claim supported
- Confidence/limitations
- Artifact or report link
- Follow-up

Same-machine Pi comparisons must follow `../benchmarks/pi-comparison-methodology.md`: clean Pi baselines should disable user extensions/skills/templates/themes/context files unless a report explicitly chooses a configured-Pi comparison, and comparison claims must document timing-boundary equivalence.

## Existing evidence

| Date | Workload | Result summary | Claim supported | Confidence / limitations | Link |
|---|---|---|---|---|---|
| 2026-04-23 | Protocol parsing, dispatch, serialization, transcript operations | Protocol layer operations are sub-microsecond to low-microsecond in benchmarked paths. | Protocol internals are unlikely to be the bottleneck. | Does not measure full TUI render loop, startup, keypress-to-paint, heavy tool output, large paste, or Pi comparison. | `../benchmarks/baseline-2026-04-23.md` |
| 2026-04-26 | M2 checkpoint audit of performance instrumentation | Render duration/total render tracking and `/perf` overlay exist. | M2 has basic render instrumentation, not user-perceived latency evidence. | Checkpoint only audits implementation surfaces; it does not add new benchmark measurements. | `../status/m2-tui-checkpoint.md` |
| 2026-04-27 | Headless TUI app/event/render replay: idle keypress, active stream, heavy tool output, paste, transcript viewport | Criterion: idle keypress proxy ~239 µs; 100 prompt deltas ~12.1 ms; 1,000 prompt deltas ~118.8 ms; 100 KiB paste ~3.42 ms; 10,000-entry transcript viewport ~16.6 ms. Direct release sampler: idle p95 ~236 µs, active-stream-100 p95 ~13.1 ms, 100 KiB paste p95 ~4.5 ms, 10,000-entry viewport p95 ~17.7 ms. | Repeatable headless proxy harness exists with report-friendly p50/p95/p99/max summaries; transcript viewport cost appears to scale with total transcript size. | Headless component evidence only; no live terminal, user-perceived latency, queue contention, startup, or Pi comparison evidence. | `../benchmarks/replay-2026-04-27.md` |
| 2026-04-27 | Headless backend-ready-to-first-interactive startup path | Synthetic ready state + first render + one keypress + second render measured ~243 µs. | Repeatable startup-oriented measurement path exists for component regression tracking. | Headless proxy only; excludes backend/process startup, PTY/terminal flush, terminal paint, and OS input delivery, so it does not prove the PRD startup SLO. | `../benchmarks/startup-2026-04-27.md` |
| 2026-04-27 | Live Crossterm terminal draw/flush startup path | Synthetic ready state + first live terminal draw + one keypress + second live terminal draw. 1,000-sample run: p50 1.212 ms, p95 2.115 ms, p99 2.496 ms, max 8.187 ms. | Narrow live terminal evidence for backend-ready-to-interactive path; supports that this synthetic path is below the PRD `<250 ms after backend ready` startup target on this machine. | Excludes backend/process startup, Pi RPC transport, auth/model setup, and real OS input delivery. | `../benchmarks/terminal-2026-04-27.md` |
| 2026-04-27 | Live Crossterm terminal idle, active-stream, stream-backlog proxy, and heavy-output keypress-to-draw/flush paths | Idle 1,000-sample run: p50 632.042 µs, p95 900.125 µs, p99 1.521 ms, max 2.073 ms. Synthetic active-stream run: p50 857.250 µs, p95 1.996 ms, p99 3.897 ms, max 9.462 ms. Stream-backlog proxy with 10 prompt deltas inside each timed sample: p50 4.027 ms, p95 16.798 ms, p99 19.124 ms, max 20.781 ms. Heavy-output compact-summary run: p50 630.000 µs, p95 1.049 ms, p99 1.609 ms, max 2.956 ms. | Narrow live terminal evidence for keypress-handler-to-terminal-draw/flush; supports that synthetic idle, active-stream, stream-backlog proxy, and heavy-output compact-summary paths are below current PRD targets on this machine. Stream-backlog proxy is the current warning signal. | Excludes OS keyboard event delivery and crossterm event polling; stream-backlog proxy is not true async queue contention; heavy-output workload measures compact-summary behavior, not full-output expansion. | `../benchmarks/keypress-2026-04-27.md` |
| 2026-04-27 | Clean Pi and yach TUI PTY first-output methodology prototype | Pi 0.70.2 clean PTY first-output: p50 44.181 ms, p95 98.281 ms, p99/max 104.374 ms. Yach full TUI PTY first-output: p50 88.422 ms, p95 193.653 ms, p99/max 228.000 ms. Yach synthetic-ready TUI PTY first-output: p50 34.952 ms, p95 42.749 ms, p99/max 54.897 ms. Asymmetric yach CLI first-output: p50 36.420 ms, p95 45.210 ms, p99/max 53.118 ms. | Demonstrates clean Pi PTY, yach full TUI PTY, yach synthetic-ready TUI PTY, and yach CLI first-output samplers exist; full-vs-ready yach delta suggests backend spawn/initialize contributes materially. | Methodology prototype only; first byte is not first stable frame or prompt readiness. Pi and yach TUI startup internals differ, so no product yach-vs-Pi claim is supported. | `../benchmarks/pi-comparison-2026-04-27.md` |

## PRD SLO evidence map

| PRD target | Evidence status | Current evidence | Next measurement |
|---|---|---|---|
| Startup to interactive prompt `<250 ms after backend ready` | `live-terminal measured / partial` | Headless synthetic ready-to-interactive path ~243 µs. Live Crossterm terminal draw/flush path over 1,000 samples: p50 1.212 ms, p95 2.115 ms, p99 2.496 ms, max 8.187 ms. | Add backend/process startup split; OS input delivery and real backend/Pi startup remain unmeasured. |
| p95 keypress-to-paint idle `<16 ms` | `live-terminal measured / partial` | Headless idle keypress replay ~239 µs central estimate; direct sampler p95 ~236 µs. Live terminal keypress-handler-to-draw/flush p95 900.125 µs over 1,000 samples. | Add OS input/event-stream timing before claiming full hardware keypress-to-paint. |
| p95 keypress-to-paint active stream `<32 ms` | `live-terminal measured / partial` | Sequential headless replay: 100 prompt deltas ~12.1 ms central / p95 ~13.1 ms; 1,000 prompt deltas ~118.8 ms central. Live synthetic active-stream keypress-handler-to-draw/flush p95 1.996 ms. Stream-backlog proxy with 10 deltas inside each timed sample p95 16.798 ms. | Add true async queue/backlog active-stream workload and OS input/event-stream timing before claiming full active keypress-to-paint; stream-backlog proxy is close to the 60 Hz frame budget and should be watched. |
| p99 keypress-to-paint heavy tool output `<50 ms` | `live-terminal measured / partial` | Headless heavy tool output replay: 10 KiB ~379 µs, 100 KiB ~400 µs central / p99 ~430 µs, 1 MiB ~601 µs. Live terminal heavy-output compact-summary keypress-handler-to-draw/flush p99 1.609 ms over 1,000 samples. | Add full-output expansion benchmark when that UI exists; this is high value because large expanded tool output is known to feel slow in Pi. Add OS input/event-stream timing before claiming full keypress-to-paint. |
| Large paste handling: no corruption/accidental submit | `headless-proxy measured / live paste unknown` | 100 KiB headless prompt replacement with slash-prefixed multiline Unicode payload ~3.42 ms central / p95 ~4.5 ms; no command execution in component path. | Add live bracketed-paste/terminal evidence. |
| Huge transcript viewport changes avoid full-buffer render behavior | `headless-proxy measured / optimization likely` | Headless viewport replay: 1,000 entries ~2.15 ms, 10,000 entries ~16.6 ms central / p95 ~17.7 ms; current behavior appears to scale with transcript size. | Add larger stress tiers and live scroll/resize timing; plan optimization separately if confirmed. |
| Beats Pi on at least one important tail-latency workload | `unknown` | None yet | Define same-machine Pi comparison workload. |

## Target interpretation

PRD SLOs are product minimum bars, not the final engineering quality bar. Current startup and idle live-terminal evidence is far below the PRD thresholds, so future work should treat those PRD targets as user-facing guardrails while adding stricter internal regression guards once methodology stabilizes. Active-stream, heavy-output, huge-transcript, and Pi-comparison workloads still need stronger evidence before setting stricter guards.

The current Pi sidecar is transitional scaffolding for TUI dogfooding, not a long-term backend architecture. Full yach-with-Pi-sidecar startup measurements should stay visible, but they are not optimization targets unless they block dogfooding. Native Rust backend work is the intended path for removing that bottleneck.

## Benchmark buildout placeholder

Detailed suite scaffolding lives in `../benchmarks/README.md`. The current intended buildout is:

1. Measurement scaffolding: record/replay-friendly TUI harness, stable fixtures, p50/p95/p99/max reporting, and report indexing.
2. Core dogfood latency workloads: startup-to-interactive, idle keypress-to-paint, active-stream keypress-to-paint, heavy tool-output tail latency, large paste correctness/responsiveness, and long transcript scroll/resize behavior.
3. Pi comparison workloads: same-machine comparison against current Pi for at least one important tail-latency workload before making performance claims.

## Measurement backlog

1. Startup-to-first-frame / interactive prompt.
2. Keypress-to-paint idle and active streaming.
3. Heavy tool-output render latency.
4. Large transcript navigation.
5. Giant paste behavior.
6. Same-machine Pi comparison for the strongest workload.

Use `templates/performance-evidence-template.md` for new entries.
