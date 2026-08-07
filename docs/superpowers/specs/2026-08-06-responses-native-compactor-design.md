# OpenAI Responses Provider-Native Compaction

**Date:** 2026-08-06
**Status:** Accepted (design)
**Research:** `docs/project/records/2026-08-05-responses-native-compactor-research.md`
(authoritative for API facts, cohort evidence, and the Rig 0.41 capability
audit). Board: context-system queue, "Responses provider-native compactor".

## Problem

Yach's compaction slice 1 compacts by summarizing: core selects a cut, a text
summary replaces the folded range in provider-visible context, and the
append-only session log stays whole. OpenAI's Responses API now offers a
stronger option: `POST /v1/responses/compact` takes a full context window and
returns the canonical replacement window — an opaque, encrypted compaction
item plus any retained items — which the next `/responses` call consumes
as-is. Codex and omp use this path with materially better continuation
quality than local summarization.

The blocker is Rig: its Responses provider cannot carry the opaque artifact.
Its typed `InputItem`/`InputContent` model has no passthrough variant, so a
`compaction` item can never be put back into a request through Rig, and its
own documentation only covers text-summary compaction. The research verdict:
build only with a portable fallback and exact replay; without both, wait.

## Cohort evidence (from the research record, 2026-08-05)

- **Codex** selects remote compaction by provider capability (V2/V1) and
  falls back to a local summarization task when unsupported. Custom
  instructions only apply on the local path — native compaction takes no
  focus. A user report of `/responses/compact` failing for an otherwise
  usable model is treated as reliability evidence: model-level capability
  data and a fallback are mandatory.
- **omp** persists the native artifact (`preserveData.openaiRemoteCompaction`)
  and falls back to local summary on native failure.
- **opencode, Pi core, Claude Code** ship local reduction/summarization;
  the maintained Pi OpenAI extension adds native compaction while also
  keeping a portable text summary.

Consensus: keep an append-only local record, retain a portable local
reduction, and add a provider-native artifact only when the next turn can
replay it exactly.

## Owner decisions (2026-08-06)

1. **Capability gating is catalog data.** The model catalog gains a
   `responses_compact` capability (baked + fetched layers, versioned like
   `tool_call`). Unknown or absent means native never engages. No runtime
   probing.
2. **Selection is `auto` by default.** `compaction.compactor` accepts
   `"auto"` (default), `"summary"`, `"openai-responses"`. `auto` uses native
   when the active connection is OpenAI Responses and the catalog marks the
   model capable, else summary. `"openai-responses"` forces native and falls
   back to summary with a visible warning when unsupported or failed.
3. **Every checkpoint carries both artifacts.** A successful native
   compaction also runs the existing summary pass; the checkpoint's
   `summary` holds real portable text and `details.native` holds the opaque
   window. Model switches, cross-provider resume, and every fallback keep
   full content.
4. **Focus augments the compact call's instructions.** The compact endpoint
   accepts an optional `instructions` parameter (current API reference). The
   request always carries the session's current system instructions exactly
   (exact-reuse rule); `/compact [focus]` appends a short focus directive to
   them, so focus shapes the native artifact itself, on the same compaction
   path, and also flows into the portable summary as today. (An earlier
   draft of this decision assumed no instructions parameter existed; the
   reference was re-verified 2026-08-06.)
5. **Rig is patched in-repo, upstreamably.** A vendored `[patch.crates-io]`
   fork adds the input-side passthrough Rig lacks; the yach side isolates
   the dependency behind one seam so an upstream merge or a future Rig exit
   is a clean cutover. The patch is offered upstream in parallel.

## Design

### Compactor dispatch

`run_compaction` stops hard-rejecting every `compaction.compactor` value
except `"summary"` and dispatches through the existing `Compactor` trait.
Effective selection per run:

