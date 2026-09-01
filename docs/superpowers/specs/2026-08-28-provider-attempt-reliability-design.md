# Provider Attempt Reliability Design

Status: accepted 2026-08-28 — owner decisions incorporated; implementation not started

## Purpose

Yach retries transient provider failures today, but the policy is split across
adapter and runner details:

- the runner retries `timeout`, `network`, `rate_limited`, and
  `provider_internal` errors with fixed one- and five-second sleeps;
- OpenAI Responses can resume from a completed raw-output prefix;
- other providers can restart only before live text reaches a client;
- once a non-prefix provider has streamed text, the turn fails rather than
  regenerate text the client cannot retract;
- provider error classification understands HTTP status and a small shared
  `type`/`code` vocabulary, while the catalog's provider-level
  `error_dialect` ID is not consumed;
- the pinned Rig error surface preserves response status and body but discards
  response headers, so Yach cannot honor the standard `Retry-After` header.

This design replaces that stopgap with one bounded provider-attempt policy. It
owns typed error classification, server-aware delay selection, prefix resume or
restart semantics, and a negotiated prompt-attempt reset event. It does not
move the provider loop or durable evidence out of Yach.

## Owner Decisions

Accepted 2026-08-28:

1. Three total attempts with deterministic one-second then two-second local
   delays. `Retry-After` may extend a wait within the thirty-second cumulative
   budget; Yach fails rather than retry earlier than a hint outside the budget.
2. An `openai-compatible` connection without an explicit baked dialect ID uses
   the conservative generic parser. Provider kind alone does not select the
   OpenAI-compatible typed parser.
3. Protocol v0.3 is a strict cutover. A mismatched `Initialize` is rejected
   before `Ready`; no v0.2 compatibility shim or filtered legacy surface is
   retained.

## Goals

1. Retry only failures categorized as safe to retry.
2. Honor a provider's standard `Retry-After` instruction without unbounded
   waits.
3. Keep prefix-resume output byte-exact and represented once.
4. Allow a negotiated client to retract only the live text from a failed
   restartable attempt, then accept regenerated text.
5. Preserve the current fail-closed rule for clients that cannot reset.
6. Consume the catalog's error-dialect ID through reviewed, compiled-in typed
   parsers.
7. Keep raw provider bodies, headers, credentials, endpoints, and unbounded
   provider messages out of protocol and session evidence.
8. Concentrate attempt policy behind one backend interface rather than grow
   more provider-string branches in `runner.rs`.

## Current Evidence

### Attempt seam

`ProviderRequester::request_attempt_streaming` is the backend seam for one
attempt. The Rig adapter returns `ProviderStreamAttempt::Complete(events)` or
`Partial { events, error, tool_round_complete }`. The collector emits
`Started`, retains every mapped event, forwards text deltas immediately through
`LiveDeltaSink`, and returns a partial attempt on stream error or per-item idle
timeout.

`provider_request_with_retry` currently owns policy. It accepts terminal
complete attempts, converts a completed transient tool round into a synthetic
`Completed(ToolCalls)`, retries four transient error kinds, and sleeps according
to the fixed `[1000, 5000]` millisecond ladder. It retains completed OpenAI raw
output and text/tool events so a native Responses continuation projects once.
For any other provider, a partial attempt restarts the same request; when live
deltas were already sent, the 2026-08-18 owner ruling stops instead because the
wire cannot retract them.

A clean stream EOF without `Completed` currently leaves the attempt layer as
`Complete`; the round collector later rejects it as
`StreamEndedWithoutCompletion`. A provider-emitted `Failed` event follows the
same later failure path. This design does not silently make either case
retryable.

### Evidence ordering

The user entry is durable before the provider task starts. Failed attempt
prefixes and reset events are display-only and are not session events. After a
canonical successful round, Yach persists one full assistant entry and the
turn terminal. A failed or cancelled turn persists only its terminal evidence,
not a partial assistant entry. Resume therefore cannot replay text that a live
client reset.

### Error classification

