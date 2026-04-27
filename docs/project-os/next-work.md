# Next Work

This is the tactical queue for yach. It should be short, current, and source-linked.

Last updated: 2026-04-26

## Priority protocol

- **Committed priority** means the owner or a source document clearly supports the item as current work.
- **Candidate** means an agent proposes it, but it should not silently displace committed work.
- Agents may add candidates with sources and rationale.
- Agents should not reorder committed priorities without an owner decision or a source document that clearly supersedes the old priority.

## Current queue

| Priority | Item | Status | Owner/source | Why next | Done when | Freshness / notes |
|---|---|---|---|---|---|---|
| P0 | Implement project OS skeleton | `verified` | `../brainstorms/2026-04-26-project-os-requirements.md`, `../plans/2026-04-26-001-feat-project-os-skeleton-plan.md` | Reduces loose agent-driven work selection before more implementation accumulates. | `docs/project-os/` exists, entry points link to it, and dry-run acceptance passes. | Completed 2026-04-26; keep here briefly as provenance for the next queue. |
| P1 | Create M2/TUI current-state checkpoint | `verified` | `../status/m2-tui-checkpoint.md`, `../status/m0-m1-checkpoint.md`, `../plans/2026-04-21-m2-tui-alpha-design.md`, `../plans/2026-04-24-tui-ux-backlog.md` | Existing checkpoint ended at M1, but M2 docs/code had progressed. | A status doc summarizes M2 completion, gaps, evidence, and next TUI work. | Completed 2026-04-26. |
| P2 | Plan M2 alpha hardening pass | `planned` | `../status/m2-tui-checkpoint.md` | The checkpoint marks M2 `implemented-unverified / partial` and identifies hardening needed before a verified alpha. | A focused plan exists for terminal cleanup, stream semantics, control gating/correlation, dialog Unicode safety, slash consistency, and minimal transcript scrolling. | Current recommended next work. |
| P3 | Implement M2 alpha hardening pass | `planned` | Future M2 hardening plan | Implementation should follow the focused hardening plan rather than expanding scope opportunistically. | Highest-priority M2 hardening items are implemented and verified. | Blocked on P2. |
| P4 | Expand compatibility tracker with first real evidence pass | `planned` | `../../PRD-v0.1.md`, `compatibility.md`, `../status/m2-tui-checkpoint.md` | M3 depends on knowing which Pi parity targets are implemented, unknown, or blocked. | Tracker rows link real evidence or explicit unknowns for Tier A/session/resource surfaces. | Do after M2 hardening plan unless combined deliberately. |
| P5 | Expand performance evidence toward PRD SLOs | `planned` | `../../PRD-v0.1.md`, `performance-evidence.md`, `../benchmarks/baseline-2026-04-23.md`, `../status/m2-tui-checkpoint.md` | Yach’s thesis depends on measured responsiveness, not Rust assumptions. | Evidence tracker includes next benchmark workloads and at least one new PRD-SLO-oriented measurement plan. | Existing baseline covers protocol internals, not full UI tail latency. |

## Candidate work

Use this section for agent-proposed tasks that are not yet committed priorities.

| Candidate | Proposed by/date | Rationale | Source | Promotion condition |
|---|---|---|---|---|
| _None yet_ |  |  |  |  |

## Claimed work

If two agents may work concurrently, add a short claim here. Remove or update it when work is done.

| Item | Claimed by/session | Date | Notes |
|---|---|---|---|
| _None_ |  |  |  |
