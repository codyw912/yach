# Session Diagnostics Design

**Outcome:** plane:YACH-15

Status: accepted in conversation 2026-09-30; written for owner review.

## Problem and outcome

M1 needs real dogfood sessions to produce evidence that explains failures. The
session JSONL log today cannot do that:

- No event carries a time. A log orders events but cannot say when anything
  happened or how long it took.
- The only duration is the per-prompt `prompt_total` metric. Tool executions
  have no duration.
- Provider attempts are invisible. Retries, delays, and per-attempt failures
  are display-only status; a failed turn persists only a terminal reason such
  as `provider_error kind=provider_internal`, which also covers errors the
  adapter could not classify (`rig_adapter.rs` fallback).
- Nothing records what Yach actually sent to the model, so "why did the model
  do that" questions cannot be answered after the fact.
- There is no inspector. Reading a session means reading raw JSONL.

A real host session from 2026-09-22 shows the gap: one turn ran 6.9 s and ended
`provider_error kind=provider_internal` with nothing else recorded.

Outcome: a dogfood session can be diagnosed from its durable evidence alone:
when each event happened, how long each tool call and provider attempt took,
why an attempt failed, and, with capture enabled, exactly what Yach asked the
model.

## Relationship to existing designs

- **Provider attempt reliability (2026-08-28).** That design keeps raw bodies,
  headers, endpoints, credentials, and unbounded messages out of protocol and
  session evidence, and keeps provider request IDs out of status output. This
  design keeps every one of those rules except one: a bounded, sanitized
  provider request ID may enter **session JSONL** (not status, not protocol).
  Rationale: the session log is already a private `0600` file that stores exact
  tool arguments and results; the request ID is the only join key to gateway
  and provider-side logs; the value is bounded and charset-restricted so it
  cannot smuggle payload. Opt-in request capture writes raw request bodies,
  but only to a separate private directory the user names, never to session
  JSONL, protocol, or status. Failed attempt prefixes and reset events remain
  display-only; attempt events record facts about an attempt, never its text.
  The vendored Rig patch does not grow: header and body access comes from a
  Yach-owned HTTP transport.
- **Performance measurement framework (2026-09-08).** `YACH_TRACE` lifecycle
  marks remain the tool for sub-millisecond phase attribution. They are opt-in,
  label-only, and separate from session content, so this design does not build
  on them.
- **Headless protocol boundary (2026-08-18).** `yach-proto` does not mirror
  `SessionEvent`; nothing here changes the wire protocol.

## Design

### Event timestamps

Every JSONL line Yach writes for a session gains a top-level `at_ms` (Unix
milliseconds) beside `type`.

- Stamped at serialization, not on the 22 `SessionEvent` variants. Adding a
  field per variant would touch roughly three hundred construction sites and
  every equality assertion in tests for no gain in precision. Both writers,
  the append store (`JsonlSessionStore::write_events`) and the rewrite path
  (`SessionLog::write_to_file`), share one stamped-line encoder. It reads the
  clock once per write call; lines written together share that value, and a
  rewrite stamps rewrite time.
- `at_ms` is therefore **write time**. The runner flushes pending events after
  each tool batch, provider round, and terminal, and the provider sink defers
  tool request and finish events until terminal enrichment, so write time is
  batch-granular. Events whose timing matters carry their own measured timing
  instead: `ToolExecutionFinished` and `ProviderAttemptFinished` record
  `started_at_ms` and `duration_ms` at the source (below). The inspector
  prefers explicit timing and falls back to `at_ms`. Per-event stamping at
  push time is a follow-up only if batch skew proves to matter.
- Wall clock, not monotonic: it must correlate with gateway logs and across
  process restarts. Durations are measured with `Instant` and never derived by
  subtracting timestamps.
- `SessionEvent` itself does not gain the field. The encoder serializes a
  borrowed `{ at_ms, #[serde(flatten)] event }`; the normal loader keeps
  deserializing bare `SessionEvent` (an unknown top-level key is ignored on
  internally tagged struct variants), and the inspector deserializes
  `StampedSessionEvent` to recover `at_ms`. Old logs load with no `at_ms`.

### Tool durations

`ToolExecutionFinished` gains `started_at_ms: Option<u64>` and
`duration_ms: Option<u64>` (`serde(default)`, skipped when `None`). The clock
starts immediately before dispatch to the builtin handler, the extension
executor, or the shell spawn, after any approval or review decision, and
stops when dispatch returns. Timing covers execution only: validation,
permission, and review waits are separate events and would otherwise inflate
tool time.

