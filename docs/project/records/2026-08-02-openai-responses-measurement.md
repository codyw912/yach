# OpenAI Responses Measurement (2026-08-02)

Verification for the OpenAI Responses provider
(`specs/2026-08-02-openai-responses-provider-design.md`): OpenAI
proper moved from the chat-completions shape to its canonical
endpoint via `RigProviderConfig::OpenAi` on rig's default client.
Same method as every measurement: 125 cells — 5 tasks x 5 profiles x
5 repeats — with the `openai` profile flipped to the new variant
(model `gpt-5.4-mini`, no output-budget spelling override: rig maps
`max_tokens` -> `max_output_tokens` natively on this path). Runtime
image guard-verified; gate 7/7 plus driver checks; a live
`smoke-rig-openai` confirmed the new wire before the sweep spent
cells.

## Rates (passes / runs)

| task | anthropic-haiku | zen-qwen | zen-nemotron | zen-deepseek | openai (Responses) |
|---|---|---|---|---|---|
| tool-call-economy | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| tool-result-dependence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| multi-round-sequence | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| compaction-continuation | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |
| notes-tally-fix | 5/5 | 5/5 | 5/5 | 5/5 | 5/5 |

**125/125**, zero launch failures. The openai column is the
Responses path's first full-sweep baseline — every task class
passes: multi-round tool loops (function-call/output binding across
turns), compaction continuation, and token round-tripping through
byte-exact text results. Spot-checked outcome documents carry
provider-reported usage (`"reported": true`) with real counts, so
Responses usage flows through `GetTokenUsage` into yacht evidence.

## Error-dialect findings

None surfaced — no cell hit a provider error, so the slated
tiered-classifier item gains no new data from this run. The
`smoke-rig-openai` command exists for cheap first-contact checks
when it does.

## Spec correction (from the final whole-branch review)

The spec's misconfiguration story was mechanically wrong: setting
`YACH_RIG_PROVIDER_MAX_TOKENS_PARAM=max_completion_tokens` with
`provider=openai` does NOT inject an invalid parameter the API would
reject. The spelling rides `additional_params`, and rig's Responses
request type silently drops unknown keys — so the real failure mode
is a request with **no output cap at all and no error signal**. Still
bounded to the explicit-misconfiguration case the spec accepts
(document, don't guard), but quieter than the spec predicted; the
env var's doc comment states the boundary correctly.

## Consequences

- The `max_completion_tokens` workaround now serves only the
  compatible shape; its motivating case on OpenAI proper is retired
  with the endpoint, not patched around.
- The queued OpenAI Responses provider-native compactor (behind the
  `Compactor` seam) is now unblocked: yach talks to the API whose
  server-side state that design needs.
- chatgpt-subscription remains unmeasured (standing token-directory
  gap) but now shares its wire family with a measured path for the
  first time.
