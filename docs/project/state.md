# Project State

Last updated: 2026-05-11

## Thesis

Yach is a Rust-native coding harness. The validated near-term shell is Pi-shaped, but the durable product direction is a yach-owned Rust UI, protocol, backend runtime, session model, tool loop, and file-first resource system.

Pi remains useful as a compatibility/reference backend. It is not the long-term architecture target.

## Current Posture

- `main` includes PR #14: native-backend branch wrap-up and retirement of cockpit-style workflow artifacts.
- M0/M1/M2 foundations are considered verified enough for forward planning: workspace, protocol seed, Pi RPC adapter, TUI alpha loop, session/fork groundwork, and performance harness exist.
- Native backend work is in progress behind explicit opt-in boundaries. Pi remains the default backend for now, but Native MVP work is framed around yach-owned backend primitives rather than Pi compatibility.
- Native sessions now have an append-only JSONL store seam, restart-safe turn indexing, provider transcript resume context, low-frequency session metric events, and append/load/projection benchmark coverage.
- Native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, a metadata-only project path tool, a backend-only autonomous tool loop that records session evidence while shaping safe provider tool results, and backend-only continuation mapping into adapter-ready provider request input.
- The planning-flow cutover is complete: `docs/project/` is the active planning fast path, while cockpit and Project OS docs are reference-only.

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

The plan is sufficient to plan explicit native-provider one-round integration for safe read-only tool results. It is not sufficient for file writes, process execution, network tools, extension runtime, provider-native tool-result block support, or default-backend changes. Those need dedicated Superpowers specs/plans and explicit approval.

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
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
