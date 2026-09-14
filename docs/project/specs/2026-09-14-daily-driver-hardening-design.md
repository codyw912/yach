# Daily Driver Hardening Design

Status: proposed 2026-09-14

Outcome: plane:YACH-1 (M1: Evidence-driven dogfood loop)

## Motivation

Yach cannot yet absorb a working day without losing productivity, and the
reasons are not mainly missing capabilities. A session-surface audit found
truthful-state defects, dead controls, and lossy feedback in the interaction
layer itself. These distort dogfood evidence: a session records friction
caused by the harness reporting its own state incorrectly, rather than
evidence about the agent loop.

Two examples set the tone. `/status` always prints `compactions: 0` because
the counter is a hardcoded `0` (`crates/yach-ui/src/transcript.rs:541-543`),
which also means the status bar's `⟲` indicator can never appear, since it is
gated on `> 0` (`crates/yach-ui/src/status_bar.rs:85-90`). And
`/compact <focus>` is unreachable: the parser builds a valid
`CommandWithArgs` (`crates/yach-ui/src/slash_commands.rs:154-167`) that the
dispatcher then discards into "slash command arguments are not supported yet"
(`crates/yach-ui/src/app.rs:3365-3368`).

This slice fixes what the session surface reports and what it lets the user
do. It adds no new model-facing tools and no new capability classes; the
extension capability contract and the embedding seam are a separate design.

## Principles

- **Truthful state first.** A surface that reports state must report the real
  state or omit it. A hardcoded placeholder is worse than absence: it
  silently asserts a fact.
- **No new protocol classes.** Every item here is reachable with existing
  `ClientEvent`/`ServerEvent` variants, or with a field already carried but
  dropped. Where a field must be added, it is additive and optional.
- **Bounded output stays bounded.** Raising a visibility cap is not removing
  it. Terminal rendering cost and context accounting both depend on bounds.
- **Repetition is a safety problem.** Approval fatigue trains habitual
  approval, which the approval-modes design already names as weakening real
  escalations (`docs/project/specs/2026-08-24-approval-modes-design.md:5-11`).

## Slice 1: Truthful session state

### Compaction count

`SessionEvent::CompactionCheckpoint` already carries `checkpoint_id`,
`tokens_before`, and `first_kept_entry_id`
(`crates/yach-backend/src/session.rs:436-442`), and hydration projects
checkpoint markers into the transcript
(`crates/yach-backend/src/runner/session_state.rs:19-268`). The UI counts
them with a stub.

The count is **backend-owned**, not derived from the transcript.
`SessionStats` gains an optional `compaction_count` computed from the
session log's checkpoint events. A client-side count would be wrong in two
ways: `/clear` would erase a session fact, and scrollback archiving drains
older entries. Deriving it from the log also makes it correct live and
after resume without a new event class.

The UI additionally gains a `Compaction` transcript entry kind. Hydration
previously dropped the backend's `system`-role checkpoint message entirely,
so a resumed session showed no sign that compaction had occurred.

### Token accounting

`SessionStats.total_tokens` is public protocol
(`crates/yach-proto/src/lib.rs:479`) and the stats projection hardcoded
`total_tokens: None`.

A first reading of this deferred the slice, claiming no session event
persists provider usage. That was wrong, and the error is worth recording:
it came from checking `MetricRecorded` (which carries a `DurationMetric`),
finding no tokens, and concluding usage was never persisted — without
checking the assistant entry. `SessionEvent::EntryAppended` carries
`ProviderMetadata.usage` (`crates/yach-backend/src/session.rs:93-103`) and
the runner writes `round.usage` into it. A real fixture log confirms it:
`tests/visual/session.jsonl` holds `total_tokens: 554` on its assistant
entry.

The projection therefore sums `ProviderMetadata.usage.total_tokens` across
assistant entries. Each entry's figure is already summed across that turn's
requests, which is the billing-correct turn total
(`crates/yach-backend/src/runner.rs:4354-4356`), so summing entries gives
the session total and survives resume. The result is `None` when no entry
reported usage, so an unknown total is never rendered as zero.

The status bar gains a token segment below context percentage in priority,
so a narrow terminal drops the cumulative total before the actionable
context meter.

Cost display remains out of scope: pricing is catalog metadata, not a
per-session computation, and inventing one would assert precision the
harness does not have.

### Acceptance

- A session that compacts twice reports `compactions: 2` in `/status` and
  shows `⟲2`.
- A resumed session with prior checkpoints reports them.
- `/clear` does not change the reported compaction count, because the count
  is a backend session fact rather than a property of the rendered view.
- A session whose turns reported 554 and 421 tokens reports 975.
- A session where no turn reported usage omits the figure rather than
  showing zero.
- A resumed session reports the total from its log.

## Slice 2: Unknown slash commands

### Correction to an earlier reading

An initial audit reported that `CommandWithArgs` is discarded at
`crates/yach-ui/src/app.rs:3365`, making `/compact <focus>` unreachable.
That is wrong. Every argument-accepting action has its own dispatch arm
earlier in the match — approval at `:3279`, compact at `:3301`, and the
three extension commands at `:3344`, `:3351`, and `:3358` — and
`slash_commands::tests` covers each parse. The arm at `:3365` is a correct
fallback for commands that legitimately take no arguments, such as
`/model foo`. No change is warranted there.

