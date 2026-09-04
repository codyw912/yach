# OpenAI Responses Provider-Native Compactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development` (recommended) or `executing-plans` to implement this plan task-by-task. Use `test-driven-development` for each behavioral slice, `requesting-code-review` before publication, and `verification-before-completion` before any completion claim.

**Goal:** Add model-capability-gated OpenAI `/responses/compact` support that preserves OpenAI's opaque response items exactly, always retains the existing portable text summary, safely replays native windows across turns/resume, and cannot repeat already-executed tools after failure or cancellation.

**Architecture:** Keep the session JSONL append-only and authoritative for human-readable history. Add a versioned native artifact only to compaction checkpoint `details`, plus a process-local, shared replay store for post-checkpoint raw round pairs. The runner owns one canonical Responses input-chain assembler and passes its `NativeRequestEnvelope` to the native compactor and, after a native checkpoint exists, to normal turn requests. A vendored, upstreamable Rig patch supplies raw input passthrough, typed request send methods, and value-equal terminal output capture; yach isolates those APIs in `responses_replay.rs`.

**Tech Stack:** Rust workspace, Tokio, reqwest 0.13, serde/serde_json, rig-core 0.41 vendored through `[patch.crates-io]`, Jujutsu, `just`/devenv.

**Accepted design:** `docs/project/specs/2026-08-06-responses-native-compactor-design.md`

**Research/API sources:**
- `docs/project/records/2026-08-05-responses-native-compactor-research.md`
- `https://developers.openai.com/api/reference/resources/responses/methods/compact`
- `https://developers.openai.com/api/docs/guides/conversation-state#compaction-advanced`

## Global constraints

- Run Rust commands through `just`: `just dev cargo ...`, `just test`, `just fmt-check`, `just lint`.
- Use Jujutsu only. Do not run `git add` or `git commit`. End each completed task with `jj describe -m "<intent>" && jj new`.
- No credential bytes in `Debug`, status, checkpoint details, test failures, or fixture observations.
- Unknown/absent `responses_compact` is unsupported. Never probe capability at runtime.
- Native replay is valid only in the same session when the active connection is OpenAI Responses, the exact model id matches, and capability is `Some(true)`.
- A successful native compaction is not a replacement for the summary pass. The checkpoint must contain both real portable summary text and `details.native`.
- Never append the fallback kept tail to a matching native window. The compact endpoint saw the full window and its returned `output` wholly replaces it.
- Never reconstruct a completed OpenAI model round from stream deltas. Capture `response.completed.response.output` raw and ordered.
- Commit replay state only at completed round-pair granularity: terminal model `output` plus one `function_call_output` for every call.
- Do not persist raw post-checkpoint round outputs as new session events in this slice.
- Do not adopt automatic `context_management`, Conversations state, or `previous_response_id` session authority.
- Do not add Anthropic native compaction; it has separate artifact and replay semantics.

---

### Task 1: Vendor Rig and add exact Responses passthrough

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Add: `vendor/rig-core/**` (copy the resolved `rig-core 0.41.0` crate, including its license/readme)
- Modify: `vendor/rig-core/src/providers/openai/responses_api/mod.rs`
- Modify: `vendor/rig-core/src/providers/openai/responses_api/streaming.rs`

- [ ] **Step 1: Create the vendored source without changing behavior**

Copy the exact crate currently resolved at `.devenv/state/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rig-core-0.41.0` into `vendor/rig-core`. Add:

```toml
[patch.crates-io]
rig-core = { path = "vendor/rig-core" }
```

Refresh the lockfile with `just dev cargo check -p yach-backend`. Confirm `cargo metadata` resolves `rig-core` from `vendor/rig-core` and the unmodified workspace tests still compile.

- [ ] **Step 2: Write failing Rig tests for raw input passthrough**

In `responses_api/mod.rs`, add tests proving:

1. `InputContent::Unknown(Value)` deserializes an unknown `type` object and serializes value-equal.
2. `InputItem::unknown(value)` serializes a caller-supplied known or unknown item byte/value-equivalently, including extra fields on a known `message` item.
3. Malformed/non-object raw input fails at the `InputItem` object boundary rather than emitting an invalid request.

Run the targeted vendored-crate tests via:

```bash
just dev cargo test --manifest-path vendor/rig-core/Cargo.toml responses_api::tests::input_content_unknown
```

Expected: fail because the variant/constructor do not exist.

- [ ] **Step 3: Implement `InputContent::Unknown` with hand-written serde**

