# Provider Library Evaluation Spike

Date: 2026-04-28  
Status: initial docs/fixture-grounded evaluation  
Related plan: `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`

## Summary

Yach should keep the provider library below its own `yach-backend` provider seam. Based on the first U4 seam and documentation review, the safest next implementation path is:

1. Keep the current yach-owned `ProviderRequest`, `ProviderStreamEvent`, and `ProviderError` types as the canonical backend-facing seam.
2. Use recorded/golden fixtures to compare provider-library event fidelity before adding provider SDK dependencies.
3. Treat Rig as a candidate, but **limit** it to provider/stream translation unless a fixture spike proves its agent/tool loop can stay below yach-owned sessions/tools/resources.
4. Also evaluate Siumai and GenAI as lower-agent-gravity alternatives before making a durable provider choice.

No provider crate dependency was added in this pass. No network/provider calls were run.

## Evaluation Criteria

| Criterion | What yach needs |
|---|---|
| Session ownership | Provider/library IDs can be stored as metadata, but yach session ids, entry ids, parent links, and turn ids remain canonical. |
| Streaming fidelity | Started, text delta, completed, failed, and cancelled stream states must map to `ProviderStreamEvent` without hidden terminal states. |
| Tool fidelity | Tool-call id, name, argument JSON, finish status, and ordering metadata need to survive as fixtures before native tool execution enters scope. |
| Error taxonomy | Auth, rate limit, invalid request, context length, unavailable model, timeout/network, provider internal, safety/refusal, malformed stream, and cancellation need normalized `ProviderErrorKind` mappings. |
| Replaceability | Replacing Rig with Siumai/GenAI/direct SDKs should change adapter code only, not yach sessions, UI protocol, or canonical runtime state. |
| Security/trust | Credentials, raw debug payloads, provider extension maps, and tool arguments must be handled as untrusted adapter inputs. |
| Dependency cost | Startup/binary-size impact and feature flags should be measured before committing to a dependency-heavy adapter. |

## Candidate Review

### Rig

Sources reviewed:

- `https://docs.rig.rs/docs/concepts/streaming`
- `https://docs.rig.rs/docs/concepts/provider_clients`
- `https://crates.io/crates/rig-core`

Observed strengths:

- Multi-provider support and a unified interface are aligned with yach's desire not to own provider churn immediately.
- Streaming concepts include prompt/chat/completion levels, text chunks, tool-call deltas, final response access, multi-turn streaming, pause control, and per-chunk error handling.
- Rig docs explicitly call out tool-call delta buffering and stream backpressure concerns, both relevant to yach's native runtime policy.

Risks / questions:

- Rig's higher-level agent abstractions may want to own the agent loop. Yach must not let that happen: sessions, tool permissions/execution, resource loading, transcript persistence, and UI events stay yach-owned.
- Need fixture proof that tool-call deltas expose enough provider call-id and argument boundaries for yach to execute tools itself.
- Need fixture proof that final response/usage/finish state can be mapped without relying on Rig-owned session history.

Initial recommendation: **limit / evaluate further**. Rig remains viable if used as a thin provider adapter, but should not be added until golden streaming/tool/error fixtures prove adapter thinness.

### Siumai

Sources reviewed:

- `https://docs.rs/siumai/latest/siumai/`
- `https://crates.io/crates/siumai`
- `https://github.com/yumchalabs/siumai`

Observed strengths:

- Library-first positioning and capability-separated traits appear closer to yach's desired adapter layer than a full agent framework.
- Supports multiple providers including OpenAI, Anthropic, Gemini, Ollama, Groq, xAI, and OpenAI-compatible vendors.
- Public docs expose chat, streaming, tools, provider capabilities, retry, model listing, and custom provider concepts.
- Shared parameters plus extension points match yach's requirement that provider-specific options live behind adapter-owned validation.

Risks / questions:

- Need fixture proof that streaming events include enough start/delta/usage/end/cancellation detail for yach's `ProviderStreamEvent` shape.
- Built-in retry/performance/tracing layers are useful but could conflict with yach-owned error/cancellation/backpressure policy if enabled implicitly.
- Provider capability checks are described as permissive hints; yach may still need stricter model/profile validation at its own config boundary.

Initial recommendation: **strong alternate candidate**. Siumai may be a better first adapter spike than Rig if the priority is avoiding agent-framework gravity.

### GenAI

Sources reviewed:

- `https://docs.rs/genai/latest/genai/chat/index.html`
- `https://github.com/jeremychone/rust-genai`

Observed strengths:

- Chat API exposes requests, responses, streaming events, stream chunks, stream end, usage, tools, tool calls, and tool chunks.
- Stream end can capture usage/content/tool calls, which is useful for completion/failure accounting and session metadata.
- Appears focused on provider-normalized chat primitives rather than owning an agent/session runtime.

Risks / questions:

