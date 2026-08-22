# Wave 2 Review And Transcript Rows Design

Status: accepted 2026-08-19 — all owner forks decided
Date: 2026-08-19
Scope: UX sprint Wave 2 — inline tool approvals, transcript-native edit diff
review, and expandable/collapsible tool output rows through one row-state model.

## Problem

Yach's backend review seam is stronger than its TUI presentation:

- `ToolReviewRequested` already carries generic edit or command payloads over
  `yach-proto` and `ToolReviewDecisionSubmitted` correlates the response.
- Provider tool batches execute sequentially, so at most one review blocks the
  current turn.
- Edit transactions persist prepared evidence before review; command review and
  generic review history are not represented as their own session events.
- The TUI converts both edit and command review into `LocalEditReview` and opens
  a centered modal that hides the transcript and status context.
- The modal reuses edit-shaped names and `LocalEditDecision::Apply/Reject` even
  for bash commands.
- Transcript tool rows are flat strings. A running `ToolCall` becomes a
  `ToolResult`; the original preview, live tail, and review payload are replaced.
- `tool_output_summary` discards generic successful output in favor of line/byte
  counts. Read/search/list display payloads happen to bypass that summary, so
  different tools already have different accidental expansion behavior.
- Resume hydration rebuilds only finished tool-result rows. It cannot reconstruct
  a review request, decision, diff card, or interrupted review.

The result is unnecessary review friction, lost context, and a row model that
cannot support either interaction or reliable collapsed/expanded views.

## Goals

1. Routine edit and command review happens inline at the matching transcript
   row, not in a popup.
2. An edit review shows the bounded diff needed to make the decision.
3. Finished tool rows have compact and expanded views over the same bounded
   payload, including search/list previews and bash output.
4. Live events and resumed session projection produce the same row shape.
5. Review request, decision, interruption, and final tool outcome remain
   separate facts; display semantics do not rewrite provider continuation.
6. The full composed lifecycle is exact-testable through `yach rpc`.
7. The row model can later render extension-provided protocol descriptors from
   the accepted extension-first posture without accepting host-supplied TUI code.

## Non-goals

- No approval-model redesign, session grants, sandbox posture, or auto-review
  implementation.
- No parallel review queue; provider tool execution remains sequential.
- No unbounded output or diff storage.
- No artifact store for output beyond the existing bounded provider-visible
  result payload.
- No arbitrary extension renderers or client-side extension runtime.
- No general transcript editor, comments, or Wave 3 visual redesign.
- No crash-safe re-application of a prepared edit after process restart.
- No change to which provider tool-result statuses are valid for continuation.

## Cohort evidence

Pi models each tool execution as one component with call, partial result, final
result, and an `expanded` presentation input. Its generic fallback shows ten
lines when collapsed and the full bounded result when expanded. One global
key toggles all expandable chat components. Extension tool renderers receive the
same expanded state. This is simple and keeps expansion out of canonical session
state.

Pi also demonstrates the broader direction: tool result renderers and custom
transcript entries use the same component path, rather than a separate popup
system for each tool.

OpenCode 2 keeps permission state in its client/session surface and exposes
permission requests through the same server/event boundary as other session
state. Its broader app uses a permission dock and a separate diff review surface,
not a blocking terminal popup. The useful lesson is semantic rather than visual:
review belongs to the session flow and transport, not an unrelated modal API.

Yach should keep Pi's unified tool-row lifecycle, but preserve its own remote-
capable protocol and deterministic session evidence.

## Decision: one semantic row, orthogonal states

A tool call is one row identified by a stable tool request/call ID. It accumulates
semantic data across the call lifecycle instead of being replaced by unrelated
string entries.

Conceptually:

```rust
struct ToolTranscriptRow {
    id: String,
    tool_name: String,
    preview: Option<String>,
    stream_tail: String,
    review: Option<ToolReviewRowState>,
    result: Option<ToolResultRowState>,
    presentation: RowPresentation,
}

enum ToolReviewRowState {
    AwaitingDecision { request: ToolReviewRequestView },
    DecisionSubmitted { decision: ToolReviewDecision },
    Resolved { decision: ToolReviewDecision },
    Interrupted,
}

enum RowPresentation {
    Collapsed,
    Expanded,
}
```

These are design shapes, not mandated Rust names. The invariants matter:

- lifecycle and presentation are orthogonal;
- the bounded detail payload is retained when a summary is shown;
- review payload survives the final tool result in the row;
- a final tool outcome does not erase the user's review decision;
- row identity uses the protocol correlation ID, never tool name fallback when
  an ID exists;
- presentation state is client-local and is not persisted as session truth.

User, assistant, harness outcome, and error rows can remain non-interactive in
this slice. The row abstraction should not force every transcript entry into a
large generic widget hierarchy.

## Review semantics

### Generic names

Review protocol and UI names must stop pretending every request is a local edit:

