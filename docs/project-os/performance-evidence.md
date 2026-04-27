# Performance Evidence

Yach’s performance claims require repeatable evidence. This file indexes measurements and highlights what each result supports. Detailed benchmark reports may live in `../benchmarks/`.

Last updated: 2026-04-26

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

## Existing evidence

| Date | Workload | Result summary | Claim supported | Confidence / limitations | Link |
|---|---|---|---|---|---|
| 2026-04-23 | Protocol parsing, dispatch, serialization, transcript operations | Protocol layer operations are sub-microsecond to low-microsecond in benchmarked paths. | Protocol internals are unlikely to be the bottleneck. | Does not measure full TUI render loop, startup, keypress-to-paint, heavy tool output, large paste, or Pi comparison. | `../benchmarks/baseline-2026-04-23.md` |

## PRD SLO evidence map

| PRD target | Evidence status | Current evidence | Next measurement |
|---|---|---|---|
| Startup to interactive prompt `<250 ms after backend ready` | `unknown` | None yet | Add startup measurement harness. |
| p95 keypress-to-paint idle `<16 ms` | `unknown` | None yet | Add render/input latency instrumentation. |
| p95 keypress-to-paint active stream `<32 ms` | `unknown` | None yet | Replay active stream workload through TUI. |
| p99 keypress-to-paint heavy tool output `<50 ms` | `unknown` | None yet | Create heavy tool-output replay. |
| Large paste handling: no corruption/accidental submit | `unknown` | None yet | Add paste characterization workload. |
| Huge transcript viewport changes avoid full-buffer render behavior | `unknown` | Transcript internals benchmarked, not viewport behavior | Add large transcript TUI/render benchmark. |
| Beats Pi on at least one important tail-latency workload | `unknown` | None yet | Define same-machine Pi comparison workload. |

## Measurement backlog

1. Startup-to-first-frame / interactive prompt.
2. Keypress-to-paint idle and active streaming.
3. Heavy tool-output render latency.
4. Large transcript navigation.
5. Giant paste behavior.
6. Same-machine Pi comparison for the strongest workload.

Use `templates/performance-evidence-template.md` for new entries.
