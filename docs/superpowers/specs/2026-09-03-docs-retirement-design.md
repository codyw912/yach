# Docs Retirement Design

**Outcome:** external (docs retirement, 2026-09-03)

Status: accepted 2026-09-03 (owner decision in session)
Date: 2026-09-03

## Problem

`docs/` accumulated four generations of in-repo planning surfaces
(`docs/plans/`, `docs/project-os/`, `docs/archive/project-cockpit/`,
`docs/project/{state,next,board}.md`) alongside product documentation. Fresh
sessions paid a reading tax on stale history, the root README linked to
planning files as if they were current, and the newly configured external
planning system would have been a second authority beside the in-repo files.

## Decision

`docs/` holds product documentation and dated history only. Roadmap outcomes,
milestone status, and work queues live in the maintainer's external planning
system. The repository keeps one read-only mirror of product direction.

Role split:

- External planning system: outcome authority (gates, status, dependencies).
- External execution tracker: durable cross-session task state, PR-gated
  completion.
- `docs/project/roadmap.md`: public product direction. Vision, non-goals,
  milestone titles with one-line outcomes, principles. No done-when detail,
  no status fields.

Public entry points (`AGENTS.md`, `README.md`, `docs/README.md`,
`roadmap.md`) are provider-neutral: they name no tracker, workspace, project,
or configuration path. Tracker configuration is local and gitignored.

Planning mutation fails closed: an agent whose session lacks the local
planning configuration treats `roadmap.md` as read-only and does not create
in-repo substitutes. Ordinary contribution work is unaffected.

## Scope

Delete: `docs/project/{README,state,next,board}.md`, `docs/plans/`,
`docs/project-os/`, `docs/archive/`, `docs/status/`, `docs/spikes/`,
`docs/brainstorms/`.

Keep as product docs: root `README.md` (corrected), `PRD-v0.1.md`,
`docs/protocol/`, `docs/benchmarks/`, `docs/project/roadmap.md` (shrunk),
new `docs/README.md` index.

Keep as dated history: `docs/superpowers/{specs,plans}/`,
`docs/project/records/`.

Historical specs, plans, and records that cite deleted paths keep those
citations as written; `docs/README.md` explains the retirement once. Rewriting
history to point elsewhere would falsify what those documents said.

## Non-goals

- Editing accepted specs or plans to remove historical citations.
- Moving product docs into the external planning system.
- Any code behavior change.