Replace the derived internally-tagged serde on `InputContent` with a private derived `KnownInputContent` helper plus manual `Serialize`/`Deserialize`:

```rust
pub enum InputContent {
    Message(Message),
    Reasoning(OpenAIReasoning),
    FunctionCall(OutputFunctionCall),
    FunctionCallOutput(ToolResult),
    Unknown(serde_json::Value),
}
```

Known variants retain their current wire format. Unknown `type` values retain the original `Value`; malformed known variants remain errors. Add `pub fn InputItem::unknown(value: Value) -> Self` with no role overlay so caller-supplied known items with provider-added fields also pass through value-equally. Do not alter Rig's generic message types.

- [ ] **Step 4: Write failing tests for caller-built typed request sends**

Use Rig's existing mock HTTP clients to test public Responses model methods that accept a caller-built `responses_api::CompletionRequest` for both non-streaming and streaming sends. The observed request body must contain the caller's raw `input` unchanged and must retain tools, `store`, output budget, and instructions from the typed request.

Expected: fail because only generic `completion::CompletionRequest` entry points are public.

- [ ] **Step 5: Expose narrow typed request APIs**

Make `GenericResponsesCompletionModel::create_completion_request` public, then extract the existing send bodies into public inherent methods with unambiguous names:

```rust
pub async fn completion_with_request(
    &self,
    request: responses_api::CompletionRequest,
    record_telemetry_content: bool,
) -> Result<completion::CompletionResponse<CompletionResponse>, CompletionError>;

pub async fn stream_with_request(
    &self,
    request: responses_api::CompletionRequest,
    record_telemetry_content: bool,
) -> Result<streaming::StreamingCompletionResponse<StreamingCompletionResponse>, CompletionError>;
```

The existing trait methods must build the typed request and delegate, preserving behavior for all existing callers.

- [ ] **Step 6: Write the failing terminal-output fidelity test**

Feed an SSE `response.completed` whose `response.output` contains:

- a known `message` item with an extra provider field;
- a reasoning item with encrypted content;
- an unknown hosted-tool item.

Assert the terminal `StreamingCompletionResponse.output: Vec<Value>` is ordered and value-equal to the raw event array. Re-encode those values through `InputItem::unknown`; assert the next typed request's `input` is value-equal.

- [ ] **Step 7: Capture raw terminal output before typed decoding loses fields**

Add `pub output: Vec<serde_json::Value>` to Responses `StreamingCompletionResponse`. Extend `RawChoiceAccumulator` with the raw ordered output. In `record_response_chunk`, parse `raw_event_data`, extract `/response/output` only for `response.completed`, and retain the raw array before the typed `CompletionResponse` projection drops provider additions. `finish` carries that array on the final payload. Missing output becomes an empty array; malformed terminal output remains a completion error, not a fabricated window.

Update every Rig-local `StreamingCompletionResponse` literal. Run the complete vendored crate suite:

```bash
just dev cargo test --manifest-path vendor/rig-core/Cargo.toml
```

- [ ] **Step 8: Checkpoint the dependency patch**

```bash
jj describe -m "build: vendor rig Responses passthrough patch"
jj new
```

---

### Task 2: Add catalog capability truth and `auto` configuration

**Files:**
- Modify: `crates/yach-catalog/src/lib.rs`
- Modify: `crates/yach-catalog/data/catalog.json`
- Modify: `crates/yach-cli/src/catalog_refresh.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-cli/src/provider_connections.rs`
- Modify: `crates/yach-backend/src/compaction.rs`
- Modify: `crates/yach-backend/src/runner.rs`

- [ ] **Step 1: Add failing catalog precedence/schema tests**

Cover these observable contracts:

- `CatalogEntry.responses_compact: Option<bool>` resolves per field as project > user > fetched > baked, preserving explicit `false` as a disable.
- absent at every layer resolves to `None`, never inferred from provider/model spelling;
- transformed models.dev data carries a source `responses_compact` boolean when one exists;
- a cache missing the new capability schema marker is stale and refreshes;
- JSON round-trip preserves the marker and field.

Use `ModelProfile.responses_compact: Option<Sourced<bool>>`; callers engage native only on `Some(Sourced { value: true, .. })`.

- [ ] **Step 2: Implement the catalog field and schema marker**

Add a separate version constant/JSON marker for `responses_compact`, update `Catalog::empty`, cache validation, fetched transformation, and `resolve`. Replace catalog refresh's tool-call-only freshness check with a check requiring every current capability schema marker; keep diagnostics explicit about the stale marker.