- config names `"summary"` or an unknown value → summary path (unknown keeps
  today's fail-closed warning);
- config `"openai-responses"` → native when provider + capability allow,
  else summary with a warning status;
- config `"auto"` → native when the active connection is OpenAI Responses
  and `layers.resolve(...).responses_compact == Some(true)`, else summary.

Core continues to own cut selection, token accounting, and checkpoint
writes. The `Compactor` trait keeps its
`fn compact(&self, preparation) -> CompactionFuture` shape, but
`CompactionPreparation` is extended — the current struct only carries the
serialized conversation string, prior summary/details, boundary, tokens,
reason, and focus, none of which can produce exact tool items or authorize
a provider call. It gains two fields:

- `provider: Arc<CompactionProviderContext>` — provider kind, active model
  id, and the resolved adapter configuration (base URL, credential, model
  profile) needed to authenticate a provider-native request.
- `native_request: Option<NativeRequestEnvelope>` — populated when the
  active connection is OpenAI Responses, carrying both the assembled
  canonical input chain defined below and the session's exact resolved
  `instructions`. The runner builds the envelope because instructions are
  assembled per turn from static context
  (`provider_messages_from_log_with_static_context`) and never reach the
  compactor otherwise; the compactor must not re-derive them from provider
  config. Manual `/compact` assembles the same static context, so the
  compact call's instructions equal the normal turn request's, with the
  focus directive as the only appended delta.

Both are inert to the summary compactor; `serialized_conversation` stays
for the summary prompt. The OpenAI implementation is the first real second
`Compactor`.

### Canonical input chain

One runner-owned assembler produces the chain the next turn starts from,
used identically by turn building and by compaction. The base is one of
three, keyed on the newest checkpoint:

1. **Matching native window** — the checkpoint's `details.native.window`
   matches the active model, connection, and capability: the chain is
   `window ++ items for events appended after the checkpoint event`
   (raw committed round-pair suffix when present, log-converted
   otherwise). Events between `first_kept_entry_id` and the checkpoint are
   *not* appended: that kept slice was part of the compact call's input
   and lives inside the returned window.
2. **Summary-only or non-matching checkpoint** — summary fallback,
   different model, or different provider: the chain is
   `[summary message] ++ items from first_kept_entry_id onward ++
   post-checkpoint events`. This is the existing summary rebuild; it is
   also the input a native compaction uses after an earlier native attempt
   fell back to summary, so a native-fail → summary → later-native-success
   sequence loses nothing.
3. **No checkpoint** — the full log, converted.

`/responses/compact` receives this full chain as its input window — the
endpoint's contract is "send the full currently-fitting window; pass the
returned window as-is, unpruned." Yach's cut selection is unchanged and
still governs the portable summary and the fallback kept tail
(`first_kept_entry_id`), but the native path does not splice: the returned
window wholly replaces the pre-compaction context, including whatever
retained items OpenAI chose to keep. Reconstructing only the fold range
would silently drop all pre-checkpoint context on the second and later
compactions, since `select_compaction_cut` starts at the prior checkpoint
boundary.

`CompactionPreparation` therefore gains
`native_request: Option<NativeRequestEnvelope>` — the assembled chain plus
the exact resolved instructions, populated when the active connection is
OpenAI Responses — instead of a folded-events field, plus the
`Arc<CompactionProviderContext>` described above (provider kind, active
model id, resolved adapter with base URL and credential).

### Native compaction call

`OpenAiResponsesCompactor` POSTs
`{ model, input: envelope.input, instructions: envelope.instructions }` to
`{base}/responses/compact`, appending the `/compact [focus]` directive to
`instructions` when present. It performs no event conversion or
instruction assembly itself.
On a first compaction the chain is log-converted items only, so reasoning
items are absent (Rig discarded them pre-artifact) — the endpoint accepts
message/tool windows. On later compactions the chain carries the prior
window and raw suffixes, so reasoning fidelity is retained in-session end
to end.

The call is a yach-owned authenticated request with the active model id.
On success the response's `output` array is the
canonical next window: stored verbatim, never pruned or edited (per OpenAI's
explicit contract). On any failure — HTTP error, unsupported model, timeout,
decode failure — the log is untouched and the existing summary compactor
runs instead, with a status warning on the `"openai-responses"` config only.

### Checkpoint record

`CompactionCheckpoint` fields are unchanged. The native window lives in
`details.native`:

```json
{
  "version": 1,
  "provider": "openai",
  "wire": "openai-responses",
  "model": "<exact model id>",
  "window": [ /* verbatim output items, including the compaction item */ ]
}
```

No new schema field, no migration; logs without `details.native` read as
summary-only. The portable summary (with focus instructions when given) is
always generated after a successful native call and stored in `summary` as
today. `details` merges with the existing file-detail bookkeeping.

### Replay path

When the newest checkpoint carries `details.native` whose `model` equals the
active model, the active connection is OpenAI Responses, and the catalog
still marks the model capable, the runner builds turn requests as Responses
input arrays:

```
window ++ round-pair suffixes committed after that checkpoint ++ new user input
```

No kept tail is appended under native replay: the compact call already saw
the full chain, and the returned window wholly replaces it — OpenAI's
retained items cover what it chose to keep. The kept tail (converted from
the log through Rig's standard message conversion; text and tool calls
with real call IDs are exactly expressible) is used only when the window
does not exist: first compaction, fallback, or resume rebuilds of turns
whose raw suffixes were not persisted. The request goes through patched Rig
with `store: false` unchanged. Any mismatch — model switch, non-Responses
provider, absent artifact, capability removed — rebuilds
`summary + kept tail` through the unchanged Rig path.

### In-session chain authority

While a native checkpoint is in effect, the replay authority is the
**complete ordered input-item chain**, not only model outputs. Each
completed model round contributes its terminal ordered `output` array as
raw `serde_json::Value` items — value-equal to the wire, including
provider-added fields on known item types (surfaced by patch item 2) —
followed by the yach-generated `function_call_output` items produced when
that round's calls execute.

**Commit rule:** the atomic unit is a completed model round. Its output
items plus one `function_call_output` per call — real output on success,
error tool-result on failure, synthetic `cancelled` output for calls never
started — commit together, so the chain never contains an unpaired call. On
turn failure or cancellation the chain retains every completed round in
order; the only excluded piece is a round whose stream never finished, which
by construction executed no tools, so dropping it repeats nothing. A
cancelled turn therefore leaves the model seeing that its remaining calls
were cancelled (provider-visible), matching Codex's behavior and preventing
repeat execution of side-effecting tools. Because the chain is always
structurally complete, the next `/responses/compact` call sends it directly
as the window — subsequent compactions compact from the artifact with full
reasoning fidelity, not from a log reconstruction.

**Enabling fixes (both paths):** two current-lifecycle gaps must close
first, and both correct summary-path evidence too.

1. *Tool batch discards on first error.* The batch returns on the first
   error and drops earlier `tool_results` (`runner.rs:4925-5005`). It
   changes to always emit exactly one result per call — success output,
   error result, or cancelled marker — retaining successes before a
   failure.
2. *Cooperative cancellation finalization.* `PromptCancelled` currently
   `abort()`s the provider task immediately, then persists a generic
   cancelled turn (`runner.rs:1096-1109`) — an aborted mid-batch turn can
   never synthesize outputs or commit rounds. The active turn gains a
   shared cancellation token: cancel sets it; the streaming request and
   the tool batch observe it at the next boundary (between calls, or by
   interrupting a cancellable tool such as the bash executor's
   process-group kill); the turn then finalizes itself — real results for
   completed calls, synthetic `cancelled` outputs for unstarted ones, the
   partial turn persisted, completed round-pairs committed to the chain —
   and returns its own `TurnFinished(Cancelled)`. The outer actor waits a
   bounded grace period and keeps today's hard `abort()` as the backstop
   for a task that never observes the token, falling back to the generic
   persist path only in that case.
3. *Rebuild admits paired evidence from non-completed turns.*
   `provider_messages_from_log` currently admits only
   `TurnOutcome::Completed` turns (`runner.rs:2421-2435`), so a persisted
   cancelled or failed turn's entries are dropped wholesale on rebuild and
   executed side effects can be repeated after a restart. The rebuild rule
   changes to admit structurally paired evidence from `Cancelled`/`Failed`
   turns: with fixes 1-2 every recorded call has a result, so the whole
   recorded turn is admissible; a turn that died via the hard-abort
   backstop or a crash may still carry an unpaired trailing call, which is
   trimmed at the last paired point (the orphaned-call healing the board
   already queues, scoped to rebuild).

### Resume

Checkpoint `details.native` persists in the session log, so a resumed
session whose newest checkpoint matches (same model, OpenAI Responses
connection, capability still true) replays `window` plus post-checkpoint
turns rebuilt from the log via standard conversion. Raw turn suffixes from
before the restart are not persisted in this slice, so those rebuilt turns
lose only reasoning-item fidelity (text and tool calls are exact). Persisting raw per-turn output blobs was considered
and deferred: new log schema, larger files, and retention questions for
encrypted provider state, against a fidelity loss that the next compaction
erases anyway.

### Rig patch

`[patch.crates-io]` points rig-core at a yach fork with three additive
changes to the Responses module:

1. `InputContent::Unknown(Value)` — a verbatim input-item passthrough with
   hand-written (de)serialization mirroring the existing
   `Output::Unknown(Value)`, letting the chain's opaque items (compaction
   item, encrypted reasoning, retained items) round-trip into
   `CompletionRequest.input`.
