# Planning Flow Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the active Project OS workflow with a lighter `docs/project/` planning flow while preserving Project OS and cockpit docs as reference-only history.

**Architecture:** `docs/project/` becomes the active project-orientation layer with three live docs plus records. Stock Superpowers specs/plans remain task-level workflow artifacts under `docs/project/`; the live project docs link only the currently relevant artifacts.

**Tech Stack:** Markdown docs, existing repo docs, `AGENTS.md`, git.

---

### Task 1: Create Project Planning Entry Point

**Files:**
- Create: `docs/project/README.md`

- [ ] **Step 1: Create the active planning directory**

Run:

```bash
mkdir -p docs/project/records
```

Expected: command exits 0 and creates `docs/project/` plus `docs/project/records/`.

- [ ] **Step 2: Add `docs/project/README.md`**

Create `docs/project/README.md` with:

```md
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

- `docs/project/specs/`
- `docs/project/plans/`

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
```

- [ ] **Step 3: Verify the file exists**

Run:

```bash
test -f docs/project/README.md
```

Expected: command exits 0.

- [ ] **Step 4: Commit**

Run:

```bash
git add docs/project/README.md
git commit -m "docs: add active project planning entry point"
```

Expected: commit succeeds.

### Task 2: Seed Current Project State

**Files:**
- Create: `docs/project/state.md`

- [ ] **Step 1: Add `docs/project/state.md`**

Create `docs/project/state.md` with:

```md
# Project State

Last updated: 2026-05-09

## Thesis

Yach is a Rust-native coding harness. The validated near-term shell is Pi-shaped, but the durable product direction is a yach-owned Rust UI, protocol, backend runtime, session model, tool loop, and file-first resource system.

Pi remains useful as a compatibility/reference backend. It is not the long-term architecture target.

## Current Posture

- `main` includes PR #14: native-backend branch wrap-up and retirement of cockpit-style workflow artifacts.
- M0/M1/M2 foundations are considered verified enough for forward planning: workspace, protocol seed, Pi RPC adapter, TUI alpha loop, session/fork groundwork, and performance harness exist.
- Native backend work is in progress behind explicit opt-in boundaries. Pi remains the default backend.
- The current planning cutover is replacing both cockpit and Project OS as active workflows with this lighter `docs/project/` surface.

## Architecture Beliefs

- `yach-proto` is the UI/backend seam.
- The TUI should not speak Pi RPC, provider SDK, or native backend internals directly.
- Yach owns sessions, tools, resources, protocol events, and user-facing runtime semantics.
- Provider libraries can sit below yach-owned seams, but they do not own sessions, tool execution, or canonical transcript state.
- File-first configuration and inspectable local state remain product values.
- Compatibility and performance claims need evidence, not assumptions.

## Current Risks

- Native-provider dogfood can grow into a chat-only path unless tools, resources, persistence, cancellation, and error semantics stay yach-owned.
- Local project data exposure needs deny-by-default policy until provider-visible resource rules are explicit.
- Planning docs can become stale if live summaries accumulate history instead of pointing to records.
- Same-machine Pi comparison evidence is still imperfect, so performance claims should stay scoped to measured surfaces.

## Plan Sufficiency

The current planning-flow cutover plan is sufficient for the next step: establish `docs/project/` as the active planning fast path, then resume native-backend hardening from a clearer project state.

The plan is not sufficient for broad native-backend expansion into file reads, file writes, process execution, network tools, or default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.

## Currently Relevant Records

- `docs/project/specs/2026-05-09-planning-flow-cutover-design.md`
- `docs/project/plans/2026-05-09-planning-flow-cutover.md`
```

- [ ] **Step 2: Check for unfinished draft markers**

Run:

```bash
rg -n "T[B]D|T[O]DO|implement [l]ater|fill [i]n|\\?\\?" docs/project/state.md
```

Expected: no matches; command exits 1.

- [ ] **Step 3: Commit**

Run:

```bash
git add docs/project/state.md
git commit -m "docs: seed current project state"
```

Expected: commit succeeds.

