# Planning Flow Cutover

Date: 2026-05-09
Status: accepted

## Decision

Retire both the old project cockpit and `docs/project-os/` as active workflow systems. Use `docs/project/` as the active planning fast path.

## Rationale

The project owner needs a planning surface that is useful for both human review and agent pickup. Prior systems preserved useful context, but maintaining parallel cockpit and Project OS surfaces created waste.

The new flow keeps only a few live docs:

- `docs/project/README.md`
- `docs/project/state.md`
- `docs/project/next.md`

History moves into dated records or remains in existing reference docs.

## Relationship to Superpowers

Stock Superpowers remains the task-level workflow for brainstorming, specs, implementation plans, execution, and verification.

Use:

- `docs/project/specs/`
- `docs/project/plans/`

Do not move Superpowers artifacts into `docs/project/records/` by default. Link relevant specs and plans from `state.md` or `next.md`.

## Reference-Only Material

These docs remain available but are not active workflow instructions:

- `docs/project-os/`
- `docs/archive/project-cockpit/`

They should be consulted only when active docs do not answer a specific question.

## Expansion Criteria

Expand the system only when repeated real work proves a missing surface is needed.

Examples:

- a decision index if dated records become hard to scan;
- a dedicated evidence index if benchmark or compatibility claims become difficult to trace;
- a project-specific skill if a repeated planning flow becomes stable enough to encode.
