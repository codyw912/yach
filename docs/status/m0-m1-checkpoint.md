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
- the CLI now has a basic command/result/presentation split, which gives us a better place to evolve smoke checks and future real output

## Current gaps

- command results are internally renderable but not intentionally displayed yet
- no end-to-end real Pi session reporting path exists yet
- no documentation of exact wire semantics existed before this checkpoint
- no PRD progress note existed before this checkpoint

## Suggested stopping point outcome

This is a reasonable first validation pause if the goal is to confirm that the repository is no longer just scaffolding:

- the project has a credible M0 foundation
- the project has early M1 adapter reality, not just stubs
- the code structure still matches the PRD's architecture intent

Before moving much further, the next useful validation step should be:

1. make the CLI surface command results intentionally
2. run a real Pi RPC smoke path and capture what actually works
3. update this checkpoint with empirical notes instead of only code-shape notes
