# Rig Upgrade: Own The Loop

Date: 2026-07-31

Status: draft for owner review. Implements the owner decision of
2026-07-31 (board, provider thread): yach owns its tool loop, so the
upgrade drops rig's Agent abstraction rather than porting onto it.

## Context

yach pins `rig-core` 0.38.2, aliased as `rig`. The upgrade was queued
on the owner principle that building around an old version of a
fast-moving dependency is the wrong shape while yach is early
(2026-07-29), with the `additional_params` workaround for
`max_completion_tokens` explicitly "works for now, not the resting
state".

Two earlier readings of the migration cost were wrong, both from
diffing symbols instead of compiling, and the record is corrected here:

- `stream_chat` disappearing was reported as a cost. yach never used
  it. Irrelevant.
- The `agent` module disappearing was reported as a rewrite of request
  construction and streaming. It never disappeared. Upstream PR 2197
  split rig into `rig-core` (portable contracts: clients, messages,
  streaming, tools) and `rig-agent` (the classic runtime: agent
  builder, multi-turn driver, hooks), behind a `rig` facade
  re-exporting both. yach depends on `rig-core` directly, so the
  compile that "proved" the module was gone was looking at the half
  that never had it.

## Decision and why

**yach owns the loop, so the Agent abstraction goes.**

The forcing argument is extensions: they register tools yach executes,
which a provider-side driver cannot know about. The settling argument
is everything else the loop does between rounds — review/approval
gating per call, the sensitive-path deny chokepoint, session
persistence of every tool request and result for replay fidelity,
compaction between rounds, streamed tool output to the TUI, budget
accounting. None of that is expressible in rig's driver. Handing over
the loop would cost the product, not a customization.

The confirming evidence is that the Agent is already vestigial here:
**yach never gives rig executable tools.** It passes `ToolDefinition`s
for advertising and executes every call itself, so the Agent has only
ever been a request builder and a stream source. That is also why the
current code reads awkwardly at the seam — `stream_completion` ->
builder -> apply tool definitions -> `.stream()` is yach reaching past
the abstraction to get at the request underneath.

Staying on the runtime would additionally mean fighting it: 0.41's
`StreamingPromptRequest` is a multi-turn driver (`max_turns`,
`tool_concurrency`, `add_hook`, `final_response`) that wants to run the
loop, and it no longer hands out the request builder the
tool-advertising step needs.

## Premise correction (upstream exploration, 2026-07-31)

A third earlier claim needs narrowing, and it matters for what this
upgrade is *for*. "yach cannot talk to current OpenAI models" was too
broad. It cannot on the surface yach opted into.

`openai::Client` in rig-core **defaults to the Responses API** — the
type alias is literally commented "Responses API client (default)" —
and has since roughly 0.30. Chat completions is the opt-in
`CompletionsClient`, reached via `.completions_api()`, which yach calls
deliberately for its `OpenAiCompatible` variant. On the default path
rig already maps `max_tokens` -> `max_output_tokens`
(`responses_api/mod.rs`), so the parameter gap never arises there.
Verified in yach's own pinned 0.38.2, not just at upstream HEAD.

That also explains the absence of any upstream issue, which was the
question worth asking: reaching the 400 requires rig >= 0.33, an
explicit `.completions_api()`, OpenAI proper rather than a compatible
server, *and* actually setting `max_tokens`. Chat completions in rig is
overwhelmingly exercised against OpenAI-compatible backends that accept
`max_tokens` fine.

This does not change the decision in this spec — owning the loop is
orthogonal to which OpenAI surface yach uses — but it does mean the
`max_completion_tokens` workaround exists because yach models OpenAI
proper as an openai-compatible endpoint. Whether to use the Responses
surface for it instead is now a separate board item, and it carries a
second prize: the queued OpenAI Responses provider-native compactor
behind the `Compactor` seam depends on exactly that surface.

## Design

The replacement seam is `rig-core`'s model-level API, which exists in
both 0.38.2 and 0.41:

```rust
// today, via the Agent
let agent = client.agent(model_id).preamble(&preamble).max_tokens(n).build();
let mut builder = agent.stream_completion(prompt, chat_history).await?;
builder = apply_rig_tool_definitions(builder, rig_tools);
let stream = builder.stream().await?;

// after: the level yach actually operates at
let model = client.completion_model(model_id);
let mut builder = model
    .completion_request(prompt)
    .preamble(preamble)
    .messages(chat_history);
builder = apply_max_tokens(builder, config);      // existing MaxTokensParam branch
builder = apply_rig_tool_definitions(builder, rig_tools);   // unchanged
let stream = model.stream(builder.build()).await?;
```

`CompletionRequestBuilder` carries everything the three provider
branches need — `preamble`, `messages`, `tools`, `max_tokens` /
`max_tokens_opt`, `additional_params`, `build` — and
`apply_rig_tool_definitions` already takes that type, so it survives
untouched. `CompletionModel::stream` returns the same
`StreamingCompletionResponse` the collector already consumes, so
`collect_rig_completion_stream` and the whole event-mapping path should
be unaffected.

### What changes

- Dependency stays `rig-core` (no facade, no `rig-agent`): the runtime
  half is the part being dropped.
- The three provider branches (anthropic, chatgpt-subscription,
  openai-compatible) each lose their agent construction and gain
  model + request construction. They are near-identical today, which is
  worth collapsing into one helper as part of the move rather than
  triplicating the new shape.
- `GetTokenUsage::token_usage` returns `Usage` in 0.41 where 0.38.2
  returned `Option<Usage>`. yach's usage accounting must stop treating
  absence as a variant — and note the hybrid provider-usage accounting
  item (context thread) depends on knowing when usage is genuinely
  unreported, so this needs care rather than a mechanical unwrap.
- `StreamingCompletion` and `StreamingPrompt` traits leave the imports.

### What does not change

The native tool-call mapping (`rig_messages_from_request` and the
`AssistantContent::ToolCall` / `UserContent::ToolResult` construction)
is expressed in `completion::message` types that survive the split.
That work is the most recently measured part of the system and should
come through untouched; if it does not, that is a signal the upgrade
went wrong rather than a thing to redesign.

## Does this foreclose code mode?

Owner question, 2026-07-31, prompted by upstream rig issue 1439: rig is
discussing official "code mode" support — the model emits JavaScript
executed in an embedded engine (Boa) against Rust APIs exposed by a
macro, driven by structured outputs, instead of emitting JSON tool
calls. Does dropping the Agent abstraction lock yach out of it?

**No, and the reverse is closer to true.** Code mode decomposes into
two parts, and neither needs rig's driver:

- *Getting the model to emit code* is a request-construction concern —
  response format and structured output. Building requests directly
  gives yach strictly more control over that than the Agent did.
- *Executing the emitted code against tool bindings* is a tool-execution
  concern, which yach already owns. That is the whole point of this
  design.

More pointedly: yach could not adopt rig's code mode wholesale even if
it stayed on the Agent, for exactly the reasons this design exists.
Tools reached from inside rig's runtime would bypass review gating, the
sensitive-path deny chokepoint, session persistence, and budget
accounting. Any code-mode adoption has to route tool effects back
through yach's policy layer regardless of which rig abstraction is
underneath.

**Owner preference, 2026-07-31: code mode belongs in an extension, not
in core.** Core stays minimal; opinionated setups ship as extensions or
distributions on top. That resolves the remaining coupling worry rather
than deferring it: if code mode is an extension, yach needs nothing
from rig for it, so whether upstream ships it inside the agent runtime
stops mattering to this decision entirely.

It also suggests a shape worth noting for whoever designs it. Code mode
as an extension is naturally *one tool* — an `execute`-style tool whose
implementation runs a sandbox whose bindings call back through yach's
tool executor. That preserves the policy layer for free: calls out of
the sandbox traverse the same review gate, sensitive-path chokepoint,
and session persistence as any other tool call, because they are tool
calls. The tension to design against is approval granularity — a single
code execution may make many bound calls, and per-call review is the
wrong texture there. That is the approval-model item's problem
(UX sprint), and code mode is a good forcing case for it.

