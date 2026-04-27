# Roadmap

This roadmap operationalizes `../../PRD-v0.1.md`. It is source-linked and intentionally not a fresh implementation audit.

Last updated: 2026-04-26

## Milestone status

| Milestone | Status | Source | Notes / next action |
|---|---|---|---|
| M0 — bootstrap | `verified` | `../status/m0-m1-checkpoint.md` | Cargo workspace, protocol seed, capability model, and benchmark skeleton were marked complete in the checkpoint. |
| M1 — stock Pi RPC adapter | `verified` | `../status/m0-m1-checkpoint.md` | RPC spawn/connect, prompt streaming, prompt send, Tier A dialog/fire-and-forget surfaces, and basic model/session controls were marked complete. |
| M2 — TUI alpha | `in-progress` | `../../PRD-v0.1.md`, `../plans/2026-04-21-m2-tui-alpha-design.md`, `../plans/2026-04-24-tui-ux-backlog.md` | Needs a current M2 status checkpoint after this project OS exists. Do not infer full completion from code presence without audit. |
| M3 — compatibility beta | `planned` | `../../PRD-v0.1.md` | Load real Pi settings/resources, open real Pi sessions, tree navigation/fork, canonical logic suite, benchmark comparison. |
| M4 — rich parity beta | `planned` | `../../PRD-v0.1.md` | SDK sidecar adapter, rich UI surfaces, canonical rich UI suite. |
| M5 — validation gate | `planned` | `../../PRD-v0.1.md` | Decide whether native Rust backend work is justified by feel, compatibility, architecture, and performance evidence. |

## Current focus

The current committed focus is to make planning and execution less loose by using this project OS. After this lands, the next useful checkpoint is M2/TUI current-state review, because existing docs stop at the M0/M1 checkpoint while TUI docs and code have progressed.

## Roadmap update rules

Update this file when:

- A milestone changes status.
- A new checkpoint supersedes an older source.
- The owner changes the committed priority sequence.
- Evidence from `compatibility.md` or `performance-evidence.md` changes a milestone assessment.

Do not update this file for every small implementation task. Use `next-work.md` for tactical queue changes.
