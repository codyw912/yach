# Project Now

Last updated: 2026-04-28

## Current objective

Implement the native backend path from `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`, committing logical slices as they land.

## Current branch

`feat/provider-seam-spike`

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
- Provider-library evaluation spike in progress:
  - Report: `docs/spikes/2026-04-28-rig-provider-evaluation.md`
  - Initial recommendation: keep Rig limited/evaluate further; compare Siumai and GenAI with the same fixtures before adding provider dependencies.
  - Fixture-backed seam pass added tool-call streaming placeholders, usage/finish metadata, provider response id metadata, normalized error fixtures, and cancellation coverage.
- U6 fake/fixture native dogfood runner slice in progress:
  - Added explicit `yach tui --backend native` selection while preserving Pi as default.
  - Native mode advertises `yach-native-dogfood`, reports limited status/model/session state, streams fixture prompt responses through existing TUI protocol events, and persists an inspectable `.yach/native-sessions/default.jsonl` event log.
  - Fixture prompts `/native-fixture-fail` and `/native-fixture-cancel` now exercise failed/cancelled native turn persistence; dropped UI receivers mark the active native turn cancelled before returning.
  - Added narrow protocol events for `PromptCancelled` and `PromptFinished`, with native-only Ctrl+C cancel emission from the TUI and native runner completion/failure/cancel finish events.
  - No provider SDK dependency or network/API credential path added.

## Validation status

Latest validation:

- `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` passed for runner session launch/review-fix slices.
- `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` passed for provider stream seam slice.
- `just dev cargo test -p yach-backend -p yach-cli` passed for runner session launch/review-fix slices.
- `just dev cargo test -p yach-backend` passed for provider stream seam slice.
- `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` passed after native dogfood runner slice.
- `just dev cargo test -p yach-backend -p yach-cli` passed after native dogfood runner slice.
- `just dev cargo clippy -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli --all-targets -- -D warnings` passed after protocol-level cancel/finish slice.
- `just dev cargo test -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli` passed after protocol-level cancel/finish slice.
- `just dev cargo test --workspace` passed after earlier implementation slices.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on committed implementation/docs slices.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: mostly complete for current phase; CLI now launches through shared backend session state, while Pi process IO remains CLI-local by design for now.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: first committed P0 slice complete; no provider SDK dependencies added.
- U5 provider-library spike: fixture-backed seam pass in progress; no provider SDK dependency added.
- U6 native backend dogfood runner: first fake/fixture slice in progress; explicit CLI selection, limited status/model/session responses, fixture prompt streaming, native JSONL persistence, fixture-backed failed/cancelled turn persistence, native-only UI cancel emission, and explicit prompt finish events are implemented. Remaining follow-up: bounded internal queue/backpressure tests beyond receiver-drop handling and richer provider-error envelopes.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Runner session launch exists but is intentionally small; avoid moving Pi process IO into `yach-backend` unless a later unit justifies it.
- Provider seam is P0-only and should be refined by U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

### 1. U5 real adapter dependency decision / fixture comparison

- **Why it matters:** The yach-owned seam now has fixture coverage for text, errors, cancellation, usage/finish metadata, and tool-call placeholders. The next decision is whether to add a real provider-library adapter spike and which candidate to try first.
- **Expected files/areas:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`; optional new provider crate/module only with approval; `Cargo.toml` only if adding a dependency.
- **Max scope:** Decide and, if approved, add one minimal provider-library adapter spike behind the existing seam; no native dogfood runner yet.
- **Dependencies/blockers:** Requires human approval before adding provider SDK dependencies or making a durable provider choice.
- **Validation command:** If code changes, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` and `just dev cargo test -p yach-backend`.
- **Risk level:** Medium/high due dependency choice.
- **Stop/ask condition:** Before adding Rig/Siumai/GenAI/direct SDK dependencies, before network/API credentials, or before broad seam split.
- **Human approval needed:** Yes.

### 2. U6 native dogfood runner follow-up: bounded queue/backpressure semantics

- **Why it matters:** Native cancel/finish semantics are now typed, but real provider streams still need a tested policy for slow consumers and bounded internal queues.
- **Expected files/areas:** `crates/yach-cli/src/main.rs`, possibly `crates/yach-backend/src/lib.rs`, and `docs/protocol/yach-proto-v0.md`.
- **Max scope:** Add fixture-backed slow-consumer/backpressure behavior or a small backend-owned queue helper with tests; no real provider SDK, tools, or resource loading.
- **Dependencies/blockers:** Current fake native runner slice; protocol changes should stay narrow and typed.
- **Validation command:** `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` and `just dev cargo test -p yach-backend -p yach-cli`.
- **Risk level:** Medium.
- **Stop/ask condition:** If protocol changes become broad, native mode needs credentials, or scope expands into real tools/resources.
- **Human approval needed:** Ask before real provider credentials/dependencies; not needed for fake/fixture native runner hardening.

### 3. U4 dogfood-minimum provider seam follow-up, if fixture pressure exposes gaps

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