Mark only model ids documented by OpenAI. The current compact OpenAPI example explicitly names `gpt-5.1-codex-max`; add a sparse baked row if the models.dev snapshot lacks it. Do not infer aliases or mark all `gpt-5*` models. If the current official model reference names additional compact-compatible ids during implementation, record the exact source and add only those ids.

- [ ] **Step 3: Thread capability through activation and switching**

Add `responses_compact: Option<bool>` to backend `CatalogModelEntry` and `ProviderConfig`. Populate it from the same resolved `ModelProfile` used for window/output budget in:

- environment provider setup;
- connection activation/replacement;
- discovered and curated catalog rows;
- model selection.

On an unlisted model switch, set capability to `None` even though numeric window/budget fields retain their existing floor. Capability must fail closed because stale `true` is unsafe.

Update all struct literals found by rust-analyzer references, including fixtures.

- [ ] **Step 4: Change compactor config default and parsing tests**

Set `CompactionConfig::default().compactor` to `"auto"`. Accept exactly:

- `auto`;
- `summary`;
- `openai-responses`.

Retain today's fail-closed behavior for any unknown name: visible warning and no checkpoint. Add config tests for default, project override, forced native, summary, and unknown.

- [ ] **Step 5: Verify focused crates and checkpoint**

```bash
just dev cargo test -p yach-catalog
just dev cargo test -p yach-cli catalog_refresh
just dev cargo test -p yach-backend compaction
jj describe -m "feat: add Responses compaction capability data"
jj new
```

---

### Task 3: Introduce the native envelope and isolated replay seam

**Files:**
- Add: `crates/yach-backend/src/responses_replay.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/provider.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-backend/Cargo.toml`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-cli/src/provider_connections.rs`

- [ ] **Step 1: Add failing envelope/message-conversion tests**

Define the target public request shape in `provider.rs`:

```rust
pub struct NativeRequestEnvelope {
    pub input: Vec<serde_json::Value>,
    pub instructions: String,
}

pub struct ProviderRequest {
    // existing fields
    pub native_request: Option<NativeRequestEnvelope>,
}
```

Tests must prove:

- system messages become byte-exact top-level `instructions` using the same join/default rule as today's Rig request;
- user/assistant/tool messages become Responses input items in the same order and shape as Rig's existing conversion;
- function calls retain provider call ids and JSON arguments;
- function outputs use the exact provider-visible text returned by `provider_tool_result_block`;
- serialization keeps `native_request` absent for legacy requests.

Expected: fail before the types/seam exist.

- [ ] **Step 2: Implement `responses_replay.rs` as the only Rig-specific yach seam**

Move/factor the provider-message-to-Rig-message conversion so both `rig_adapter::rig_messages_from_request` and native input assembly use one implementation. Convert Rig messages to patched `responses_api::InputItem` through its existing `TryFrom<Message>`, then serialize each item to `Value`. Do not hand-maintain a second Responses wire schema in the runner.

Centralize:

- `instructions_from_messages`;
- `input_items_from_messages`;
- `function_call_output_items`;
- `NativeReplayTarget { session_id, provider, model }`;
- `NativeReplayState { target, instructions, input, synced_event_count }`;
- a short-lock shared store `Arc<std::sync::Mutex<Option<NativeReplayState>>>`;
- versioned `NativeCompactionArtifact` parse/serialize and target matching.

No mutex guard may cross an await.

- [ ] **Step 3: Add raw output as an internal stream event**

Add:

```rust
ProviderStreamEvent::ResponseOutput {
    turn_id: TurnId,
    items: Vec<serde_json::Value>,
}
```

Update `turn_id`, lifecycle-boundary/buffer handling, exhaustive matches, and tests. This event is internal replay evidence; it is not rendered or written as a new session event.

Extend the provider-round collector with `raw_output: Option<Vec<Value>>`. Multiple terminal output events or a terminal event after completion are malformed-stream errors.

- [ ] **Step 4: Capture OpenAI final payload and send replay input through patched Rig**

Refactor `PreparedCompletion::run` to accept a final-payload extractor. Other providers return `None`. The OpenAI Responses branch extracts `StreamingCompletionResponse.output` and emits `ResponseOutput` immediately before `Completed`.

When `ProviderRequest.native_request` is present on the OpenAI Responses branch:

1. build the normal generic Rig completion request so tools, output budget, telemetry, and provider defaults remain unified;
2. call public `create_completion_request`;
3. replace only `input` with `OneOrMany<InputItem::unknown(...)>` and set exact `instructions`;
4. explicitly retain stateless behavior with `additional_parameters.store = Some(false)`;
5. send with `stream_with_request`.

Reject `native_request` on any non-Responses provider as `InvalidRequest`; never silently flatten it. Add optional `base_url` to `RigProviderConfig::OpenAi` so the same real adapter can target a local fixture; production defaults to `https://api.openai.com/v1`. Keep `OpenAiCompatible` on the Completions API.

