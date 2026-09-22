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

- protocol version is currently `0.3.0`
- event enums use tagged `snake_case` serialization
- transport messages include explicit direction
- transport metadata carries request/stream correlation fields


The `auto_review` capability gates automatic-review mode and reviewer status events. Both peers
must advertise it before a client selects `ApprovalMode::AutoReview` (wire value `auto-review`).
The capability list is forward-tolerant when decoded by current binaries: recognized entries are
kept in their original order (including duplicates), while unknown entries are dropped.

Compatibility within protocol version `0.3.0` is asymmetric:

| Client | Backend | Result |
| --- | --- | --- |
| Legacy (no `auto_review`) | Current | Supported; negotiation omits `auto_review` and review remains manual |
| Current | Legacy strict decoder | Unsupported; the old backend rejects the new client's unknown `auto_review` capability |
| Current | Current | Supported; automatic review is available only when negotiation includes `auto_review` |

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
- correlated model activation requested with exact target and explicit session-only or session-and-default intent
- session fork requested (current-branch clone or entry-id fork with before/at position)
- dialog resolved
- widget cleared
- thinking level selected
- tool review decision submitted
- extension lifecycle requested (`extension_lifecycle_requested`)
- extension diagnostic snapshot requested (`extension_diagnostic_snapshot_requested`)


## Currently modeled server events

- ready
- backend state updated
- prompt delta
- prompt finished
- tool call started
- tool review requested or resolved
- tool call finished, including optional structured outcome and payload metadata
- status updated
- session changed
- available models updated
- fork messages updated
- session messages updated
- session stats updated
- correlated model activation finished with independent session and default-update outcomes
- prompt attempt reset with prompt-wide sequence and exact UTF-8 suffix byte count
- structured model selection required with a categorical resolution reason
- dialog requested
- notification raised
- widget updated
- title changed
- extension lifecycle finished (`extension_lifecycle_finished`)
- extension diagnostic snapshot updated (`extension_diagnostic_snapshot_updated`)
- reviewer status changed (`reviewer_status_changed`)


## Structured tool review

`Capability::StructuredReviewRows` gates actionable command and edit reviews. A backend sends
`ToolReviewRequested` with a typed `ToolReviewPayload`; the client answers with
`ToolReviewDecisionSubmitted` using the request, preview, and permission-decision identifiers from
that payload. After durably recording the decision or interruption, the backend sends
`ToolReviewResolved` with its authoritative `ToolReviewResolution`. Backends fail closed rather
than issuing an actionable review when the capability was not negotiated. `ToolCallFinished`
replaces the same transcript row with its terminal output and may include a structured
`HarnessOutcomeKind` plus `ToolResultMetadata` (`byte_count`, `truncated`, and optional reason).

`CommandReviewSummary` and `LocalEditPreviewSummary` optionally carry `review_origin`. An omitted
field (or `null` when decoding) means the ordinary user-ask path. `risk` means the automatic
reviewer flagged risk, `reviewer_error` means review was unavailable and fell back to a person, and
`human_performs` means the action is reserved for the user. Serializers omit the field for the
ordinary user-ask path so older clients continue to decode review payloads.

When `auto_review` is negotiated, `ReviewerStatusChanged` reports `reviewer_id`, monotonically
ordered `generation`, and `state` (`selected`, `unavailable`, or `reloaded`). The backend emits a
selection before review work for that reviewer generation. A reload advances the generation and
emits `reloaded` before review work from the replacement generation; clients must use generation
order rather than arrival text to reject stale status. No reviewer status event is emitted unless
`auto_review` was negotiated.

## Extension lifecycle and diagnostics

`Capability::ExtensionLifecycle` negotiation gates whether a client should expose and send these events. Negotiation only establishes that both peers understand the surface; it does not grant extension capabilities and does not replace the backend's authority check before activation.

The lifecycle messages are:

| Wire event | Fields |
| --- | --- |
| `extension_lifecycle_requested` | `request_id: String`, `action` (`stop`, `reload`, `trust`, or `revoke`), `selector: String` |
| `extension_lifecycle_finished` | `request_id: String`, `action` (`stop`, `reload`, `trust`, or `revoke`), `selector: String`, `outcome` (`completed`, `not_found`, `not_active`, or `failed`), required `message: String` |

Lifecycle selectors are trimmed and must not be empty. Stop matches the live snapshot by id, source reference, install source, or package root. Reload and trust match the current manifest scan by id, source reference, package root, or manifest path. Revoke resolves a discovered selector to its manifest id, but also accepts a valid extension id after uninstall; an unmatched path or other non-id selector fails. Consequently the precise non-success outcome depends on the action: for example, an unknown selector is `not_found`, stopping a known but inactive extension is `not_active`, and invalid input or storage failure is `failed`.

The diagnostic messages are:

