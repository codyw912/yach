# Stdio RPC Mode Implementation Plan

Status: ready — design accepted
(`docs/superpowers/specs/2026-08-18-headless-protocol-boundary-design.md`)
Date: 2026-08-18

## Wire contract

- `yach rpc [--project-root <dir>] [--session <id> | --session-path <file>]`
  runs the backend loop and serves the protocol over stdio.
- stdin: one `ClientEvent` JSON object per LF line (`ClientEvent::from_jsonl`).
- stdout: one `ServerEvent` JSON object per LF line (`ServerEvent::to_jsonl`).
  Nothing else ever writes to stdout; diagnostics go to stderr.
- `BackendEvent::Connected`/`Disconnected` are in-process session plumbing
  and do not cross the wire. Negotiation happens in-protocol: the client
  sends `Initialize(handshake)`; the server answers with
  `ServerEvent::Ready { handshake }` (existing semantics). The Ready
  frame is the readiness signal (omp precedent).
- Malformed input lines are recoverable: emit a
  `ServerEvent::StatusUpdated` error frame on stdout (or a dedicated
  error event if review prefers), skip the line, continue (omp
  precedent; never abort the process on client typos).
- Secrets: `DialogResolved{Secret}` is accepted on stdin (owner ruling);
  any session recording uses `to_record_jsonl` and keeps the exclusion.
- Lifecycle: stdin EOF → graceful shutdown (cancel active turn, flush
  stdout, exit 0). Kill remains the hard fallback. No transport flags in
  stage 1; a future `--listen` follows the accepted stage-2 boundary.

## Slices

1. **`yach rpc` subcommand.** New `rpc.rs` module in `yach-cli` beside
   `headless.rs`: arg parsing (share session/project-root resolution
   with `run`), spawn `run_native_loop` via `start_backend_session`
   exactly as the TUI does, then two pumps: stdin lines →
   `ClientEvent::from_jsonl` → `client_tx`; `backend_rx` →
   `BackendEvent::Server(event)` → stdout. Unit tests: arg parsing,
   malformed-line recovery, EOF shutdown, stdout purity (no non-JSONL
   bytes). Update `--help` and README's command list.
2. **Matrix harness.** Integration-test helper in `yach-cli` tests:
   spawn the built `yach` binary in `rpc` mode against a temp project
   root + fixture backend runtime (reuse the task-7 fixture patterns,
   with readiness = the Ready frame, not wall-clock), plus a scenario
   runner: `Vec<ClientEvent>` in, assertions over the collected
   `ServerEvent` stream / session log / project files. Deadlines
   generous and readiness-gated (Test-reliability lesson).
3. **First scenarios.** (a) remove-last-connection: connect fixture
   runtime → activate → remove active connection → assert honest
   unconfigured prompt failure + `Provider Not Configured` model change
   → reconnect recovers; (b) review deny for bash and edit → `! denied`
   metadata (`outcome_kind`) on the wire; (c) cancel mid-stream →
   `PromptFinished{Cancelled}`; (d) resume parity: replay a session
   log, assert hydrated `SessionMessagesUpdated` matches live shaping;
   (e) capability drift guard: Ready handshake capabilities are
   asserted exactly, so surface changes are deliberate.
4. **Docs.** `docs/protocol/yach-proto-v0.md` gains the transport
   section (framing, lifecycle, secrets, recoverability); board/next
   updates.

## Acceptance

- `printf '{"type":"initialize",...}\n' | yach rpc` round-trips a Ready
  frame; a scripted fixture prompt completes end to end from another
  process with no TTY.
- The five scenario families run in `cargo test` and CI, deterministic,
  readiness-gated.
- No new provider-visible behavior; session log formats unchanged; the
  TUI path is untouched (still in-process channels).

## Non-goals

Socket/remote listeners, daemon lifecycle, auth, SDK packaging, TUI
migration, MCP serving, extension-host protocol changes.
