# Yach Project

This directory is the active planning fast path for yach. It exists to help humans and agents answer two questions quickly:

- Where is the project now?
- What should happen next, and why?

## Fast Path

For nontrivial work, read these files in order:

1. `state.md` - current project status, direction, risks, and plan sufficiency.
2. `next.md` - recommended next work and near-term alternatives.

Use the rest of the repository as source material when the task touches it, but do not treat every historical planning document as required reading.

## Active Docs

- `state.md` is the concise current-truth snapshot.
- `next.md` is the current work-selection surface.
- `records/` stores dated plans, decisions, checkpoints, and retrospectives that should remain available without bloating the live docs.

## Relationship to Superpowers

Superpowers manages how work is designed and executed. Keep stock Superpowers artifacts in:

- `docs/superpowers/specs/`
- `docs/superpowers/plans/`

The live project docs should link only the currently relevant Superpowers artifacts. Do not duplicate full specs or implementation plans into `state.md` or `next.md`.

## Reference-Only Docs

These paths are historical or source material, not active workflow instructions:

- `docs/project-os/`
- `docs/archive/project-cockpit/`
- older docs under `docs/plans/`, `docs/status/`, `docs/benchmarks/`, `docs/protocol/`, and `docs/spikes/`

Read reference docs when they answer a specific question. Do not maintain them in parallel with this directory unless a later decision revives them.

## Update Rule

After nontrivial work, update `state.md` or `next.md` only if the work changes current status, direction, recommended next work, risk, or plan sufficiency.

If a change would not affect what a future human or agent needs to know before choosing the next task, no project-planning update is required.