Two existing threads this touches when it becomes real: execution
isolation (deliberately open — running model-authored JS is an
isolation decision, not just a feature), and the extension system,
since extension-provided tools would need exposing to the binding
surface.

## Risks

- **The collector is the unknown.** `collect_rig_completion_stream` and
  the raw-event mapping (`RawStreamingChoice`, `StreamedAssistantContent`,
  `ToolCallDeltaContent`) are assumed compatible because the response
  type is the same, but that is an inference from types rather than a
  compile. Establish it early; if the event shapes moved, the estimate
  changes materially.
- **Usage semantics.** A non-optional `token_usage` can silently turn
  "provider reported nothing" into a zero, which would corrupt the
  meter quietly rather than loudly. This is the one change that can
  produce wrong numbers rather than a compile error.
- **Three near-identical branches.** Collapsing them is the right move
  but grows the diff; doing it in the same slice mixes a refactor with
  a migration.

## Validation

The evals exist for exactly this. In order:

1. `just eval-validate` — verifiers still accept their oracles. No
   credentials.
2. `just runtime-image` then `just eval-gate` — 7 tasks, 3 driver
   checks, real provider. The stale-image guard now makes this
   trustworthy; before it existed, a run like this could silently
   measure the old binary.
3. The 100-cell sweep against
   `records/2026-07-30-tool-call-after-measurement.md`. This is a
   **regression check, not an improvement hypothesis**: the numbers
   should land where they are now (95/100 overall; `tool-call-economy`
   5/5 on haiku; `notes-tally-fix` 14/15 on the Zen shapes;
   `compaction-continuation` at or above 18/20). A drop localizes the
   damage by task and provider shape.
4. The OpenAI cell, which is the newest and least exercised path.

## Slices

1. **Seam swap.** Replace agent construction with model + request
   construction across the three branches, keeping them separate.
   Dependency and imports updated; `token_usage` handled deliberately.
   Green on tests, gate, and the 100-cell sweep before anything else.
2. **Collapse the branches.** Now that all three build requests the
   same way, fold them into one helper. Pure refactor, re-verified by
   the same evals.

## Owner decisions (2026-07-31)

1. **Reportedness: a boundary predicate, not hand-rolled tracking.**
   The signal really is lost at rig's edge — `Usage`'s fields are
   non-optional `u64` in both versions, so only the outer `Option`
   ever meant "the provider reported nothing", and 0.41 drops it.
   Today that `Option` flows through `provider_usage_from_rig` into
   `sum_log_usage`, which sets `reported` iff some entry carried one,
   and on into the outcome document and yacht's `usage_source`. Left
   alone, 0.41 turns every unreported response into a reported zero.

   yach will not track reportedness itself. A completed provider
   response always consumes input tokens, so an all-zero `Usage` is
   the unreported case:

   ```rust
   fn provider_usage_from_rig(usage: Usage) -> Option<ProviderUsage> {
       if usage.input_tokens == 0 && usage.output_tokens == 0 && usage.total_tokens == 0 {
           return None; // 0.41 dropped the Option that carried this
       }
       Some(..)
   }
   ```

   One predicate at the boundary restores the existing signal, and
   everything downstream is untouched. It is a heuristic, and it errs
   toward "do not trust this number" rather than "trust this zero",
   which is the safe direction for a meter and for yacht evidence.

2. **Slice 2 lands separately**, so the migration is validated on its
   own before a refactor moves the same code again.

3. **The `max_completion_tokens` workaround is not a catalog stopgap
   and stays.** It was recorded as one; that was imprecise. rig 0.41
   has no `max_completion_tokens` anywhere, so this is a gap in rig,
   not in yach's configuration layer — every rig client hitting
   current OpenAI models has it. A model catalog changes *who supplies
   the spelling*, retiring the `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM`
   env var; it does not remove the need to send the parameter that
   way. The mechanism retires when rig sends the right field for the
   OpenAI provider natively — which makes reporting it upstream the
   durable fix, and a reasonable part of the bargain of staying on rig
   (see the open own-thin-layer-vs-middleware question).
