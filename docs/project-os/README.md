> **Retired workflow:** Active project planning now starts at `../project/README.md`.
> This directory is reference-only and should not be maintained in parallel with `docs/project/`.

# Yach Project OS

This directory is the retired repo-first operating system for yach planning, architecture, evidence, and agent handoff. It remains available as source material for understanding earlier project state and decisions.

## Reference use

For current task selection, start at `../project/README.md`.

Use these retired Project OS docs only when they answer a specific historical or evidence question:

1. `next-work.md` — previous committed priorities and candidate work.
2. `roadmap.md` — previous milestone context and source-linked status.
3. `agent-handoff.md` — previous handoff/update rules.

Other reference surfaces:

- `architecture-invariants.md` — protocol, adapter, UI/runtime, extensibility, and phase-gate constraints.
- `decisions.md` — product or architecture decisions and their consequences.
- `compatibility.md` — Pi parity status and evidence.
- `performance-evidence.md` — benchmark and responsiveness evidence.
- `templates/` — old project OS templates.

For current command/environment rules, read `../../AGENTS.md`.

## Source docs preserved by this OS

- Product thesis and milestone ladder: `../../PRD-v0.1.md`
- M0/M1 implemented-reality checkpoint: `../status/m0-m1-checkpoint.md`
- M2 TUI alpha design: `../plans/2026-04-21-m2-tui-alpha-design.md`
- TUI UX backlog: `../plans/2026-04-24-tui-ux-backlog.md`
- Protocol current-contract note: `../protocol/yach-proto-v0.md`
- Benchmark baseline: `../benchmarks/baseline-2026-04-23.md`
- Project OS requirements: `../brainstorms/2026-04-26-project-os-requirements.md`
- Project OS implementation plan: `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`

## Historical status vocabulary

The retired Project OS used these labels in its roadmap, queue, compatibility, and evidence docs:

- `planned` — intended future work, not started.
- `in-progress` — actively underway, not complete.
- `implemented-unverified` — code or docs exist, but acceptance/evidence is incomplete.
- `verified` — behavior/status has been checked against an explicit criterion.
- `measured` — performance or compatibility claim has repeatable evidence.
- `unknown` — not yet investigated.
- `blocked` — cannot proceed until a named blocker is removed.
- `deferred` — intentionally out of the current phase/pass.

When reading retired docs, remember that one label may have been too coarse for some surfaces. For example, compatibility rows may distinguish implementation status from evidence status.

## Historical requirement coverage

This table records what the Project OS originally set out to cover. It is historical context, not the current planning contract.

| Origin requirement | Project OS surface |
|---|---|
| R1 Repo-first docs | This directory and linked Markdown sources |
| R2 Obvious entry point | `README.md`, plus pointers from `../../AGENTS.md` and `../../README.md` |
| R3 Living roadmap | `roadmap.md` |
| R4 Architecture invariants | `architecture-invariants.md` |
| R5 Decision log | `decisions.md` |
| R6 Compatibility tracking | `compatibility.md` |
| R7 Performance evidence | `performance-evidence.md` |
| R8 Next-work checklist | `next-work.md` |
| R9 Agent handoff rules | `agent-handoff.md` |
| R10 Drift visibility | `architecture-invariants.md`, `decisions.md`, `agent-handoff.md` |
| R11 Separate plan/status/evidence | This README’s vocabulary plus dedicated tracker docs |
| R12 Skeletons first | First pass uses source-linked seeds only, not a full audit |
| R13 Concrete templates | `templates/` |
| R14 Preserve existing docs | Source links above; no wholesale replacement |

## Historical first-pass caveat

The Project OS was seeded from existing docs only. It was not a full audit of the implementation at that time. When a retired row says M0/M1 were complete, the source was `../status/m0-m1-checkpoint.md`; when a retired row says M2 was active, the source was the PRD and then-existing TUI docs, not a fresh code audit.
