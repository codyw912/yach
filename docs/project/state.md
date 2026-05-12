# Project State

Last updated: 2026-05-12

## Thesis

Yach is a Rust-native coding harness. The validated near-term shell is Pi-shaped, but the durable product direction is a yach-owned Rust UI, protocol, backend runtime, session model, tool loop, and file-first resource system.

Pi remains useful as a compatibility/reference backend. It is not the long-term architecture target.

## Current Posture

- `main` includes PR #24: native startup profiling, native-default TUI behavior, native-provider tool advertising, one-round provider tool continuation, and earlier native-backend branch wrap-up are merged.
- M0/M1/M2 foundations are considered verified enough for forward planning: workspace, protocol seed, Pi RPC adapter, TUI alpha loop, session/fork groundwork, and performance harness exist.
- Native backend work is now the default `yach tui` path. Pi remains available only as an explicit comparison/reference backend via `--backend pi`; Native MVP work is framed around yach-owned backend primitives rather than Pi compatibility.
- Native sessions now have an append-only JSONL store seam, restart-safe turn indexing, provider transcript resume context, low-frequency session metric events, and append/load/projection benchmark coverage.
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, backend-only continuation mapping into adapter-ready provider request input, explicit native-provider one-round handling for completed safe read-only tool calls, and schema-only `project_path_info` advertising on explicit native-provider initial requests through `yach.provider_tool_advertising.v1`. Continuation requests strip that advertising so the one-round/fail-closed boundary remains intact.
- The planning-flow cutover is complete: `docs/project/` is the active planning fast path, while cockpit and Project OS docs are reference-only.
- Native startup profiling shows traced Rust `main` to first render is sub-millisecond p95 on the local benchmark run; extension discovery and activation should stay off the default first-frame path.

## Architecture Beliefs

- `yach-proto` is the UI/backend seam.
- The TUI should not speak Pi RPC, provider SDK, or native backend internals directly.
- Yach owns sessions, tools, resources, protocol events, and user-facing runtime semantics.
- Provider libraries can sit below yach-owned seams, but they do not own sessions, tool execution, or canonical transcript state.
- File-first configuration and inspectable local state remain product values.
- Compatibility and performance claims need evidence, not assumptions.

## Profiling And Traceability

Yach should be designed so performance work can use real tools and evidence.

- Core primitives should have clear measurable boundaries: provider requests, stream handling, tool validation/execution, file read/search/edit, verification commands, session append/load/projection, and TUI render/update.
- Correlation IDs should flow across layers where they exist: session ID, turn ID, entry ID, tool request ID, provider response ID, and future edit or verification IDs.
- Instrumentation should stay low-noise. Canonical session logs should keep durable evidence, while high-frequency metrics should be summarized or stored separately if they become necessary.

For each Native MVP slice, ask: can this be benchmarked in isolation, and can we explain a slow run after the fact?

## Current Risks

- Native-provider dogfood can grow into a chat-only path unless tools, resources, persistence, cancellation, and error semantics stay yach-owned.
- Local project data exposure needs deny-by-default policy until provider-visible resource rules are explicit.
- Planning docs can become stale if live summaries accumulate history instead of pointing to records.
- Same-machine Pi comparison evidence is still imperfect, so performance claims should stay scoped to measured surfaces.

## Plan Sufficiency

The current planning surface is sufficient to continue Native MVP implementation from the accepted MVP definition.

The accepted provider tool advertising plan is sufficient for schema-only `project_path_info` advertising behind explicit native-provider opt-in. It is not sufficient for file writes, process execution, network tools, extension runtime implementation, provider-native tool-result block support, or additional default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.

The proposed extension tool registration design is sufficient for discussion of the first extension-owned tool contribution surface, but it is not an implementation plan yet.

## Currently Relevant Records

- `docs/superpowers/specs/2026-05-09-planning-flow-cutover-design.md`
- `docs/superpowers/plans/2026-05-09-planning-flow-cutover.md`
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-store-resume-metrics.md`
- `docs/superpowers/plans/2026-05-11-native-read-search-context.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-tool-loop-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-tool-loop.md`
- `docs/superpowers/specs/2026-05-11-native-readonly-provider-continuation-design.md`
- `docs/superpowers/plans/2026-05-11-native-readonly-provider-continuation.md`
- `docs/superpowers/specs/2026-05-11-native-provider-one-round-tools-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-one-round-tools.md`
- `docs/superpowers/specs/2026-05-11-native-provider-tool-advertising-design.md`
- `docs/superpowers/plans/2026-05-11-native-provider-tool-advertising.md`
- `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
