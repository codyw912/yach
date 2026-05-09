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

- `docs/superpowers/specs/2026-05-09-planning-flow-cutover-design.md`
- `docs/superpowers/plans/2026-05-09-planning-flow-cutover.md`
- `docs/project/records/2026-05-09-planning-flow-cutover.md`