- `ToolReviewDecision`: `Approve | Reject`;
- `ToolReviewRequestView` with `Edit` and `Command` payloads;
- one pending-review state keyed by `request_id` and tool call ID;
- `/debug-edit` may keep its local-only prepare/decision DTOs until the temporary
  harness is removed, but agent tool review uses generic names end to end.

A compatibility migration is unnecessary inside the repository: update every
caller and JSONL fixture in one cutover.

### Separate axes

Three facts must not be collapsed:

1. **review decision** — approved, rejected, or interrupted;
2. **tool execution outcome** — completed or failed according to the tool and
   provider-continuation contract;
3. **display outcome** — denied/failed/cancelled/etc. for user comprehension.

Bash rejection currently records `ToolOutcome::Failed`; edit rejection records
`ToolOutcome::Completed` with reason `user_rejected`. Provider continuation
accepts those existing shapes and rejects `Denied`. Wave 2 should not force both
through `ToolOutcome::Denied` merely to make the UI coherent.

Instead, persist and render the generic review decision. `Reject` displays as
`denied` while preserving the tool-specific provider/evidence outcome. This
resolves the source-semantics question without weakening continuation validity.
Sensitive-path policy denial is not a user review decision and continues to use
its structured reason plus display refinement.

### One pending review

The backend's sequential tool batch means the TUI may assume one active review
for this slice. A second distinct request while one is pending is protocol drift:
record a visible error, keep the first request actionable, and fail closed rather
than overwriting it.

Repeated delivery of the same request ID is idempotent. Decisions with stale
request, preview/review, or permission IDs remain rejected by the backend.

### Cancellation and interruption

Turn cancellation while awaiting review cancels the pending tool and transitions
the row to interrupted/cancelled. Client disconnect or backend shutdown does the
same semantically.

A session resumed after a crash may show that a review was interrupted, but it
must not offer Apply/Approve: the in-memory pending executor/transaction is gone.
Crash-safe reconstruction and revalidation need a separate design.

## Inline interaction

A pending review row is the current interactive row and is kept visible. It does
not clear or replace the transcript. The user can still scroll the transcript to
inspect surrounding evidence; a "review pending" marker remains visible when the
row is off-screen, and a jump-to-review action returns to it.

The review row contains:

- tool name and bounded call preview;
- request-specific metadata;
- command, workdir, and timeout for bash;
- path, operation, and bounded diff for edits;
- truncation markers from the backend;
- a selected action from Approve and Reject;
- submitted/resolved/interrupted state.

Interaction follows the cohort's general selector grammar rather than inventing
review-only keys:

- Up and `k` select Approve;
- Down and `j` select Reject;
- Enter activates the selected action;
- Esc safely rejects/cancels the pending review;
- Space remains available for a future multiple-choice question surface and has
  no special review meaning.
Pending edit review shows its bounded diff detail by default; the user should not
need a second action to see the evidence required for approval. After a choice,
the same row collapses while retaining an expansion affordance.

The modal remains appropriate for secrets, account login, and genuinely isolated
choices. Tool reviews stop using `AppMode::LocalEditReview` and do not render a
`Clear`-backed centered overlay.

## Collapsed and expanded output

A finished tool row stores two display forms derived from one bounded payload:

- **summary:** outcome, tool/call preview, line/byte counts, truncation, and a
  short error excerpt when needed;
- **detail:** the exact bounded `ToolResult.output` received from the backend.

`tool_output_summary` becomes a pure summary derivation; it must not replace or
consume the detail string. Read/search/list and bash then use the same mechanism.

Collapsed defaults:

- ordinary finished tool: header, summary, and a useful bounded output preview;
- command-like output: bounded tail with an omitted-earlier-lines marker;
- other tool output: first ten lines with an omitted-later-lines marker;
- failed/denied/cancelled tool: header, summary, and short reason excerpt;
- pending review: header plus full bounded decision payload/diff;
- running tool: header plus current bounded live tail.

Expanded view shows the full bounded detail, not an unbounded source:

- search/list: all returned bounded matches/entries and incomplete marker;
- read: all returned bounded text;
- bash: captured bounded head/tail payload and truncation marker;
- edit: review diff plus final bounded result metadata;
- masked resumed result: the canonical compaction mask marker only; original
  content is no longer available and the UI must not imply otherwise.

No network, disk, provider, or backend round trip occurs when toggling expansion.

## Persistence and resume parity

Review history needs bounded semantic session events. Add generic events for:

- review requested (IDs, tool name, bounded typed payload, truncation metadata);
- review decision recorded (IDs and approve/reject);
- review interrupted when a turn ends without a decision.

The request event must be flushed before `ToolReviewRequested` is emitted. The
decision event must be flushed before execution resumes. This closes the queued
"persist before review wait" durability gap for the review path rather than
adding a display-only shadow record.

Payload policy:

- edit review may persist the already bounded prepared diff summary and relative
  path; no file bodies beyond that diff;
- command review may persist the validated command, project-relative workdir,
  and timeout because the provider-visible tool arguments are already durable
  under the accepted session payload policy;
