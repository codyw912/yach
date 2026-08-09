# Task 5 provider-connection runtime integration

## Correctness changes

- Reserved the transient environment row outside the 64 persisted-connection limit and kept it selectable but read-only.
- Retained environment listing and model discovery when persisted metadata is unavailable, with secret-free warnings.
- Preserved provider timeout and test-delay defaults in the runtime; restored the native legacy discovery fallback if runtime setup is unavailable.
- Added a locked ready-credential repair path after validation, preserving pending-record repair behavior.
- Seeded native connection refreshes with the configured environment active target.
- Discarded stale runtime publications, installed accepted runtime catalog snapshots into active provider configuration, and applied catalog profile fields on environment activation.
- Preserved typed authentication/network validation outcomes; reported bounded discovery and truncation warnings while retaining successful rows.
- Kept all models of the active connection ahead of aggregate truncation and synthesized an active row when discovery omits or cannot load it.


## Re-review hardening

- Gave the environment runtime a separate outer adapter allocation while `ProviderSecret` stays opaque and shared; rejected listed-model mutations now leave the active model unchanged.
- Clear accepted runtime snapshots on every successful mutation, including active replacement candidates, so removed and renamed rows cannot remain selectable.
- Coalesced a pending runner refresh and added a runtime publication generation: an older discovery cannot overwrite a newer snapshot.
- Rechecked ready-record credential absence after acquiring the per-ID repair lock; a restored credential now conflicts without being overwritten.
- Reused the environment catalog row as the active backend fallback, preserving its reserved connection ID and keeping a 4096-row snapshot at 4096 rows.

- Changed runner refresh handling to one active plus one latest pending task: every request retires the active generation immediately, stale settlement is discarded, and the pending task starts with the current active configuration.
- Clear `ProviderConfig.catalog_models` and publish the active-only fallback synchronously after every successful mutation, including resolved active replacement candidates; a fresh discovery failure leaves that fallback intact.
## Focused evidence

- RED: CLI cache invalidation and replacement-candidate snapshot tests failed while stale rows remained; the overlapping-refresh test failed when the older refresh published last.
- RED: backend shared-adapter selection left `provider.model` changed; repeated picker requests started two refreshes; an environment catalog at cap rendered 4097 rows. Ready-repair accepted and overwrote a restored credential. Runner mutation-refresh tests failed because the catalog clear helper and queued-refresh state machine were absent.
- GREEN: `cargo test -p yach-backend runner::tests` (124 passed) and `cargo test -p yach-backend` (520 passed).

## Note

The requested `xd://lsp` mount was unavailable in this workspace; exported-boundary edits were checked with crate test compilation instead.

## Fix Round 2 completion

- Manual native compaction now sends a focus-only envelope clone and installs the unfocused normal instructions for subsequent turns.
- Native replay invalidation is target-scoped across session/provider/model changes. A failed or malformed target stays tombstoned, while A→B→A reloads A's matching checkpoint.
- Missing, duplicate, and late terminal raw output now tombstone the active target and produce one warning; the runner also tombstones before returning an early collection error.
- Replay round commits reset to the exact envelope sent, then append terminal raw output and all converted tool results under one short-lived store lock. This preserves a live nudge once, with the exact empty-output → nudge-input → retry-output order.
- Runner startup resolves the same effective fallback project context for manual compaction and ordinary provider turns, so static instructions match even without an explicit project root.
- Coverage: no checkpoint; summary/nonmatching checkpoint rejection; native failure then summary and later native recovery; second native checkpoint; pre-turn, overflow, and mid-turn estimates/refill; retry; final round commit; tool-output pair; missing/duplicate/late output tombstone-and-warn-once; nudge ordering; sync finalization; static-instruction changes; manual equality; focused manual clone/install isolation; and target A→B→A reload.
- RED: `manual_native_compaction_sends_focus_but_installs_normal_instructions` failed because the installed replay retained focus instructions. `native_replay_target_switch_reloads_prior_session_checkpoint` failed because a global invalidation prevented A from reloading after B.
- GREEN: `just dev cargo test -p yach-backend native_replay` (13 passed) and `just dev cargo test -p yach-backend compaction` (32 passed).


## Fix Round 4

- Preserved a target-scoped tombstone when manual compaction returns `NotApplied`: the loop now republishes the returned local replay, and the test proves that republishing `None` cannot restore the stale window before canonical assembly.
- Made `switch_native_session` transactional for both `JoinError` and the inner `io::Result`: either load failure emits only its status and leaves session path/id, store, log, indices, and replay unchanged.
- Exercised both client routing paths: `SessionSelected` loads B and `SessionPathSelected` reloads A's exact two persisted messages, with no B prompt in A.
- Re-ran the cumulative native replay/compaction lifecycle coverage, including native failure then summary fallback and recovery, canonical checkpoint rejection, exact replay commit ordering, static instructions, focus isolation, and the pre-turn/overflow/mid-turn compaction paths. Canonical vectors now require one returned window plus exactly one post-checkpoint item; a valid artifact with capability disabled and a newest foreign native checkpoint are both rejected. The active OpenAI pre-turn runner route now captures the full production compact envelope, proves its instruction-plus-input estimate is the checkpoint's exact trigger estimate, and asserts its returned native window is the next provider request. Overflow compaction uses the same native-envelope estimate whenever the failed request carried one.

### Focused evidence

- RED: `switch_native_session_inner_load_error_preserves_current_session_and_replay` failed with the selected directory replacing A's current path. `native_replay_tombstone_survives_not_applied_before_assembly` failed when the returned local replay still contained the stale window. `active_openai_pre_turn_compaction_sends_full_envelope_and_refills_window` showed checkpoint accounting used the generic-message estimate rather than the exact prospective OpenAI envelope.
- Focused GREEN: `loop_switches_to_selected_session_path`, `switch_native_session_inner_load_error_preserves_current_session_and_replay`, and `active_openai_pre_turn_compaction_sends_full_envelope_and_refills_window` each passed.

- The repeated-compaction regression keeps one `SessionLog` through native failure, summary checkpoint, live turns, recovered native window, and second native replacement; it captures all three preparations and rejects any dropped or duplicated chain item.
- Active OpenAI coverage now drives pre-turn, overflow, and mid-turn paths with exact native instruction/input accounting, a captured `/responses/compact` envelope, native-window refill, summary detachment, and display-only mid-turn text. It also verifies raw-output/tool-result ordering, final synchronized cursor advancement, pre-checkpoint output isolation, instruction/capability refresh, and replay-store survival when a prompt task aborts.
- Production-static-context coverage compares normal and manual assembly when `project_root` is absent; focused native instructions apply only to compact calls, the mandatory summary retains focus, and installed replay returns to normal instructions.
- GREEN: `just dev cargo test -p yach-backend native_replay` — 30 passed. `just dev cargo test -p yach-backend compaction` — 34 passed.