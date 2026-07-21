# Context Compaction Design

Date: 2026-07-20

Status: designed; slice 1 (summary checkpoint compaction) not yet
implemented.

## Context

Yach has no context compaction: long sessions hit the provider's context
limit and die with no recovery. All four comparison harnesses compact.
The design session (2026-07-20) worked from two research passes — the
cohort's implementations at source level and the broader research
landscape — recorded in
`docs/project/records/2026-07-20-context-compaction-research.md`. The
architecture below adopts the cohort's convergent skeleton, orders the
mechanisms by the published evidence, and keeps the mechanism pluggable
because the owner expects to explore this area continuously.

## Owner Decisions (2026-07-20)

1. Two-tier target architecture: deterministic tool-result masking as a
   pre-pass, model-written summary checkpoints as the recovery mechanism.
   Summary checkpoints ship first (they are what stops sessions dying);
   masking follows as slice 2.
2. Compaction runs behind a yach-owned seam (`NativeCompactor`), selected
   by config, so novel compaction approaches can plug in later without
   protocol or log-schema changes.
3. The session log is never truncated. Compaction appends a checkpoint
   event; display history and audit stay intact.
4. Revisit-often topic: this design is the starting mechanism, not a
   settled direction.

## Goal

- Sessions that approach the context limit compact automatically and
  continue working, with the model reoriented by a structured summary
  plus a verbatim recent tail.
- A context-overflow provider error recovers by compacting and retrying
  instead of failing the turn.
- `/compact [focus instructions]` gives manual control.
- Resumed compacted sessions rebuild the same provider context as live
  post-compaction sessions (live-parity preserved).
- The user can see context pressure, when compaction happens, and what
  the summary said.

## Non-Goals (slice 1)

- Masking/eviction of old tool results (slice 2; the checkpoint
  architecture and trigger accounting are designed to accommodate it as
  a pre-pass).
- Alternative compactors (different summarizer models, memory-file
  writers, importance scoring) — the seam exists for them; each is its
  own later exploration.
- Provider-native compaction/context-editing APIs (provider-locking;
  client-side stays portable and inspectable).
- Branch summarization (yach has no session branching).
- Automatic post-compaction file re-reading (Claude Code re-reads ~5
  recent files; yach's summary carries file lists and the model re-reads
  on demand, consistent with the stale-state guidance).

## Architecture

### Checkpoint event

A new session event, appended like any other:

```
NativeSessionEvent::CompactionCheckpoint {
    session_id, turn_id,
    checkpoint_id: NativeCompactionCheckpointId,
    summary: String,
    first_kept_entry_id: NativeEntryId,
    tokens_before: u64,
    tokens_after_estimate: u64,
    reason: threshold | manual | overflow,
    compactor: String,            // e.g. "summary"
    details: serde_json::Value,   // compactor-specific (e.g. cumulative
                                  // read/modified file lists)
}
```

### Context assembly

Provider context assembly (the shared live/resume path) changes in one
place. With no checkpoint, behavior is unchanged. With checkpoints,
context =

1. baseline guidance + static context (unchanged, never summarized);
2. the newest checkpoint's summary, wrapped in a continuation frame
   ("Earlier work in this session was compacted; the summary below is
   authoritative for it. The full transcript remains in the session log
   at <path>." — the log-path sentence included only when the session
   log lies inside the project root and is therefore tool-readable);
3. transcript events from `first_kept_entry_id` forward, rendered
   exactly as today.

Resume gets compaction for free because it rebuilds through this same
function; a resumed compacted session and a live post-compaction session
produce identical provider context.

### The compactor seam

```rust
trait NativeCompactor {
    fn compact(&self, preparation: CompactionPreparation<'_>) -> CompactionFuture;
}
```

`CompactionPreparation` carries: the serialized messages to fold, the
previous checkpoint's summary and details (if any), the chosen
`first_kept_entry_id`, token accounting (context window, usable budget,
tokens before), the trigger reason, and optional user focus
instructions. It returns a checkpoint payload (summary + details) or a
structured failure. Core owns cut-point selection and log writes; the
compactor only produces the summary. Config selects the implementation
(`compaction.compactor`, default `"summary"`); unknown names fail closed
with an actionable error, mirroring `shell.executor`. Masking (slice 2)
is not a compactor: it is a deterministic pre-pass that runs before the
compactor is consulted.

### Cut-point rules

The kept tail targets `keep_recent_tokens`, walking back from the newest
entry. Cuts land at turn boundaries (a turn = user message through its
last tool result). A tool call is never separated from its result. If a
single turn exceeds the whole budget, the cut falls at an
assistant-message boundary inside it and the turn prefix folds into the
same summary call (no dual-summary merge). Repeated compactions
summarize from the previous checkpoint's `first_kept_entry_id`, not from
the checkpoint itself, so previously-kept messages fold into the next
summary rather than dropping.