`completion_error_metadata` can recover a Rig completion error's variant, HTTP
status, and JSON body. It already fixes the concrete OpenAI case where
`type=invalid_request_error` and `code=unsupported_parameter` must classify as
`invalid_request` rather than `unavailable_model`. A shared keyword ladder is
the final fallback.

The catalog contains a provider-level `error_dialect: Option<String>` and an
accessor, but the committed catalog has no IDs and no consumer. `Catalog::insert`
initializes it to `None`; source transforms omit it; `merge_provider` copies
models but not provider metadata. The field is an identifier only. It does not
contain field paths, mappings, regular expressions, or executable parser data.

### Retry-After prerequisite

The pinned vendored Rig `ProviderResponseError` retains only status and body.
The non-success HTTP helper reads the body and discards response headers.
Consequently, standard `Retry-After` is irrecoverable by the Yach adapter today.
The implementation needs a focused vendored/upstream Rig patch that preserves
this one bounded hint or exposes response headers long enough for Yach to
extract it. Re-parsing JSON cannot recover an HTTP header.

## Domain Model

### Provider identity

One attempt receives immutable, secret-free identity:

```text
ProviderIdentity {
    provider,       # configured logical provider label
    model,
    error_dialect,  # typed registry selection
}
```

The configured logical provider is authoritative. In particular,
`openai-compatible` never infers a dialect from a model ID or a models.dev
fallback row. Connection IDs, configuration keys, credentials, endpoints, and
account identifiers are not part of error identity.

### Classified error

`ProviderErrorKind` remains the stable behavioral category. `ProviderError`
gains bounded optional metadata sufficient for policy and redacted diagnostics:

```text
ProviderErrorMetadata {
    status_code?,
    provider_code?,        # finite typed code recognized by a dialect parser
    retry_after_ms?,
    timeout_phase?,        # request_start | first_event | idle_stream
    classification_source # typed_dialect | status | keyword | variant
}
```

The existing safe `message` and `redacted_debug` remain separate from this
metadata. Raw bodies and raw headers never enter `ProviderError`, protocol
frames, debug output, or session JSONL.

`retry_after_ms` is advice extracted at the adapter seam. The retry policy, not
the dialect parser, decides whether and when to retry.

### Attempt continuity

A partial attempt reports one of two continuity modes:

```text
AttemptContinuity {
    ResumePrefix { next_request, retained_events, retained_output },
    Restart,
}
```

`ResumePrefix` means the adapter/request shape can continue from the completed
provider-visible prefix. `Restart` means a new attempt regenerates the response
from the original request. The runner no longer tests `provider == "openai"`;
continuity is derived from the request/adapter capability. Initially only native
OpenAI Responses produces `ResumePrefix`.

### Attempt policy

The policy is bounded and process-local:

- at most three total attempts (two retries), preserving the current ceiling;
- local delay uses deterministic capped exponential backoff: one second, then
  two seconds;
- a valid provider hint is a minimum delay, so the chosen delay is the greater
  of the local delay and `Retry-After`;
- one delay and all cumulative retry delay are capped at thirty seconds;
- if a provider hint exceeds the remaining delay budget, Yach returns the
  categorized provider failure rather than retry earlier than instructed;
- cancellation, authentication, invalid request, context length, unavailable
  model, safety refusal, malformed stream, backpressure, and unknown failures
  are never retried by default;
- timeout, network, rate limit, and provider-internal failures remain retryable;
- a cancellation token is an attempt-executor input; cancellation is checked
  before and during delay, before reset emission, and before every attempt;
- retry policy is not user-configurable in this slice.

The policy's clock and sleeper are injectable for deterministic tests. This
slice does not add jitter: Yach is a local harness, retry count is already
bounded, and randomized scheduling is unnecessary policy and test weight.

### Timeout phase

The existing configured provider timeout remains the duration for this slice,
but errors distinguish where it expired:

- `request_start`: creating the provider stream;
- `first_event`: stream established but no first item arrived;
- `idle_stream`: an established stream stopped producing items.

All three remain `ProviderErrorKind::Timeout`. The phase is diagnostic and
allows a later design to tune separate deadlines without changing retry
semantics now.