- [ ] **Step 5: Migrate request literals and run focused tests**

Set `native_request: None` at every legacy `ProviderRequest` construction identified by rust-analyzer. Add tests that legacy Anthropic/OpenAI-compatible/ChatGPT requests are byte/behavior unchanged and OpenAI replay uses raw input.

```bash
just dev cargo test -p yach-backend rig_adapter
just dev cargo test -p yach-backend provider_request
jj describe -m "feat: add exact Responses replay request seam"
jj new
```

---

### Task 4: Implement the OpenAI native compactor and dispatch

**Files:**
- Modify: `crates/yach-backend/src/compaction.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Extend compaction preparation with authenticated context**

Add:

```rust
pub struct CompactionProviderContext {
    pub provider: String,
    pub wire: String,
    pub model: String,
    pub responses_compact: Option<bool>,
    pub adapter: Arc<RigProviderAdapterConfig>,
}

pub struct CompactionPreparation {
    // existing fields
    pub provider: Arc<CompactionProviderContext>,
    pub native_request: Option<NativeRequestEnvelope>,
}
```

Remove `PartialEq/Eq` from `CompactionPreparation` if the authenticated adapter makes those derives dishonest; compare observable fields in tests instead. Update CLI smoke and test literals with explicit contexts.

- [ ] **Step 2: Write failing fixture tests for `OpenAiResponsesCompactor`**

Using the repo's `TcpListener` fixture pattern, assert:

- request path is `/v1/responses/compact` (or `<base_url>/responses/compact` without duplicate slashes);
- bearer auth is present but never printed;
- JSON body is exactly `{ model, input, instructions }` for the contract fields;
- response `output` is retained value-equal as `details.native.window`;
- artifact contains `version: 1`, `provider: "openai"`, `wire: "openai-responses"`, exact model id, and window;
- timeout, non-2xx, malformed JSON, missing/non-array output, and unsupported provider return distinct redacted `CompactionError` variants.

- [ ] **Step 3: Implement a native-only `Compactor` outcome**

Keep the existing mandatory summary request runner-owned. Do not introduce a `SummaryCompactor`: today's summary call needs the live generic `ProviderRequester`, status channel, and existing response collection path, none of which the owned `CompactionPreparation` trait input carries.

Cleanly complete the trait seam for provider-native artifacts instead. Replace the unused summary-shaped outcome with:

```rust
pub struct NativeCompactionOutcome {
    pub artifact: NativeCompactionArtifact,
}

pub type CompactionFuture = Pin<
    Box<dyn Future<Output = Result<NativeCompactionOutcome, CompactionError>> + Send>,
