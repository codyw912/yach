# Async Tool Execution — Research Record

Date: 2026-09-22
Trigger: Unreal Agent release (unreallabs.ai/blog/unreal-agent, github.com/unreallabsai/unreal-agent)
Status: research only — no spec or plan; lifecycle mutation unavailable without planning provider

## Question

Should yach restructure its sequential tool loop into an asynchronous model
where tool calls return "in-progress" immediately and results arrive as
events, letting the model continue working while tools run?

## Yach constraints (verified against source)

### Execution is sequential

`execute_native_provider_agent_tool_batch` (runner.rs:8882) iterates
`requests` in a `for` loop at line 8945, `.await`ing each tool inline.
Results are collected in order; the next provider request is built only
after the loop completes. No `JoinSet`, `join_all`, or spawned tasks.

### Journal already splits tool lifecycle

`SessionEvent::ToolRequestRecorded` (session.rs:330) and
`SessionEvent::ToolExecutionFinished` (session.rs:345) are separate events
keyed by `tool_request_id`. The persistence layer already supports
in-flight tools — a `ToolRequestRecorded` without a matching
`ToolExecutionFinished` is a valid journal state.

### Provider projection skips in-flight tools

`provider_messages_from_event_slice` (runner.rs:3765) only emits a
`ProviderMessage::tool_results` when `ToolExecutionFinished` exists. A
`ToolRequestRecorded` alone produces nothing provider-visible — the model
never sees a pending call. This is one relevant seam, but the live
continuation path (runner.rs:5575-5614) builds the next request from
`prior_messages` + returned `tool_results` without consulting the
projection — so a projection change alone does not enable async.

### Provider API contract

Yach targets Anthropic, OpenAI, ChatGPT subscription, and OpenAI-compatible
via `rig` (`RigProviderConfig`, runner.rs:249-252). The Responses replay path
(`responses_replay.rs:368`) hard-sets `"status":"completed"` on every
`function_call_output` item. The `ProviderToolResultBlock` type
(provider.rs:20) has no status field — only `call_id` and `content`.

The blog's footnote [2] confirms the Responses API's in-progress
`function_call_output` is underspecified and rejected by some providers.
Whether yach's target providers accept the two-item pattern is unverified.

### Spec ordering decision

The accepted multi-round tool loop spec
(`2026-05-18-native-provider-multi-round-tool-loop-design.md:274`)
requires "multiple tool calls in one round preserve order and evidence."
Whether completion-order results satisfy this requirement is unresolved —
the spec does not define "order" as call-order vs completion-order.

### Review/permission holds

Edit tools (`execute_native_provider_edit_tool_request`, runner.rs:7592)
and bash (`execute_native_provider_bash_tool_request`, runner.rs:8073) can
enter `NeedsUserReview` or `ReviewRoute::Hold` states. The batch is blocked
on `wait_for_agent_edit_review_decision` (runner.rs:7809-7814) /
`wait_for_command_review_decision` (8042-8071), and the single
`AgentEditDecisionReceiver` makes a non-matching decision a terminal
`stale_tool_review_decision` — one outstanding review is a design
assumption, not just a loop artifact.

### Cancellation varies by executor

`batch.cancellation.is_cancelled()` is checked per-tool at runner.rs:8949.
Bash execution (runner.rs:8593-8600) selects cancellation against running
work and kills the process group on drop (shell.rs:347-356, 429-451).
Review waits select cancellation (runner.rs:9180-9188). Extension tools
(`execute_native_provider_extension_tool_request`, runner.rs:7406-7411)
call synchronous `execute_with_resources` without a cancellation select.
Readonly tools (runner.rs:8977-8981) are synchronous.

### Prompt admission boundary

`runner.rs:2055-2065` rejects a new prompt while `active_provider_turn`
exists ("native provider: prompt already in progress"). A turn owns a
`CancellationToken` shared by review and bash execution. Unreal's steering
model (cancel model request, keep tools running) does not map directly —
yach would need to decouple turn ownership from tool lifetime.

## Unreal Agent findings

Source: github.com/unreallabsai/unreal-agent (MIT), blog 2026-09-22.

### Architecture

Go coordinator (`harness/coordinator/loop.go`, ~985 lines) around the OpenAI
Responses API. Model tool calls are translated synchronously into durable
async operations; an in-progress `function_call_output` placeholder is staged
in context; terminal results replace the placeholder before the next model
turn.

### Async mechanics

- `scheduleToolCall` resolves a translator, runs it synchronously on the
  coordinator loop, allocates operation IDs, persists `ItemToolCallStatus`,
  and dispatches to an operation manager (loop.go:455-475, 763-828).
- The in-progress record is a `function_call_output` item whose output is the
  literal text: "Tool call is still running. Its result arrives in a later
  turn: continue with independent work, or end your turn to wait for it."
  (contextbuilder/builder.go:103-122).