## Typed Error-Dialect Registry

### Ownership

The registry lives in `yach-backend`, close to provider error normalization.
`yach-catalog` remains a data crate and supplies only an optional dialect ID.
There is no catalog-to-backend dependency and no executable semantics in remote
or user data.

The backend defines a finite typed registry:

```text
KnownErrorDialect = OpenAi | Anthropic | OpenAiCompatible | ChatGptSubscription
DialectSelection = Known(KnownErrorDialect) | Missing | Unknown
```


An explicit baked dialect ID maps to reviewed parser code. Unknown and missing
IDs use the conservative generic parser and are recorded only as bounded
classification-source diagnostics. An unknown catalog ID never prevents
startup, selection, or a provider request.

In this slice, only the baked repo-reviewed catalog supplies dialect IDs. User,
project, fetched, and environment model layers do not configure parser
selection.

At the CLI adapter-construction boundary, both environment and managed
connection paths read
`baked_catalog().provider_error_dialect(catalog_provider_label)`, call the
backend's typed registry selector, and store the resulting `DialectSelection`
inside immutable `RigProviderAdapterConfig`. `yach-backend` never depends on or
re-queries `yach-catalog`.

`openai-compatible` with no baked ID uses the generic parser. A compiled-in
`OpenAiCompatible` parser is selected only by an explicit baked ID; model IDs,
models.dev fallback rows, and configured provider kind do not imply one.
Other providers likewise need explicit baked IDs before relying on specialized
parsers.

Catalog construction gains an explicit provider-metadata setter.
`merge_provider` deliberately preserves the destination catalog's dialect ID
while merging models; fetched/model-override layers cannot replace it.
Round-trip and merge tests pin this precedence.

### Parser interface

Each compiled-in dialect parses typed, bounded input:

```text
ErrorDialect::classify(ErrorInput {
    status,
    body,
    retry_after_header,
    completion_variant,
}) -> ClassifiedProviderError
```

Known dialect parsers use typed serde envelope structs and finite provider-code
enums. They do not execute catalog-supplied paths, regexes, mappings, templates,
or scripts.

Classification and retry eligibility are separate. A typed dialect may refine
the category, but it cannot make a hard non-retryable HTTP response retryable.
Category selection uses:

1. known typed dialect envelope;
2. generic HTTP status;
3. existing bounded keyword fallback;
4. Rig completion-variant fallback.

Retry eligibility then applies raw-status guards before categorized-kind policy:
401/403 and every 4xx except 408 and 429 are non-retryable regardless of body
classification; 408/504 are timeout candidates and 429 is rate-limited. This
still lets a known code refine a 400 to `context_length` or a 404 to
`unavailable_model` for correct recovery copy without consuming retries.

A malformed, oversized, or unfamiliar body falls through without exposing raw
content. Generic status mapping handles authentication, rate limiting,
timeouts, other 4xx invalid requests, and 5xx provider failures conservatively.
A generic 404 is not automatically called an unavailable model; only a known
dialect code may make that stronger claim.

### User-facing text

This slice does not implement richer provider-message display. Existing stable,
safe status copy remains. The registry returns behavioral categories and
bounded diagnostic evidence; a separate future slice may choose which
provider-authored messages are safe and useful to show.

## Retry-After Extraction

The Rig patch captures only the standard `Retry-After` value before consuming
the body. It does not retain the whole response header map. The captured value
is bounded to 128 bytes and is omitted from `Display`/`Debug`; Yach immediately
parses and drops it. Yach accepts:

- delta-seconds;
- an HTTP date, evaluated against an injected clock.

Invalid, negative, past, non-UTF-8, or overflowing values are ignored. Parsed
values are converted immediately to bounded milliseconds. Provider-specific
body retry hints are not accepted in this slice; they need their own evidence
and policy rather than being smuggled into dialect parsing.

The preferred dependency change is a focused Rig error-surface patch, kept as a
coherent upstreamable vendor patch. Replacing Rig transport, retaining arbitrary
headers, or adding a second Yach HTTP stack is out of scope.

## Attempt Executor