>;
```

Update `Compactor` documentation to state that implementations produce a provider-native replacement artifact; core still owns cut selection, the mandatory portable summary call, accounting, detail merging, and checkpoint writes. Remove `CompactionOutcome` rather than leaving a compatibility alias: it has no implementations or production callers. Replace its summary-only error with the redacted native HTTP/unsupported/timeout/decode taxonomy required below.

Implement `OpenAiResponsesCompactor: Compactor` using `reqwest::Client::builder().timeout(adapter.timeout)`, the OpenAI base URL from resolved adapter config, and `ProviderSecret::with_exposed` only while constructing the bearer-auth request. Parse the compact response as raw JSON. Do not deserialize the output into Rig types.

- [ ] **Step 4: Write failing dispatch/fallback tests**

Cover the complete matrix:

| config | provider/capability | native result | expected |
|---|---|---|---|
| `summary` | any | n/a | summary only |
| `auto` | OpenAI Responses + `Some(true)` | success | native + summary |
| `auto` | unsupported/unknown | n/a | summary, no warning |
| `auto` | supported | native failure | summary, no forced-native warning |
| `openai-responses` | supported | success | native + summary |
| `openai-responses` | unsupported/failure | n/a/error | visible `native compaction unavailable (<reason>); used summary` |
| unknown | any | n/a | existing fail-closed warning, no checkpoint |

Also assert a native success followed by a summary failure writes no checkpoint and does not replace replay state.

- [ ] **Step 5: Refactor `run_compaction` around one preparation and mandatory summary**

Replace the boolean return with an explicit application result:

```rust
enum CompactionApplication {
    NotApplied,
    Summary,
    Native,
}
```

Select the cut once and build one `CompactionPreparation` from the canonical envelope of the actual request that triggered compaction. Native, when selected, receives that full `native_request.input`, not `cut.fold_range`. This matters mid-turn: the current continuation request contains completed assistant/tool rounds and narrative that are not yet fully represented by final session entries. Dispatch only the optional native call through `&dyn Compactor`; keep the existing summary request construction/send/collection in the runner and run it after native success or failure. Hold the optional `NativeCompactionOutcome` locally until the summary returns real text. Only then merge file details with `artifact` under `details.native` and write one checkpoint.

On native failure, leave the log/replay state untouched until summary succeeds. A summary-only checkpoint invalidates native replay state so the next turn rebuilds from `[summary] + kept tail`. A successful native checkpoint atomically replaces replay state with the returned window and sets its synchronized event index after the checkpoint event.

Use the returned window's serialized token estimate for native `tokens_after_estimate`; retain today's summary + kept-tail estimate on summary fallback. Status text distinguishes `context compacted (provider)` from summary compaction. Callers use `CompactionApplication` to choose the correct refill path rather than inferring it from config.

- [ ] **Step 6: Run focused tests and checkpoint**

```bash
just dev cargo test -p yach-backend compaction
just dev cargo test -p yach-backend native_compactor
jj describe -m "feat: add OpenAI Responses compactor dispatch"
jj new
```

---

### Task 5: Build the canonical chain for turns, repeat compaction, and resume

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-backend/src/responses_replay.rs`
- Modify: `crates/yach-backend/src/compaction.rs`

- [ ] **Step 1: Factor log conversion over an explicit event slice**

Extract the body of `provider_messages_from_log` into a helper that accepts:

- the complete log for turn-outcome/pairing knowledge;
- an explicit event slice/range;
- the current turn id;
- whether to prepend a checkpoint summary.

The existing summary path must remain byte-identical. This helper is also the lossy resume path for post-checkpoint turns.

- [ ] **Step 2: Write failing canonical-base tests**

Test all three accepted bases:

1. no checkpoint -> full converted log;
2. summary-only/non-matching native checkpoint -> summary + kept tail + post-checkpoint events;
3. matching native artifact -> `window + events strictly after the checkpoint event`, with no first-kept slice duplication.

Add malformed artifact, wrong model, wrong provider/wire, capability removed, and different session cases; each silently chooses summary base. Add native-fail -> summary checkpoint -> later-native-success and second-native-compaction tests proving no content is dropped or duplicated.

- [ ] **Step 3: Implement the runner-owned assembler**

Create one `assemble_native_replay_request` in `runner.rs`. It accepts the actual current `ProviderRequest.messages`, not only a session log, so a first mid-turn compact sees all completed continuation rounds:

- target is session/provider wire/model, matching the artifact schema exactly;
- current instructions come from the exact normal request messages;
- if the shared in-memory state matches, append only the current request's not-yet-synchronized converted suffix;
- otherwise, if the newest checkpoint has a matching valid artifact, start at its window and append post-checkpoint events/current request suffix;
- otherwise convert the complete current request messages (which already embody the no-checkpoint or summary fallback base);
- update the state's instructions and synchronized event index only when native replay is active;
- return a cloned `NativeRequestEnvelope` for the request/compactor.

The shared replay store is created once in `run_native_loop`, cloned into prompt tasks and manual compaction, and short-locked for state transitions. It must outlive a prompt task so hard abort cannot erase already-committed round pairs.

- [ ] **Step 4: Assemble before threshold selection and use native estimates**

Build the initial ordinary `ProviderRequest` before the pre-turn threshold check, then derive the canonical envelope from that exact request. Attach it to `ProviderRequest.native_request` and estimate instructions + raw input JSON only when a matching native checkpoint/replay state is already active. Before the first native checkpoint, turns keep the ordinary Rig request and estimator; the envelope exists only as the prospective compact call's current full converted input.

Refresh the prospective envelope whenever `next_request` gains a completed tool round. Pass it into threshold/overflow/mid-turn compaction. After native success, rebuild/attach from the returned window and do not append `mid_turn_text` to the native envelope because that narrative was already in the compact input. Keep today's summary refill plus `mid_turn_text` only in the fallback `messages` field. Retry clones an already-active envelope unchanged; summary fallback detaches it. This preserves the invariant that first compaction input is converted from the live request and opaque round capture becomes authoritative only after a native checkpoint exists.

