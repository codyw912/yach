# Task 8 Report: Responses Native Compaction Lifecycle

## Implementation

- Added a segmented-request-safe local `TcpListener` fixture in `runner.rs` that authenticates both `/v1/responses` and `/v1/responses/compact`, records only request path and JSON body, and deliberately does not retain the Authorization header.
- The fixture streams terminal `response.completed` SSE payloads with reasoning, a function call, a final message, and an unknown provider-added output item; it returns a provider-native compact window containing encrypted provider data without printing it.
- Added `responses_native_compaction_fixture_covers_complete_wire_lifecycle`, exercising the real Rig adapter, native compactor, and replayed continuation. It value-compares request JSON: endpoint order, `store: false` on Responses requests, equal turn/compact instructions, and the exact compact window replayed on continuation.
- Added `yach smoke-responses-compaction`. With a real OpenAI configuration it runs a Responses turn, native compaction, portable summary, replayed continuation, and optional A -> B -> A replay when `YACH_RIG_OPENAI_SMOKE_ALT_MODEL` is set. Its rendering is constrained to pass/fail labels, model IDs, stage labels, artifact count, token count, and missing prerequisite labels.
- Added parser and redacted missing-key output coverage for the smoke command.

## Files

- `crates/yach-backend/src/runner.rs`
- `crates/yach-cli/src/main.rs`

## RED/GREEN Evidence

RED:

```text
just dev cargo test -p yach smoke_responses_compaction
error[E0004]: non-exhaustive patterns: `&Command::SmokeResponsesCompaction` not covered
```

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction
1 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
2 passed; 0 failed
```

The requested `-p yach-cli` package selector is not valid in this workspace because `crates/yach-cli/Cargo.toml` declares package name `yach`; the equivalent focused command above was run instead.

## Live Smoke

`YACH_RIG_OPENAI_API_KEY` is unavailable. Exact missing prerequisite: `YACH_RIG_OPENAI_API_KEY`.

```text
just dev cargo run -p yach -- smoke-responses-compaction
responses_compaction_smoke=missing_config
artifact_item_count=0
token_count=0
prerequisite=YACH_RIG_OPENAI_API_KEY
```

No live OpenAI verification is claimed.

## Self-review

- The fixture only records JSON request bodies; it authenticates header presence transiently and sends no bearer value through its channel, assertions, diagnostics, or output.
- Request assertions are structural/value-equality assertions, not source-text assertions.
- Smoke rendering does not expose request bodies, encrypted provider content, or credentials.

## Concerns

- Live execution remains blocked solely by the unavailable `YACH_RIG_OPENAI_API_KEY` prerequisite.

## Fix round 1 evidence

- The local fixture now consumes a per-request script. It can return a terminal Responses stream, a caller-defined compact window, HTTP status failures, malformed compact JSON, or a deliberately stalled request. It continues to authenticate transiently without retaining Authorization bytes.
- Added fixture coverage mapping HTTP, decode, and timeout outcomes from `/v1/responses/compact`.
- The smoke chain now passes the initial user item with the first Responses output to compaction, and passes the compact window plus the new user item to continuation and A -> B -> A replay.
- The portable-summary call carries the serialized checkpoint input and now requires a nonempty terminal summary. Every smoke request now requires a `Completed` event and rejects failed or cancelled streams.

```text
just dev cargo test -p yach-backend responses_native_compaction
2 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed
```

## Fix round 2 runner matrix

The focused `responses_native_compaction` filter now assembles the existing runner/JSONL lifecycle contracts with the scripted HTTP fixture: threshold runner checkpoint and continuation, manual focus instruction delta, automatic versus forced native fallback, native-success/summary-failure atomicity, cumulative second-compaction kept-tail de-duplication, A -> B -> A replay re-engagement, cancellation continuity, and hard-abort persistence.

```text
just dev cargo test -p yach-backend responses_native_compaction
10 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed

just dev cargo run -p yach -- smoke-responses-compaction
responses_compaction_smoke=missing_config
prerequisite=YACH_RIG_OPENAI_API_KEY
```

## Fix round 3 runner wire evidence

Added `responses_native_compaction_runner_uses_scripted_http_fixture_and_jsonl`, which starts the production `run_native_loop` against the scripted loopback OpenAI fixture, drives a manual compact plus prompt through runner events, captures the compact and Responses wire requests, and loads the persisted JSONL. The fixture's terminal SSE shape currently makes the runner reject the portable summary as unusable; the test therefore verifies the runner's no-checkpoint failure fallback rather than claiming a native checkpoint was committed.

```text
just dev cargo test -p yach-backend responses_native_compaction
11 passed; 0 failed
```


## Final runner fixture repair

### RED/GREEN evidence

RED:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_uses_scripted_http_fixture_and_jsonl
FAILED: left: Some(Completed), right: Some(Failed)
```

The red run proved that the scripted `response.output_text.delta` was previously
missing Rig's required `item_id`, `output_index`, `content_index`, and
`sequence_number` fields: once those fields were supplied, the real Rig stream
produced the portable summary and the old failure-only expectation became false.

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_uses_scripted_http_fixture_and_jsonl
1 passed; 0 failed

just dev cargo test -p yach-backend responses_native_compaction_runner_fixture_falls_back_after_http_decode_and_timeout
1 passed; 0 failed

just dev cargo test -p yach-backend responses_native_compaction_runner_fixture_keeps_state_when_summary_is_empty
1 passed; 0 failed
```

### Self-review

- The successful lifecycle runs the production `run_native_loop` with a
  `JsonlSessionStore` and loopback Responses/compact fixture; it asserts the
  persisted portable summary and native artifact, replay window, post-checkpoint
  user input, manual-focus isolation, and `store: false` on every Responses
  request.
- The failure fixture drives HTTP, decode, and delayed timeout native compact
  failures through runner events and proves the committed summary checkpoint
  has no native artifact. Delayed timeout keeps the listener available for the
  summary and continuation requests.
- The atomicity fixture proves a usable native window is discarded when the
  real Rig summary stream contains no assistant text: no checkpoint is written,
  and the subsequent prompt still completes.
- The fixture authenticates each request transiently and captures only path and
  JSON body. No bearer value, provider payload, or encrypted fixture value is
  included in this report.

### Final focused verification

```text
just dev cargo test -p yach-backend responses_native_compaction
13 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed
```

## Runner fixture extension

- Added a strict Responses `response.output_item.done` function-call event to
  the loopback fixture. The runner fixture now exercises a real allowed
  `read_text_file` execution and the following final provider response; its
  captured continuation structurally contains the matching
  `function_call_output`.
- Added a persisted malformed-native-artifact restart fixture. It proves that
  reconstruction declines the poisoned window before request construction and
  sends an ordinary Responses request instead.
- Added a stalled Responses stream fixture case driven by
  `ClientEvent::PromptCancelled`; it proves bounded cancelled finalization and
  a persisted cancelled turn.

```text
just dev cargo test -p yach-backend responses_native_compaction
16 passed; 0 failed
```

## Retry-prefix recovery closeout

### RED/GREEN evidence

RED:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_retries_completed_prefix_as_single_native_input
FAILED: completed prefix was absent from the captured retry input
```

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction
17 passed; 0 failed

just dev cargo test -p yach-backend stream_error_retains_completed_responses_output_prefix
1 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed
```

### Implementation and self-review

- `ProviderStreamAttempt` preserves already-mapped stream events together with
  a typed `ProviderError`; the legacy adapter boundary still exposes ordinary
  `Result<Vec<_>, ProviderError>` semantics to callers that do not retry.
- The real loopback fixture emits a completed assistant-output prefix followed
  by a provider error. The retry assertion captures the second `/responses`
  body and verifies the prefix occurs once, alongside the original user input;
  the JSONL log records one completed turn.
- Retry conversion uses structured stream events to construct a canonical
  native Responses envelope. It neither encodes retry state in an error string
  nor commits the prefix independently before the successful retry.
- The focused Task 8 matrix continues to cover persisted cancellation
  continuity, second-compaction kept-tail de-duplication, and A -> B -> A
  checkpoint re-engagement. No request bodies, bearer values, or encrypted
  fixture content are reported.

## Final Task 8 lifecycle closure

### Added runner/JSONL cases

- `responses_native_compaction_runner_replays_cancelled_tool_remainder_once_after_jsonl_restart`
  uses production `run_native_loop`, real Rig/OpenAI loopback SSE, and
  `JsonlSessionStore`. The first response emits a completed file read followed
  by a command awaiting review; cancellation persists the completed read and
  synthetic cancelled command remainder. A fresh runner process then resumes
  from JSONL, and the captured `/responses` input has exactly one structurally
  paired request/result for each call.
- `responses_native_compaction_runner_keeps_second_window_coherent_across_model_a_b_a`
  drives two manual runner compactions and actual `ClientEvent::ModelSelected`
  transitions. It value-checks the captured compact and Responses inputs:
  first A window plus one kept tail, second compact without duplicating that
  tail, B's ordinary request without A's window, and A's latest window plus
  the post-second-checkpoint tail once. It also verifies two native checkpoint
  artifacts persisted in JSONL.

### RED/GREEN evidence

RED:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_keeps_second_window_coherent_across_model_a_b_a -- --nocapture
FAILED: Some(Failed) != Some(Completed)
```