### Unknown commands

The real defect at that site is one line. `SlashParseResult::Unknown` falls
through and is submitted to the model as an ordinary prompt
(`crates/yach-ui/src/app.rs:3369`). A leading-slash token that is not a
known command is a typo far more often than a prompt: `/aproval` silently
becomes a turn, spending a provider round and leaving the user's intent
unexecuted.

Unknown commands report the unknown name, suggest the nearest command by
shared prefix, and are not sent to the provider.

An earlier draft accepted catching `/usr/bin/foo is broken` as collateral,
reasoning that a suggestion is cheaper to recover from than a spent turn.
Implementation showed that is the wrong call: a test written for that case
failed, and reading it back, a path in a sentence is a realistic prompt
while a mistyped command is always a single bare token. The guard therefore
applies **only to single-token input**. Multi-word text beginning with `/`
stays a prompt, so the fix carries no collateral at all.

Suggestion uses shared-prefix length with a two-character floor, which
corrects truncations and tail typos without guessing at unrelated input.

### Acceptance

- `/aproval` reports an unknown command, suggests `/approval`, and sends no
  prompt.
- `/compact focus on the parser` still reaches the backend with its focus
  text, proving the existing dispatch is untouched.
- `/model foo` still reports unsupported arguments.
- `/usr/bin/env is missing, can you check` is still submitted as a prompt.

## Slice 3: Approval memory

Approval today has three modes — `review`, `accept-edits`, `full-access`
(`crates/yach-proto/src/lib.rs:261-278`) — and no scoped memory, so an
identical command is re-reviewed indefinitely. The approval-modes design
already anticipates this: scoped grants are named as a later slice
(`docs/project/specs/2026-08-24-approval-modes-design.md:18-21`).

This slice adds **session-scoped grants only**:

- A review offers "approve once" and "approve for this session".
- A session grant is keyed by the exact decision identity the permission
  engine already computes for the request, not by fuzzy command matching.
- Grants live in session memory, are never persisted, and do not survive
  restart. Durable allowlists remain user-config authority
  (`~/.yach/config.json`), preserving the hard authority boundary that
  repository content cannot grant execution authority
  (`docs/project/specs/2026-08-24-approval-modes-design.md:23-39`).
- Every auto-approval from a grant still records permission-decision evidence
  (`crates/yach-backend/src/session.rs:335-382`) with its provenance, so an
  audit shows why a call ran without a prompt.

Denials stay one-shot: remembering a rejection risks silently blocking work
whose context has changed.

### Acceptance

- Approving a command for the session auto-approves a byte-identical repeat
  and records evidence naming the grant.
- A different command still prompts.
- Restarting the session prompts again.
- Grants are absent from the session log's persisted state.

## Slice 4: Output visibility

Live tool output shows the last 8 lines (`crates/yach-ui/src/transcript.rs:38-40`) and is
replaced by a bounded final result. For a build or test run — the most
common long-output commands in real work — 8 lines is usually the tail of a
progress spinner rather than the failure.

The live tail grows to a bounded scrollable region, and completed tool output
remains expandable with `Ctrl+O` as it is today. The specific bound is an
implementation decision recorded in the plan; it stays a bound.

### Acceptance

- A command emitting 200 lines shows substantially more than 8 while running.
- Rendering cost stays bounded: the transcript never holds unbounded
  per-tool output.

## Slice 5: Prompt history

There is no history recall in the prompt (no handler in
`crates/yach-ui/src/input.rs` or the key dispatch at `crates/yach-ui/src/app.rs:1840-1974`).
Retyping a prompt after a cancellation or a failed turn is pure friction.

Up and Down traverse this session's submitted prompts when the cursor is on
the first or last line respectively, preserving in-line cursor movement
otherwise. History is session-scoped and in-memory, consistent with the
approval-grant decision above.

### Acceptance

- Up recalls the previous submission; Down returns toward the current draft.
- An in-progress multi-line draft is preserved when history is entered and
  restored when leaving it.
- Cursor movement inside a multi-line draft is unaffected.

## Explicitly out of scope

- **Reconnect after disconnect.** `mark_disconnected`
  (`crates/yach-ui/src/app.rs:1076-1100`) is a deliberate, complete reset of
  pending dialogs, edits, tools, and reviews. Making it recoverable means
  defining resumption semantics for in-flight authority decisions — a
  correctness design belonging to M2 (long-session correctness), not a
  hardening slice. Recorded here so it is not mistaken for an oversight.
- **Cost display.** See Slice 1.
- **New tools, capability classes, and the embedding API.** Separate design.
- **@-file mentions and editor integration.** Real friction, but each is a
  feature with its own design surface rather than a defect.

## Risks

- Approval grants weaken review if scoped too loosely. Mitigated by exact
  decision identity, session-only lifetime, and unchanged evidence recording.
- Raising output bounds affects TUI render cost, which is measured by the
  `keypress/*` and `replay/*` workloads. The implementation plan runs the
  paired latency comparison rather than assuming the effect is negligible.
