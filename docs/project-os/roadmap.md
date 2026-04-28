# Roadmap

This roadmap operationalizes `../../PRD-v0.1.md`. It is source-linked and intentionally not a fresh implementation audit.

Last updated: 2026-04-27

## Milestone status

| Milestone | Status | Source | Notes / next action |
|---|---|---|---|
| M0 — bootstrap | `verified` | `../status/m0-m1-checkpoint.md` | Cargo workspace, protocol seed, capability model, and benchmark skeleton were marked complete in the checkpoint. |
| M1 — stock Pi RPC adapter | `verified` | `../status/m0-m1-checkpoint.md` | RPC spawn/connect, prompt streaming, prompt send, Tier A dialog/fire-and-forget surfaces, and basic model/session controls were marked complete. |
| M2 — TUI alpha | `verified` | `../status/m2-tui-checkpoint.md`, `../../PRD-v0.1.md`, `../plans/2026-04-21-m2-tui-alpha-design.md`, `../plans/2026-04-24-tui-ux-backlog.md`, `../plans/2026-04-27-001-feat-m2-basic-tui-polish-plan.md` | Core M2 dogfood loop is verified with caveats: fullscreen TUI, streaming/input/status, backend-backed model selection, readable help, slash completion, transcript scrolling, local stop-following, and Tier A dialog smoke. Broader session tree/fork UX, compatibility evidence, and latency evidence move to later work. |
| M3 — compatibility beta | `in-progress` | `../../PRD-v0.1.md`, `../status/compatibility-evidence-2026-04-27.md`, `compatibility.md`, PR #8, `../plans/2026-04-27-004-feat-native-backend-path-plan.md` | Evidence pass is complete and session/fork groundwork has landed: entry-id fork, fork-message parsing, recent session discovery/labels, and live fork/resume smokes. Owner direction now treats Pi RPC as compatibility/reference rather than exhaustive parity target; next implementation should begin native backend seams and provider-evaluation spike. |
| M4 — rich parity beta | `planned` | `../../PRD-v0.1.md` | SDK sidecar adapter, rich UI surfaces, canonical rich UI suite. |
| M5 — validation gate | `planned` | `../../PRD-v0.1.md` | Decide whether native Rust backend work is justified by feel, compatibility, architecture, and performance evidence. |

## Current focus

The current committed focus has moved past verified M2 TUI alpha polish, the first broader compatibility evidence pass, and the first M3 session/fork groundwork PR. Active performance evidence toward PRD SLOs remains useful via `../plans/2026-04-27-002-feat-performance-evidence-harness-plan.md`, but `../plans/2026-04-27-004-feat-native-backend-path-plan.md` now frames the durable product path: keep Pi RPC as reference/migration support, define native backend/session/provider seams, evaluate Rig or alternatives below a yach-owned provider abstraction, then build a minimal native dogfood runner.

## Roadmap update rules

Update this file when:

- A milestone changes status.
- A new checkpoint supersedes an older source.
- The owner changes the committed priority sequence.
- Evidence from `compatibility.md` or `performance-evidence.md` changes a milestone assessment.

Do not update this file for every small implementation task. Use `next-work.md` for tactical queue changes.