2. Ordered raw output on streaming. The public streaming accumulator
   reduces `Output::Message` to a bare `MessageId`, decomposes reasoning
   into separate choices, defers function calls in strict mode, and drops
   the terminal response's `output` array entirely — the complete ordered
   item list is unrecoverable from stream choices
   (`responses_api/streaming.rs:302-336,344-389`). Nor is the typed
   `Vec<Output>` sufficient: known `message`/`function_call`/`reasoning`
   items decode into typed structs and re-serialize only modeled fields,
   so any provider-added field on a known item is dropped
   (`responses_api/mod.rs:2067-2086`) — violating the pass-as-is
   invariant. The patch therefore captures the terminal
   `response.completed.response.output` as raw `Vec<serde_json::Value>`
   while the raw event data is available and surfaces it on
   `StreamingCompletionResponse`. Round-granular capture then needs no
   incremental item events: a completed round's canonical ordered items
   arrive with the terminal response, value-equal to the wire.
3. A public entry point to send a caller-built typed
   `responses_api::CompletionRequest` (completion and streaming), so yach
   sets `input` to the verbatim chain instead of the converted message
   model.

No changes to Rig's generic message types; no behavior change for existing
callers. Yach's adapter additionally stops discarding the stream choices it
already receives where they inform status/telemetry, but chain capture is
built on the terminal ordered output, not on choice reconstruction.