Once dispatch starts, every exit carries the timing: success, error, result
rejected as too large after execution, host timeout, and cancellation. The
bash cancellation branch records its finish event with elapsed timing before
returning, so the batch fallback (`record_missing_provider_tool_batch_events`)
finds it and keeps it; the fallback only creates a timing-less event for a
tool that never started. Construction sites that never dispatch (validation
failure, denial, review rejection) set both to `None`.

### Provider attempt events

A new variant, one per started provider attempt, recorded when the attempt
settles:

```text
ProviderAttemptFinished {
    session_id, turn_id,
    purpose: turn | compaction_summary | compaction_native
    attempt_sequence: u64             # existing turn-wide counter (key)
    retry_index: u8                   # 0 first try, 1-2 retries of one request
    outcome: succeeded | partial | failed | cancelled
    error_kind: Option<ProviderErrorKind>
    classification_source: Option<ClassificationSource>
    error_variant: Option<String>     # bounded Rig error variant label
    status_code: Option<u16>
    provider_code: Option<String>     # existing bounded typed code
    timeout_phase: Option<TimeoutPhase>
    retry_after_ms: Option<u64>       # parsed provider advice
    next_delay_ms: Option<u64>        # delay Yach chose before the next attempt
    started_at_ms: u64                # wall clock at request start
    duration_ms: u64                  # request start to settle (Instant)
    first_event_ms: Option<u64>       # request start to first stream event
    provider_request_id: Option<String>
    model: String                     # model id the attempt targeted
    capture: Option<String>           # capture file name, when enabled
}
```

- **Identity.** A tool-loop turn issues several provider requests, and each
  request can retry. `attempt_sequence` is the existing turn-wide counter
  (`ProviderRetryContext::attempt_sequence`, already surfaced as
  `PromptAttemptReset.attempt_sequence`) and uniquely keys an attempt within a
  turn. `retry_index` says whether the attempt was a retry of the same request.
  Manual compaction reserves a fresh turn ID and its own counter; once an
  attempt event under that turn ID is written (below), a restart does not
  reuse the turn. `create_new` capture files guard the case where that write
  failed.
- **Start and settle.** An attempt starts immediately before the requester is
  invoked; a cancellation or reset-delivery failure before that point starts
  no attempt and records nothing. Every started attempt records exactly one
  event before any return or retry wait, on every path: a complete stream, a
  partial stream, a direct `Err` from the requester (adapter preparation,
  authentication, setup), and cancellation during the attempt.
- **Outcome.** A `Complete` collection is not proof of success.
  `succeeded` requires a `Completed` stream event. A `Failed` stream event
  gives `failed` with that error's fields; a `Cancelled` stream event or
  cancellation during the attempt gives `cancelled`; a stream that ends
  without completion gives `failed` with `malformed_stream`. A stream that
  ended with an error after a complete tool round that Yach then used is
  `partial` and keeps the error's fields, so the inspector never shows a
  `succeeded` row carrying an error. Retry decisions do not change.
- **Where.** Turn and portable-summary compaction attempts are recorded in
  `provider_request_with_retry_context`, which owns their retries;
  `ProviderRetryContext` gains the purpose and an attempt sink that records
  each attempt as it settles. Native compaction
  (`/responses/compact`) goes through the `Compactor` seam with its own HTTP
  client and no retries; the runner records its single attempt around
  `compactor.compact`, using the sequence it already reserves. Native
  outcomes map `Timeout` to `timeout`, `Transport` to `network`,
  `HttpStatus` to the existing status classification with `status_code`, and
  `Decode` or unreplayable output to `malformed_stream`; the
  unsupported-provider and missing-request results never reach HTTP and
  record nothing. Callers that pass no sink (unit tests, benchmarks) record
  nothing.
- **Persistence.** With a session store, attempt events are appended to the
  store as soon as they settle, following the existing direct-append pattern
  (`let _ = store.append_event(&event)`), and pushed to the in-memory log;
  they are never held in the pending batch. So they survive exits that
  discard pending events, including a failed or unusable manual compaction
  and a rolled-back checkpoint write. A failed append is ignored like other
  direct appends; it never changes the turn. Without a store they join the
  in-memory log and pending batch like any other event.
  File order between attempt events and batched events is not guaranteed;
  the inspector orders a turn's items by their timing.
- **Classification.** Unchanged, as is retry behavior. The event carries the
  existing `classification_source`. The ladder in `error_dialect.rs` sets it to
  `status`, `typed_dialect`, or `keyword` when those match; otherwise it stays
  `variant` and the kind comes from the Rig error variant. So
  `classification_source = variant` with `error_kind = provider_internal` marks
  the unexplained fallback. `ProviderErrorMetadata` skips serializing the
  `variant` default, so the attempt event stores the source explicitly and the
  inspector renders an absent value as `variant`. `error_variant` is the
  bounded variant label already produced for `redacted_debug`.

### Recording transport

