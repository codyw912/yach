# Decision Log

Use this log for product and architecture decisions that should outlive a chat session or commit message. Keep entries concise. If a decision becomes large or controversial, add a dedicated ADR-style doc later and link it here.

## Statuses

- `proposed` — suggested but not accepted.
- `accepted` — current decision.
- `superseded` — replaced by a later decision.
- `revisit` — accepted for now, but known to need re-evaluation.

## Decision template

```md
### DYYYYMMDD-NN — <Decision title>

- **Status:** proposed | accepted | superseded | revisit
- **Date:** YYYY-MM-DD
- **Context:** <What forced the choice?>
- **Decision:** <What are we doing?>
- **Rationale:** <Why this option?>
- **Consequences:** <What becomes easier/harder?>
- **Related docs:** <Links>
- **Follow-up:** <Optional>
```

## Current decisions

### D20260426-01 — Keep project OS repo-first

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** Work selection was too dependent on the active agent, and the project owner wanted durable planning context before more implementation accumulated.
- **Decision:** Canonical planning context starts as Markdown in the repo under `docs/project-os/`.
- **Rationale:** Repo docs are available to agents, humans, worktrees, and future contributors without depending on external tracker state.
- **Consequences:** GitHub issues/projects can still be derived later, but they are not the first source of truth.
- **Related docs:** `../brainstorms/2026-04-26-project-os-requirements.md`, `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`

### D20260426-02 — Start with a single decision log, not ADR files

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** Yach needs a durable decision surface, but the first pass should avoid process overhead.
- **Decision:** Use this single decision log for now; introduce separate ADR files only when a decision needs more depth.
- **Rationale:** One log is easier for agents to keep current and discover.
- **Consequences:** Older decisions embedded in PRD/plans can be linked first and extracted later only when needed.
- **Related docs:** `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`

### D20260426-03 — Seed project OS from existing docs only

- **Status:** accepted
- **Date:** 2026-04-26
- **Context:** The project OS should be useful immediately, but the origin requirements explicitly avoid a full audit in the first pass.
- **Decision:** Seed roadmap, next work, compatibility, and performance surfaces from existing source docs with freshness/provenance labels.
- **Rationale:** This creates useful structure without laundering unverified claims into canonical status.
- **Consequences:** The next useful project task is a current M2/TUI checkpoint that can update these docs with fresh evidence.
- **Related docs:** `../status/m0-m1-checkpoint.md`, `roadmap.md`, `next-work.md`

## Linked prior decisions not yet extracted

These decisions are important but remain in their source docs until extraction is useful:

- Stock Pi RPC first, SDK sidecar later: `../../PRD-v0.1.md`
- UI talks through `yach-proto`, never Pi RPC directly: `../../PRD-v0.1.md`, `../protocol/yach-proto-v0.md`
- Tokio from the start for TUI alpha: `../plans/2026-04-21-m2-tui-alpha-design.md`
- MCP is a separate lane, not the only extension model: `../../PRD-v0.1.md`