- [ ] **Step 5: Commit successful round outputs**

For every completed OpenAI round while a matching native checkpoint/replay state is active:

- require exactly one `ResponseOutput` before `Completed`;
- final no-tool round: append its raw output to the in-memory chain;
- tool round: defer state mutation until tool execution yields one result per call, then append raw output followed by generated `function_call_output` items as one atomic round-pair commit;
- set `synced_event_count` after the corresponding tool events;
- on absent/malformed terminal output, invalidate native replay, warn once, and continue through the ordinary Rig conversion rather than fabricating opaque state.

Before the first native checkpoint, the adapter may surface `ResponseOutput`, but the runner ignores it for replay authority and reconstructs from the append-only log as specified.

At prompt finalization, advance the synchronized index past the assistant entry/`TurnFinished` so the next turn does not append a lossy duplicate of the raw final output.

- [ ] **Step 6: Make manual `/compact` use the same static context**

Factor current static-context assembly so the manual branch can build the same baseline + project/extension system messages as a normal turn. Initialize/refresh the replay assembler from that request before `run_compaction`. Assert manual compact `instructions` are byte-equal to a normal turn's instructions.

Apply focus with one helper that clones the native envelope and appends only:

```text


Additional compaction focus: <user focus>
```

The replay state's normal instructions remain unchanged after the compact call. The existing summary prompt receives focus as today.

- [ ] **Step 7: Run chain tests and checkpoint**

```bash
just dev cargo test -p yach-backend native_replay
just dev cargo test -p yach-backend compaction
jj describe -m "feat: replay native compaction windows across turns"
jj new
```

---

### Task 6: Make tool batches produce a structurally complete round pair

**Files:**
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-backend/src/responses_replay.rs`

- [ ] **Step 1: Write failing partial-failure tests**

Create a three-call round where call 1 succeeds, call 2 fails validation/execution, and call 3 never starts. Assert:

- the batch returns three results in call order;
- statuses are completed, failed, cancelled;
- the successful result is not discarded;
- every recorded request has exactly one `ToolExecutionFinished`;
- native round-pair output has model raw items, real output 1, error output 2, cancelled output 3;
- the continuation is not sent after a terminal batch error.

Also cover a failure while persisting or emitting UI progress: preserve all constructible results and return the original terminal error after commit.

- [ ] **Step 2: Replace fail-fast result loss with an explicit batch outcome**

Introduce:

```rust
struct ProviderToolBatchOutcome {
    results: Vec<ProviderToolResult>,
    terminal_error: Option<ProviderRoundError>,
}
```

Pre-build stable `PendingToolRequest`s for every provider call. On the first terminal error:

- retain earlier successes;
- create/record one failed or cancelled provider-visible result for the current request if execution did not already finish it;
- create/record synthetic cancelled results for all unstarted calls;
- persist the completed event prefix;
- return the full result vector plus terminal error.

Use a single helper to add missing request/result events without duplicating events already recorded by a tool implementation. Synthetic content must be explicit and actionable, not empty.

- [ ] **Step 3: Commit before propagating the batch error**

The caller converts all batch results to Responses `function_call_output` values, commits the raw model output + all outputs to shared replay state, then returns `terminal_error`. This makes side effects visible on the next turn even though the current turn fails/cancels.

- [ ] **Step 4: Admit paired evidence from failed/cancelled turns on rebuild**

Change `provider_messages_from_log` to admit any persisted finished turn (`Completed`, `Failed`, `Cancelled`). Emit tool-call/result pairs only when both events exist; ignore an orphaned trailing request from a crash/hard-abort log. Add tests proving:

- completed pairs from a failed/cancelled turn replay;
- an orphan call is trimmed;
- a result never appears without its call;
- restart after one success + later failure does not ask the model to repeat the success.

- [ ] **Step 5: Verify and checkpoint**

```bash
just dev cargo test -p yach-backend tool_batch
just dev cargo test -p yach-backend provider_messages_from_log
jj describe -m "fix: retain complete tool batch evidence"
jj new
```

---

### Task 7: Add cooperative prompt cancellation with a hard-abort backstop

**Files:**
- Modify: `crates/yach-backend/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/yach-backend/src/runner.rs`
- Modify: `crates/yach-backend/src/shell.rs` only if a regression test exposes a missing process-group kill

- [ ] **Step 1: Add Tokio cancellation support and failing lifecycle tests**

Add `tokio-util = "0.7"` to yach-backend and use `tokio_util::sync::CancellationToken`.

Tests:

- cancel during provider stream drops an incomplete model round and persists `TurnFinished(Cancelled)`;
- cancel after one completed tool gives later calls synthetic cancelled outputs and preserves the first real result;
- cancel while bash runs drops the executor future and kills the process group;
- cancel while review waits finalizes without waiting for a decision;
- an intentionally token-ignorant task exceeds grace, is hard-aborted, and uses generic cancellation persistence;
- already-finished active handles are joined, not dropped.

- [ ] **Step 2: Thread a shared token through active prompts**

`ActiveProviderTurn` owns a token clone. Pass it through `handle_started_native_provider_prompt`, `ProviderPromptRequest`, `ProviderAgentToolRound`, `ProviderAgentToolBatch`, and `CompactionRun`.

Observe it with `tokio::select!` at await boundaries:

- provider stream/retry;
- native/summary compaction;
- tool review;
- async edit/bash tool execution;
- between every tool call.

Dropping `HostCommandExecutor::run` must retain its existing `kill_on_drop` + process-group guard behavior.

- [ ] **Step 3: Cooperatively finalize, then backstop**

Extract cancellation handling from the event loop:

1. call `token.cancel()`;
2. wait a bounded grace interval for the task;
3. on normal completion, replace outer `session_log` with the returned log (which already contains real/synthetic tool results and its own cancelled turn);
4. on timeout, call `abort()`, await it, persist the generic cancelled turn, then reload the session log from `JsonlSessionStore` so tool events persisted before abort remain in memory;
5. do not append a second `TurnFinished` when cooperative finalization succeeded.

Use a small testable grace constant/function; do not make production cancellation unbounded.

- [ ] **Step 4: Verify cancellation and checkpoint**

```bash
just dev cargo test -p yach-backend cancellation
just dev cargo test -p yach-backend host_executor
jj describe -m "fix: finalize provider turns cooperatively on cancel"
jj new
```

---

### Task 8: Add end-to-end fixture coverage and live smoke

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` tests or add a focused test module under `crates/yach-backend/src/`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `justfile` only if a dedicated smoke recipe improves the existing interface