Rig's builders accept any `HttpClientExt` implementation
(`ClientBuilder::http_client`). Yach adds `RecordingHttpClient`, a small
wrapper around `reqwest::Client` in `yach-backend`, and uses it for the
OpenAI-compatible, OpenAI, and ChatGPT subscription clients. It is
`Clone + Default + Debug`, as Rig's completion models require; the default
records nothing. Per attempt it:

- records the response's `x-request-id`, falling back to `request-id`, on both
  success and error responses. At most 128 bytes, ASCII alphanumerics and
  `-_.:` only; anything else drops the whole value;
- when capture is enabled, writes the exact request body bytes about to be
  sent (below);
- otherwise sends unchanged. On a non-success status it reads the body chunk
  by chunk, keeping at most `ERROR_BODY_MAX_BYTES` and dropping the response
  as soon as that bound is exceeded, as Rig's reqwest path does, then builds
  the error from that already-bounded body with Rig's public
  `BoundedErrorBody` and `retry_after_from_headers`. Status, bounded error
  body, and `Retry-After` match Rig's reqwest implementation; a body read
  error yields an empty message, as in Rig. An unbounded `bytes()` read is
  never used on error responses.

The vendored Rig patch does not change. Capture sees the true wire request
rather than a reconstruction of Yach's `ProviderRequest`, on success and
error paths alike. ChatGPT OAuth token refresh and device flow build their
own `reqwest::Client` inside Rig (`chatgpt/auth/native.rs`), so token traffic
never passes through the wrapper and cannot be captured. The Anthropic client
is left on plain reqwest (out of scope).

The native compactor, which builds its own `reqwest::Client`, uses the same
per-attempt recorder for request ID and capture. The recorder travels from the
adapter, or the compaction call site, to the attempt event, never through
`ProviderErrorMetadata`, so status output and `ProviderError` are unchanged.
`ProviderErrorMetadata` gains only `error_variant`, the bounded Rig variant
label, skipped when serializing so existing metadata output is unchanged.

### Unknown-variant tolerance

`SessionEvent` gains a catch-all `#[serde(other)] Unknown` variant. Loads that
meet a future event type skip it silently instead of emitting
`SessionLoadWarning::InvalidJson`; genuinely malformed lines still warn.
`Unknown` is never written: the stamped-line encoder skips it. Readers built
before this change still warn on `provider_attempt_finished`; that one-time
cost is accepted.

Every exhaustive `SessionEvent` match (session, compaction, session-state
projection, runner) gets explicit arms. Attempt events and `Unknown` are
non-transcript and contribute zero to token estimates. Attempt events count
toward `next_turn_index`, so a reserved turn stays reserved, but not toward
the turn-scoped context filter.

### Opt-in request capture

`YACH_CAPTURE_REQUESTS=<absolute dir>` enables capture; unset means off. For
each attempt whose completion request body reaches a recording transport, it
writes the exact body:

```text
<dir>/<session-id>/<turn-id>-<purpose>-<attempt_sequence>.json
```

- `purpose` plus `attempt_sequence` is unique within a turn, so tool rounds,
  retries, and compaction never overwrite each other. Files open with
  `create_new`; a collision records `capture = None` for that attempt, never
  overwrites, and does not disable later capture. Credentials live in
  headers, which are not captured.
- Directories `0700`, files `0600`. Any other capture failure disables
  capture for the process and warns once on stderr; it never fails a turn.
- The attempt event's `capture` field names the file so the inspector can
  link to it.
- Never written to session JSONL, protocol frames, or status. Relative paths
  are rejected, matching `YACH_SESSION_DIR`.

### `yach sessions` command

Read-only; added to the hand-written parser in `yach-cli/src/main.rs`, its
dispatch, and `yach --help`.

```text
yach sessions list [--json]
yach sessions show <session-id|latest> [--json]
```

- Project resolution reuses `project_session_log_dir`, so `YACH_SESSION_DIR`
  and the project key behave exactly as for the TUI and `yach run`.
- `list`: newest first by mtime: id, start time (first `at_ms`, else mtime),
  turn count, last turn outcome, model.