### Task 3: Seed Next Work Surface

**Files:**
- Create: `docs/project/next.md`

- [ ] **Step 1: Add `docs/project/next.md`**

Create `docs/project/next.md` with:

```md
# Next Work

Last updated: 2026-05-09

## Recommended Next Move

Complete the planning-flow cutover.

Why: the project is intentionally retiring both cockpit and Project OS as active workflows. Before more native-backend work accumulates, humans and agents need a short active planning surface that shows current state and next work without maintaining duplicate systems.

Done when:

- `docs/project/README.md`, `state.md`, `next.md`, and `records/` exist.
- `AGENTS.md` points to `docs/project/README.md`.
- `docs/project-os/` and `docs/archive/project-cockpit/` are marked reference-only.
- A dated cutover record exists.
- The old Project OS fast path is no longer described as active.

## Ready After Cutover

### Native backend tool/resource/session hardening

Recommended first slice: backend-only `project_path_info` or provider tool-result continuation primitives, depending on owner preference at implementation time.

Why: native backend dogfood is the durable product path, but local data exposure and provider continuation need small yach-owned slices before broader tools/resources work.

Relevant sources:

- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`
- `docs/project/specs/2026-05-09-planning-flow-cutover-design.md`

### Performance evidence follow-up

Keep performance work scoped to claims that affect product direction or native-backend decisions.

Why: yach's thesis depends on measured responsiveness, but the next product move is native-backend hardening unless a performance question blocks it.

Relevant sources:

- `docs/benchmarks/current-baseline-2026-05-05.md`
- `docs/project-os/performance-evidence.md`

## Not Ready Without a New Spec

- Defaulting to the native backend.
- Sending local file contents to a provider.
- File mutation tools.
- Process or shell execution tools.
- Network tools.
- Broad provider settings UI.
- Moving or deleting `docs/project-os/`.

Each of these needs a focused Superpowers design before implementation.
```

- [ ] **Step 2: Check for unfinished draft markers**

Run:

```bash
rg -n "T[B]D|T[O]DO|implement [l]ater|fill [i]n|\\?\\?" docs/project/next.md
```

Expected: no matches; command exits 1.

- [ ] **Step 3: Commit**

Run:

```bash
git add docs/project/next.md
git commit -m "docs: seed next work surface"
```

Expected: commit succeeds.

### Task 4: Add Cutover Record

**Files:**
- Create: `docs/project/records/2026-05-09-planning-flow-cutover.md`
- Modify: `docs/project/state.md`

- [ ] **Step 1: Add the dated record**

Create `docs/project/records/2026-05-09-planning-flow-cutover.md` with:

```md
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
```

- [ ] **Step 2: Add the record to current relevant records**

Append this bullet under `## Currently Relevant Records` in `docs/project/state.md`:

```md
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
```

- [ ] **Step 3: Verify the record exists**

Run:

```bash
test -f docs/project/records/2026-05-09-planning-flow-cutover.md
```

Expected: command exits 0.

- [ ] **Step 4: Verify the state doc links the now-created record**

Run:

```bash
rg -n "docs/project/records/2026-05-09-planning-flow-cutover.md" docs/project/state.md
```

Expected: one match.

- [ ] **Step 5: Commit**

Run:

```bash
git add docs/project/records/2026-05-09-planning-flow-cutover.md docs/project/state.md
git commit -m "docs: record planning flow cutover"
```

Expected: commit succeeds.

### Task 5: Update Agent Instructions

**Files:**
- Modify: `AGENTS.md`

- [ ] **Step 1: Add a short Project Planning section**

Add this section near the top-level project instructions, after the cockpit archive note and before language-specific environment notes:

```md
## Project Planning

- Active project planning starts at `docs/project/README.md`.
- For nontrivial work, read `docs/project/state.md` and `docs/project/next.md` before choosing the next task.
- `docs/project-os/` and `docs/archive/project-cockpit/` are reference-only, not active workflow instructions.
```

- [ ] **Step 2: Verify the note is present**

Run:

