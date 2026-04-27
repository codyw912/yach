# Roadmap

This roadmap operationalizes `../../PRD-v0.1.md`. It is source-linked and intentionally not a fresh implementation audit.

Last updated: 2026-04-26

## Milestone status

| Milestone | Status | Source | Notes / next action |
|---|---|---|---|
| M0 — bootstrap | `verified` | `../status/m0-m1-checkpoint.md` | Cargo workspace, protocol seed, capability model, and benchmark skeleton were marked complete in the checkpoint. |
| M1 — stock Pi RPC adapter | `verified` | `../status/m0-m1-checkpoint.md` | RPC spawn/connect, prompt streaming, prompt send, Tier A dialog/fire-and-forget surfaces, and basic model/session controls were marked complete. |
| M2 — TUI alpha | `in-progress` | `../status/m2-tui-checkpoint.md`, `../../PRD-v0.1.md`, `../plans/2026-04-21-m2-tui-alpha-design.md`, `../plans/2026-04-24-tui-ux-backlog.md` | Current checkpoint marks M2 `implemented-unverified / partial`; next action is an M2 alpha hardening pass before calling M2 complete. |
| M3 — compatibility beta | `planned` | `../../PRD-v0.1.md` | Load real Pi settings/resources, open real Pi sessions, tree navigation/fork, canonical logic suite, benchmark comparison. |
| M4 — rich parity beta | `planned` | `../../PRD-v0.1.md` | SDK sidecar adapter, rich UI surfaces, canonical rich UI suite. |
| M5 — validation gate | `planned` | `../../PRD-v0.1.md` | Decide whether native Rust backend work is justified by feel, compatibility, architecture, and performance evidence. |

## Current focus

The current committed focus is M2 alpha hardening. The project OS skeleton exists, and `../status/m2-tui-checkpoint.md` identifies the highest-priority gaps before M2 can be called a verified alpha.

## Roadmap update rules

Update this file when:

- A milestone changes status.
- A new checkpoint supersedes an older source.
- The owner changes the committed priority sequence.
- Evidence from `compatibility.md` or `performance-evidence.md` changes a milestone assessment.

Do not update this file for every small implementation task. Use `next-work.md` for tactical queue changes.