The first complete lifecycle script lacked the final fixture response for the
return-to-A request. Completing the real eight-request fixture exposed the
correct post-second-checkpoint tail boundary; the test now distinguishes the
tail retained by the second compact window from the earlier tail it replaced.
No production replay defect or diagnostic seam was required.

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_replays_cancelled_tool_remainder_once_after_jsonl_restart -- --nocapture
1 passed; 0 failed

just dev cargo test -p yach-backend responses_native_compaction_runner_keeps_second_window_coherent_across_model_a_b_a -- --nocapture
1 passed; 0 failed

just dev cargo test -p yach-backend responses_native_compaction
19 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed
```

### Self-review and concerns

- The loopback fixture keeps only request path and JSON body. Authorization is
  checked transiently and is never captured; this addition adds no credential,
  request-body, or encrypted-content diagnostics.
- The cancellation assertion operates at the persisted/request boundary:
  matching call IDs occur once as both `function_call` and
  `function_call_output`, so an unpaired request, missing synthetic result, or
  duplicate replay fails.
- The model lifecycle uses a single configured OpenAI provider with
  model-specific Responses compaction capability. It tests runner event
  selection rather than direct compactor helpers.
- No production change was necessary: both cases passed against the existing
  typed replay/recovery path once the loopback scripts represented all real
  requests.

## Review round 4: retry prefix and runner boundaries

### RED/GREEN evidence

RED:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_retries_completed_prefix_as_single_native_input
FAILED: left: "fixture summary"
right: "completed prefixfixture summary"
```

The tightened loopback test proved that the retry request contained the
completed prefix, but the returned stream and persisted assistant entry still
discarded it.

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_retries_completed_prefix_as_single_native_input
1 passed; 0 failed

just dev cargo test -p yach-backend responses_native_compaction
21 passed; 0 failed

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed
```

### Added coverage and self-review

- `provider_request_with_retry` now retains interrupted-attempt text and tool
  lifecycle events when it can construct the corresponding native retry
  envelope, then prepends them to the successful stream. It folds interrupted
  raw output into the successful response payload. The successful attempt
  remains the sole source of terminal lifecycle, usage, and response id.
- The real loopback prefix test verifies the captured retry input contains one
  assistant prefix and one user item, and verifies both UI deltas and JSONL
  contain the prefix plus suffix exactly once.
- `responses_native_compaction_runner_automatically_checkpoints_native_window_over_threshold`
  configures a low automatic threshold, sends only a normal prompt, and
  verifies the real runner's `/responses/compact` request plus a threshold
  native checkpoint in JSONL.
- `responses_native_compaction_runner_uses_silent_auto_and_visible_forced_summary_fallback`
  drives unsupported and unknown capability states through the real runner and
  loopback fixture. It proves no compact endpoint call occurs, summary fallback
  completes, and forced selection alone emits the capability warning.
- The pre-existing real A -> B -> A lifecycle continues to cover two compact
  windows, kept-tail de-duplication, ordinary B input, and A re-engagement.

No request bodies, bearer values, or encrypted provider values are written to
diagnostics or this report. The only remaining concern is that live-provider
smoke remains dependent on the unavailable OpenAI credential described above.

## Final automatic replay lifecycle fix

### RED/GREEN evidence

RED:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_automatically_checkpoints_native_window_over_threshold
FAILED: the restart request did not match the second native window plus its post-checkpoint result and new prompt
```

GREEN:

```text
just dev cargo test -p yach-backend responses_native_compaction_runner_retries_completed_prefix_as_single_native_input
1 passed; 0 failed; 623 filtered out

just dev cargo test -p yach-backend responses_native_compaction_runner_automatically_checkpoints_native_window_over_threshold
1 passed; 0 failed; 623 filtered out
```

### Self-review and boundary note

- The automatic lifecycle fixture drives two genuine threshold compactions through `run_native_loop`, `/responses/compact`, `/responses`, and `JsonlSessionStore`. It verifies the second compact input contains the pending tail once, the returned second window replaces that pre-checkpoint tail, and a relaunched runner sends exactly the second window, remaining post-checkpoint assistant result, and new user item.
- `provider_retry_merges_typed_partial_output_without_duplicate_terminal_events`
  covers that raw-output merge at the `ProviderRequester::request_attempt`
  seam: ordered reasoning (including encrypted content), function call,
  message, and unknown raw items replay once; text/tool lifecycle is retained
  once; only the successful Started/Completed lifecycle, usage, and response
  id survive. The Rig stream adapter exposes raw Responses output only at its
  final payload boundary, so a malformed/truncated loopback stream can
  exercise transport retry text/lifecycle behavior but cannot expose partial
  raw output without widening the vendored protocol.
- No credentials, request bodies, encrypted content, or temporary diagnostics were added.

## Final review correction

- A transient stream error after a completed tool call no longer retries past
  the tool boundary. The provider round returns its mapped tool request and raw
  function-call item with one synthesized `Completed(ToolCalls)` event, so the
  normal runner executes the tool and sends a paired continuation.
- Non-tool partial raw output still follows the retry path. The typed seam test
  now keeps reasoning, message, and unknown items in the retry input and merged
  terminal output exactly once, while the completed-tool subcase proves no
  second provider attempt occurs before tool execution.
- The automatic lifecycle now commits a complete user/assistant pair after the
  second threshold checkpoint. After relaunch, the captured request is exactly
  the second native window, its remaining assistant result, the reconstructed
  user/assistant pair, and the new user prompt.

Focused correction evidence:

```text
just dev cargo test -p yach-backend provider_retry_merges_typed_partial_output_without_duplicate_terminal_events
1 passed; 0 failed; 624 filtered out

just dev cargo test -p yach-backend responses_native_compaction_runner_automatically_checkpoints_native_window_over_threshold
1 passed; 0 failed; 624 filtered out
```

### Retry-boundary hardening

- Terminal partials are classified before the retry budget. An already
  completed provider response remains terminal even after all retries.
- The adapter carries a `tool_round_complete` fact derived from Rig's stable
  internal call IDs; public stream-item and provider call IDs may differ.
  Mixed complete/incomplete batches return the stream error instead of
  executing a subset or retrying an unpaired function call.
- Earlier retry text and raw output are merged through the same terminal path
  when a later attempt ends at a completed tool boundary.
- Only transient stream failures may synthesize `Completed(ToolCalls)`;
  authentication, invalid-request, context-length, and unavailable-model
  errors remain failures even when a tool item was already complete.

```text
just dev cargo test -p yach-backend provider_retry_merges_typed_partial_output_without_duplicate_terminal_events
1 passed; 0 failed; 624 filtered out

just dev cargo test -p yach-backend responses_native_compaction
21 passed; 0 failed; 604 filtered out

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed; 150 filtered out
```

### Live smoke evidence (2026-08-07)

The owner ran the smoke through 1Password credential resolution with
`YACH_RIG_OPENAI_MODEL=gpt-5.6-luna`:

```text
op run -- just dev cargo run -p yach -- smoke-responses-compaction
responses_compaction_smoke=passed
model=gpt-5.6-luna
stage=responses_turn
stage=native_compact
stage=portable_summary
stage=replayed_continuation
artifact_item_count=2
token_count=164
```

The owner then set `YACH_RIG_OPENAI_SMOKE_ALT_MODEL=gpt-5.6-terra` and
re-ran the smoke through 1Password credential resolution:

```text
responses_compaction_smoke=passed
model=gpt-5.6-luna
stage=responses_turn
stage=native_compact
stage=portable_summary
stage=replayed_continuation
stage=model_switch_replay
artifact_item_count=2
token_count=158
```