The current retry function becomes a deep backend module with one behavioral
interface. A caller supplies:

- `ProviderRequester`;
- the canonical `ProviderRequest`;
- `ProviderAttemptPolicy`;
- prompt-wide `ProviderAttemptSequence`;
- cancellation token;
- optional `LiveDeltaSink`;
- whether prompt-attempt reset was negotiated;
- the backend event sender.

It returns canonical provider events or one terminal `ProviderError`. It does
not own prompt persistence, tool-loop continuation, compaction, review, or turn
terminal evidence.

The internal state machine is:

```text
Start
  -> AwaitAttempt
  -> CompleteTerminal
     | CompleteToolBoundary
     | RetryableBeforeVisible
     | RetryableResumePrefix
     | RetryableRestartVisible
     | TerminalFailure
```

Rules:

1. A complete attempt with a valid terminal is returned.
2. A completed transient tool round keeps the current synthetic
   `Completed(ToolCalls)` behavior and returns; tool execution may continue the
   turn without regenerating that tool call.
3. A transient failure before live text retries without a reset event.
4. A prefix-resumable transient failure retains canonical prefix events and raw
   output, builds the continuation request, and retries without reset.
5. A restartable transient failure after live text:
   - retries only if prompt-attempt reset was negotiated;
   - emits retry status, then waits using the cancellation-aware sleeper;
   - rechecks cancellation;
   - allocates the next prompt-wide attempt sequence and emits reset immediately
     before invoking the replacement attempt;
   - retries the original request from scratch;
   - discards failed-attempt canonical events.
6. The same failure without negotiated reset preserves today's recoverable turn
   failure.
7. Non-transient failures return immediately.
8. Cancellation never emits reset or starts another attempt.
9. Clean EOF without a terminal and provider-emitted terminal failure preserve
   their current non-retry behavior unless a later design explicitly classifies
   them otherwise.

Prefix-resume merging retains one logical `Started`, one terminal `Completed`,
one provider-visible raw output sequence, and exactly-once text/tool events.
Restart retries discard all canonical events from the failed attempt.

## Prompt-Attempt Reset Protocol

### Capability and event

Protocol v0.3.0 adds:

```text
Capability::PromptAttemptReset

ServerEvent::PromptAttemptReset {
    session_id,
    attempt_sequence,      # nonzero, prompt-wide, monotonically increasing
    discarded_utf8_bytes, # exact live text suffix to retract
}
```

`ProviderAttemptSequence` is created once per active prompt and shared across
every provider round, tool continuation, and attempt-executor call. Every
physical provider attempt takes the next sequence value, including first
attempts; reset names the replacement attempt's value. Clients reject zero,
duplicate, or decreasing values.

Protocol v0.3 is a clean breaking cutover, not wire-compatible with v0.2's
closed `Capability` enum. Initialize handling compares protocol versions before
ordinary capability negotiation. A mismatch emits one existing
`ServerEvent::StatusUpdated` with bounded `protocol version mismatch` copy,
flushes, and closes without `Ready`. It uses no v0.3-only enum variant, so the
older peer can parse the rejection. `Ready` is emitted only to a
version-compatible peer and carries the negotiated/filterable capability set,
not an unfiltered backend handshake. Within v0.3, a peer that omits
`PromptAttemptReset` retains the current fail-after-visible-prefix behavior and
never receives reset.

The current protocol permits one active provider turn and keys prompt lifecycle
by session ID. Reset follows that invariant rather than introducing a reset-only
turn ID that no existing `PromptDelta` or `PromptFinished` can correlate. A
future multiplexed-prompt protocol must add one prompt identity consistently to
the whole lifecycle, not only to reset.

`discarded_utf8_bytes` is the sum of successfully forwarded UTF-8 delta byte
lengths from the failed attempt's baseline. It is more precise than clearing the
whole assistant response: completed text from earlier tool rounds remains. The
backend emits reset immediately before the replacement attempt; replacement
deltas follow it on the same channel.

The event controls live presentation only:

- it is not a session event;
- it does not interrupt reviews;
- it does not clear tool rows, tool output, the user message, completed prior
  rounds, status history, or hydrated transcript entries;
