# yach M0-M1 checkpoint

This document captures the current implementation state against `PRD-v0.1.md` so we have a deliberate pause point before going deeper into runtime and TUI work.

## Scope of this checkpoint

This checkpoint focuses on:

- M0 bootstrap
- M1 stock Pi RPC adapter completion
- the CLI/protocol seams that exercise those layers

It does not claim progress on TUI, performance validation, resource compatibility, or session-file compatibility yet.

## Milestone status

### M0 -- bootstrap

Status: complete

- `repo created` -- done
- `Cargo workspace created` -- done via `Cargo.toml` and the phase-1 crate layout under `crates/`
- `yach-proto v0 spec` -- done, with typed transport envelope, capability negotiation, and client/server events
- `adapter capability model defined` -- done in `crates/yach-proto/src/lib.rs` and `crates/yach-adapter-pi-rpc/src/capabilities.rs`
- `baseline benchmark harness skeleton` -- done at skeleton level via `crates/yach-bench`

Assessment:

M0 is established. The protocol is no longer hand-wavy; there is a real typed transport shape with JSONL helpers, request/stream correlation metadata, and capability negotiation.

### M1 -- stock Pi RPC adapter

Status: complete

- `spawn/connect to pi --mode rpc` -- done; `PiRpcSession::spawn` exists and works
- `stream transcript` -- done; prompt delta parsing, transcript accumulator, and live streaming loop exist
- `send prompts` -- done; `PiRpcSession::submit_prompt` sends user prompts through the typed serialization layer
- `handle Tier A dialogs and fire-and-forget UI` -- done; dialog dispatch, resolution, and round-trip exist for select/confirm/input/editor
- `basic session/model controls` -- done; session fork, model selection, stats queries all exercised through smoke and interactive paths

Assessment:

M1 is complete. The CLI now has a `run` command that spawns Pi, initializes the RPC session, runs an interactive readline loop, streams transcript deltas to stdout, handles dialog requests interactively, and accumulates a typed transcript. The adapter seam is fully validated end-to-end.

### Empirical note on the current smoke path

The CLI now intentionally emits command results.

Observed on this machine:

- `cargo run -p yach-cli -- print-capabilities` -- works and prints the current stock RPC capability set
- `cargo run -p yach-cli -- smoke-pi-rpc` -- runs and reports structured success for a broader documented smoke sequence
- `cargo run -p yach-cli -- run` -- starts an interactive session with prompt streaming and dialog handling
- `pi` is installed and discoverable at `/run/current-system/sw/bin/pi`
- direct probing and the installed RPC type definitions show that stock Pi RPC is type-driven, not method-driven
- the documented command shape lives in `dist/modes/rpc/rpc-types.d.ts`, where commands are JSON objects with a `type` field
- there is no documented `initialize` command in stock RPC; the available commands include `prompt`, `get_state`, `set_model`, `switch_session`, `fork`, `get_session_stats`, and others
- Pi returns `{"type":"response","success":false,"error":"Unknown command: undefined"}` when sent the old method-based shape, which confirmed our previous bootstrap assumption was wrong
- direct probing with the documented type-based shape succeeds and yields real RPC events, including `response`, `agent_start`, `turn_start`, `message_start`, `message_end`, `turn_end`, and `agent_end`
- the current smoke result now reports initialization success because Yach treats stock RPC bootstrap as a documented `get_state` probe rather than inventing an unsupported `initialize` command
- the smoke command also successfully exercises additional documented stock RPC commands on this machine: `get_state`, `set_model`, `fork`, `get_session_stats`, and `get_messages`

Assessment:

This is useful progress because the adapter now follows documented stock RPC behavior instead of guessing. For stock Pi RPC, bootstrap should be treated as capability-assumed plus a documented state query, not as a separate negotiated initialize handshake.

## Architecture validation against the PRD

### What is already matching the PRD well

- `yach-ui` does not speak Pi RPC directly; the shared seam goes through `yach-proto`
- the workspace shape matches the intended early crate layout from section 5.3 of the PRD
- the adapter strategy is reflected in code structure: `yach-adapter-pi-rpc` exists now and `yach-adapter-pi-sdk` exists as a seed for later work
- capability negotiation is present and tested
- a Yach-owned transport envelope exists, with JSONL helpers and request/stream correlation metadata
- the RPC adapter is split into capability, parse, serialize, session, and dispatch layers, which is consistent with the PRD emphasis on process boundaries and cleaner architecture
- the CLI `run` command validates the full adapter contract before TUI work begins

### What remains intentionally incomplete

- no fullscreen TUI yet (M2)
- no settings/package/resource loading compatibility (M3)
- no real session file compatibility work (M3)
- no rich SDK sidecar behavior (M4)
- no benchmark harness implementation beyond crate scaffolding

## Tier A compatibility snapshot

Current state of the stock RPC surface from PRD section 6.4:

- `prompt streaming` -- done; transcript accumulator collects deltas, CLI streams to stdout
- `dialogs: select, confirm, input, editor` -- done; dispatch, resolution, and round-trip all work
- `notifications` -- done; mapped to dispatch actions
- `status entries` -- done; mapped to dispatch actions with stream completion detection
- `widgets` -- done; mapped to dispatch actions
- `title changes` -- done; mapped to dispatch actions
- `editor text updates` -- not implemented yet as an explicit protocol event
- `session switching/forking/stats/export` -- partial; session switch/fork events are seeded, stats/export are not yet modeled

## Current strengths

- tests and strict Clippy are green across the workspace (59 tests, 0 warnings)
- the protocol is no longer hand-wavy; there is a real typed transport shape
- the adapter seam is modular and testable without requiring Pi to be installed for unit tests
- the CLI now has a full interactive session runner (`yach-cli run`) that validates the adapter end-to-end
- transcript accumulation and event dispatch are cleanly separated from I/O, making TUI replacement straightforward

## Current gaps

- command results are still line-oriented diagnostics rather than a polished CLI UX
- no settings/package/resource loading compatibility
- no real session file compatibility work
- no benchmark comparison against Pi

## Suggested stopping point outcome

M0 and M1 are now complete. The project has:

- a credible M0 foundation with protocol spec and capability model
- a fully working M1 adapter with interactive session validation
- code structure that matches the PRD's architecture intent
- clean separation between adapter logic and I/O layer, ready for TUI work

The next useful step is M2: fullscreen TUI alpha with transcript/tool panes, input composer, and slash completion.
