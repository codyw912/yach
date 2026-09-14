# Daily Driver Hardening Implementation Plan

Status: IMPLEMENTED 2026-09-14 — all five slices landed; 1380 tests pass,
clippy clean on the dev pin and on CI stable, fmt clean, and the three
interactive surfaces verified visually against the real binary.
Date: 2026-09-14
Design: `docs/project/specs/2026-09-14-daily-driver-hardening-design.md`

Four corrections during execution, each recorded in the spec:

1. Slash arguments already dispatched. The claim that `CommandWithArgs` was
   discarded was wrong; only the unknown-command fall-through needed fixing.
2. Token totals were first deferred on a false blocker. `MetricRecorded`
   carries only durations, but `EntryAppended.provider` carries
   `ProviderMetadata.usage`, so usage *is* persisted and summable across
   turns. The slice shipped as specified.
3. The compaction count is backend-owned, not transcript-derived: a
   client-side count is erased by `/clear` and undercounted by scrollback.
4. Grants are a shared handle, not a value moved into a turn. The first
   implementation took the set for the duration of an awaited turn and wrote
   it back afterward, which would have discarded every prior approval if
   that task were cancelled.

Also fixed, found while verifying slice 2: `apply_backend_state` overwrote
`status_message` on every periodic update, so any response to a user action
could be replaced moments after appearing. Backend state may now only
replace a status it owns, and `"compacting"` is included in that set so it
cannot linger after compaction ends. This was found by reading the overwrite
path, not by reproducing it as the cause of a specific missing message.

### Verification status

Verified: 1380 unit tests, clippy clean on the dev pin (1.94.0) and on CI
stable, fmt clean.

Verified against the real binary over `yach rpc`: a resumed log with two
checkpoints reports `compaction_count: 2`, and the fixture log replays to
`total_tokens: 554`, matching the usage persisted on its assistant entry.
Both fields previously did not exist on the wire.

Verified visually against the real binary under vhs
(`tests/visual/hardening.tape`, captures in `target/tui-visual/`):

- `/status` renders `tokens: 554` and `compactions: 0`; the status bar shows
  `Σ554`.
- A mistyped `/aproval` shows "unknown command /aproval — did you mea…",
  leaves the text in the prompt, and sends no turn.
- Up recalls a submitted prompt back into the input box.

An earlier note here claimed the visual harness was unusable. That was half
wrong and is corrected: `Wait+Screen` does not match the TUI's alternate
screen once it takes over, which is why the pre-existing `session.tape`
waits fail, but screenshots and key delivery work. The new tape settles on
timing and proves state through captures.

Visual verification also found a defect unit tests could not: the status
message was the lowest-priority segment, so it was dropped first whenever
the bar overflowed — losing the answer to the user's last action exactly on
narrow terminals. It now outranks the ambient indicators and truncates
rather than disappearing.

Five independent slices. Each lands with its own tests and leaves the tree
green. Slices 1-2 are truthful-state and dead-control defects; 3-5 are
friction features. Order is by evidence value, not size.

## Slice 1: Compaction count

Currently `Transcript::compaction_count` is `fn (&self) -> usize { 0 }`
(`crates/yach-ui/src/transcript.rs:541-543`). Two consumers read it:
`/status` (`crates/yach-ui/src/app.rs:3228-3229`) and the status-bar `⟲`
segment, which is gated on `> 0` so it is currently unreachable
(`crates/yach-ui/src/status_bar.rs:85-90`).

1. Determine how a compaction checkpoint reaches the transcript. The backend
   emits `SessionEvent::CompactionCheckpoint`
   (`crates/yach-backend/src/session.rs:436-442`) and hydration projects
   checkpoint markers (`crates/yach-backend/src/runner/session_state.rs`).
   Identify the transcript entry kind that represents one; if none exists,
   the count must come from a field the UI already receives, not a new
   protocol event.
2. Implement the count over actual entries. Keep it O(1) amortized by
   maintaining a counter on insert rather than scanning on every render —
   `/status` is cheap but the status bar renders per frame.
3. Tests: a transcript with two checkpoint entries reports 2; an empty one
   reports 0; a hydrated resumed session reports its prior checkpoints.
   Delete the existing `assert_eq!(transcript.compaction_count(), 0)` at
   `crates/yach-ui/src/transcript.rs:1283` — it pins the stub.

## Slice 2: Token total and slash controls

Two unrelated defects, batched because both are small and touch disjoint
files.

### 2a. Token total

The stats projection hardcoded `total_tokens: None`. Usage is persisted per
assistant entry as `ProviderMetadata.usage`
(`crates/yach-backend/src/session.rs:93-103`), already summed across that
turn's requests (`crates/yach-backend/src/runner.rs:4354-4356`), so the
projection sums those entries.