- [ ] **Step 1: Build one local OpenAI fixture for both endpoints**

Reuse the repo's segmented-request-safe `TcpListener` conventions. The fixture must:

- authenticate `/v1/responses` and `/v1/responses/compact`;
- record request bodies without storing/printing the bearer token;
- stream terminal `response.completed` arrays containing known items with extra fields, encrypted reasoning, function calls, and final messages;
- return configurable compact windows/failures;
- support a deliberately stalled response for cancellation tests.

- [ ] **Step 2: Add the full native lifecycle test**

Exercise through the real Rig adapter and runner:

1. capability-on OpenAI Responses turn with an interleaved two-round tool exchange;
2. threshold native compaction;
3. portable summary and native artifact in the same checkpoint;
4. next turn input equals returned window + committed post-checkpoint round pairs + new user input;
5. second native compaction has no kept-tail duplication;
6. process restart reconstructs post-checkpoint tail from JSONL;
7. A -> B -> A model switching falls back on B and safely re-engages A;
8. all `/responses` requests keep `store: false`;
9. compact/turn instructions are equal, with manual focus as the only compact delta.

Assert request JSON structurally/value-equally; do not assert source text or incidental formatter output.

- [ ] **Step 3: Add failure/recovery fixture cases**

Cover:

- unsupported/timeout/HTTP/decode compact failures -> summary fallback;
- forced-native warning vs silent `auto` fallback;
- native success + summary failure -> no checkpoint/state replacement;
- malformed checkpoint artifact on resume;
- response replay error mid-turn -> completed prefix retained and retry reuses it;
- cancel after one tool -> restart -> next turn sees real and cancelled results;
- hard-abort backstop.

- [ ] **Step 4: Extend the existing smoke idiom**

Add `yach smoke-responses-compaction` (or extend `smoke-compaction` without breaking its current output contract) to perform against a real OpenAI key:

- normal Responses turn on a capability-marked model;
- native compact call;
- portable summary call;
- replayed continuation from returned window;
- A -> B -> A selection/replay check when `YACH_RIG_OPENAI_SMOKE_ALT_MODEL` is present.

