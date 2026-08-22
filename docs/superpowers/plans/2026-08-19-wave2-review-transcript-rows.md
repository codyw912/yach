# Wave 2 Review And Transcript Rows Implementation Plan

Status: active 2026-08-19
Date: 2026-08-19
Design: `docs/superpowers/specs/2026-08-19-wave2-review-transcript-rows-design.md`

## Contract

- One transcript entry owns a tool call from start through live output, optional review, and final result. Finishing a call mutates that entry; it does not append a second semantic row.
- `ToolResult.output` and resumed session tool text carry the exact bounded backend result payload. Summary text is derived in `yach-ui` without consuming the detail.
- Agent edit and command review use `ToolReviewDecision::{Approve, Reject}`. The temporary `/debug-edit` harness keeps `LocalEditDecision::{Apply, Reject}` until that harness is removed.
- A pending review is rendered inline in its tool row. Left/`h` selects Approve and Right/`l` selects Reject; Enter submits. The decision collapses the row while retained detail remains available.
- Ctrl+O toggles all expandable finished rows, matching Pi's default tool-output expansion action. The model keeps per-row expansion state so a later granular action is additive.
- Review request, decision, and interruption are append-only session events. A request is flushed before the backend emits `ToolReviewRequested` and waits.
- Resume reconstructs the same finished row, including review payload and resolution, but never restores an actionable review wait.

## Slices

1. **Protocol and durable evidence.** Add the generic review decision, tool-result display metadata, and resumed review DTOs in `yach-proto`. Add review requested/decision/interrupted events in `yach-backend::session`; update JSONL fixtures and exhaustive matches. Preserve the temporary local-edit protocol unchanged.
2. **Backend emission and persistence.** Emit raw bounded tool-result detail plus structured display metadata. Persist review requests before waiting and persist the terminal decision or interruption for native edit and Bash reviews. Project review history into `SessionMessage` on resume.
3. **Semantic transcript row.** Extend `TranscriptEntry` with bounded detail, expansion state, and optional review state. Keep live output tails bounded. Derive collapsed summaries in one pure function and render expanded detail without duplicating the tool row.
4. **Inline interaction.** Route agent reviews into the matching transcript row instead of `AppMode::LocalEditReview`; retain that modal only for `/debug-edit`. Implement selector navigation, Enter submission, submitted-state input suppression, collapse-after-decision, review-pending status, and Ctrl+O global expansion.
5. **Invariant matrix and actual TUI.** Extend `rpc_review`/`rpc_matrix` for approve, reject, stale decision, interruption, persistence ordering, and resume parity. Exercise edit and command review in the actual TUI and confirm the transcript remains visible, decisions are single-submit, output expands/collapses, and post-review prompt input works.

## Acceptance

- A reviewed tool has one semantic transcript row through pending, submitted, and finished states.
- Pending edit review shows the bounded diff; command review shows command, workdir, and timeout. Neither clears the transcript.
- Selector keys work without review-specific approve/reject shortcuts; Enter sends exactly one correlated decision.
- Approve/reject and interruption produce distinct durable evidence. Crashing after request persistence cannot erase that the wait occurred.
- Finished rows start collapsed. Ctrl+O reveals the exact bounded result and review detail, then collapses all rows again.
- Live and resumed rows have equal summary, detail, outcome, and review resolution. Resumed rows are never actionable.
- The RPC invariant matrix and an actual-TUI smoke scenario pass.

## Non-goals

Per-row keybindings, arbitrary extension-rendered components, multi-review concurrency, restoring pending waits after restart, changing provider-continuation outcome semantics, and removing the `/debug-edit` harness.