- no absolute host paths, secrets, raw provider payloads, or unbounded output;
- events carry the same tool request, provider call, permission decision, and
  preview/review IDs used elsewhere.

Session projection combines these events with `ToolRequestRecorded` and
`ToolExecutionFinished` into tool rows. Live events and resume hydration must
call the same row reducer or share exact row-building functions; separately
formatted live and resume strings are prohibited.

Rows reconstructed from older logs without review events remain valid finished
rows with `review: None`.

## Protocol

Keep semantic events rather than sending pre-rendered terminal components.
Expected protocol evolution:

- generic `ToolReviewDecision` names;
- `ToolReviewRequested` remains the actionable request event;
- an explicit resolved/interrupted event is optional live if the final tool event
  plus submitted decision is sufficient, but recorded session events remain the
  source for resume;
- `SessionMessage` may be extended with bounded structured tool-row metadata or
  replaced by a versioned transcript-row projection; choose the smaller shape
  during implementation, but do not embed ANSI/ratatui presentation;
- capability negotiation must make the new structured row/review behavior
  explicit so older clients fail visibly rather than dropping an approval.

A review-capable backend must never send an actionable request to a client that
did not negotiate the corresponding capability.

## Extension posture

The row model is first-party proof for the accepted protocol-descriptor posture.
A later extension contribution may provide bounded row labels, sections, and
actions only through validated protocol descriptors. It may not provide
ratatui code, mutate canonical review state, approve its own tool, or bypass the
backend decision ID.

Wave 2 does not implement extension row contributions. It keeps the semantic/UI
split clean enough that they remain additive later.

## Invariant matrix

Add deterministic `yach rpc` scenarios, not only TUI reducer tests:

1. edit tool call -> review requested -> persisted request visible -> approve ->
   apply -> persisted decision -> finished result;
2. same path with reject -> no file change -> display decision denied while the
   provider receives its existing valid rejected result shape;
3. command review approve and reject, with command/workdir/timeout correlation;
4. cancellation while awaiting review -> interrupted row/event -> no execution;
5. EOF while awaiting review -> interrupted persisted state;
6. stale or mismatched decision IDs fail closed and never apply/run;
7. resume after approved, rejected, and interrupted reviews reconstructs the
   same semantic row data;
8. older log without review events still hydrates a finished tool row;
9. exact capability negotiation includes the structured review-row capability.

TUI tests cover presentation and keys:

- review is inline and no review overlay is rendered;
- pending review detail is visible and kept/jumpable through scrolling;
- submission is single-shot and duplicate keys do not send duplicate decisions;
- collapsed rows preserve detail; expanded rows render the full bounded payload;
- live and hydrated row building are byte-equivalent before styling;
- narrow widths wrap without losing action state or truncation markers.

## Implementation slices

1. **Row model:** retain preview, bounded detail, derived summary, lifecycle,
   review, and presentation state in `Transcript`.
2. **Generic review protocol/evidence:** clean-cut rename, bounded review session
   events, flush-before-wait/continue, and projection.
3. **Inline review:** replace modal state/rendering with one interactive row,
   scroll/jump behavior, and single-shot decision submission.
4. **Expansion:** wire collapsed/expanded rendering over retained detail for
   live and resumed rows.
5. **Matrix and smoke:** extend RPC scenarios, then verify the actual TUI with
   fixture edit and command reviews plus expansion.

Each slice should remain reviewable, but the feature is complete only when the
composed matrix and actual TUI pass.

## Owner decisions

1. **DECIDED — global expansion first, granular later remains reachable.** One
   cohort-familiar action expands or collapses all expandable rows in Wave 2.
   The row model still owns presentation per row so a later transcript-focus
   design can add individual toggles without changing row data. Pending review
   remains independently expanded.
2. **DECIDED — use the generic selector grammar.** Review actions use
   Up/Down and `j`/`k`, with Enter to select and Esc to reject/cancel. Review-
   specific letter shortcuts are not the default; a configurable shortcut layer
   may add them later. Space stays reserved for future multiple-choice selection.
3. **DECIDED — persist review history now.** Bounded generic request, decision,
   and interruption events close the pre-review durability gap and guarantee
   resume parity.
4. **DECIDED — collapse after decision.** Pending edit diffs are expanded for
   informed review. Approve/reject collapses the row while preserving detail for
   the global expansion action.

## Sources

- Agent edit tool surface design:
  `docs/superpowers/specs/2026-05-15-native-agent-edit-tool-surface-design.md`
- Extension-first product posture:
  `docs/superpowers/specs/2026-08-19-extension-first-product-posture-design.md`
- Pi tool execution component:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/modes/interactive/components/tool-execution.ts`
- Pi interactive global expansion behavior:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/modes/interactive/interactive-mode.ts`
- OpenCode plugin/event contract:
  `https://github.com/anomalyco/opencode/blob/dev/packages/plugin/src/index.ts`
