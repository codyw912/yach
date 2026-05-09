# Native MVP Definition Design

Date: 2026-05-09
Status: proposed

## Context

Yach has moved past the initial Pi-backed TUI bootstrap. The Pi backend was useful for getting the basic Rust TUI off the ground and remains useful as a comparison/reference implementation, but it is not a real backend target for yach.

The next product goal is a native MVP: yach should be usable by the project owner on this repository for real coding work, even with rough edges. The MVP should prove that yach can become a minimal, extensible, performance-focused harness rather than a Pi-compatible shell.

## Product Thesis

Native yach should own the harness primitives that determine reliability, safety, performance, and extensibility:

- provider loop and provider-neutral runtime events;
- session log and resume;
- resource root and path policy;
- tool registration, validation, policy, execution, and result handling;
- edit transactions;
- verification actions;
- metrics and benchmarking;
- extension registration/runtime boundary.

Everything beyond the minimal core should prefer plugins/extensions, including features that many users may want. MCP support is a good example of a likely reference extension rather than an MVP core requirement.

## MVP Audience

The MVP target user is the project owner dogfooding yach on this repository.

The MVP does not need polished onboarding, broad external-user ergonomics, a plugin marketplace, or complete provider coverage. It does need to support a real work loop:

1. start yach in a real repository;
2. ask questions about the project;
3. read/search/contextualize code;
4. let the model request tools autonomously;
5. edit files;
6. run verification;
7. inspect and resume the session.

## Pi Backend Framing

Pi is not a long-term backend goal.

For Native MVP:

- Pi compatibility is not required.
- Pi session import is not required.
- Pi extension compatibility is not required.
- Pi remains useful for feature-shape reference and performance/completeness comparison.
- Native yach may intentionally diverge from Pi when yach-owned architecture, extensibility, or performance goals require it.

## Required MVP Capabilities

### Native Provider Profiles

Native MVP should support multiple native provider profiles through one yach-owned provider seam.

At least two provider paths should work through the same core request, stream, tool-call, error, cancellation, and metadata abstractions. Provider-specific auth/config should remain profile-owned or adapter-owned. Provider-specific capabilities should be explicit rather than assumed globally.

### Session Log and Resume

Native MVP requires session resume, not branch/fork.

The native session log should preserve enough information to restart yach, select a prior native session, reconstruct the useful transcript/context, and continue work.

The log should include transcript events, provider metadata, tool calls, edit actions, verification actions, errors, cancellations, and failed turns. Failed or cancelled turns must not be recorded as completed turns.

Branch/fork sessions are deferred.

### Read/Search/Context

File modification depends on safe project understanding. Native MVP should include read-only project inspection before mutation:

- project root policy;
- project-relative path resolution;
- path metadata;
- text file reads with size/encoding policy;
- search over project files;
- context packaging for provider requests.

Local data exposure should remain explicit and policy-governed. Reading local data for yach's own context does not imply provider visibility by default.

### Autonomous Tool Use

Native MVP should support model-requested tool use.

The model should be able to request tools; yach should own validation, policy checks, execution, result shaping, redaction, size limits, session evidence, and provider continuation.

User-invoked commands may be useful for testing, but they are not the product shape for MVP tool use.

### Policy-Gated Execution

Native MVP does not require a built-in interactive approval UI.

It does require policy/config-gated execution:

- deny by default for dangerous actions;
- explicit allow policies for selected safe tools/actions;
- clear denial/error records;
- session evidence for every allowed or denied action.

Richer human approval UX is deferred and may be implemented as a reference extension.

### File Edits

File edits are required for Native MVP.

The MVP edit model should be patch/edit-transaction based:

- apply edits to existing files through reviewable patch-like transactions;
- create new files through the same edit transaction model;
- record the intended change and applied result;
- preserve inspectable diffs/session evidence;
- fail safely if a patch cannot apply;
- avoid corrupting the workspace on partial failure.

Delete and rename operations are deferred unless a real MVP dogfood workflow forces them into scope.

### Verification Actions

Native MVP should treat verification as a structured harness primitive.

Yach should be able to run configured checks, tests, or commands as verification actions with captured status, summarized output, exit code, timing, and session evidence.

This does not mean arbitrary unrestricted shell access is required in MVP. The initial implementation can be policy/config based and scoped to project verification commands.

### Metrics and Benchmarking

Metrics are core to the MVP because yach's motivation includes performance that exceeds Pi's practical responsiveness.

Native MVP should include granular benchmark and telemetry surfaces for:

- startup-to-interactive time;
- prompt submission to first visible response;
- provider stream event handling;
- tool-call lifecycle timing;
- file read/search/edit timing;
- verification command timing;
- transcript render/update latency;
- queue/backpressure behavior;
- session log append/resume timing.

Benchmarking should produce repeatable local evidence that can compare native yach against Pi where useful. Metrics should be granular enough to identify bottlenecks rather than only reporting end-to-end totals.

Performance claims should not be made without recorded evidence.

## Extension Boundary

Native MVP should include a minimal extension mechanism proof, not a full extension platform.

Required:

- versioned extension boundary;
- extension registration;
- extension execution/isolation failure handling;
- at least two smoke extensions that prove extension integration with core primitives.

Recommended smoke extensions:

1. `toy_tool`: registers a safe read-only tool through the same tool registry as built-ins.
2. `static_context_provider`: loads static project guidance/context from a file.

Deferred:

- MCP support;
- plugin marketplace;
- rich UI renderers;
- hot reload;
- dynamic package resolution;
- broad lifecycle hook surface;
- Pi extension compatibility.

Reference extensions should become the preferred home for useful-but-not-minimal features. MCP support, approval UI, richer git workflows, project-specific context providers, slash commands, prompt/skill packs, and migration/import tools are likely reference-extension candidates.

## Non-goals for Native MVP

- Pi compatibility or import.
- Pi extension compatibility.
- Branch/fork session trees.
- MCP support in core.
- Built-in polished approval UI.
- Broad provider settings UI.
- Plugin marketplace or package manager.
- File delete/rename unless dogfood proves it necessary.
- Default support for arbitrary shell/process/network tools.

## Acceptance Criteria

Native MVP is reached when the project owner can use native yach on this repository to complete a small real coding task that includes:

- selecting or using a native provider profile;
- reading/searching relevant project files;
- autonomous model-requested tool use;
- at least one file edit or file creation;
- running a verification action;
- seeing enough TUI/session evidence to understand what happened;
- restarting yach and resuming the native session;
- reviewing performance/metrics evidence for the run or relevant primitives.

The task should not require Pi as the active backend.

## Likely Implementation Sequence

1. Native provider/profile baseline across at least two providers.
2. Native session log/resume.
3. Read/search/context tools.
4. Tool-call loop with policy gating, provider continuation, and session evidence.
5. Patch-based file edit/create transactions.
6. Verification action primitive.
7. Metrics/benchmarking hooks across the native work loop.
8. Minimal extension runtime plus `toy_tool` and `static_context_provider`.
9. Tighten TUI/status evidence enough for sustained dogfooding.

The sequence may change if implementation reveals a dependency, but the MVP definition should not drop file edits, session resume, autonomous tool use, or metrics.

## Reference Inputs

- `PRD-v0.1.md`
- `docs/project/state.md`
- `docs/project/next.md`
- `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
- `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`
- `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`
- Harness engineering references:
  - https://github.com/walkinglabs/learn-harness-engineering
  - https://github.com/ai-boost/awesome-harness-engineering
- Rust implementation reference, not architecture template:
  - https://github.com/openai/codex
