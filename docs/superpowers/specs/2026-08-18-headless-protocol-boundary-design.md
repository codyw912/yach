# Headless Protocol Boundary Design (Draft)

Status: draft — owner forks marked OPEN below
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

## Cohort

- opencode 2: TUI is a client over an HTTP/SSE server; plugins and
  behavior live server-side. Full UI parity headlessly.
- Codex: `proto` mode — the agent core served over stdio JSONL to any
  client; `exec` for one-shot runs. No daemon by default.
- Pi: stdio RPC mode (yach's original M1 adapter target).
- Claude Code: headless `-p` mode and an SDK; closed source.

Convergent shape: the agent core is a protocol server; the interactive
UI is one client of it. The cohort splits on transport: stdio child
process (Codex, Pi) vs local HTTP daemon (opencode).

## Direction (proposed)

Stage 1 — **stdio protocol mode** (`yach proto`-shaped): a child
process serving the full `ClientEvent`/`ServerEvent` surface over
stdin/stdout JSONL. Rationale:

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

## Open owner forks

1. OPEN — Stage-1 command surface: new `yach proto` subcommand
   (recommended; mirrors cohort naming) vs extending `yach run` with an
   event mode.
2. OPEN — Secret events over the stage-1 pipe: allowed (recommended;
   process-ownership trust, matches in-process semantics) vs excluded
   until the daemon design.
3. OPEN — Matrix placement: workspace integration tests under the CLI
   crate (recommended; deterministic, CI-native) vs a separate
   harness/course under `evals/`.
4. OPEN — Sequencing vs the extension-posture design: headless first
   (owner leaning 2026-08-18), posture second, Wave 2 after both.
