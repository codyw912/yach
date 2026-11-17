# SDD ledger — plan: docs/project/plans/2026-09-21-intent-aware-auto-review.md

Kata parent: yach#79bk
Task 1: Kata yach#xzr7
Task 2: Kata yach#m7ay
Task 3: Kata yach#zad4
Task 4: Kata yach#09qk
Task 5: Kata yach#1mnq
Task 6: Kata yach#mgrd
Task 7: Kata yach#j453
Task 8: Kata yach#c0jc

## Pre-flight scan (2026-09-21)

| Pair / task | Shared surface | Finding | Ruling |
|---|---|---|---|
| T1 → T4 | `PolicyRevision`, `ReviewPolicy`, reason codes, SessionEvent variants | T4 consumes T1 types verbatim; names match plan | clean |
| T2 → T3 | `ReviewAssess`/`ReviewResult`/`ReviewReady` wire shapes | T3 implements contract T2 defines; sequential order enforced | clean |
| T2 → T5 | reviewer `ExtensionHostSession::review` | T5 wires coordinator to it; signature fixed in T2 brief | clean |
| T4 → T5 | `ReviewCoordinator::review_action`, `ReviewRoute` | T5 matches on all three variants; Hold → ToolReviewRequested | clean |
| T4 → T8 | `AUTO_REVIEW_EXECUTION_ENABLED` const | defined T4 (false), flipped T8 same commit as eval record | clean |
| T5 → T6 | `review_origin` field on review payloads | T5 emits holds before T6 adds the field — ordering conflict | Ruling: T5 emits `ToolReviewRequested` with reason text only; T6 adds `review_origin` and T7 renders it. T5 tests assert hold occurs, not the field. — avoids forward dep — cost if wrong: T6 must retrofit field emission, small |
| T6 → T7 | `Capability::AutoReview`, `ApprovalMode::AutoReview`, `ReviewerStatusChanged` | T7 consumes all three; sequential | clean |
| T1 self-consistency | tests vs code specified | store load/replace/revision tests match signatures | clean |
| T5 self-consistency | `for_review_target` vs EditEngine unconditional metadata rejection | scout confirmed: authority paths stay rejected, exact-allowlist only for granted file | clean |
| T8 self-consistency | enablement gate | amended to compile-time const per review advisory | clean |

Ruling: plan's `### Task` heading level verified against materialize-plan.mjs parser (### required, contiguous 1..8) — parse check passed locally before commit.
