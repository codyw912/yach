---
title: feat: Evaluate jcode as yach comparator
status: active
date: 2026-04-28
---

# feat: Evaluate jcode as yach comparator

## Overview

Evaluate `https://github.com/1jehuang/jcode` as an external benchmark and architecture comparator for yach. This is explicitly not a dependency-adoption path. The goal is to understand what jcode's published startup, input-readiness, memory, multi-session, provider, and session architecture claims should teach yach before yach makes stronger native backend or performance claims.

This plan starts with a docs-first evidence review and benchmark design. Local install/run benchmarking is optional and requires explicit human approval because it may involve a large clone, running third-party binaries, or changing local tool state.

## Problem Frame

Yach is actively building native backend seams while preserving `yach-proto` as the UI/backend boundary and keeping Pi RPC as a compatibility/reference adapter. jcode is a newly surfaced Rust coding-agent harness with similar positioning: multi-session workflows, performance/resource efficiency, customizability, and provider/runtime architecture. Its README publishes strong comparative numbers against Pi and other tools, including time-to-first-frame, time-to-first-input, and memory scaling.

Yach should respond with evidence discipline, not reaction. The useful output is a durable comparator report that says which jcode claims are comparable, which need local reproduction, which benchmark boundaries differ from yach's current harness, and which architecture patterns are relevant or intentionally out of scope.

## Requirements Trace

- R1. Treat jcode as a comparator/evaluation target only, not as a dependency or source to vendor.
- R2. Preserve yach's performance evidence discipline: exact commands, versions, environment, sample counts, timing boundaries, exclusions, and limitations.
- R3. Compare only fair or explicitly approximate workloads; do not compare yach internals/headless shortcuts to jcode end-to-end UI behavior.
- R4. Identify whether existing yach benchmark harnesses can measure comparable startup/readiness/memory surfaces before adding new harness code.
- R5. Review jcode architecture for lessons relevant to yach-owned provider/session seams without weakening yach invariants.
- R6. Avoid secrets and real provider/network calls for comparator benchmarking unless a later explicit benchmark plan requires them.
- R7. Ask before force-cloning the large jcode repo or installing/running jcode binaries.

## Scope Boundaries

- Do not add jcode as a dependency.
- Do not vendor or copy jcode code.
- Do not run jcode binaries, install scripts, or force-clone the full repository without human approval.
- Do not claim yach is faster/slower than jcode unless same-machine evidence with clear timing boundaries exists.
- Do not expand the native backend implementation as part of this comparator pass.
- Do not choose Rig/Siumai/direct SDKs based solely on jcode's architecture; that decision belongs to the provider-library spike.

## Context & Research

### Local yach references

- `docs/benchmarks/README.md` — benchmark report conventions.
- `docs/benchmarks/pi-comparison-methodology.md` — fairness rules for same-machine competitor comparisons.
- `docs/project-os/performance-evidence.md` — where measured evidence is indexed.
- `docs/project-os/architecture-invariants.md` — invariants comparator work must not weaken.
- `docs/project-os/decisions.md` — durable decision log if the evaluation changes strategy.
- `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md` — native backend/provider/session context.
- `crates/yach-bench/src/main.rs` and related benchmark modules — existing harness capabilities.
- `.project/now.md` — cockpit state already records jcode as comparator/reference only.

### Existing yach benchmark surfaces

Current benchmark commands include startup, terminal, keypress, active stream, heavy output, transcript scroll, Pi clean startup, yach CLI startup, yach TUI startup, and yach TUI ready startup reports. The closest comparator surfaces for jcode are likely startup/readiness and memory/multi-session, but current yach timing boundaries must be checked before any comparison is labeled equivalent.

### jcode initial observations

GitHub API fetch indicates jcode is a large Rust workspace (~371 MB) with crates including:

- `crates/jcode-agent-runtime`
- `crates/jcode-provider-core`
- `crates/jcode-provider-gemini`
- `crates/jcode-provider-openrouter`
- `crates/jcode-provider-metadata`
- `crates/jcode-desktop`
- `crates/jcode-tui-workspace`
- `crates/jcode-mobile-core`

Visible docs include:

- `docs/MULTI_SESSION_CLIENT_ARCHITECTURE.md`
- `docs/MEMORY_ARCHITECTURE.md`
- `docs/MEMORY_BUDGET.md`
- `docs/TERMINAL_BENCH.md`
- `docs/PROVIDER_SESSION_SHARED_CONTRACT_AUDIT.md`
- `docs/SERVER_ARCHITECTURE.md`
- `docs/SAFETY_SYSTEM.md`
- `docs/SECURITY_DEPENDENCIES.md`

README-published claims include time to first frame, time to first input, 1-session memory, 10-session memory, and marginal memory per added session. These are claims to evaluate and potentially reproduce, not facts to import into yach evidence without local methodology review.

## Key Decisions

- Start with a docs-only comparator report. This avoids running untrusted third-party binaries and lets yach first determine benchmark equivalence and architecture questions.
- Put the first report under `docs/spikes/` unless actual same-machine measurements are run. If measurements are run later, create a `docs/benchmarks/` report and index it in `docs/project-os/performance-evidence.md`.
- Treat jcode architecture as a source of questions and lessons, not a design authority. Yach-owned sessions, resources, tools, and protocol events remain invariant.

