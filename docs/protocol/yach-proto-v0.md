# yach-proto v0 note

This note documents the current `yach-proto` seed as implemented in code. It is not a final spec, but it is the current contract the workspace is building around.

## Goals of the current protocol seed

- keep the Rust UI talking to a Yach-owned protocol rather than Pi RPC directly
- give adapters a typed transport envelope with JSONL round-tripping
- support capability negotiation early so UI/adapter mismatches are explicit
- leave room for stream correlation and request tracking before runtime complexity grows

## Current core types

Implemented in `crates/yach-proto/src/lib.rs`:

- `Capability` -- shared feature vocabulary between UI and adapters
- `Handshake` -- identity + protocol version + advertised capabilities
- `NegotiatedCapabilities` -- intersection of UI and adapter capabilities
- `MessageMeta` -- message id, optional correlation id, optional stream id
- `TransportMessage` -- top-level envelope with direction and typed body
- `ClientEvent` -- outbound UI/client intents
- `ServerEvent` -- inbound adapter/backend events

## Wire shape

The current transport uses JSONL helpers on the typed messages.

Important characteristics:

- protocol version is currently `0.1.0`
- event enums use tagged `snake_case` serialization
- transport messages include explicit direction
- transport metadata carries request/stream correlation fields

This is intentionally close to the PRD's Pi-RPC-shaped phase-1 direction without exposing Pi RPC details directly to the UI.

## Currently modeled client events

- initialize
- prompt submitted
- prompt cancelled
- session selected
- available models requested
- fork messages requested
- session messages requested
- session stats requested
- legacy model selected
- detailed model selected (`provider`, `model_id`)
- session fork requested (current-branch clone or entry-id fork with before/at position)
- dialog resolved
- widget cleared
- thinking level selected

## Currently modeled server events

- ready
- backend state updated
- prompt delta
- prompt finished
- tool call started
- tool call finished
- status updated
- session changed
- available models updated
- fork messages updated
- session messages updated
- session stats updated
- model changed
- dialog requested
- notification raised
- widget updated
- title changed

## Dialog model

The current protocol explicitly models the Tier A dialog shapes needed for stock RPC parity groundwork:

- select
- confirm
- input
- editor

This is represented through:

- `DialogKind`
- `DialogRequest`
- `DialogResponse`

## Native backend session groundwork

`crates/yach-backend` now contains provisional native session/event-log records for the first dogfood runner. These are backend-internal, append-only JSONL records, not a `yach-proto` wire commitment yet. They currently cover yach-owned session ids, entry ids, turn ids, roles, parent links, provider metadata annotations, and completed/failed/cancelled turn outcomes so the native backend can persist and reload a minimal prompt/assistant exchange before richer tree/fork/import semantics are finalized.

`crates/yach-cli` has native dogfood paths that reuse existing protocol events for the first backend-owned runners: `yach tui` / `yach tui --backend native` for the fake/fixture runner and `yach tui --backend native-provider` for an approval-gated Rig provider dogfood path. Both use `Ready`, `StateUpdated`, `AvailableModelsUpdated`, `PromptDelta`, `PromptFinished`, `StatusUpdated`, `SessionMessagesUpdated`, `SessionStatsUpdated`, and `RecentSessionsUpdated`; native-provider additionally routes prompts through `ProviderRequest -> run_provider_request(...) -> ProviderStreamEvent` below `yach-backend` when explicit provider env is configured. Native-provider setup/runtime failures currently surface through existing `StatusUpdated` / `PromptFinished` copy with normalized provider error kind hints; no typed protocol error event has been added yet. Pi remains available only as an explicit comparison/reference backend via `yach tui --backend pi`.

The fake native runner currently recognizes `/native-fixture-fail`, `/native-fixture-malformed`, and `/native-fixture-cancel` prompt markers to persist failed/malformed/cancelled turn outcomes in the backend-internal JSONL log. If the UI/backend receiver is dropped during fixture streaming, the runner records the active turn as cancelled before returning. `PromptCancelled` is modeled as a protocol event and the UI sends it only when `PromptCancellation` is negotiated; native/native-provider dogfood handshakes now advertise that capability, native-provider prompts run in an abortable active-turn task, and cancellation persists a cancelled turn marker. Pi RPC remains local-cancel-only because the stock adapter does not yet expose a compatible cancel command. Fixture prompt markers and `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` are runtime test hooks, not stable user-facing protocol commands.

`yach-backend` also has a backend-internal `BoundedProviderStreamBuffer` fixture policy for native provider streams. The current policy coalesces text deltas when the buffer is full, preserves lifecycle boundaries by dropping queued text where possible, and returns a structured backpressure failure when the buffer cannot make progress. This policy is not yet a stable protocol guarantee and does not claim that the outer UI channel is fully backpressure-bounded.

## Known omissions

The following are still missing or intentionally underspecified:

- stock Pi RPC abort/cancellation mapping for the new native-oriented `PromptCancelled` client event
- editor text update events as a first-class protocol surface (`set_editor_text` in stock Pi RPC)
- structured session export response
- protocol-level session tree records (the TUI now derives a local branch summary from typed session messages; fork-message lists and recent-session discovery are modeled)
- dynamic slash commands from Pi prompts/skills/extensions (`get_commands` in stock Pi RPC)
- compaction, auto-compaction, auto-retry, steering mode, and follow-up mode controls
- settings/resource/package/theme discovery and reload surfaces
- richer stream lifecycle events beyond the current prompt-level finish/cancel markers
- explicit protocol-level error message envelopes for unsupported features or malformed backend input
- a documented stability promise for field names beyond the current code/tests

See `../status/compatibility-evidence-2026-04-27.md` for the current compatibility gap audit.

## Next likely protocol steps

1. add explicit error and stream-complete events
2. model the remaining Tier A editor/session surfaces beyond the entry-id fork request shape
3. decide SDK sidecar vs direct Rust file/resource loading for settings/resources/session discovery
4. decide which parts of this wire shape should be documented as stable for external adapters versus still experimental