1. Thread the accumulated total into the stats projection. Confirm the
   accumulator's lifetime matches a session rather than a turn; if it is
   per-turn, sum across the session's turns at projection time rather than
   inventing new state.
2. Add a status-bar segment below context percentage in priority order, so
   narrow terminals drop tokens before context health
   (`crates/yach-ui/src/status_bar.rs`).
3. Tests: projection carries a provider-reported total; omits the segment
   when usage is absent; a resumed session reports the total from its log.

### 2b. Unknown slash commands

Investigation retracted the original claim that `CommandWithArgs` is
discarded. Every argument-accepting action already dispatches earlier in the
match: approval `crates/yach-ui/src/app.rs:3279`, compact `:3301`, extension
stop/reload/status `:3344`, `:3351`, `:3358`. `slash_commands::tests` covers
the parses and passes. The arm at `:3365` correctly rejects arguments to
commands that take none. No change there.

The remaining defect is `crates/yach-ui/src/app.rs:3369`: `Unknown` falls
through and is sent to the provider as a prompt.

1. Handle `Unknown` explicitly: report the unknown name, suggest the nearest
   command by prefix using the existing `match_slash_commands` helper, and
   do not submit a prompt.
2. Tests: an unknown command produces a suggestion and sends nothing;
   `/compact <focus>` still reaches the backend, proving existing dispatch
   is untouched; `/model foo` still reports unsupported arguments.

## Slice 3: Session-scoped approval grants

`PermissionDecisionEngine::decide_shell`
(`crates/yach-backend/src/permission.rs:426-463`) already takes
`user_allowlisted: bool` and emits a provenance `reason`. A session grant is
a second pre-check with its own reason, not a new decision path.

1. Add session-scoped grant state owned by the runner, keyed by the exact
   identity the engine already computes for a request. Do not invent fuzzy
   command matching; byte-identical requests only.
2. Extend the decision inputs so a grant produces
   `PermissionDecision::Allowed` with reason `session_grant` — distinct from
   `shell_user_allowlist` and `approval_mode_full_access`, so evidence
   distinguishes all three.
3. Protocol: the review decision gains a scope. Prefer extending the
   existing decision enum additively over a new event. Rejections stay
   one-shot; do not add a deny scope.
4. TUI: the review row offers approve-once and approve-for-session; the
   selector already supports multi-option rows
   (`crates/yach-ui/src/app.rs:1421-1448`).
5. Grants are memory-only. Assert they never reach the session log; the
   permission-decision evidence records that a grant applied, which is the
   durable artifact.
6. Tests: identical repeat auto-approves with `session_grant` evidence; a
   different command prompts; grants absent from persisted state; a denial
   does not persist.

## Slice 4: Live output visibility

`STREAM_TAIL_MAX_LINES = 8` (`crates/yach-ui/src/transcript.rs:38-40`).

1. Raise the bound. It remains a constant; the value is chosen against
   render cost, not removed.
2. Measure rather than assume: run the paired latency comparison for the
   `keypress/*` and `replay/*` workloads before and after
   (`just perf` with a filter). The spec names this explicitly as a risk.
   Record the result in the commit message; if a row regresses beyond
   budget, lower the bound rather than raising the budget.
3. Tests: a long-output tool retains the raised bound and no more.

## Slice 5: Prompt history

No history handler exists in `crates/yach-ui/src/input.rs` or the key
dispatch (`crates/yach-ui/src/app.rs:1840-1974`).

1. Record submitted prompts in a session-scoped in-memory ring on submit
   (`crates/yach-ui/src/app.rs:3235` `submit_input`).
2. Bind Up/Down only when the cursor is on the first/last line of the draft,
   preserving in-line movement. Stash the in-progress draft on entry and
   restore it when navigating past the newest entry.
3. Tests: recall previous submission; draft preserved and restored;
   multi-line cursor movement unaffected; empty history is a no-op.

## Verification

- `just test`, `just lint`, `just fmt-check` after each slice.
- CI clippy reproduction on stable before publishing, per AGENTS.md — the
  dev shell pins 1.94.0 and CI resolves newer.
- TUI smoke test against the real binary for the interactive slices (2b, 3,
  5): these are user-facing behaviors that unit tests cannot fully prove.
  Done via `tests/visual/hardening.tape`.
- Slice 4 additionally requires the paired latency comparison above.

## Non-goals

Reconnect after disconnect (M2 correctness work), cost display, new tools or
capability classes, the embedding API, @-file mentions, editor integration.
