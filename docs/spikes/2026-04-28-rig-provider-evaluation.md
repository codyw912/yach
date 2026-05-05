# Provider Library Evaluation Spike

Date: 2026-04-28
Status: initial docs/fixture-grounded evaluation
Related plan: `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`

## Summary

Yach should keep the provider library below its own `yach-backend` provider seam. Based on the first U4 seam and documentation review, the safest next implementation path is:

1. Keep the current yach-owned `ProviderRequest`, `ProviderStreamEvent`, and `ProviderError` types as the canonical backend-facing seam.
2. Use recorded/golden fixtures to compare provider-library event fidelity before adding provider SDK dependencies.
3. Treat Rig as the approved first provider-library spike candidate, but **limit** it to provider/stream translation unless the spike proves its agent/tool loop can stay below yach-owned sessions/tools/resources.
4. Keep GenAI as the serious fallback/control candidate. Drop Siumai from serious consideration for now because its maturity/adoption signal is too weak for this dependency tier.

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

Updated recommendation: **drop from serious contention for now**. Siumai's lower agent-framework gravity is attractive, but its maturity/adoption signal is too weak for a core provider dependency decision at this stage. Revisit only if adoption/support changes materially or Rig/GenAI/direct paths fail.

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

Updated recommendation: **serious fallback/control candidate**. GenAI is useful for comparing how thin a provider adapter can be without a full agent framework, but the first dependency spike should try Rig unless strong evidence shows Rig cannot preserve yach-owned loop/tool/session semantics.

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

Updated after owner decision: do **not** choose a final provider library yet, but approve **Rig as the first provider-library adapter spike candidate**. The spike should be deliberately thin and should stop if Rig cannot stay below yach-owned runtime semantics.

Next path:

1. Add a minimal Rig dependency spike behind existing yach-owned provider seam types.
2. Exercise existing golden fixtures against Rig mapping code before any native dogfood network/provider path.
3. Keep GenAI as the serious fallback/control candidate if Rig leaks session/tool/loop ownership or loses stream/tool/error fidelity.
4. Keep direct SDKs as the escape hatch if provider libraries cannot preserve yach semantics.
5. Do not add credentials, network calls, real native provider dogfood, or durable provider-specific core types in the first Rig spike.

Current status by candidate:

| Candidate | Recommendation | Why |
|---|---|---|
| Rig | `approved first spike candidate` | More mature/wider used; strong provider/stream support; acceptable if constrained below yach-owned loop/tools/sessions. |
| GenAI | `serious fallback/control candidate` | Thin chat/stream/tool primitives; useful if Rig leaks too much or proves too heavy. |
| Siumai | `drop for now` | Adoption/maturity signal is too weak for this dependency tier despite appealing lower agent gravity. |
| Direct SDKs | `keep as escape hatch` | Use only if libraries cannot preserve yach semantics or event fidelity. |

Rig spike acceptance gates:

- Yach owns canonical session ids, entry ids, turn ids, transcript persistence, and UI-facing protocol events.
- Yach owns tool definitions, permission checks, execution, and result persistence; Rig may only surface tool-call requests below the provider seam.
- Rig types do not leak into `yach-ui`, `yach-proto`, or native session records.
- Rig maps text deltas, completion/failure/cancellation, usage/finish metadata where available, provider response ids as metadata only, tool-call id/name/arguments/boundaries, and normalized errors into yach-owned types.
- Built-in retry/history/agent-loop behavior is disabled, bypassed, or contained below adapter code.

Stop and switch to GenAI/direct SDK evaluation if Rig requires its agent abstraction to own the loop, hides tool execution boundaries, loses stream/tool-call fidelity, requires provider session/thread/history as canonical state, or brings unexpectedly invasive dependency/runtime behavior.

## Minimal Rig Adapter Spike Findings

The first Rig dependency spike added `rig-core = { version = "0.35.0", default-features = false }` to `yach-backend` and mapped Rig streaming fixture shapes into existing yach-owned provider seam types.

Fixture-backed mappings now cover:

| Rig shape | Yach mapping | Finding |
|---|---|---|
| `RawStreamingChoice::Message` | `ProviderStreamEvent::TextDelta` | Text deltas map directly. |
| `RawStreamingChoice::FinalResponse` | `ProviderStreamEvent::Completed` | Completion can be represented; `RigStreamMapper` carries prior message id into `provider_response_id`, but usage/finish detail requires real provider response inspection. |
| `RawStreamingChoice::ToolCall` / `RawStreamingToolCall` | `ProviderStreamEvent::ToolCallCompleted` / `ProviderToolCall` | Provider call id/name/argument JSON survive without yach executing tools. |
| `RawStreamingChoice::ToolCallDelta` | `ToolCallStarted` or `ToolCallDelta` | Rig exposes provider id plus internal id; yach preserves provider id and falls back to internal id when provider id is absent. Parallel tool-call ids remain distinct in fixture tests. |
| `RawStreamingChoice::MessageId` | accumulated in `RigStreamMapper` | Message id is available and can be carried into completion metadata without becoming canonical yach session state. |
| Reasoning events | currently ignored by thin mapper | Not part of P0 dogfood seam; can become an extension later without changing core UI/session ownership. |
| Stream cancellation | `ProviderStreamEvent::Cancelled` via adapter helper | Fixture mapping can represent cancellation without marking the turn completed; real Rig/provider abort behavior still needs network/provider evidence. |

Dependency note from `just dev cargo tree -p yach-backend -e normal --depth 2`: even with default features disabled, `rig-core` pulls in `reqwest`, `eventsource-stream`, `schemars`, `tracing`, `url`, and related normal dependencies. This is acceptable for the spike but should be measured before a durable dependency commitment.

Validation passed:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
just dev cargo tree -p yach-backend -e normal --depth 2
```

A follow-up lifecycle accumulator pass added fixture coverage for message id accumulation into completion metadata, provider-id fallback to Rig internal tool-call ids, parallel tool-call id preservation, and cancellation mapping without completion. Validation passed:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
```

## Supported Claim

Supported: Rig can be compiled below `yach-backend` and its raw streaming/tool-call fixture shapes can map into yach-owned provider seam types without leaking Rig types into `yach-ui`, `yach-proto`, or native session records. Message ids can remain provider metadata rather than canonical sessions, and tool-call ids can be preserved without yach surrendering tool execution.

Not supported: Rig is definitively chosen for the native backend, approved for credentials/network/native dogfood, or proven sufficient for real provider usage/finish/error/cancellation behavior yet.

## Real Provider Smoke Design

The proposed first real-provider smoke is documented in `docs/plans/2026-05-03-001-spike-rig-openai-compatible-smoke-plan.md`.

Key design points:

- Target an OpenAI-compatible endpoint rather than official OpenAI only.
- Use explicit env vars: `YACH_RIG_OPENAI_COMPAT_BASE_URL`, `YACH_RIG_OPENAI_COMPAT_API_KEY`, and `YACH_RIG_OPENAI_COMPAT_MODEL`.
- Add only an opt-in smoke command after approval: `smoke-rig-openai-compatible`.
- Keep the prompt tiny: `Reply with exactly: yach-rig-smoke-ok`.
- No TUI/native provider dogfood integration, default backend change, tools, resources, credential persistence, raw payload persistence, or retry loop.
- Stop if Rig requires an agent loop that owns history/tools/sessions, panic-prone env loading, persistent credential config, or provider-specific core protocol changes.

Implementation status:

- `smoke-rig-openai-compatible` validates missing environment variables before network.
- Direct OpenCode Zen curl checks succeeded for `/models` and streaming `/chat/completions` with `big-pickle`, proving credentials/connectivity and endpoint shape outside Rig. The streamed response spent the small token cap on reasoning and hit `finish_reason=length`, so later real smokes should use at least `YACH_RIG_OPENAI_COMPAT_MAX_TOKENS=128`.
- Rig OpenAI-compatible smoke failed against OpenCode Zen and OpenRouter with zero events and a collapsed HTTP client error at `/chat/completions`.
- A direct Rust `reqwest` OpenAI-compatible control command, `smoke-openai-compatible-http`, succeeded against the same env/endpoint shape: `status=200`, `content_type=application/json`, `matched_expected_text=true`, `response_chars=17`.
- Stock Rig Anthropic smoke, `smoke-rig-anthropic`, succeeded with Claude: `event_count=5`, `text_delta_count=2`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.
- Rig ChatGPT/Codex subscription OAuth smoke, `smoke-rig-chatgpt-subscription`, succeeded after device login: `event_count=4`, `text_delta_count=1`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.