Print only model ids, request stages, artifact item counts, token counts, and pass/fail labels. Never print request bodies, encrypted content, or credentials.

- [ ] **Step 5: Run fixture suite and, when credentials are available, live smoke**

```bash
just dev cargo test -p yach-backend responses_native_compaction
just dev cargo test -p yach-cli smoke_responses_compaction
just dev cargo run -p yach-cli -- smoke-responses-compaction
```

The real-key command is required evidence before publication when `YACH_RIG_OPENAI_API_KEY` is available. If unavailable, report that exact missing prerequisite; do not claim live verification.

- [ ] **Step 6: Checkpoint integration coverage**

```bash
jj describe -m "test: cover Responses native compaction lifecycle"
jj new
```

---

### Task 9: Clean up, verify the workspace, and measure the slice

**Files:**
- Modify only as required by formatter/lints or review findings
- Add: `docs/project/records/2026-08-07-responses-native-compactor-measurement.md`
- Modify: `docs/project/board.md`
- Modify: `docs/project/next.md` only if this changes the recommended next move/status

- [ ] **Step 1: Run `simplify` over the complete diff**

Remove duplicated conversion/dispatch branches, stale comments, unused helpers, compatibility aliases, and any direct Responses JSON construction outside `responses_replay.rs` and the HTTP compactor body. Re-run focused tests after any behavioral simplification.

- [ ] **Step 2: Run formatter, lints, and full suite**

```bash
just fmt
just fmt-check
just lint
just test
```

Do not claim success unless every command exits 0. Inspect `jj diff` and `jj log -r 'main..@'`; ensure the stack contains only this slice and the vendored Rig source.

- [ ] **Step 3: Run the changed path, not only tests**

Run the local fixture smoke through the CLI/backend entry point. Run the real-key smoke if credentials exist. Capture exact commands, model ids, stage outcomes, item counts, and status labels.

The 125-cell provider matrix is not a per-slice requirement (`docs/project/next.md:29-32`). Do not run it here; its driver credential fix remains the pre-release prerequisite.

- [ ] **Step 4: Write the measurement record**

Record:

- accepted design and implementation revisions;
- focused/full command outputs;
- fixture scenarios and results;
- live smoke result or exact missing credential prerequisite;
- native vs summary selection/fallback observations;
- known residual risk: post-checkpoint raw suffixes are lossy across restart until the next native compaction;
- upstream Rig PR URL/status.

Update the board item from designed to measured only if all required non-secret evidence exists. Update `next.md` to the next queue item (sweep-driver credential fix, then masking slice 2) only after the implementation/measurement lands.

- [ ] **Step 5: Final checkpoint**

```bash
jj describe -m "docs: measure Responses native compaction"
jj new
```

---

### Task 10: Review and publish yach plus the upstream Rig patch

**Files:**
- No planned product-code changes; review fixes may touch files above

- [ ] **Step 1: Request two-stage code review**

First review spec compliance against every owner decision and failure-taxonomy row. Then review implementation quality/security, emphasizing:

- credential redaction;
- raw encrypted artifact persistence scope;
- no duplicate kept tail;
- no unpaired tool calls;
- cancellation side-effect visibility;
- exact raw value preservation;
- fallback behavior under capability/config mismatch.

Fix all Critical/Important findings, rerun affected focused tests, then rerun `just fmt-check`, `just lint`, and `just test` once.

- [ ] **Step 2: Shape the Jujutsu stack**

Inspect:

```bash
jj status
jj diff
jj log -r 'main..@'
```

Squash/rebase into reviewable intent commits. Keep the vendored upstream patch separable from yach integration where practical. Create a publication bookmark, push with `jj git push`, and open the yach PR. Include the accepted spec, measurement record, verification commands, and full upstream Rig PR URL in its body.

- [ ] **Step 3: Offer the Rig changes upstream**

Create a clean patch/branch against Rig's current upstream default branch containing only:

- `InputContent::Unknown` passthrough;
- caller-built typed request send APIs;
- terminal raw ordered output capture;
- their Rig-local tests.

Do not include yach-specific types or behavior. Open the upstream PR and link it from the yach PR and measurement record. If upstream moved since 0.41, rebase the additive patch without broad refactors; yach remains pinned to the reviewed vendored source until an upstream release contains the APIs.

- [ ] **Step 4: Verify publication state**

Confirm yach PR URL, CI status, bookmark stack, and upstream PR URL. Before handoff, run:

```bash
jj log -r 'main..@'
```

Return both full PR URLs and any still-running CI check by exact name/status.
