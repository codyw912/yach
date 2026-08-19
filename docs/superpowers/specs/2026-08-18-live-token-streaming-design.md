# Live Token Streaming Design

Status: draft — one owner fork (retry seam) OPEN below
Date: 2026-08-18
Finding: the rpc invariant matrix proved provider turns emit no live
token deltas (board, UX sprint section, 2026-08-18): the runner drains
the whole rig stream, then bursts synthetic `PromptDelta` chunks from
the finished round text. A long single-round generation shows nothing
until it completes.

## Current mechanics (evidence)

- `collect_rig_completion_stream` (rig_adapter.rs) maps stream items to
  `ProviderStreamEvent`s but returns them as one `Vec` after the loop.
- The runner then sends `response_chunks(&round.text)` as synthetic
  `PromptDelta`s (runner.rs ~7500), and mid-turn round narratives burst
  per completed round (~4151). Perceived streaming is post-hoc.
- Retry (`provider_request_with_retry`, ~4485): transient failures
  retry up to `PROVIDER_RETRY_DELAYS_MS.len()` times with a visible
  status line per attempt.
  - `provider == "openai"`: a partial attempt's completed output/text
    is salvaged; the retry request RESUMES from that prefix
    (`provider_retry_request_with_completed_prefix`), and
    `finish_provider_retry_events` prepends the prefix so it projects
    exactly once. Live streaming composes seamlessly here: streamed
    prefix + streamed continuation is the final text.
  - Every other provider: a mid-stream partial failure discards the
    received deltas and retries the same request from scratch — the
    retried attempt REGENERATES. With live streaming, the discarded
    partial would already be on the wire: this is the only seam.

## Design

1. **Emission.** Thread an optional live-delta sink through
   `ProviderRequester::request_attempt` into
   `collect_rig_completion_stream`; forward `TextDelta` items as they
   arrive (the round owner maps them to `PromptDelta` with its session
   id). Tool-call events keep their current lifecycle (already live via
   `ToolCallStarted`/`ToolCallOutput`).
2. **Suppression.** When a round's deltas streamed live, skip the
   post-round `response_chunks` burst and the mid-turn narrative burst
   for that round. Persistence is unchanged: the assistant entry still
   carries the full round text, so resume parity holds (pinned by the
   matrix resume scenario).
3. **Retry seam (OPEN fork, non-prefix-resume providers only).**
   - (a) Accept the seam: keep retrying, emit a visible attempt marker
     (e.g. a status line plus a `[retrying — restarting response]`
     transcript row); the failed attempt's text remains in the
     transcript above the regenerated text. Full retry coverage,
     honest but ugly.
   - (b) RECOMMENDED — once any delta of the current round has been
     streamed live, a partial failure stops retrying and fails the turn
     recoverably (existing failed-turn UX; user re-prompts). Retry
     coverage is unchanged for the dominant transient class
     (connect/start failures, rate limits before first token) and for
     the full openai prefix-resume path. No protocol change, no garble,
     boring. The resilience pass (already slated: retry policy,
     partial-stream salvage) supersedes this with a real design later.
   - (c) Negotiated attempt-boundary/reset event: a new capability +
     `ServerEvent` telling clients to clear the in-progress assistant
     text; TUI and rpc clients that negotiate it get seamless retries.
     Durable answer, but a protocol addition that the resilience pass
     should own — premature to mint here.
4. **Verification.** Extend the matrix slow-SSE scenario to assert the
   first `PromptDelta` arrives well before the stream ends (flipping
   the scenario from documenting the defect to pinning the fix); add a
   before-first-byte transient failure scenario proving silent retry
   still works; keep resume parity green.

## Not sufficient for

Retry-policy redesign, partial-stream salvage generalization,
attempt-boundary protocol events, reasoning-delta rendering, or
provider adapter ownership questions — all resilience-pass scope.
