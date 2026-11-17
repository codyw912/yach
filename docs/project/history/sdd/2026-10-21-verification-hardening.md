# SDD ledger — plan: docs/project/plans/2026-10-21-verification-hardening.md
Fix-round budget: 3
Source: linear:CEW-147. Parent: yach#j03s. Original approved umbrella: yach#j2rz.
Task 1: Kata yach#2bqw; worker FreshStateImplementation.
Task 2: Kata yach#n862; worker SemanticVerificationImplementation.
Task 3: Kata yach#h6cz; worker ModalVerificationImplementation; README owned by controller.
Task 4: Kata yach#0hhc; controller integration and cohort reassessment.
Base: c27911fe7a11459ab329ba4a4598782477eae8c3.
Baseline: just test exit 0 artifact://581; seven-drive pack exit 0 evidence 20261021T172320Z-3858360 through 172324Z-3858983.

Ruling: Initial bounded classification was wrong once shared protocol fixture was selected. Paused workers, persisted spec and numbered plan before further implementation. Owner had approved the three improvements and subsequent gate review in conversation; artifacts record that approved scope, not a new product design.
Ruling: Use disjoint worker file ownership concurrently under controller integration, as required by harness delegation policy. Workers run no shared builds/lint/tests/formatters; parent runs final checks once. Standalone real-tmux probe is allowed for regression evidence.

## Preflight scan
| Tasks | Shared interface/files | Finding |
| --- | --- | --- |
| 1,2 | ui_wait/ui_send/ui_keys in common.sh | Task 1 owns helper, Task 2 consumes contract; no overlapping edits. |
| 1,3 | same UI helper APIs | Task 3 consumes contract; no overlapping edits. |
| 2,3 | isolated default run state | Runtime sequential under parent, not workers. |
| 1,4 | common.sh and skill API guidance | Parent updates docs after smoke; copied driver unchanged. |
| 2,4 | two new semantic features and full pack | Parent integrates index; local protocol not live proof. |
| 3,4 | modal feature + README handoff | Parent owns README, no conflicting edits. |
| 1 | current frame, changed action, settled match | Tests include stale history and unchanged frame refusal. |
| 2 | real request predicates + durable session proofs | FIFO scripted replies insufficient; std-only localhost server selected. |
| 3 | edit rejection + Escape focus | Two-option edit differs from three-option command; disk bytes + denial required. |
| 4 | final checks and unchanged portfolio | Existing failures remain until actual approved-input evidence resolves them. |

Task 1 partial edits before pause: common.sh and three existing drives; no project runtime run. RED legacy false positive /tmp/yach-fresh-ui-probe.VeBhFe/before.log exit 1; GREEN helper probe after.log exit 0. Not accepted pending integrated checks/review.
Task 2 before pause: research only, negative jq acceptance controls rejected fixture resume/no checkpoint. No code written.
Task 3 before pause: research only, no mutations.

Task 1: complete (source review clean after current-pane history and transient-status fixes; actual migrated drives pass in complete pack). Helper RED/GREEN probe final.log exit0; copied tmux driver unchanged.
Task 2: complete (provider recovery, checkpoint/context continuation and request-ID correlations exercised; final identity collision fix reviewed clean and negative rejected). Automatic restoration NOT fixed; explicit /model full-ID search + Thinking off required.
Task 3: complete (modal runtime passed; local fixture/cache prevent external discovery; one-Down rejection/bytes/denial and Escape/fresh-turn proofs). README human checklist added; no human score claimed.
Task 4: final checks in progress. Full10 pack exit0 artifact://681 (20261021T182147Z-3895939 through 182206Z-3897351). Eight refusal controls exit1 each. just test+hk exit0 artifact641; final stricter checker independent re-verification pending.
Ruling: Provider-resume drive is explicit supported recovery, not automatic restoration. Actual automatic restore failure retained. Nonempty full model-ID search reveals complete discovered list; selection needs Thinking Level confirmation. This follows portfolio allowance for recorded re-selection; cannot resolve automatic restore product finding.
Ruling: Local compaction uses keep_recent_tokens=1 and short foldable history to prove checkpoint/reconstruction, not context-pressure or live-cohort pass. Do not modify original cohort inputs to claim resolution.
Ruling: New recipes seed isolated current-schema models.dev cache and use local /models so no external discovery is needed. Existing older scripted startup may refresh public catalog; no claim all old recipes are network-free.
Final broad review: identity collision P2 fixed (fresh turn and entry IDs, exact parent linkage), scoped rereview correct confidence0.96; no open actionable findings. Runtime after fix complete pack exit0 and identity-negative exit1.
Cohort reassessment: gateway still 654/zero Messages models; wvbm remains open unchanged failure evidence. Original live session session-3782212-1792599565299051607 successfully recovered with actual approved provider turn-2 request ccf6d4c5-d608-448b-9611-6e48664cf268 after exact model search/reselect. Evidence hardening-live-resume-20261021. Live compaction and person assessments remain open. Dated record docs/project/records/2026-10-21-verification-hardening.md.
