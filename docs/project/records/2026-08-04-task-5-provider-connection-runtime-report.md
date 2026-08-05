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
