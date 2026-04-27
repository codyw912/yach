# Roadmap

This roadmap operationalizes `../../PRD-v0.1.md`. It is source-linked and intentionally not a fresh implementation audit.

Last updated: 2026-04-27

## Milestone status

| Milestone | Status | Source | Notes / next action |
|---|---|---|---|
| M0 — bootstrap | `verified` | `../status/m0-m1-checkpoint.md` | Cargo workspace, protocol seed, capability model, and benchmark skeleton were marked complete in the checkpoint. |
| M1 — stock Pi RPC adapter | `verified` | `../status/m0-m1-checkpoint.md` | RPC spawn/connect, prompt streaming, prompt send, Tier A dialog/fire-and-forget surfaces, and basic model/session controls were marked complete. |
| M2 — TUI alpha | `verified` | `../status/m2-tui-checkpoint.md`, `../../PRD-v0.1.md`, `../plans/2026-04-21-m2-tui-alpha-design.md`, `../plans/2026-04-24-tui-ux-backlog.md`, `../plans/2026-04-27-001-feat-m2-basic-tui-polish-plan.md` | Core M2 dogfood loop is verified with caveats: fullscreen TUI, streaming/input/status, backend-backed model selection, readable help, slash completion, transcript scrolling, local stop-following, and Tier A dialog smoke. Broader session tree/fork UX, compatibility evidence, and latency evidence move to later work. |
| M3 — compatibility beta | `planned` | `../../PRD-v0.1.md`, `../status/compatibility-evidence-2026-04-27.md` | Evidence pass is complete; next M3 planning should choose SDK sidecar vs direct file/resource loading, then model real Pi sessions/tree/fork and canonical compatibility fixtures. |
| M4 — rich parity beta | `planned` | `../../PRD-v0.1.md` | SDK sidecar adapter, rich UI surfaces, canonical rich UI suite. |
| M5 — validation gate | `planned` | `../../PRD-v0.1.md` | Decide whether native Rust backend work is justified by feel, compatibility, architecture, and performance evidence. |

## Current focus

The current committed focus has moved past verified M2 TUI alpha polish and the first broader compatibility evidence pass. Active work is now performance evidence toward PRD SLOs via `../plans/2026-04-27-002-feat-performance-evidence-harness-plan.md`, while M3 compatibility planning should use `../status/compatibility-evidence-2026-04-27.md` to choose the session/resource implementation path.

## Roadmap update rules

Update this file when:

- A milestone changes status.
- A new checkpoint supersedes an older source.
- The owner changes the committed priority sequence.
- Evidence from `compatibility.md` or `performance-evidence.md` changes a milestone assessment.

Do not update this file for every small implementation task. Use `next-work.md` for tactical queue changes.