- Need verify provider coverage and active maintenance relative to yach's target providers.
- Need fixture proof for malformed streams, safety/refusal, context-length errors, and cancellation behavior.
- Some options such as reasoning effort/service tier are provider-specific; yach should keep these behind validated extension maps.

Initial recommendation: **viable control candidate**. GenAI is useful for comparing how thin a provider adapter can be without a full agent framework.

## Golden Fixture Set for Next Spike

Before adding dependencies, define or collect fixtures that can be mapped into existing yach-owned types:

| Fixture | Required yach outcome |
|---|---|
| Plain streaming text | Ordered `Started`, multiple `TextDelta`, and `Completed` events with one `NativeTurnId`. |
| Rate limit error | `ProviderErrorKind::RateLimited` with redacted debug details. |
| Auth failure | `ProviderErrorKind::Authentication`, no retry by default. |
| Context length error | `ProviderErrorKind::ContextLength`. |
| Malformed/unknown stream payload | `ProviderErrorKind::MalformedStream` or redacted debug metadata, not panic. |
| Safety/refusal | `ProviderErrorKind::SafetyRefusal` only when provider semantics justify it; do not collapse refusal into transport failure. |
| Single streamed tool call | Preserve call id, tool/function name, argument JSON, and completion boundary. |
| Parallel tool calls | Preserve distinct ids and ordering metadata where provider exposes it. |
| Multi-turn response ids | Identify whether response/conversation ids are required for continuation/retry/cache/cost or merely optimization metadata. |
| Cancellation/drop | Adapter can stop producing events or emit `Cancelled` without marking a turn complete. |

## Fixture Pass Findings

The first fixture-backed seam pass added additive backend-owned types for the gaps identified above without adding any provider SDK dependency:

- `ProviderToolCall` preserves provider call id, tool/function name, and raw JSON arguments.
- `ProviderUsage` records optional input/output/total token counts.
- `ProviderFinishReason` records coarse completion reasons.
- `ProviderStreamEvent` now includes tool-call started/delta/completed events plus completion usage, finish reason, and optional provider response id.

Covered fixtures in `crates/yach-backend/src/lib.rs` now include:

| Fixture | Status |
|---|---|
| Plain streaming text | Covered by ordered started/delta/completed lifecycle test. |
| Normalized errors | Covered for auth, rate limit, invalid request, context length, unavailable model, safety refusal, and malformed stream. |
| Single streamed tool call | Covered for call id, name, argument deltas, and completed JSON argument payload. |
| Cancellation/drop | Covered by a cancelled event that does not mark the turn completed. |
| Multi-turn response ids | Partially covered by optional `provider_response_id` on completion; semantic necessity still requires a real adapter/provider fixture. |
| Parallel tool calls | Shape supports distinct call ids, but no dedicated fixture yet. |

## Seam Implications

The current U4/U5 seam is now sufficient for P0 text streaming, normalized errors, cancellation, usage/finish metadata, and first-pass tool-call streaming placeholders. These types should stay in `crates/yach-backend/src/lib.rs` until at least one real adapter consumer exists. Do not split `yach-llm` or provider crates solely for marker boundaries.

Potential remaining additive shapes after real adapter pressure:

- provider tool schema render type;
- structured tool-call ordering metadata for parallel calls;
- redacted raw payload handle or debug classification;
- adapter-validated extension map helpers.

## Security Notes

- Do not persist raw provider payloads unless explicit debug mode exists.
- Redact credentials and obvious secret patterns before putting details into `ProviderError::redacted_debug`.
- Treat tool-call arguments as untrusted JSON; yach-owned tool execution must schema-validate before use.
- Provider-specific extension keys should be allowlisted by the adapter, not passed through from arbitrary config unchecked.

## Recommendation

For the next implementation slice, do **not** choose a final provider library yet. Instead:

1. Add fixture-backed mapping tests for yach-owned provider events/errors and tool-call placeholders.
2. Evaluate Siumai and GenAI alongside Rig using the same fixtures.
3. Add a provider dependency only after one candidate demonstrates thin adapter code, complete-enough stream/tool/error fidelity, and acceptable dependency/startup impact.

Current status by candidate:

| Candidate | Recommendation | Why |
|---|---|---|
| Rig | `limit / evaluate further` | Strong provider/stream support, but agent abstractions may leak upward. |
| Siumai | `evaluate as strong alternate` | Library-first, capability-separated, multi-provider; likely lower agent gravity. |
| GenAI | `evaluate as control candidate` | Chat/stream/tool primitives look thin and useful for adapter comparison. |
| Direct SDKs | `keep as escape hatch` | Use only if libraries cannot preserve yach semantics or event fidelity. |

## Supported Claim

Supported: yach has enough provider seam structure to run fixture-backed adapter comparisons without adding provider dependencies yet.

Not supported: Rig, Siumai, GenAI, or direct SDKs are definitively chosen for the native backend.