- `show`: per turn, grouped by turn ID in first-appearance order. Within a
  turn, items are sorted by their timing (explicit `started_at_ms` where
  present, else the line's `at_ms`), keeping file order for equal times;
  items with no timing keep their file position relative to the preceding
  timed item. Offsets are measured from the turn's earliest known timing.
  Shows the user prompt first line (bounded); each provider attempt with
  purpose, sequence, retry index, outcome, duration, first-event time,
  status, error kind and source (absent rendered as `variant`), request ID,
  delay, capture file; each tool with name, outcome, duration, bounded
  argument summary; approval and review decisions; compaction checkpoints;
  terminal outcome and reason; provider usage. Logs without any timing render
  in file order without offsets.
- `--json` emits the same model as one JSON document for scripts.
- Load warnings are reported, not fatal.

## Error handling

- Clock before epoch: the line is written without `at_ms`; explicit
  `started_at_ms` falls back to `0` and the inspector treats `0` as unknown.
- Header values that fail bounds or charset are dropped, not truncated.
- The inspector never mutates or locks session files.
- Capture and inspector failures never change turn outcomes.

## Testing

- Encoder: both writers stamp every line and skip `Unknown`;
  `SessionLog::load_from_file` still loads stamped lines; `StampedSessionEvent`
  loads both stamped and pre-change lines.
- Round-trip: new `ToolExecutionFinished` fields and `ProviderAttemptFinished`
  serialize and load; a line with an unknown `type` loads silently; a
  malformed line still warns; a pre-change fixture loads with `None` fields.
- Scripted provider: success first try; retry then success; three failures;
  a direct requester `Err`; a `Complete` collection holding a `Failed` event;
  a stream ending without completion; cancellation during an attempt;
  cancellation during the retry delay; a two-round tool turn. Each asserts one
  event per started attempt, distinct `attempt_sequence` values,
  `retry_index`, outcomes, delays, and that no attempt text reaches the log.
- Compaction: a portable-summary run records `compaction_summary`; native
  success, HTTP failure, and cancellation each record one
  `compaction_native` event; a failed manual compaction leaves its attempt
  in the store, and after reload the next turn index skips its turn.
- Recording transport, against a local test server: request ID from success
  and error responses; bounds and charset rejection; error status, body, and
  `Retry-After` identical to Rig's reqwest path; capture writes the exact body.
- Tools: builtin, extension, and bash paths record `started_at_ms` and
  `duration_ms`; a timed-out extension and a cancelled running bash command
  still record them; bash timing excludes the approval wait.
- Capture: file mode, layout, `create_new` collision, disable-on-failure,
  relative-path rejection, and absence from session JSONL.
- CLI: `sessions list` and `sessions show --json` against a fixture log.
- Performance: `just perf` deterministic rows (allocation counts, binary size)
  stay within budget; per-write cost is one clock read and a small struct.

## Acceptance

1. Every newly written session line carries `at_ms`; pre-change logs load
   unchanged.
2. Builtin, extension, and bash tool executions record `started_at_ms` and
   `duration_ms`, including when cancelled.
3. Every started provider attempt, including portable-summary and native
   compaction attempts, produces exactly one `ProviderAttemptFinished`,
   written to the store as it settles, even when the turn or compaction
   fails (a failed append is best-effort, like other direct appends).
   On a recording transport, a valid request ID is kept whenever the
   provider sends one.
4. With `YACH_CAPTURE_REQUESTS` set and capture healthy, each attempt whose
   completion request reaches a recording transport writes one private file;
   nothing is written when unset.
5. `yach sessions list` and `show` render a real session, including attempt
   and tool timings.
6. One real session against a current-generation `openai-codex` model through
   the host gateway (OpenAI-compatible chat path) is inspected with
   `yach sessions show`, and the result is recorded in `docs/project/records/`.

## Out of scope

- Anthropic through the host gateway. Its `/v1/messages` route rejects Rig's
  `x-api-key` header and may need client-side request shaping; that needs its
  own exploration before any client change.
- Streaming token-level timing beyond first event.
- OpenTelemetry or any export pipeline.
- Changes to retry policy or error classification outcomes.
- Session replay or re-execution.

## Sources

- `crates/yach-backend/src/session.rs` — `SessionEvent`, load warnings,
  `SessionLog::write_to_file`, `next_turn_index`.
- `crates/yach-backend/src/session_store.rs` — `write_events`, the append
  store.
- `crates/yach-backend/src/runner.rs` — `provider_request_with_retry_context`,
  `ProviderRetryContext::attempt_sequence`, compaction runs, tool dispatch,
  bash cancellation, `record_missing_provider_tool_batch_events`,
  `project_session_log_dir`.
- `crates/yach-backend/src/compaction.rs` — `Compactor`, native
  `/responses/compact` client, `turn_scoped_event_turn_id`.
- `crates/yach-backend/src/rig_adapter.rs` — provider client construction and
  attempt stream collection.
- `crates/yach-backend/src/error_dialect.rs` — classification ladder and
  variant label.
- `vendor/rig-core/src/http_client/mod.rs` — `HttpClientExt`,
  `error_from_response`, `BoundedErrorBody`, `retry_after_from_headers`.
- `vendor/rig-core/src/client/mod.rs` — `ClientBuilder::http_client`.
- `docs/project/specs/2026-08-28-provider-attempt-reliability-design.md` —
  evidence exclusions revised above.
