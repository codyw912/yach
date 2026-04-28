# Project Now

Last updated: 2026-04-28

## Current objective

Implement the native backend path from `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`, committing logical slices as they land.

## Current branch

`feat/native-backend-seams`

## Completed in this branch

- `eac5aba feat(backend): add native runner seam groundwork`
  - Added `crates/yach-backend`.
  - Added backend metadata/capability types and channel helpers.
  - Extracted initial Pi TUI backend startup/run shape in CLI.
- `7573304 feat(backend): add native session log skeleton`
  - Added provisional native session/event primitives.
  - Added append-only JSONL persistence/reload tests.
  - Documented that native session records are backend-internal, not protocol wire commitments yet.
- `74b992c docs(project): initialize cockpit state`
  - Added `.project/` cockpit for durable local continuity.
- `1240615 refactor(backend): centralize TUI backend session launch`
  - Added `BackendSession` / `start_backend_session(...)` in `yach-backend`.
  - Updated CLI TUI/bench launch paths to use the shared backend session launch helper.
- `0683be0 feat(backend): define provider stream seam`
  - Added dogfood-minimum provider request/model/message/extension types.
  - Added provider stream events and normalized provider errors.
- `ea2ee0c fix(review): apply autofix feedback`
  - Renamed native session log persistence to `write_to_file` with truncate semantics.
  - Added JSONL blank-line load coverage and Pi TUI initialize-failure coverage.
  - Cleaned cockpit ready-next chunk formatting.

## Validation status

Latest validation:

- `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` passed for runner session launch/review-fix slices.
- `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` passed for provider stream seam slice.
- `just dev cargo test -p yach-backend -p yach-cli` passed for runner session launch/review-fix slices.
- `just dev cargo test -p yach-backend` passed for provider stream seam slice.
- `just dev cargo test --workspace` passed after implementation slices.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on committed implementation/docs slices.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: mostly complete for current phase; CLI now launches through shared backend session state, while Pi process IO remains CLI-local by design for now.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: first committed P0 slice complete; no provider SDK dependencies added.
- U5 provider-library spike: not started.
- U6 native backend dogfood runner: not started.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Runner session launch exists but is intentionally small; avoid moving Pi process IO into `yach-backend` unless a later unit justifies it.
- Provider seam is P0-only and should be refined by U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

### 1. U5 provider-library evaluation fixtures/spike scaffold

- **Why it matters:** Provider-library evaluation needs yach-owned request/event/error types before Rig/Siumai/direct SDKs are considered.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs` for seam refinements if needed; `docs/spikes/2026-04-27-rig-provider-evaluation.md`; optional fixture module/files only if useful.
- **Max scope:** Characterization-first fixtures and evaluation notes; no permanent provider SDK dependency unless explicitly approved.
- **Dependencies/blockers:** Current U4 P0 provider seam commit.
- **Validation command:** `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` and `just dev cargo test -p yach-backend`.
- **Risk level:** Medium.
- **Stop/ask condition:** If the spike requires network/API credentials, large dependency churn, or a durable Rig/direct-provider decision.
- **Human approval needed:** Ask before adding provider SDK dependencies or making a durable provider choice.

### 2. U4 dogfood-minimum provider seam follow-up, if fixture pressure exposes gaps

- **Why it matters:** Provider-library evaluation needs yach-owned request/event/error types before Rig/Siumai/direct SDKs are considered.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs` initially; split only if concrete consumers justify it.
- **Max scope:** P0 text request/stream/error types and fixture-style tests; no provider SDK dependencies; no real API calls.
- **Dependencies/blockers:** Prefer finishing enough of U2 first, but can proceed after current session skeleton if runner work stalls.
- **Validation command:** `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` and `just dev cargo test -p yach-backend`.
- **Risk level:** Medium.
- **Stop/ask condition:** If common types start encoding provider-specific options outside adapter-owned extension metadata.
- **Human approval needed:** No.

### 3. U7 checkpoint docs after next implementation slice

- **Why it matters:** Project OS should reflect that native backend implementation has moved from planning into active seams.
- **Expected files/areas:** `docs/project-os/next-work.md`, possibly `docs/project-os/roadmap.md`; `docs/project-os/decisions.md` only for a new durable decision.
- **Max scope:** Status/provenance update only; no broad doc rewrite.
- **Dependencies/blockers:** Do after a clean committed implementation slice.
- **Validation command:** N/A for docs; optionally `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** If updating priority order rather than status/provenance.
- **Human approval needed:** No for factual status, yes for reordering committed priorities.