| Wire event | Fields |
| --- | --- |
| `extension_diagnostic_snapshot_requested` | `request_id: String`, optional/nullable `selector: String` |
| `extension_diagnostic_snapshot_updated` | `request_id: String`, `outcome` (`completed`, `not_found`, or `failed`), `records: ExtensionDiagnosticRecord[]`, optional/nullable `message: String` |

For both optional fields, an omitted field or explicit `null` decodes as `None`. Yach's serializer emits the field as either its value or `null`. A selector string is trimmed; an empty or whitespace-only string also becomes unfiltered. A nonempty selector with no match produces `not_found`, an empty `records` array, and a message. An unfiltered empty snapshot is `completed` with an empty array.

`ExtensionDiagnosticRecord` has this exact current shape:

| Field | Rust / JSON value |
| --- | --- |
| `id` | `Option<String>` / string, `null`, or omitted |
| `version` | `Option<String>` / string, `null`, or omitted |
| `scope` | `String` |
| `package_root` | `String` |
| `manifest_path` | `Option<String>` / string, `null`, or omitted |
| `source_ref` | `Option<String>` / string, `null`, or omitted |
| `install_source` | `Option<String>` / string, `null`, or omitted |
| `activation_state` | `String` |
| `generation` | `u64` / non-negative JSON integer |
| `last_error_kind` | `Option<String>` / string, `null`, or omitted |
| `last_error_summary` | `Option<String>` / string, `null`, or omitted |
| `registered_tools` | `Vec<String>` / string array |
| `provider_visible_tools` | `Vec<String>` / string array |
| `capabilities` | `Option<Vec<String>>` / string array, `null`, or omitted |
| `capability_grant` | `Option<Vec<String>>` / string array, `null`, or omitted |

For both capability fields, missing or `null` means unknown: the producer did not have enough information to make the claim. An empty `capabilities` array means the known manifest requests no grant-requiring capabilities. An empty `capability_grant` array means there is no current approval; that remains true when a revoked authority document is retained on disk for its history. A nonempty array contains the current snake_case capability names.

Yach's native backend sorts records by id (using `none` for a missing id), then package root. Its capability arrays are produced from ordered sets. These are properties of records produced by the current native implementation, not general wire-order guarantees: external adapters may emit records or capability arrays in another order, and consumers must not attach meaning to array order.

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

## Stdio JSONL transport

The `yach rpc` command exposes the same `ClientEvent`/`ServerEvent` surface
over a process-owned stdin/stdout boundary.

### Framing

- stdin and stdout are UTF-8 JSONL streams
- each event occupies exactly one LF-terminated line
- client lines decode with `ClientEvent::from_jsonl`; server lines encode with
  `ServerEvent::to_jsonl`
- stdout contains only server-event JSONL frames; diagnostics go to stderr

### Lifecycle and recoverability

- the client begins with `Initialize(handshake)`; the backend answers with
  `Ready { handshake }`, which is the readiness signal
- `BackendEvent::Connected` and `Disconnected` are in-process plumbing and
  never cross the wire
- stdin EOF requests graceful shutdown; the client channel is dropped, pending
  backend events are drained, stdout is flushed, and the child exits
- malformed input is recoverable: the server emits a `StatusUpdated` frame
  whose message begins `rpc: invalid client event:`, skips that line, and
  continues reading

### Secrets and recording

`DialogResolved { Secret }` is legal on the direct wire because the child is
trusted by the process that launched it. Direct transport uses `to_jsonl`.
Session recording uses `to_record_jsonl`, which retains the existing secret
exclusion and never persists the secret payload.

## Known omissions

The following are still missing or intentionally underspecified:

- stock Pi RPC abort/cancellation mapping for the new native-oriented `PromptCancelled` client event
- editor text update events as a first-class protocol surface (`set_editor_text` in stock Pi RPC)
- structured session export response
- protocol-level session tree records (the TUI now derives a local branch summary from typed session messages; fork-message lists and recent-session discovery are modeled)
- dynamic slash commands from Pi prompts/skills/extensions (`get_commands` in stock Pi RPC)
- compaction, auto-compaction, steering mode, and follow-up mode controls
- richer stream lifecycle beyond prompt finish/cancel and attempt reset
- explicit protocol-level error envelopes beyond the current bounded status rejection
- a documented stability promise for field names beyond the current code/tests

The executable contract is `crates/yach-cli/tests/rpc_matrix.rs` and `crates/yach-proto/src/lib.rs`; those, not this note, are authoritative where they disagree.

## Next likely protocol steps

1. add explicit error and stream-complete events
2. model the remaining Tier A editor/session surfaces beyond the entry-id fork request shape
3. decide SDK sidecar vs direct Rust file/resource loading for settings/resources/session discovery
4. decide which parts of this wire shape should be documented as stable for external adapters versus still experimental