- it does not change prompt cancellation or terminal outcome.

### TUI

The TUI tracks the latest prompt-wide sequence in `StreamState::Streaming`. On a
matching reset it truncates exactly `discarded_utf8_bytes` from the contiguous
assistant-text suffix produced by the failed attempt, stays in streaming state,
and keeps the viewport at the bottom when it was already there.

The transcript operation validates UTF-8 boundaries and fails closed. An
impossible byte count, non-boundary, zero/decreasing sequence, or non-assistant
row before the requested suffix does not delete content. Instead the client:

1. preserves existing transcript rows;
2. marks the active prompt `Desynchronized`;
3. sends `PromptCancelled` once;
4. ignores subsequent deltas and resets for that prompt until its terminal;
5. surfaces one bounded protocol-state failure.

This prevents regenerated text from being appended to an unretracted prefix.
The operation may walk back through contiguous assistant text entries but stops
at any non-assistant row.

Current provider tool lifecycle events are not forwarded from an incomplete
attempt before collection, so no tool-attempt reset is needed. If tool events
become live at the adapter seam later, that change must extend the reset
contract rather than silently leave stale tool rows.

### Headless

The headless client applies the same sequence and UTF-8 validation, truncating
the exact suffix from `TurnRun.response` so the final outcome contains only
replacement text. On invalid reset it marks the turn desynchronized, sends
cancellation, ignores later deltas, and reports a failed outcome regardless of
a contradictory completed terminal. Stderr is intentionally append-only and
cannot retract bytes already written; it prints a concise retry reset marker
before replacement output.

### RPC and external clients

The stdio pump forwards reset as an ordinary flushed JSONL `ServerEvent`.
External v0.3 clients that advertise `PromptAttemptReset` must apply ordered
suffix retraction and enter the same desynchronized behavior on invalid reset.
Same-version clients that omit the capability retain the current
no-retry-after-visible-text behavior and never receive reset.

## Status and Observability

Each retry emits one bounded status before its cancellation-aware delay:

```text
provider <category>; retrying in <duration> (attempt <n> of 3)
```

For a visible restart, reset follows the delay immediately before replacement
attempt startup. Status may state that provider delay advice was honored, but
never includes raw headers, bodies, endpoints, provider request IDs, or
credentials.

No new telemetry system is introduced. Existing provider metrics remain; typed
classification source and timeout phase may enter redacted debug/evaluation
attributes only where those attributes already exist.

## Failure and Safety Invariants

- A reset is emitted only after retry delay completes and cancellation is
  rechecked, immediately before replacement attempt startup.
- A reset never deletes durable or completed transcript state.
- Replacement deltas are ordered after reset on the same backend channel.
- Prompt-wide attempt sequences never repeat across tool rounds.
- An invalid reset desynchronizes the client; replacement deltas are ignored.
- Prefix resume never resets already-valid prefix text.
- A same-version client without the reset capability never sees reset and never
  receives regenerated text after a visible failed prefix.
- Provider `Retry-After` is a minimum wait, never permission to exceed Yach's
  bounded retry budget.
- Yach never retries earlier than a valid provider hint.
- Raw HTTP status guards prevent typed body classification from making hard 4xx
  failures retryable.
- Unknown dialect IDs and malformed provider bodies fall back conservatively.
- Catalog data selects reviewed parser code; it never defines parser behavior.
- Raw provider error payloads and headers are not protocol or session evidence.
- Cancellation wins before delay completion, reset emission, and attempt start.

## Implementation Order

1. Patch vendored Rig to preserve one bounded, non-debug `Retry-After` hint
   through `CompletionError`; add the coherent upstream patch record.
2. Add catalog provider-metadata construction/merge support and seed reviewed
   baked dialect IDs.
3. Add the typed backend dialect registry, generic fallback, bounded error
   metadata, raw-status retry guards, and classification tests.
4. Resolve baked dialect IDs at every CLI adapter-construction path and thread
   typed `ProviderIdentity` through every `map_completion_error` callsite.
