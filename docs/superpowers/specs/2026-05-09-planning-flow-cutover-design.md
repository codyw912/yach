# Planning Flow Cutover Design

Date: 2026-05-09
Status: proposed

## Context

Yach has used multiple repo-first planning systems. The old project cockpit is already archived under `docs/archive/project-cockpit/` and is reference-only. The newer `docs/project-os/` surface was maintained in parallel with cockpit-style docs, which created duplication and overhead.

The project needs a lighter planning flow that is useful to both the owner and agents. It should make current status and next work easy to understand without requiring every historical plan, checkpoint, or queue entry to stay live.

## Goals

- Give humans and agents a fast path for understanding current project state.
- Make it clear what work is recommended next and why.
- Preserve important plans, decisions, checkpoints, and retrospectives as history.
- Avoid another heavy project management workflow while the preferred process is still being discovered.
- Keep enough structure that the flow can evolve into a more formal system later.

## Non-goals

- Delete or rewrite old planning history.
- Require agents to read all historical project docs before ordinary work.
- Maintain duplicate active systems.
- Encode a full issue tracker, backlog manager, or priority-table process.
- Implement a project-specific Codex skill in this cutover.

## Proposed Structure

Create a new active planning surface under `docs/project/`:

- `README.md`: entry point and rules of use.
- `state.md`: current thesis, project status, milestone posture, risks, and plan-sufficiency notes.
- `next.md`: recommended next work, near-term options, readiness, and rationale.
- `records/`: dated plans, decisions, checkpoints, and retrospectives.

The live docs summarize the current truth. Record docs preserve how the project got there.

## Active vs Reference Docs

After the cutover:

- `docs/project/` is the active planning fast path.
- `docs/project-os/` is retired as an active workflow and becomes reference-only.
- `docs/archive/project-cockpit/` remains reference-only.
- Existing detailed docs under `docs/plans/`, `docs/status/`, `docs/benchmarks/`, `docs/protocol/`, and `docs/spikes/` remain valid source material, but they are not part of the everyday fast path.
- Stock Superpowers artifacts under `docs/superpowers/specs/` and `docs/superpowers/plans/` remain task-level workflow outputs. `docs/project/` should link the few currently relevant Superpowers specs or plans instead of replacing those paths.

The cutover should not delete `docs/project-os/`. Leaving it in place makes the transition reversible and keeps source context available while the new flow is evaluated.

## Relationship to Superpowers

Superpowers manages how work is designed and executed. The new `docs/project/` flow manages what the project currently believes and what should be picked up next.

Use the stock Superpowers paths for per-task design and execution artifacts:

- `docs/superpowers/specs/`
- `docs/superpowers/plans/`

Use `docs/project/state.md` and `docs/project/next.md` to summarize only the currently important implications of those artifacts. Do not duplicate full Superpowers specs or plans into live project docs.

## Operating Rules

For nontrivial work, agents should read:

1. `docs/project/README.md`
2. `docs/project/state.md`
3. `docs/project/next.md`

Live docs should stay short and current. They should answer:

- Where is the project now?
- Is the current plan still coherent?
- What should happen next?
- What risks or uncertainties matter before choosing work?

Historical detail belongs in dated records, not in the live docs.

After work, update live docs only when the work changes current status, direction, next work, risk, or plan sufficiency. If the change would not affect what a future human or agent needs to know before choosing the next task, no planning-doc update is required.

## AGENTS.md Note

Add a short note to `AGENTS.md` during implementation:

```md
## Project Planning

- Active project planning starts at `docs/project/README.md`.
- For nontrivial work, read `docs/project/state.md` and `docs/project/next.md` before choosing the next task.
- `docs/project-os/` and `docs/archive/project-cockpit/` are reference-only, not active workflow instructions.
```

Keep this note short so the agent context remains lightweight. More specific flows can become skills later if they prove useful.

## Root README Note

Update the root `README.md` planning pointer so it sends readers to `docs/project/README.md` instead of the retired `docs/project-os/README.md`. Keep the wording short and avoid duplicating the project planning rules there.

## Seed Content

The first implementation should seed `state.md` and `next.md` from active project context, not from a full historical audit.

Recommended source order:

1. `PRD-v0.1.md`
2. `README.md`
3. `docs/project-os/roadmap.md`
4. `docs/project-os/next-work.md`
5. `docs/project-os/architecture-invariants.md`
6. `docs/project-os/decisions.md`
7. recent merged branch context from PR #14

Archived cockpit material should only be consulted if active docs contain a gap that cannot be resolved otherwise.

## Initial Record

Create one dated record for the cutover decision:

`docs/project/records/2026-05-09-planning-flow-cutover.md`

It should explain:

- why cockpit and project OS are being retired as active workflows;
- what the new active docs are;
- what remains reference-only;
- what would justify expanding the system later.

## Success Criteria

The cutover is successful when:

- `docs/project/README.md`, `state.md`, `next.md`, and `records/` exist.
- `AGENTS.md` points to `docs/project/README.md` and marks `docs/project-os/` plus cockpit archive as reference-only.
- The root `README.md` points planning readers to `docs/project/README.md`.
- `state.md` gives a concise current project snapshot.
- `next.md` identifies recommended next work and near-term alternatives.
- A dated cutover record exists.
- Ordinary work can start by reading the three active docs without reading `docs/project-os/` or archived cockpit docs.

## Open Questions

- Whether `docs/project-os/` should eventually move under `docs/archive/`.
- Whether recurring planning patterns should become Codex skills after the new flow has been used for a while.
- Whether decisions should stay as dated records or later get a dedicated active decision index.
