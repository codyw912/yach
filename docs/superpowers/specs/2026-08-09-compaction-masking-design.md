# Compaction Masking (Slice 2): Deterministic Tool-Result Clearing

**Date:** 2026-08-09
**Status:** designed (owner, 2026-08-09, interactive design session)
**Prior work:** context compaction design
(`2026-07-20-context-compaction-design.md` — slice 1 summary
checkpoints, landed and live-validated), text tool results
(`2026-08-01-text-tool-results-design.md`, #216), OpenAI Responses
native compactor (#239). Board: "Compaction slice 2: masking pre-pass"
(queued); owner ruling 2026-08-09: **design both masking mechanisms,
implement only the blanket rule in this slice.**

## Problem

Slice 1 stops sessions dying at the context limit, but every compaction
pays a summarization model call and rewrites all pre-cut history into a
summary. Most long sessions don't need that yet: the bulk of context
pressure is stale tool-result bodies (file reads, search hits, command
output) whose calls the model has already acted on. The cohort evidence
(`records/2026-07-20-context-compaction-research.md`) is direct:
observation masking alone halves cost and matches or beats LLM
summarization; the hybrid (mask first, summarize later) gains a further
7–11%.

The 2026-08-09 prime-agent cohort survey (research agent report; source
revision a18809e) confirmed the gap persists even in the newest
Pi-derived harness: Prime has production-time output truncation and
summarizer-only truncation, but no old-result masking, no pinning, no
useless flags. opencode remains the sole masking reference; omp remains
the sole pinning reference.

## Owner Decisions (2026-08-09)

1. **Mask event supersedes body.** Masking appends a
   `ToolResultMasked` event referencing the result; the session log is
   never mutated. The original body stays in the log for audit and UI
   reveal. This extends slice 1's append-only checkpoint discipline.
2. **Protection window = recent-token budget.** Walk tool results
   newest-first, protecting up to `compaction.keep_recent_tokens` of
   result content. One budget concept shared with the summarizer tail —
   masking and summarization agree about what "recent" means.
3. **Trigger: at the compaction threshold, with a min-savings floor.**
   Masking runs when the context estimate crosses the compaction
   threshold, before the compactor is consulted. If maskable savings
   fall below the floor, skip masking and summarize as today. If masking
   alone brings the estimate under threshold, record the mask events and
   skip the summary call entirely. Otherwise mask first, then summarize
   (the summarizer's input is already smaller).
4. **Scope: result bodies only.** Only
   `ToolExecutionFinished.result_content` is masked. Tool-call arguments
   always survive — they are what the model needs to understand what
   happened (which file, which command), and they are usually small.
5. **Config surface: on/off only.** `compaction.masking` (bool, default
   true). No tuning knobs until measurement justifies them; the floor
   and protection window are derived from existing config.
6. **Pinning/useless flags are designed but not implemented.** They
   require the extension-tool contract work slated separately; see the
   design section below.

## Mechanism

### Mask event

```rust
SessionEvent::ToolResultMasked {
    session_id: SessionId,
    turn_id: TurnId,          // turn that ran the masking pass
    masked_turn_id: TurnId,   // turn owning the masked result
    tool_request_id: ToolRequestId,
    bytes_freed: u64,         // original result_content byte length
    reason: MaskReason,       // ThresholdPrePass in this slice
}
```

Appended only at the successful compaction transaction boundary.
`push_native_session_event` eagerly mutates both the in-memory log and
the pending-persist buffer, so masks are staged in a local overlay during
the pass — re-estimation and summary serialization read through the
overlay — and pushed only when the transaction commits: mask-only success
pushes and persists them explicitly; mask-then-summarize/native success
pushes them together with the checkpoint under the existing single
persist. A failed or unapplied transaction drops the staged overlay, so a
failed compaction never leaves orphan mask events. Mask events are
idempotent: a second mask event for the same `tool_request_id` is a
no-op (already masked).

### Provider message assembly

`provider_messages_from_event_slice` walks the event slice as today,
with one index added: the set of `(turn_id, tool_request_id)` pairs
covered by `ToolResultMasked` events anywhere in the complete log.
When a masked result renders, the body is replaced with a stable
marker:

```
[result masked by compaction: N bytes; re-read the source if needed]
```

The assistant tool-call message and the tool-result message both
survive with full arguments and metadata (outcome, tool name). Only
the body changes. Call/result adjacency is never broken — Prime's
cut-point discipline (never cut at a tool result) applies here as the
masking invariant: never drop the pair, only elide the body.

The marker deliberately differs from the production-time truncation
marker (Prime bash pattern: bounded view + full-output path). A model
reading history can distinguish "this output was too big when it ran"
from "this old result was reclaimed for context."

### Masking pass

Runs inside `run_compaction_with` after the threshold check and before
cut-point selection:

1. Collect candidate results: `ToolExecutionFinished` events with
   `Some(result_content)`, not already masked, in completed or failed
   terminal turns (never the current turn — its results may still be
   load-bearing).
2. Walk candidates newest-first, accumulating protected bytes until
   `keep_recent_tokens` (tokens, estimated with the existing chars/4
   estimator) of result content is protected. Everything older is a
   mask candidate.
3. Sum **net** candidate savings: per result,
   `max(0, estimate_tokens(result_content) - estimate_tokens(marker))`
   where `marker` is the exact elision string for that result's byte
   count (~70 chars; masked results stay provider-visible as markers, so
   gross body bytes overstate the reclaim). Candidates with non-positive
   net savings are excluded — masking them would grow or churn context
   for nothing. If the net total < `max(5% of usable window, 8192
   tokens)`, mask nothing and proceed to summarization unchanged.
4. Append mask events for all candidates. Re-estimate. The next step
   depends on which context the provider request will actually use:
   - **Client-rebuilt (summary) context:** if the masked estimate is
     under the threshold, return `CompactionApplication::Masked` (new
     variant) with no checkpoint and no summary call — the next request
     is assembled from the log and genuinely shrinks.
   - **Native (OpenAI Responses) context:** the mask-only
     short-circuit does not apply; see the native section below.
     Native compaction runs whenever the pre-mask estimate crossed the
     trigger, regardless of the post-mask estimate.
5. If the masked estimate is still over the threshold (summary path),
   continue into the existing cut-point and summarization path; the
   summarizer's serialized input already reflects the masks because
   serialization reads through the same event slice the assembly path
   uses.

### Accounting and surfaces

- The context meter reflects masking immediately: the estimate after
  masking is the new baseline, same as after a checkpoint.
- Status line distinguishes the outcomes: "context masked (N tokens
  reclaimed)" vs. "context compacted (summary)" vs. both.
- The TUI renders masked results with the marker inline; the full body
  remains reachable because the original event is still in the log
  (slice 1's append-only rule). A reveal affordance is a TUI follow-up,
  not part of this slice.
- Outcome documents and metrics record `masked_results` and
  `masked_bytes` per compaction transaction so the measurement record
  can quantify masking's contribution separately from summarization.

### Interaction with the native (OpenAI Responses) compactor

The native path replaces provider-side state wholesale: the server
holds the full opaque replay window, and client-side mask events do
not shrink it. A local post-mask estimate dropping below the threshold
therefore says nothing about the server's context size — masking MUST
NOT skip or delay native compaction.

The rule: the decision to invoke native compaction is made from the
**pre-mask** estimate. When the pre-mask estimate crossed the trigger
and native compaction is selected and applicable, native compaction
runs even if masking alone brought the client-side estimate under
threshold. Masking still runs first in this case, because mask events
remain load-bearing for the summary/fallback context: if the native
compactor is unavailable or errors and the runner falls back to summary
context, that context is assembled from the log and benefits from the
masks.

Concretely, on the native path a masking pass that reclaims enough
produces `CompactionApplication::Native` (compaction ran) with the mask
events recorded alongside — never `Masked`. `Masked` is only reachable
when the request path rebuilds context from the log — and the
transaction enforces that by construction: the mask-only path clears
any active native replay (`*native_replay = None`, the same treatment
the summary path gives it), so the next request assembles from the
masked log and the reclaim is real on the wire. Without this, a `Masked`
commit on a replay-active session would leave the server chaining the
original bodies while the client believed the context shrank.

### Designed, not implemented: pinning and useless flags

omp's two mechanisms, deferred to the extension-tool contract design:

- **Pinning**: tool registry metadata marking a result as never
  maskable. The masking pass's candidate filter gains one clause:
  skip results whose tool pinned them. Requires the pin to persist in
  the session log (a field on `ToolRequestRecorded` or a registry
  lookup at mask time; the design pass decides).
- **Useless flags**: a tool marks its own result safe to elide once
  consumed; the masking pass treats flagged results as candidates
  regardless of the protection window. Same persistence question.

Neither lands until extension tools can express retention metadata, and
Prime's survey found no second reference implementation — omp remains
the only precedent, so this wants its own focused design rather than
being rushed into slice 2.

## Error handling and invariants

- **Mask events never appear without a live masking pass.** They are
  written inside the compaction transaction; a cancelled or failed
  transaction discards its pending events as today.
- **Double-masking is a no-op.** Assembly resolves masks from the full
  log, so resume, re-compaction, and manual `/compact` all see the same
  masked state.
- **Current turn is never masked.** Mid-turn compaction masks history
  from prior turns only; the in-flight turn's tool results always
  survive.
- **Failed tool results are maskable.** An old denied/failed result's
  body is no more load-bearing than a success's. The outcome field
  survives, which is the part the model needs.
- **Sensitive-path denials are already bodyless** (`result_content:
  None` with a redacted summary); the candidate filter skips them
  naturally.

## Verification

- Unit: mask-event schema round-trips; candidate selection (protection
  budget respected, current turn excluded, already-masked excluded);
  floor gate (below-floor → no masks); assembly (masked pair renders
  marker with args intact, adjacency preserved).
- Loop tests (fake provider): threshold → mask-only path reclaims
  enough on the summary path → no summary call, `Masked` application,
  meter updates; threshold → mask + still over → summary follows with
  masked input; masking disabled → byte-identical slice-1 behavior;
  native-selected path with pre-mask estimate over threshold → native
  compaction runs even when masking alone would have sufficed locally.
- Resume: a masked session resumed cross-session produces provider
  context identical to the live post-mask session.
- Eval: `compaction-continuation` task already drives a real session
  through compaction; extend its verifier to assert that a masking-eligible
  fixture produces mask events in the outcome document's checkpoint
  details. Measurement record quantifies tokens reclaimed by masking
  vs. summarization across the task matrix.

## Non-goals

- Pinning / useless flags (designed above; implementation waits on the
  extension-tool contract).
- UI reveal affordance for masked bodies.
- Masking tool-call arguments.
- Per-tool or per-path masking rules.
- Any change to the native compactor's provider-side state handling.
- Continuous (non-trigger-bound) masking.

## Slices

1. **Blanket masking (this design's implementation scope):** mask
   event, assembly marker, masking pass in the compaction transaction,
   config flag, meter/status/accounting surfaces, eval coverage.
2. **Tool-authored retention metadata:** pinning and useless flags with
   the extension-tool contract design.