```bash
rg -n "Active project planning starts|docs/project-os/.*reference-only" AGENTS.md
```

Expected: two matches, one for the active planning entry point and one for the reference-only note.

- [ ] **Step 3: Commit**

Run:

```bash
git add AGENTS.md
git commit -m "docs: point agents at active project planning"
```

Expected: commit succeeds.

### Task 6: Mark Project OS Retired

**Files:**
- Modify: `docs/project-os/README.md`

- [ ] **Step 1: Add a retirement notice and demote active language**

At the top of `docs/project-os/README.md`, before the existing heading, add:

```md
> **Retired workflow:** Active project planning now starts at `../project/README.md`.
> This directory is reference-only and should not be maintained in parallel with `docs/project/`.

```

Then replace the opening description and `## Fast path for ordinary work` section with reference-only wording:

```md
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
```

Also demote the lower historical sections so they do not read as current workflow instructions:

- Rename `## Shared status vocabulary` to `## Historical status vocabulary` and describe the labels as labels the retired Project OS used.
- Rename `## Requirement coverage` to `## Historical requirement coverage` and state that the table is historical context, not the current planning contract.
- Rename `## First-pass caveat` to `## Historical first-pass caveat` and phrase it in the past tense.

- [ ] **Step 2: Verify the notice is present**

Run:

```bash
rg -n "Retired workflow|../project/README.md" docs/project-os/README.md
```

Expected: matches for the retirement notice line and current-task-selection pointer.

- [ ] **Step 3: Commit**

Run:

```bash
git add docs/project-os/README.md
git commit -m "docs: retire project os workflow"
```

Expected: commit succeeds.

### Task 7: Update Root README Planning Pointer

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Replace the Project OS planning pointer**

In `README.md`, replace the text under `## Planning and next work` with:

```md
Active project planning starts at `docs/project/README.md`.
```

- [ ] **Step 2: Verify the root README points to the new fast path**

Run:

```bash
rg -n "Active project planning starts at `docs/project/README.md`" README.md
```

Expected: one match.

- [ ] **Step 3: Commit**

Run:

```bash
git add README.md
git commit -m "docs: point readme at active project planning"
```

Expected: commit succeeds.

### Task 8: Final Verification

**Files:**
- Verify: `docs/project/README.md`
- Verify: `docs/project/state.md`
- Verify: `docs/project/next.md`
- Verify: `docs/project/records/2026-05-09-planning-flow-cutover.md`
- Verify: `AGENTS.md`
- Verify: `README.md`
- Verify: `docs/project-os/README.md`

- [ ] **Step 1: Check required files**

Run:

```bash
test -f docs/project/README.md
test -f docs/project/state.md
test -f docs/project/next.md
test -f docs/project/records/2026-05-09-planning-flow-cutover.md
```

Expected: all commands exit 0.

- [ ] **Step 2: Check references**

Run:

```bash
rg -n "docs/project/README.md|docs/project/state.md|docs/project/next.md|docs/project-os/|docs/archive/project-cockpit/" AGENTS.md docs/project docs/project-os/README.md
```

Expected: output shows `AGENTS.md` and `docs/project/README.md` pointing to active project docs, and reference-only language for `docs/project-os/` plus `docs/archive/project-cockpit/`.

- [ ] **Step 3: Check root README pointer**

Run:

```bash
rg -n "Active project planning starts at `docs/project/README.md`" README.md
```

Expected: one match.

- [ ] **Step 4: Check markdown for unfinished draft markers**

Run:

```bash
rg -n "T[B]D|T[O]DO|implement [l]ater|fill [i]n|\\?\\?" docs/project AGENTS.md README.md docs/project-os/README.md
```

Expected: no matches; command exits 1.

- [ ] **Step 5: Check formatting whitespace**

Run:

```bash
git diff --check
```

Expected: no whitespace errors; command exits 0.

- [ ] **Step 6: Check repository status**

Run:

```bash
git status --short --branch
```

Expected: branch is ahead by the new documentation commits and has no unstaged or staged changes.
