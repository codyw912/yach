# Performance Evidence

Yach’s performance claims require repeatable evidence. This file indexes measurements and highlights what each result supports. Detailed benchmark reports and harness notes live in `../benchmarks/`; start with `../benchmarks/README.md` for the benchmark buildout placeholder.

Last updated: 2026-05-05

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
| 2026-05-12 | Native-default TUI startup profile with internal phase marks | Observed process/PTY-to-first-render-trace p95 27.043 ms, p99 30.595 ms, with one 373.077 ms max outlier. Traced Rust `main` to first render p95 535 us, p99 665 us. Legacy first-output proxy p95 18.339 ms after rebuilding release `yach-cli`. | Native TUI startup after Rust `main` is already sub-millisecond at p95 in this traced run; further startup work should focus on process/binary startup, dependency loading, PTY/terminal measurement overhead, and keeping extensions off the first-frame path. | Yach-only native-default evidence; buffered trace markers still add lightweight in-process overhead; PTY harness observes trace flush rather than exact frame time; not hardware keypress-to-focused-input; requires release `yach-cli` to be rebuilt before `yach-bench` spawns it. | `../benchmarks/native-startup-profile-2026-05-12.md` |
| 2026-05-05 | Current yach-only headless replay, live Crossterm draw/flush proxies, transcript scroll, and PTY synthetic-ready first-output | Headless p95: startup 238.834 µs, idle keypress 238.916 µs, active-stream-100 13.888 ms, 100 KiB paste 3.967 ms, 10k transcript 3.056 ms. Live p95: startup draw/flush 397.792 µs, idle 182.084 µs, active stream 137.250 µs, async backlog stress 298.875 µs, heavy output 156.250 µs, 10k transcript scroll 149.500 µs, 50k transcript scroll 636.166 µs. PTY synthetic-ready first-output p95 55.868 ms. | Current yach-only synthetic/headless/live-terminal paths remain below PRD latency guardrails on this machine; provider-delay test hook was unset and is not used by these yach-bench harnesses. | No same-machine Pi comparison; no real provider/network latency; live terminal commands are draw/flush proxies and exclude OS keyboard event delivery. | `../benchmarks/current-baseline-2026-05-05.md` |
| 2026-04-23 | Protocol parsing, dispatch, serialization, transcript operations | Protocol layer operations are sub-microsecond to low-microsecond in benchmarked paths. | Protocol internals are unlikely to be the bottleneck. | Does not measure full TUI render loop, startup, keypress-to-paint, heavy tool output, large paste, or Pi comparison. | `../benchmarks/baseline-2026-04-23.md` |
| 2026-04-26 | M2 checkpoint audit of performance instrumentation | Render duration/total render tracking and `/perf` overlay exist. | M2 has basic render instrumentation, not user-perceived latency evidence. | Checkpoint only audits implementation surfaces; it does not add new benchmark measurements. | `../status/m2-tui-checkpoint.md` |
| 2026-04-27 | Headless TUI app/event/render replay: idle keypress, active stream, heavy tool output, paste, transcript viewport | Criterion: idle keypress proxy ~239 µs; 100 prompt deltas ~12.1 ms; 1,000 prompt deltas ~118.8 ms; 100 KiB paste ~3.42 ms; 10,000-entry transcript viewport ~16.6 ms. Direct release sampler: idle p95 ~236 µs, active-stream-100 p95 ~13.1 ms, 100 KiB paste p95 ~4.5 ms, 10,000-entry viewport p95 ~17.7 ms. | Repeatable headless proxy harness exists with report-friendly p50/p95/p99/max summaries; transcript viewport cost appears to scale with total transcript size. | Headless component evidence only; no live terminal, user-perceived latency, queue contention, startup, or Pi comparison evidence. | `../benchmarks/replay-2026-04-27.md` |
| 2026-04-27 | Headless backend-ready-to-first-interactive startup path | Synthetic ready state + first render + one keypress + second render measured ~243 µs. | Repeatable startup-oriented measurement path exists for component regression tracking. | Headless proxy only; excludes backend/process startup, PTY/terminal flush, terminal paint, and OS input delivery, so it does not prove the PRD startup SLO. | `../benchmarks/startup-2026-04-27.md` |
| 2026-04-27 | Live Crossterm terminal draw/flush startup path | Synthetic ready state + first live terminal draw + one keypress + second live terminal draw. 1,000-sample run: p50 1.212 ms, p95 2.115 ms, p99 2.496 ms, max 8.187 ms. | Narrow live terminal evidence for backend-ready-to-interactive path; supports that this synthetic path is below the PRD `<250 ms after backend ready` startup target on this machine. | Excludes backend/process startup, Pi RPC transport, auth/model setup, and real OS input delivery. | `../benchmarks/terminal-2026-04-27.md` |
| 2026-04-27 | Live Crossterm terminal idle, active-stream, stream-backlog proxy, async-backlog, and heavy-output keypress-to-draw/flush paths | Idle 1,000-sample run: p50 632.042 µs, p95 900.125 µs, p99 1.521 ms, max 2.073 ms. Synthetic active-stream run: p50 857.250 µs, p95 1.996 ms, p99 3.897 ms, max 9.462 ms. Stream-backlog proxy with 10 prompt deltas inside each timed sample: p50 4.027 ms, p95 16.798 ms, p99 19.124 ms, max 20.781 ms. Async backlog with independent producer thread baseline: p50 135.375 µs, p95 234.125 µs, p99 356.209 µs, max 1.885 ms; 10,000/10,000 events drained, max 30 drained in one sample. Higher-rate stress variant: p50 164.625 µs, p95 456.292 µs, p99 1.014 ms, max 2.599 ms; 50,000/50,000 events drained, max 450 drained in one sample. Heavy-output compact-summary run: p50 630.000 µs, p95 1.049 ms, p99 1.609 ms, max 2.956 ms. | Narrow live terminal evidence for keypress-handler-to-terminal-draw/flush; supports that synthetic idle, active-stream, stream-backlog proxy, async-backlog, and heavy-output compact-summary paths are below current PRD targets on this machine. Stream-backlog proxy remains the current warning signal. | Excludes OS keyboard event delivery and crossterm event polling; async-backlog uses synthetic arrival timing that still needs calibration against real streaming sessions; heavy-output workload measures compact-summary behavior, not full-output expansion. | `../benchmarks/keypress-2026-04-27.md` |
| 2026-04-27 | Clean Pi and yach TUI PTY first-output methodology prototype | Pi 0.70.2 clean PTY first-output: p50 44.181 ms, p95 98.281 ms, p99/max 104.374 ms. Yach full TUI PTY first-output: p50 88.422 ms, p95 193.653 ms, p99/max 228.000 ms. Yach synthetic-ready TUI PTY first-output: p50 34.952 ms, p95 42.749 ms, p99/max 54.897 ms. Asymmetric yach CLI first-output: p50 36.420 ms, p95 45.210 ms, p99/max 53.118 ms. | Demonstrates clean Pi PTY, yach full TUI PTY, yach synthetic-ready TUI PTY, and yach CLI first-output samplers exist; full-vs-ready yach delta suggests backend spawn/initialize contributes materially. | Methodology prototype only; first byte is not first stable frame or prompt readiness. Pi and yach TUI startup internals differ, so no product yach-vs-Pi claim is supported. | `../benchmarks/pi-comparison-2026-04-27.md` |
| 2026-04-27 | Live Crossterm 10,000-entry and 50,000-entry transcript scroll-to-draw/flush paths | Baseline 10,000-entry p95 4.435 ms; baseline 50,000-entry p95 22.962 ms. After transcript render caching: 10,000-entry p95 152.041 µs; 50,000-entry p95 151.667 µs. | Narrow yach-only live-terminal evidence that ordinary transcript scroll renders no longer scale with total transcript size after the cache is warm; 50,000-entry scroll is now below a 16 ms frame budget on this machine. | Yach-only; synthetic fixture; excludes OS input/event polling; first render and renders after content/width changes still rebuild cache; does not prove Pi advantage. | `../benchmarks/transcript-scroll-2026-04-27.md`, `../benchmarks/transcript-scroll-optimization-2026-04-27.md` |
| 2026-04-27 | Clean Pi synthetic large-transcript fixture and scroll timing methodology prototype | Synthetic Pi session fixtures can render in clean Pi when assistant messages include usage metadata; `yach-bench pi-transcript-fixture` now generates deterministic Pi session v3 fixtures. PageUp/PageDown PTY timing produced no valid samples: raw PTY matching was unreliable, and `PI_TUI_DEBUG=1` did not emit a new render log after PageUp in the tested main transcript view. | Establishes a viable fixture path and documents the current blocker for stronger-equivalence Pi large-transcript scroll comparison. | Methodology/no-data only; no yach-vs-Pi latency claim. | `../benchmarks/pi-transcript-scroll-prototype-2026-04-27.md` |

