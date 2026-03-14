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
- session selected
- model selected
- session fork requested
- dialog resolved
- widget cleared

## Currently modeled server events

- ready
- prompt delta
- tool call started
- status updated
- session changed
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

## Known omissions

The following are still missing or intentionally underspecified:

- editor text update events as a first-class protocol surface
- session stats/export messages
- richer stream lifecycle events such as completion/failure markers
- explicit error message envelopes for unsupported features or malformed backend input
- a documented stability promise for field names beyond the current code/tests

## Next likely protocol steps

1. add explicit error and stream-complete events
2. model the remaining Tier A editor/session surfaces
3. decide which parts of this wire shape should be documented as stable for external adapters versus still experimental
