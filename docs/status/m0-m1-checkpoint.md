# yach M0-M1 checkpoint

This document captures the current implementation state against `PRD-v0.1.md` so we have a deliberate pause point before going deeper into runtime and TUI work.

## Scope of this checkpoint

This checkpoint focuses on:

- M0 bootstrap
- the earliest M1 stock Pi RPC adapter work
- the current CLI/protocol seams that exercise those layers

It does not claim progress on TUI, performance validation, resource compatibility, or session-file compatibility yet.

## Milestone status

### M0 -- bootstrap

Status: mostly complete

- `repo created` -- done
- `Cargo workspace created` -- done via `Cargo.toml` and the phase-1 crate layout under `crates/`
- `yach-proto v0 spec` -- partial, but real enough to validate the architecture direction
- `adapter capability model defined` -- done in `crates/yach-proto/src/lib.rs` and `crates/yach-adapter-pi-rpc/src/capabilities.rs`
- `baseline benchmark harness skeleton` -- done at skeleton level via `crates/yach-bench`, but not yet functionally implemented

Assessment:

M0 is effectively established as a codebase baseline. The main thing still missing for a stronger M0 closeout is a more explicit written protocol note, which this checkpoint and the companion protocol note begin to address.

### M1 -- stock Pi RPC adapter

Status: in progress

- `spawn/connect to pi --mode rpc` -- partial; `PiRpcSession::spawn` exists in `crates/yach-adapter-pi-rpc/src/session.rs`
- `stream transcript` -- partial; prompt delta parsing exists, but no live transcript loop or UI yet
- `send prompts` -- partial; outbound prompt serialization exists in `crates/yach-adapter-pi-rpc/src/serialize.rs`
- `handle Tier A dialogs and fire-and-forget UI` -- partial; typed parsing/serialization exists for dialogs, notifications, widgets, title changes, and status updates
- `basic session/model controls` -- partial; typed session/model events and outbound requests exist, but they are only exercised through smoke/bootstrap code right now

Assessment:

M1 has a credible typed adapter seam, but not a user-facing runnable adapter workflow yet. The big remaining step is moving from "we can parse/serialize/bootstrap" to "the CLI can intentionally run and report a real Pi RPC session flow."

### Empirical note on the current smoke path

The CLI now intentionally emits command results.

Observed on this machine:

- `cargo run -p yach-cli -- print-capabilities` -- works and prints the current stock RPC capability set
- `cargo run -p yach-cli -- smoke-pi-rpc` -- now runs and reports structured success for the current bootstrap/smoke sequence
- `pi` is installed and discoverable at `/run/current-system/sw/bin/pi`
- direct probing and the installed RPC type definitions show that stock Pi RPC is type-driven, not method-driven
- the documented command shape lives in `dist/modes/rpc/rpc-types.d.ts`, where commands are JSON objects with a `type` field
- there is no documented `initialize` command in stock RPC; the available commands include `prompt`, `get_state`, `set_model`, `switch_session`, `fork`, `get_session_stats`, and others
- Pi returns `{"type":"response","success":false,"error":"Unknown command: undefined"}` when sent the old method-based shape, which confirmed our previous bootstrap assumption was wrong
- direct probing with the documented type-based shape succeeds and yields real RPC events, including `response`, `agent_start`, `turn_start`, `message_start`, `message_end`, `turn_end`, and `agent_end`
- the current smoke result now reports initialization success because Yach treats stock RPC bootstrap as a documented `get_state` probe rather than inventing an unsupported `initialize` command

Assessment:

This is useful progress because the adapter now follows documented stock RPC behavior instead of guessing. For stock Pi RPC, bootstrap should be treated as capability-assumed plus a documented state query, not as a separate negotiated initialize handshake.

## Architecture validation against the PRD

### What is already matching the PRD well

- `yach-ui` does not speak Pi RPC directly; the shared seam goes through `yach-proto`
- the workspace shape matches the intended early crate layout from section 5.3 of the PRD
- the adapter strategy is reflected in code structure: `yach-adapter-pi-rpc` exists now and `yach-adapter-pi-sdk` exists as a seed for later work
- capability negotiation is present and tested
- a Yach-owned transport envelope exists, with JSONL helpers and request/stream correlation metadata
- the RPC adapter is split into capability, parse, serialize, and session layers, which is consistent with the PRD emphasis on process boundaries and cleaner architecture

### What remains intentionally incomplete

- no fullscreen TUI yet
- no actual transcript rendering or tool panes
- no settings/package/resource loading compatibility
- no real session file compatibility work
- no rich SDK sidecar behavior
- no benchmark harness implementation beyond crate scaffolding

## Tier A compatibility snapshot

Current state of the stock RPC surface from PRD section 6.4:

- `prompt streaming` -- partial; prompt delta parsing exists
- `dialogs: select, confirm, input, editor` -- partial; inbound dialog requests and outbound dialog resolution are typed
- `notifications` -- partial; inbound notification mapping exists
- `status entries` -- partial; inbound status update mapping exists
- `widgets` -- partial; inbound widget update mapping and outbound widget clear requests exist
- `title changes` -- partial; inbound title change mapping exists
- `editor text updates` -- not implemented yet as an explicit protocol event
- `session switching/forking/stats/export` -- partial; session switch/fork events are seeded, stats/export are not yet modeled

The important note is that this is protocol and adapter groundwork, not end-to-end parity yet.

## Current strengths

- tests and strict Clippy are green across the workspace
- the protocol is no longer hand-wavy; there is a real typed transport shape
- the adapter seam is modular and testable without requiring Pi to be installed for unit tests
- the CLI now has a basic command/result/presentation split and can intentionally emit command results

## Current gaps

- the smoke path is transport-successful, but it still has not validated richer session/resource compatibility against the PRD
- command results are still line-oriented diagnostics rather than a polished CLI UX
- no documentation of exact wire semantics existed before this checkpoint
- no PRD progress note existed before this checkpoint

## Suggested stopping point outcome

This is a reasonable first validation pause if the goal is to confirm that the repository is no longer just scaffolding:

- the project has a credible M0 foundation
- the project has early M1 adapter reality, not just stubs
- the code structure still matches the PRD's architecture intent

Before moving much further, the next useful validation step should be:

1. extend the smoke path beyond bootstrap so it validates more of the documented RPC command set
2. continue aligning parser/serializer behavior with the observed and documented type-based event stream
3. decide which Yach-level handshake concepts belong only in `yach-proto` versus which can be projected onto stock Pi RPC without inventing unsupported commands
