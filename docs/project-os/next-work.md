# Next Work

This is the tactical queue for yach. It should be short, current, and source-linked.

Last updated: 2026-04-27

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
| P2 | Plan M2 alpha hardening pass | `verified` | `../plans/2026-04-26-002-feat-m2-tui-hardening-plan.md`, `../status/m2-tui-checkpoint.md` | The checkpoint marks M2 `implemented-unverified / partial` and identifies hardening needed before a verified alpha. | A focused plan exists for terminal cleanup, stream semantics, control gating/correlation, dialog Unicode safety, slash consistency, transcript scrolling, and backend noise handling. | Completed 2026-04-26. |
| P3 | Implement M2 alpha hardening pass | `verified` | `../plans/2026-04-26-002-feat-m2-tui-hardening-plan.md`, `../status/m2-tui-checkpoint.md` | Implementation followed the focused hardening plan and follow-up smoke fixes: terminal guard, local cancel semantics, busy gating, dialog Unicode/queueing, slash exact parsing, transcript scroll, unknown event tolerance, Alt+M model access, blank prompt suppression, and stock RPC `clone` serialization for current-branch duplication. | Highest-priority M2 hardening items are implemented and verified. | Completed and merged 2026-04-27; session-tree/fork UX remains intentionally deferred. |
| P4 | Polish M2 basic TUI dogfood loop | `verified` | `../plans/2026-04-27-001-feat-m2-basic-tui-polish-plan.md`, `../status/m2-tui-checkpoint.md` | After hardening, the next blocker to normal dogfooding was basic-loop polish rather than deeper session features. | Backend-provided model selector path exists, initial TUI startup refreshes backend state so the status bar can show the actual model name, long model lists keep the highlight visible, selectors support j/k as well as arrow-key movement, slash-command completion is visible while typing `/`, `/help` is readable outside the status bar with `q` close support, and dialog modes have an explicit manual smoke harness/evidence path. | Completed 2026-04-27 with automated validation and manual TUI/dialog smoke. Session-tree/fork UX and performance evidence remain later work. |
| P5 | Expand compatibility tracker with first real evidence pass | `planned` | `../../PRD-v0.1.md`, `compatibility.md`, `../status/m2-tui-checkpoint.md` | M3 depends on knowing which Pi parity targets are implemented, unknown, or blocked. | Tracker rows link real evidence or explicit unknowns for Tier A/session/resource surfaces. | Do after M2 basic-loop polish unless combined deliberately. |
| P6 | Expand performance evidence toward PRD SLOs | `planned` | `../../PRD-v0.1.md`, `performance-evidence.md`, `../benchmarks/README.md`, `../benchmarks/baseline-2026-04-23.md`, `../status/m2-tui-checkpoint.md` | Yach’s thesis depends on measured responsiveness, not Rust assumptions. | Benchmark scaffolding covers startup-to-interactive, keypress-to-paint, active stream, heavy tool output, large paste, huge transcript, and same-machine Pi comparison workloads; evidence tracker links measured results. | Placeholder exists in `../benchmarks/README.md`; implementation and measurements still needed. |

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
