# Headless Protocol Boundary Design

Status: accepted 2026-08-18 — all forks owner-decided
Date: 2026-08-18
Motivation: owner decision 2026-08-18 — decide the client/server posture
before further major work; a protocol-driven invariant matrix should
catch regressions that unit tests structurally miss (e.g. the
remove-last-connection trap: reducer tests passed while the composed
runner behavior was broken).

## Current State (evidence)

- The TUI speaks only `ClientEvent`/`ServerEvent` through `yach-proto`
  0.1.0 with capability negotiation; it never touches backend internals.
- Both event directions are JSONL-serializable (`to_jsonl`/`from_jsonl`);
  the only record exclusion is `DialogResolved { Secret }`
  (`to_record_jsonl` omits it deliberately).
- The seam is in-process: `start_backend_session` hands both sides mpsc
  channels inside one process.
- `yach run` is headless but narrow: `--prompt`/`--script` accepts
  prompt lines only, with `--full-auto`; dialogs, reviews, connections,
  model activation, and extension lifecycle are unreachable headlessly.
- Reference: `docs/protocol/yach-proto-v0.md`.

## Cohort (source-verified 2026-08-18)

- Codex (current main; `proto` was deleted 2025-09-30, openai/codex
  #4520): `codex app-server` — bidirectional JSON-RPC 2.0, newline-
  delimited JSON over stdio by default, experimental `ws://` and
  `unix://` (websocket over HTTP upgrade) via one `--listen`
  abstraction. Broad UI-parity surface: threads/turns/items, streaming
  notifications, server→client approval requests, auth endpoints. The
  bundled TUI and `exec` are themselves clients of the same app-server
  semantics through an in-process typed-channel client
  (`codex-app-server-client`); JSON serialization happens only at
  external transport boundaries. `codex exec --json` is a one-shot
  JSONL event projection. An MCP server exists as experimental.
- pi (badlogic/pi-mono main): `pi --mode rpc` — strict LF JSONL over
  stdin/stdout; commands with optional id correlation, streamed agent
  events; surface covers prompt/steer/abort/state/messages/models/
  login/session ops. TS embedders are pointed at `AgentSession`
  directly.
- omp: `omp --mode rpc` — NDJSON over stdio with a versioned ready
  frame (protocol v1, supported [1,2]); `--mode rpc-ui` adds
  host-answered UI request frames (dialogs, tool cards). Broad surface
  including login, compaction, bash, subagent frames.
- opencode 2: TCP HTTP server (+SSE event routes, websocket tracking),
  generated OpenAPI and an SDK package; auth middleware. TCP-only —
  no stdio or unix-socket transport.
- Claude Code: headless `-p` and an SDK; closed source, documentation
  evidence only.

Weighted reading: three of the four weighted harnesses (Codex, pi,
omp) are stdio-JSONL-first; opencode alone is HTTP-first. Codex is the
strongest precedent for yach's exact shape: one protocol semantics,
an in-process typed client for the bundled UI, external transports
(stdio now, sockets later) layered on the same surface — which is
yach's current architecture plus a transport. Codex's `--listen`
abstraction is also evidence that stage 2 can be additive transports
over the same protocol rather than a redesign.

## Direction (proposed)

Stage 1 — **stdio RPC mode**: a child process serving the full
`ClientEvent`/`ServerEvent` surface over stdin/stdout JSONL. Rationale:

- The wire format already exists; this is exposure, not invention.
- Trust model is process ownership: whoever spawned the child already
  has the user's authority. No authentication, session brokering, or
  reconnect semantics are needed.
- Secrets: the direct wire uses `to_jsonl` (not `to_record_jsonl`), so
  `DialogResolved{Secret}` flows to the child exactly as the in-process
  channel does today; pipes are not persisted. Recording/replay of a
  proto session uses `to_record_jsonl` and keeps the existing
  exclusion.
- Lifecycle: child exits when stdin closes; cancellation is an event
  (`PromptCancelled`) plus process kill as the hard fallback;
  backpressure is the pipe.

Stage 2 — **local socket/HTTP daemon** stays explicitly out of scope
until a concrete multi-client need exists. It is NOT a transport swap:
it needs authentication/trust, secret transport policy, lifecycle
ownership, concurrency across clients, cancellation/backpressure, and
reconnect semantics. Own design when wanted.

The TUI does not migrate onto the stdio transport in stage 1; it keeps
in-process channels. Both paths share `run_native_loop` and the proto
types, so drift is structural only if one path grows private events —
forbidden: every backend behavior MUST be reachable as protocol events,
which stage 1 makes testable.

## Invariant Matrix

Protocol-driven scenario tests against the stdio server:

- A scenario is a JSONL script of `ClientEvent`s plus expectations over
  the `ServerEvent` stream, the session log, and project-file state.
- Runs on the fixture backend and mock providers (the task-7 style
  local fixture generalized), deterministic, in `cargo test`/CI.
- First scenarios: the connection-removal trap (remove last connection
  → honest unconfigured prompt failure → /connect recovers), review
  approve/deny for edit+bash, cancellation mid-stream, resume parity,
  compaction trigger, capability negotiation drift.
- The eval suite (`evals/`) stays behavioral/provider-facing; the
  matrix is exact-protocol and lives with the workspace tests.

## Not sufficient for

Daemon/multi-client serving, remote transport, SDK packaging, TUI
migration onto the transport, extension-host protocol changes, or
provider-visible behavior changes.

## Owner forks

1. DECIDED (owner, 2026-08-18) — `yach rpc` subcommand, pi-family
   naming. Constraint attached to the ruling: the naming and stage-1
   shape must preserve a future remotely hosted server (remote-backend
   setups). Preserved by construction: the protocol surface is
   transport-independent (Codex's `--listen` shape is the precedent —
   stdio now, socket/remote listeners as additive transports over the
   same events), and nothing in stage 1 may assume shared filesystem
   identity between client and server beyond what the protocol already
   expresses. The remote/daemon design (auth/trust, secret transport,
   reconnect, concurrency) remains stage 2's own spec.
2. DECIDED (owner, 2026-08-18) — Secret events are allowed over the
   stage-1 pipe: process-ownership trust, identical semantics to the
   in-process channel; recordings keep the secret exclusion.
3. DECIDED (owner, 2026-08-18) — The invariant matrix lives in
   workspace integration tests (deterministic, fixture-backed,
   `cargo test`/CI); `evals/` stays behavioral/provider-facing.
4. DECIDED (owner, 2026-08-18) — Sequencing: headless design →
   extension-posture design → Wave 2.
