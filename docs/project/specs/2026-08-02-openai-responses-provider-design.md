# OpenAI Proper on the Responses API

**Date:** 2026-08-02
**Status:** In review
**Prior work:** rig upgrade own-the-loop
(`2026-07-31-rig-upgrade-own-the-loop-design.md`, landed through
#208/#213/#214), the `max_completion_tokens` workaround (#205), text
tool results (`2026-08-01-text-tool-results-design.md`, #216).
Board: "Use rig's Responses API surface for OpenAI proper" (slated
2026-07-31, owner leaning: migrate to the canonical endpoint).

## Problem

yach treats OpenAI proper as just another openai-compatible endpoint:
the `OpenAiCompatible` provider opts into rig's chat-completions
surface (`.completions_api()`), which is the right wire for
aggregators but the legacy one for OpenAI itself. That choice is why
the `max_tokens` / `max_completion_tokens` gap existed at all, and it
blocks the queued OpenAI Responses provider-native compactor, whose
premise is the server-side state only the Responses API offers.

## The three questions, answered (rig 0.41 sources)

The board required these answered before any code; all three are
verified against the vendored rig-core 0.41.0 sources:

1. **Tool calls map.** `GenericResponsesCompletionModel` implements
   the standard `completion::CompletionModel` over the portable
   `CompletionRequest` (responses_api/mod.rs:2205). Tool definitions
   and tool-result messages are converted internally to Responses
   shapes (`ResponsesToolDefinition`, `responses_tool_result_output`
   at mod.rs:320). yach's native tool-call mapping is expressed in the
   portable `completion::message` types, so it is untouched.
2. **The collector is compatible.** The Responses stream emits
   `RawStreamingChoice<StreamingCompletionResponse>` (streaming.rs:20)
   — the same enum the collector consumes — and
   `StreamingCompletionResponse` is `Clone` and implements
   `GetTokenUsage` (streaming.rs:84), satisfying
   `PreparedCompletion::run`'s bounds as-is. Responses emits more
   reasoning events; the mapper already ignores
   `Reasoning`/`ReasoningDelta`/`Unknown`.
3. **One surface, not two.** chatgpt's completion model already uses
   `responses_api::streaming::StreamingCompletionResponse`
   (chatgpt/mod.rs:496), so `ChatGptSubscription` rides the same wire
   family today. This migration converges the OpenAI-family paths
   rather than adding one.

Bonus finding: rig maps builder `max_tokens` ->
`max_output_tokens` on this path (responses_api/mod.rs:1311), so the
`max_completion_tokens` workaround's motivating case disappears for
OpenAI proper, as the board predicted.

## Design

### Provider variant

```rust
RigProviderConfig::OpenAi { api_key: String }
```

Selected by `YACH_RIG_PROVIDER=openai`; env wiring
`YACH_RIG_OPENAI_API_KEY` / `YACH_RIG_OPENAI_MODEL` (stopgap surface,
same as every provider until the product-surface work). No base-URL
override: no Responses-speaking aggregator exists today; add the
field when one does. `OpenAiCompatible` survives unchanged for
aggregators and anyone else wearing the chat-completions shape.

### The arm

Construction from rig's default client — no `.completions_api()`:

```rust
RigProviderConfig::OpenAi { api_key } => {
    let client = openai::Client::builder()
        .api_key(&api_key)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let model = client.completion_model(attempt.request.model.model.clone());
    attempt.run(model).await
}
```

Same two lines as every other arm; the shared
`PreparedCompletion::run` does the rest. The associated-type
resolution (default client -> Responses model) is confirmed by
compile in slice 1.

### MaxTokensParam interaction

The default `MaxTokens` spelling is correct on this variant: the
builder's `max_tokens` becomes `max_output_tokens` inside rig.
Setting `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM=max_completion_tokens`
with `provider=openai` would inject an invalid parameter into a
Responses body; this is documented as a configuration mistake (one
line on the env var's doc: applies to the compatible shape), not
guarded in code. The mechanism itself stays — the compatible path
still needs it.

### Smoke parity

`run_openai_smoke` on the Responses model, reusing
`stream_smoke_completion` and `collect_rig_smoke_stream` (bounds
already satisfied). Same shape as the other three smoke functions.

## Measurement matrix (owner decision 2026-08-02)

`openai.env` flips to the new variant: the matrix stays 5 profiles /
125 cells, and the canonical path gets the ongoing regression
coverage. This retires live coverage of `max_completion_tokens` on
the real endpoint; the mechanism keeps unit coverage, and Zen cells
keep exercising the compatible shape. Rejected alternative: a sixth
profile preserving the compat-proper cell (~20% more sweep time and
spend on a mechanism whose motivating case is gone).

## Validation

1. Unit tests: config parsing for the new variant; smoke-shape tests
   as for other providers. Workspace clippy strict; full suite green.
2. `just runtime-image` + gate (7/7 + driver checks expected —
   anthropic-profile gate is unaffected by this change).
3. The 125-cell sweep with the flipped `openai.env`, against the
   2026-08-02 text-results reference (125/125). Regression check at
   ceiling; the openai column doubles as the Responses path's first
   full-sweep baseline.
4. Spot-check outcome documents on the openai cells:
   `"reported": true` with real token counts (Responses usage flows
   through `GetTokenUsage`).

## Risks

- **First real exercise of the Responses wire.** Error dialects
  differ from chat-completions; the keyword-ladder classifier may
  misclassify Responses errors. Known slated item
  (tiered provider-error classifier); surfaced failures are data for
  it, not blockers here.
- **Associated-type resolution is assumed from documentation and
  source reading, not yet a compile.** Slice 1 establishes it
  immediately; if the default client does not resolve to the
  Responses model, the estimate changes (a
  `responses_api()`-explicit construction exists as fallback).
- **Profile flip needs the owner** (local secret-reference file);
  scaffold the new contents with a placeholder for the key reference.

## Non-goals

- Server-side state (`previous_response_id`, `store`): that is the
  queued provider-native compactor behind the `Compactor` seam,
  which this migration unblocks but does not start.
- No `ChatGptSubscription` changes (already Responses-shaped).
- No provider/model product-surface work; env wiring stays a stopgap.
- No error-classifier work beyond recording what the new dialect
  surfaces.

## Slice

One slice: variant + arm + smoke + tests, then the measurement. The
change is four small additions to one file plus config parsing; there
is no seam worth splitting.
