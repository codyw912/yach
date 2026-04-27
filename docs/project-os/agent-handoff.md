# Agent Handoff Rules

These rules keep the project OS living without turning every small task into process work.

## Before work

### Fast path: ordinary task selection

Read:

1. `README.md` in this directory.
2. `next-work.md`.
3. `roadmap.md`.
4. `../../AGENTS.md` for command/environment rules.

### Deep path: boundary or evidence work

Also read the relevant surface when the task touches it:

- Architecture/protocol/adapter/runtime boundary → `architecture-invariants.md` and `decisions.md`.
- Pi parity, resources, sessions, extension behavior → `compatibility.md`.
- Benchmarks, responsiveness, latency claims → `performance-evidence.md`.
- Planning/status process changes → this file and `templates/`.

## During work

Flag these as soon as they appear:

- An invariant might change or become weaker.
- A new seam or public contract appears.
- Scope expands beyond the current plan.
- A compatibility or performance claim needs evidence.
- Current docs conflict or look stale.
- Work ends partially complete or blocked.

## After-work update gate

Answer these questions before closing nontrivial work:

1. Did roadmap status or next priority change?
2. Did a product or architecture decision get made?
3. Did compatibility evidence/status change?
4. Did performance evidence/status change?
5. Did the task touch an architecture invariant, boundary, or new seam?
6. Did any uncertainty, blocker, or partial state remain?

If all answers are no, no project OS update is required. If any answer is yes, update the relevant doc before final handoff or explicitly state why it was not updated.

## Where updates go

| Change | Update |
|---|---|
| Priority or tactical queue changed | `next-work.md` |
| Milestone status changed | `roadmap.md` and/or `../status/` checkpoint |
| Product/architecture decision made | `decisions.md` |
| Invariant changed or was challenged | `architecture-invariants.md` and `decisions.md` |
| Pi parity evidence changed | `compatibility.md` |
| Benchmark/performance evidence changed | `performance-evidence.md` |
| New reusable process pattern | `templates/` or this file |

## Final handoff checklist

A good final handoff includes:

- What changed.
- Files changed.
- Checks run, or why none were needed.
- Project OS docs updated, or why no update was required.
- Decisions made.
- Evidence added.
- Invariants touched.
- Remaining blockers or follow-up.

Use `templates/agent-handoff-template.md` when a handoff needs more structure.