## Implementation Units

### U1. Create jcode comparator evidence review

**Goal:** Produce a durable comparator report that captures jcode's published claims, methodology gaps, architecture observations, and recommended next benchmark actions.

**Requirements:** R1, R2, R3, R5, R6

**Dependencies:** None

**Files:**

- Create: `docs/spikes/2026-04-28-jcode-comparator-evaluation.md`
- Modify: `.project/now.md` only if ready-next state changes after the report

**Approach:**

- Use the GitHub API/fetched repository metadata already available unless the user explicitly approves a full clone.
- Summarize jcode's visible claims and architecture surfaces.
- Map jcode's claimed metrics to existing yach benchmark surfaces and classify each as equivalent, approximate, blocked, or unknown.
- Identify methodology unknowns such as exact command, sample count, viewport, warm/cold start, credentials/network usage, embedding state, and raw artifacts.
- Capture architecture questions relevant to yach's provider/session/runtime seams.
- Recommend whether a local benchmark pass is worth doing and what approval/commands would be needed.

**Test scenarios / verification:**

- Report explicitly says jcode is not a dependency candidate.
- Report includes a benchmark equivalence table for startup/readiness/memory/multi-session claims.
- Report includes limitations and no unsupported performance claims.
- Report has repo-relative links to yach docs and fetched jcode URL.
- No code changes or third-party execution are performed.

### U2. Optional local jcode benchmark plan or harness extension

**Goal:** If U1 finds a safe and useful local comparison path, prepare a repeatable benchmark plan or small harness addition.

**Requirements:** R2, R3, R4, R6, R7

**Dependencies:** U1 and explicit human approval before installing/running jcode or force-cloning the repository.

**Files:**

- Modify: `docs/benchmarks/README.md` only if adding a benchmark report convention.
- Create: `docs/benchmarks/jcode-comparison-2026-04-28.md` only if actual measurements are run.
- Modify: `docs/project-os/performance-evidence.md` only if actual measurements are run.
- Modify: `crates/yach-bench/src/main.rs` and tests only if a reusable comparator harness is justified.

**Approach:**

- Ask for approval before running jcode install scripts, binaries, or force-cloning the full repo.
- Prefer no-credential startup/readiness/memory workloads.
- Record exact jcode version/commit, install/run method, machine, viewport, sample count, timing boundary, and excluded phases.
- Add harness code only when it measures a fair reusable boundary. Otherwise keep the result as a report.

**Test scenarios / verification:**

- If docs-only benchmark plan: `git diff --check`.
- If benchmark code changes: `just dev cargo clippy -p yach-bench --all-targets -- -D warnings` and `just dev cargo test -p yach-bench`.
- If measurements are recorded: report includes raw command/environment details and limitations.

### U3. Apply project OS update gate

**Goal:** Keep project state aligned after the comparator evaluation without over-claiming evidence.

**Requirements:** R2, R5

**Dependencies:** U1; U2 only if measurements or durable strategy changes occur.

**Files:**

- Modify: `.project/now.md`
- Modify: `docs/project-os/performance-evidence.md` only if actual benchmark measurements are added.
- Modify: `docs/project-os/decisions.md` only if the evaluation changes durable native backend/provider/comparator strategy.

**Approach:**

- Update cockpit state with the report and recommended next action.
- Do not index jcode claims in performance evidence unless yach has actual measured evidence or a clearly labeled no-data/prototype report.
- Record durable decisions only if strategy changes.

**Test scenarios / verification:**

- Cockpit next chunks reflect whether the next step is local benchmark approval, U5 provider spike, or native dogfood work.
- No project OS evidence file claims benchmark results that were not measured.

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Overreacting to a competitor and derailing native backend work | Keep U1 docs-only and explicitly tie findings back to yach's existing plan. |
| Unfair benchmark comparisons | Use `docs/benchmarks/pi-comparison-methodology.md` rules and label equivalence honestly. |
| Running untrusted or credential-requiring code | Ask before install/run/force-clone; avoid credentials and network/provider calls. |
| Importing jcode architecture wholesale | Treat it as reference only; preserve yach invariants and record any durable strategy shift separately. |
| Performance claims without evidence | Put unmeasured claims in spike limitations, not `performance-evidence.md` as yach evidence. |

## Verification

For U1:

- `git diff --check`
- Review `docs/spikes/2026-04-28-jcode-comparator-evaluation.md` for unsupported claims.

For any code/harness follow-up:

- `just dev cargo fmt --check`
- `just dev cargo clippy -p yach-bench --all-targets -- -D warnings`
- `just dev cargo test -p yach-bench`

## Sources

- `https://github.com/1jehuang/jcode`
- `docs/benchmarks/README.md`
- `docs/benchmarks/pi-comparison-methodology.md`
- `docs/project-os/performance-evidence.md`
- `docs/project-os/architecture-invariants.md`
- `docs/project-os/decisions.md`
- `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`
- `.project/now.md`