5. Extract the retry loop into the attempt executor with cancellation token,
   prompt-wide attempt sequence, injected clock/sleeper, bounded policy, and
   explicit continuity mode.
6. Add strict protocol-version negotiation, v0.3.0 capability/reset event, and
   negotiated `Ready` plumbing.
7. Add exact live-suffix byte accounting to `LiveDeltaSink` and emit reset for
   negotiated restart retries after delay/cancellation recheck.
8. Update TUI and headless consumers, including desynchronized state; RPC
   remains a typed forwarding surface.
9. Remove the interim non-prefix live-delta retry guard and fixed delay ladder.
10. Update protocol docs and the resilience board entries.

## Verification

Required changed-contract coverage:

1. Non-transient errors make one attempt and emit no reset.
2. Pre-first-delta transient failure retries without reset.
3. Retryable timeout phases classify distinctly while sharing timeout policy.
4. Valid delta-seconds and HTTP-date Retry-After values are honored.
5. Invalid/past/overflowing hints fall back to local delay.
6. A hint beyond remaining delay budget fails without an early retry.
7. Delay and cumulative budget are capped; cancellation interrupts sleep.
8. Backoff tests use injected time and no wall-clock sleep or randomness.
9. Known OpenAI, Anthropic, OpenAI-compatible, and ChatGPT dialect envelopes map
   through typed parsers.
10. Missing/unknown dialect IDs, malformed JSON, empty body, and unfamiliar
    codes use conservative fallback.
11. Conflicting status/body cases cannot make 401/403 or other hard 4xx errors
    retryable; known codes may still refine their non-retryable category.
12. The concrete OpenAI `unsupported_parameter` case remains `invalid_request`.
13. No raw body/header/credential/endpoint appears in debug, protocol, status,
    or session evidence.
14. Baked catalog dialect IDs round-trip and survive provider merges with pinned
    precedence; model/fetched overrides cannot alter them.
15. Environment and every managed-connection adapter path receive the expected
    typed dialect without a backend catalog dependency.
16. OpenAI Responses prefix resume emits no reset and projects text/raw output
    exactly once.
17. Negotiated non-prefix live partial emits original delta, retry status,
    delay, reset, replacement delta, and one successful terminal in order.
18. Cancellation during retry delay emits no reset and starts no attempt.
19. Attempt sequences increase across multiple provider rounds in one prompt.
20. The reset byte count retracts exactly the failed attempt suffix while
    retaining completed prior-round assistant text.
21. TUI reset stays streaming and preserves user, tool, review, and completed
    transcript rows.
22. Invalid reset counts/sequences preserve content, desynchronize, cancel once,
    and ignore replacement deltas through terminal.
23. Headless final outcome contains only replacement text; stderr documents that
    earlier bytes cannot be retracted.
24. RPC emits one flushed valid JSONL reset frame in sequence.
25. A v0.2 initialize is rejected before `Ready`; a v0.3 peer without reset
    capability retains fail-after-live-delta behavior.
26. Tool-round completion synthesis and cancellation semantics remain unchanged.
27. Failed attempts and reset events do not appear in session JSONL or resume.
28. Clean EOF without terminal and provider-emitted failure remain non-retryable.
29. Existing slow-SSE, exactly-once, cancellation, resume, compaction, and tool
    continuation scenarios remain green.

## Non-Goals

- General provider integration ownership or replacing Rig.
- A second Yach-owned HTTP transport.
- User-configurable retry counts, delays, jitter, or error mappings.
- Provider-specific retry hints embedded in response bodies.
- Catalog-supplied field paths, regexes, mappings, templates, or parser code.
- Rich display of raw provider-authored error messages.
- Retrying invalid requests, authentication failures, safety refusals, context
  overflow, unavailable models, malformed streams, or cancellations.
- Retrying clean EOF/provider terminal failure without a separate design.
- Persisting failed-attempt text or reset events.
- Multiplexed prompt IDs or remote reconnection semantics.
- Live reasoning-delta rendering or live incomplete tool-row reset.
- Separate TTFT and idle timeout configuration.
- Generic telemetry, routing, failover, or provider substitution.