## PRD SLO evidence map

| PRD target | Evidence status | Current evidence | Next measurement |
|---|---|---|---|
| Startup to interactive prompt `<250 ms after backend ready` | `live-terminal measured / partial` | 2026-05-05 refresh: headless synthetic ready-to-interactive p95 238.834 µs; live Crossterm terminal draw/flush p95 397.792 µs; synthetic-ready TUI PTY first-output p95 55.868 ms. | Add backend/process startup split; OS input delivery and real backend/Pi startup remain unmeasured. |
| p95 keypress-to-paint idle `<16 ms` | `live-terminal measured / partial` | 2026-05-05 refresh: headless idle p95 238.916 µs; live terminal keypress-handler-to-draw/flush p95 182.084 µs. | Add OS input/event-stream timing before claiming full hardware keypress-to-paint. |
| p95 keypress-to-paint active stream `<32 ms` | `live-terminal measured / partial` | 2026-05-05 refresh: headless active-stream-100 p95 13.888 ms; live active-stream draw/flush p95 137.250 µs; live async backlog stress p95 298.875 µs with 25,000/25,000 events drained. | Calibrate async arrival rates against real streaming sessions, plus add OS input/event-stream timing before claiming full active keypress-to-paint. |
| p99 keypress-to-paint heavy tool output `<50 ms` | `live-terminal measured / partial` | 2026-05-05 refresh: headless 100 KiB heavy output p99 957.126 µs; live heavy-output compact-summary draw/flush p99 299.875 µs. | Add full-output expansion benchmark when that UI exists; this is high value because large expanded tool output is known to feel slow in Pi. Add OS input/event-stream timing before claiming full keypress-to-paint. |
| Large paste handling: no corruption/accidental submit | `headless-proxy measured / live paste unknown` | 2026-05-05 refresh: 100 KiB headless prompt replacement p95 3.967 ms, p99 5.301 ms; no command execution in component path. | Add live bracketed-paste/terminal evidence. |
| Huge transcript viewport changes avoid full-buffer render behavior | `live-terminal measured / optimized partial` | 2026-05-05 refresh: headless 10,000-entry transcript scroll p95 3.056 ms; live 10,000-entry scroll p95 149.500 µs; live 50,000-entry scroll p95 636.166 µs. Pi synthetic fixture loading is viable, but Pi scroll latency timing is unsupported until redraw detection is robust. | Pick a comparable Pi-managed timing surface or add deeper Pi-side instrumentation before same-machine large-transcript comparison; measure active streaming with frequent cache invalidation if real dogfood sessions show stream-render pressure. |
| Beats Pi on at least one important tail-latency workload | `methodology prototype / no-data` | Clean Pi startup first-output prototype exists; Pi synthetic large-transcript fixture loading is viable, but main-transcript PageUp does not appear to be a comparable app-level redraw signal. | Pick a different comparable tail-latency workload or add deeper Pi instrumentation for transcript navigation. |

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
