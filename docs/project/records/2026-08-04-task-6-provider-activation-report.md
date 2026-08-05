# Task 6 exact provider activation state

## Correctness changes

- Made the runner own one advertised runtime catalog snapshot. Cached and fresh connection-runtime catalogs now update that snapshot and picker rendering without depending on an active `ProviderConfig`.
- Required exact `(provider, connection_id, model_id)` membership in that advertised snapshot before detailed runtime activation, including stored-only sessions with no active provider.
- Made accepted activation replace both the active provider and the connection-flow active target in one event-loop transition. Subsequent replacement/removal actions therefore address the newly activated connection.
- Rejected legacy `ModelSelected` whenever a connection runtime exists; non-runtime adapters retain the legacy path.
- Kept the UI's raw model ID separate from its display label. State updates and model changes retain the raw ID for exact picker matching while rendering the display label in normal UI surfaces.
- Returned successful rename identity plus normalized display from the CLI runtime. The runner updates the active configuration display only when that completed rename still identifies the active connection, so stale completions cannot relabel a later activation.
- Kept activation invalidation on refresh and connection mutations; stale activation completion remains ignored by generation checks.

## Focused RED-to-GREEN evidence

- RED: `stored_only_runtime_catalog_activates_first_provider` timed out because advertised rows existed only in `ProviderConfig`; GREEN after runner-owned snapshot validation.
- RED: `runtime_rejects_legacy_model_selected_event` observed a `ModelChanged` from the legacy path while a runtime existed; GREEN after the runtime gate.
- GREEN: `provider_connection_flow_uses_accepted_b_target_for_replacement_and_removal` proves an A-to-B accepted target supplies B's replacement model and rejects B removal.
- GREEN: `initial_backend_state_uses_raw_model_id_for_exact_current_row` proves a divergent initial display label still marks only the exact connection/model row current.
- GREEN: `active_rename_updates_fallback_connection_display` proves an active successful rename changes the fallback connection display without rebuilding the adapter.

## Required verification

- `just dev cargo test -p yach-ui model_selector_marks_only_exact_connection_current` — passed.
- `just dev cargo test -p yach-backend connection_activation_failure_preserves_prior_config` — passed.
- `just dev cargo test -p yach provider_connection_switch_a_b_a_restores_complete_config` — passed.
- `just dev cargo test -p yach-backend connection_` — 26 passed.
- `just dev cargo test -p yach rename_blocks_old_label_generation_and_refreshes_new_label` — passed.

## Concerns

- No Task 7 process or restart-durability tests were added or run.
- The activation result is deliberately ignored when its generation has been retired by refresh or connection mutation; a later explicit selection is required after such invalidation.