### Config and catalog surface

- `compaction.compactor`: `"auto"` (new default), `"summary"`,
  `"openai-responses"`. Existing configurations without the key behave as
  `auto`; behavior only changes for OpenAI Responses connections whose model
  carries the capability.
- Catalog: `CatalogEntry.responses_compact: Option<bool>`, threaded through
  baked snapshot, fetched layer, and `layers.resolve` precedence exactly
  like `tool_call`. The baked snapshot marks the models OpenAI documents as
  compaction-capable; everything else is `None`/absent.
- Status messaging: native compaction reports `context compacted (provider)`
  vs the existing summary text; fallback under forced config warns
  `native compaction unavailable (<reason>); used summary`.

### Failure taxonomy

| Failure | Behavior |
| --- | --- |
| Compact call HTTP error / unsupported model / timeout / decode error | Log untouched; summary path runs; warning only when config forced native |
| Replay request fails mid-turn | Normal turn failure; completed-round prefix intact; retry reuses it |
| Cancel mid-turn (cooperative) | In-flight call interrupted or finished; completed calls keep real results, unstarted calls get synthetic cancelled outputs; completed round-pairs commit; turn persists as cancelled |
| Cancel ignored past grace | Hard `abort()` backstop; generic cancelled-turn persist (today's behavior); chain stays at last committed round-pair |
| Artifact/model/provider/capability mismatch at turn build | Silent structural fallback to `summary + kept tail`; native re-engages on matching re-selection |
| Malformed `details.native` on resume | Treat as absent; summary path |
| Unknown `compaction.compactor` value | Today's fail-closed warning; summary-equivalent skip |

### Explicitly deferred

- `context_management` automatic server compaction and Conversations /
  `previous_response_id` as session authority (research: would duplicate
  yach's trigger policy and add retention semantics without improving the
  explicit seam).
- Persisting post-compaction raw turn suffixes in the session log.
- Anthropic's beta server compaction (`compact_20260112`) — confirms the
  pattern but has its own artifact/replay rules; a second provider is a
  follow-up slice behind the same `Compactor` dispatch.
- Removing the vendored Rig patch (upstream merge tracked separately).

## Testing

- Instructions equality: the compact call's `instructions` equal the normal
  Responses turn request's instructions byte-for-byte (same static-context
  assembly, manual `/compact` included), with the focus directive as the
  only appended delta.
- Chain-base tests: native failure writes a summary-only checkpoint and a
  later native success builds from the summary+kept-tail base with nothing
  folded away; a second native compaction's input equals `window_1 ++
  post-checkpoint events` with no duplication of the kept slice; a
  non-matching model falls to the summary base.
- Fixture HTTP server covering `/responses/compact` and streamed
  `/responses` turns: round-pair ordering with interleaved tool outputs,
  cancel-mid-turn prefix retention via cooperative finalization (incl.
  synthetic cancelled outputs and self-persisted `TurnFinished(Cancelled)`),
  hard-abort backstop when the token is never observed, tool-batch
  partial-failure result retention, cancel-after-one-tool → restart →
  next-turn rebuild keeping the real result and cancelled markers visible,
  artifact/model mismatch
  fallback, capability on/off selection, forced-config warning, malformed
  artifact on resume, and `Unknown` item serde round-trip.
- Patched-Rig unit tests for `InputContent::Unknown` (de)serialization,
  the typed-request send entry point, and value-equal terminal-output replay:
  a known `message` item carrying an extra provider field must emerge in
  `StreamingCompletionResponse.output` with that field intact and re-enter
  the next request unchanged.
- Resume tests: native checkpoint across process restart; tail rebuilt from
  log.
- Live smoke with a real OpenAI key in the existing `smoke-*` idiom:
  threshold compaction on a capability-marked model, continuation after
  compaction, and A → B → A model switching across a native checkpoint.