## Trigger And Token Accounting

Checked before every provider round (including tool-loop rounds):

```
usable    = context_window − max_tokens − reserve_tokens
estimate  = provider-reported usage for prior rounds
            + chars/4 estimate for unmeasured content
fire when   estimate > auto_threshold_percent% of usable
```

- `reserve_tokens` (default 16,384) guarantees the summarization call
  itself always fits — the mitigation for Claude Code's documented
  "conversation too long" deadlock, where a full window makes even
  manual compaction impossible.
- `context_window` comes from provider config with env override
  (`YACH_RIG_PROVIDER_CONTEXT_WINDOW`), same posture as `max_tokens`;
  both move to model-catalog metadata in the flagged revisit.
- Precision is deliberately loose; the 10% threshold slack absorbs
  estimate error.
- Trigger reasons: `threshold` (auto; `compaction.enabled: false`
  disables), `manual` (`/compact [instructions]`), `overflow` (provider
  context-overflow error → compact → retry the failed round once).

Thrash guard: if the estimate still exceeds threshold immediately after
compacting, or a second `overflow` compaction occurs within one turn,
stop compacting and fail the turn visibly ("context refilled after
compaction; narrow the work or start a fresh session") instead of
looping summary calls. Prompt-cache economics reinforce compacting
rarely at high thresholds: every compaction rewrites the prompt prefix
and resets the cache.

## The Summary Pass

One provider call on the session's current model with a fixed yach-owned
prompt. Messages serialize to flattened text (`[User]:`, `[Assistant]:`,
`[Tool call]: name(args)`, `[Tool result]: …`) so the summarizer treats
them as material rather than a conversation to continue; tool-result
bodies truncate to 2,000 chars inside the summarization request.

Summary schema (fixed sections):

1. Goal and intent
2. User instructions and constraints — restated verbatim (the measured
   failure mode of compaction is silently dropping standing
   instructions: 0% → 30% constraint-violation rates in the governance-
   decay study)
3. Progress: done / in progress / blocked
4. Key decisions, with rationale
5. Files touched: read / modified (cumulative across checkpoints via
   `details`)
6. Errors encountered and how they were resolved
7. Next steps
8. Critical context

Anchored iteration: when a previous checkpoint exists, its summary is
supplied with instructions to treat it as the current anchored summary —
preserve still-true details, remove stale ones, merge new facts — so
repeated compactions update one living summary instead of summarizing
summaries.

With `/compact <instructions>`, the user's focus instructions are
appended to the prompt without displacing the fixed schema.

Failure: if the summary call fails, no checkpoint is written, the
session continues uncompacted, and the failure surfaces as a visible
status (for `overflow` reason, the original overflow error then fails
the turn with guidance).

## TUI Surface

- Context meter in the status area (percent of usable window, same
  accounting as the trigger), with a visible warning state as the
  threshold approaches.
- "compacting…" status while the summary call runs.
- A transcript marker for each checkpoint ("— compacted: 142K → ~31K
  tokens —") with the summary text inspectable; hydration renders the
  same marker on resume.
- `/compact [instructions]` command.

## Config

`compaction` section of `.yach/config.json`, user + project scopes
merged like `shell` (project wins scalars; invalid config fails closed
to defaults):

```json
{
  "compaction": {
    "enabled": true,
    "compactor": "summary",
    "reserve_tokens": 16384,
    "keep_recent_tokens": 20000,
    "auto_threshold_percent": 90
  }
}
```

## Verification

- Unit: cut-point selection (turn boundaries, tool-call/result pairing,
  oversized-turn fallback, repeated-compaction span), token accounting
  math, thrash guard, anchored-summary threading, checkpoint event
  serialization round-trip, config merge/fail-closed.
- Loop tests (fake provider): threshold trigger → checkpoint appended →
  next round's request contains summary + tail only; overflow error →
  compact → retried round succeeds; manual `/compact` with focus
  instructions; summary-call failure leaves session uncompacted;
  thrash guard stops the loop.
- Resume: a compacted session resumed cross-session produces provider
  context identical to the live post-compaction session and renders the
  checkpoint marker.
- Live dogfood: drive a real session past threshold, inspect the
  summary, confirm continuation quality; `/compact` at a task boundary.

## Slices

1. **Summary checkpoints** (this design's implementation scope):
   trigger, seam, checkpoint event, context assembly, TUI surface,
   config.
2. **Masking pre-pass**: deterministic eviction of old tool-result
   bodies (elision marker keeps the call visible), opencode-shaped
   (protect recent window, only when it frees enough to matter), run
   before the compactor at the same trigger.
3. **Exploration via the seam**: alternative compactors, memory-file
   patterns, importance scoring — each its own design when pursued.