- On completion, the placeholder is replaced **only in the staged suffix**
  (builder.go:108-122). If a model turn commits while a call is still
  running, the running item moves into the committed prefix and the final
  result is appended later — two items for one call.
- The Responses API `function_call_output` schema has an optional `status`
  field, but Unreal does not set it — the two-item pattern (in-progress +
  final) is the mechanism. Footnote [2] confirms some providers reject this.

### Ordering

Results are appended to the staged suffix in **reconciliation order** —
the coordinator slurps operation updates from a channel and iterates a Go
map (`current.state.toolCalls`), so order reflects arrival batching and map
traversal, not a completion-time sort (loop.go:190-209, 830-867). No causal
ordering guarantee is provided to the model.

### Steering

User input sets `callModel = true`; the loop cancels the active model request
and starts a new one with accumulated context + user message. In-flight tool
operations are NOT cancelled by steering — only `StopHard` cancels them
(loop.go:256-275, 306-310, 346-380).

### Tool surface

Three built-in tools: `Bash` (command + max_output_length), `ViewImage`
(path), `SkillUse` (name). Not literally "one bash tool" — but the surface is
minimal. The Bash description explicitly says independent commands may be
issued as parallel calls in one turn (static.go:39-47).

### Batching prompt

The system preamble instructs: "prefer to go wider with tool calls — they are
cheap — rather than chaining them across a longer sequence of turns" and
"Tool calls are asynchronous: each starts the moment you issue it and runs in
the background" (preamble.md:1-10).

### KV-cache

`committedPrefix` (immutable history) vs `stagedSuffix` (mutable tool results,
user inputs, heartbeats). `Build` concatenates without mutating the prefix;
`Commit` moves suffix into prefix at turn boundaries. The session ID is
SHA-256 hashed and used as a provider cache key with configurable placement
(header or `prompt_cache_key` field) (loop.go:371-379, adapter.go:104-112,
145-147).

### Cost mechanism

Fewer synchronization turns, more concurrent work per model request, no
routine polling (completion/input-driven wake-ups with an optional heartbeat
backup at loop.go:277-304), bounded tool outputs. The "up to 40%" figure is
not derivable from code — it's a benchmark claim vs Codex on Terminal-Bench
with GPT-6 Astra.

## Assessment

### What transfers to yach

1. **In-progress placeholder pattern.** Yach's journal already splits
   `ToolRequestRecorded`/`ToolExecutionFinished`. The provider projection
   (runner.rs:3765) currently skips in-flight tools — emitting a placeholder
   `ProviderToolResultBlock` with "still running" text is one relevant seam.
   However, the live continuation path (runner.rs:5575-5614) builds the next
   request from `prior_messages` + returned `tool_results` without consulting
   the projection — so a projection change alone does not enable async.

2. **Committed prefix / staged suffix.** Yach's `provider_messages_from_log`
   rebuilds the full message list at turn start and compaction boundaries;
   ordinary continuations append `prior_messages` (runner.rs:5575-5614,
   5754-5780). A prefix/suffix split would make the boundary explicit but
   is not obviously cheaper — the current path already avoids rebuilding
   during a turn.

3. **Batching prompt.** Yach's system prompt doesn't encourage parallel tool
   calls. Adding "issue independent calls in one turn" is a prompt-level
   change independent of the execution model.

4. **Steering during tool execution.** Yach rejects new prompts while a turn
   is active (runner.rs:2055-2065). Unreal's model (cancel model request,
   keep tools running) would require decoupling turn ownership from tool
   lifetime — a structural change, not a direct transfer.

### What doesn't transfer

- **Single bash tool.** Yach's typed tool surface (edit transactions,
  extension tools, review) is a feature. The batching insight applies but
  not the simplification.
- **Reconciliation-order results.** Unreal appends results in arrival order
  with no causal guarantee. Yach's spec requires "preserve order and
  evidence" — whether completion-order results satisfy this is unresolved.
- **Provider compatibility.** The two-item `function_call_output` pattern
  is underspecified and rejected by some providers. Whether yach's target
  providers accept it is unverified — Anthropic's tool_use/tool_result
  blocks have no in-progress concept, but that doesn't establish a
  universal prohibition.

### Open questions for a future design

- Does the Responses API's two-item `function_call_output` pattern work on
  yach's target providers (Anthropic, OpenAI, ChatGPT subscription)?
- Can yach's sequential batch executor be made concurrent without violating
  the spec's ordering requirement — e.g., by buffering results until all
  calls in a round complete, or by accepting completion-order results?
- How would review/permission holds interact with async tools — can a held
  tool's result be deferred while other tools' results reach the model?
- What is the cancellation contract for in-flight tools when a turn is
  cancelled or steered?

### Recommendation

Worth a design exploration, not an implementation commitment. The user's
assessment: "long running tools definitely dominate session time in some
cases, it just depends on the work." The cost savings claim (~40% vs
Codex) is plausible given the mechanism (fewer turns, no polling), but the
specific number is a benchmark result, not a guarantee.