Current interpretation: Rig can successfully stream through multiple stock provider paths, including API-key Anthropic and OAuth-backed ChatGPT/Codex subscription. Rust HTTP/OpenAI-compatible networking also works. The remaining failure is narrowed to the Rig OpenAI-compatible/OpenAI adapter path or our specific use of it, not to yach env validation, reqwest/TLS, OAuth, or provider endpoints generally. Do not let this block continuing with Anthropic and ChatGPT/Codex subscription paths; inspect the OpenAI-compatible/base-url API later and compare Rig non-streaming/direct-completion paths against the current agent `stream_prompt` path.

Follow-up implementation added a backend-internal `RigProviderAdapterConfig` / `RigProviderConfig` skeleton and `run_provider_request(...)` adapter entry point for the two currently working real-provider paths: Anthropic API key and ChatGPT/Codex subscription OAuth. It consumes yach-owned `ProviderRequest` and emits yach-owned `ProviderStreamEvent` without introducing Rig types into UI/protocol/session seams. Diagnostic CLI command `smoke-rig-provider-request` exercises this seam with `YACH_RIG_PROVIDER=anthropic` or `YACH_RIG_PROVIDER=chatgpt-subscription`. Manual runs succeeded for both providers: Anthropic produced `event_count=5`, `text_delta_count=2`, `completed=true`, `matched_expected_text=true`, `response_chars=17`; ChatGPT/Codex subscription produced `event_count=4`, `text_delta_count=1`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.

A subsequent explicit non-default TUI boundary, `yach tui --backend native-provider`, routes native prompt submissions through the same provider-request seam when provider env is configured. Human dogfood confirmed it launched and completed a chat/response turn successfully. The resulting `.yach/native-sessions/default.jsonl` persisted a user entry, assistant entry with provider metadata (`chatgpt-subscription`, `gpt-5.3-codex-spark`, `response_id=null`), and completed turn. Initial state/model list/status now advertise the selected native provider/model for `--backend native-provider` instead of fixture echo. Provider prompts now run in an abortable active-turn task: concurrent prompts are rejected while active, and `PromptCancelled` aborts the task and persists a cancelled turn marker. An opt-in `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` delay makes cancellation dogfood deterministic with fast models; user entries are flushed before provider calls so delayed/cancelled turns leave inspectable log evidence. Native/native-provider handshakes advertise `PromptCancellation`; human retest confirmed Ctrl+C cancellation works with the delay hook. Pi remains the default backend; fixture native mode remains `--backend native`; no provider tools/resources, retry loop, raw payload persistence, or default-backend change were added.

Provider-failure dogfood follow-up added fixture/unit coverage for auth failure, unavailable/invalid model, timeout, and network error classification. Provider stream timeouts now map to `ProviderErrorKind::Timeout` instead of cancellation, common API-key/header shapes are redacted before debug persistence, and native failed-turn reasons persist the normalized error kind plus redacted debug context. Manual owner-run provider-request failure evidence then confirmed both working providers still pass happy-path controls and return invalid-model failures through Rig with zero stream events:

- Anthropic control: `event_count=5`, `text_delta_count=2`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.
- Anthropic invalid model: failed before events with provider 404 body containing `not_found_error` and the invalid model name.
- ChatGPT/Codex subscription control: `event_count=4`, `text_delta_count=1`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.
- ChatGPT/Codex subscription invalid model: failed before events with provider 400 body saying the invalid model is not supported for Codex with a ChatGPT account.

The classifier now treats these `not_found` / `not supported` model-shaped errors as unavailable-model failures, and the CLI failure summary includes the normalized provider error kind. No credentials or raw payloads are persisted by yach; the manually pasted evidence contained no secrets.
