# Project Now

Last updated: 2026-05-03

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
  - Added backend-owned `BoundedProviderStreamBuffer` fixture policy that coalesces text deltas, preserves lifecycle boundaries by dropping queued text when possible, and returns a structured backpressure failure when the buffer cannot make progress.
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
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, and `just dev cargo test -p yach-backend -p yach-cli` passed after bounded provider stream buffer slice.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on committed implementation/docs slices.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: mostly complete for current phase; CLI now launches through shared backend session state, while Pi process IO remains CLI-local by design for now.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: first committed P0 slice complete; no provider SDK dependencies added.
- U5 provider-library spike: fixture-backed seam pass in progress; no provider SDK dependency added.
- U6 native backend dogfood runner: first fake/fixture slice in progress; explicit CLI selection, limited status/model/session responses, fixture prompt streaming, native JSONL persistence, fixture-backed failed/cancelled turn persistence, native-only UI cancel emission, explicit prompt finish events, and backend-owned bounded provider stream buffer policy are implemented. Remaining follow-up: wire/refine fixture error envelopes around malformed/backpressure cases and checkpoint native dogfood evidence.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Runner session launch exists but is intentionally small; avoid moving Pi process IO into `yach-backend` unless a later unit justifies it.
- Provider seam is P0-only and should be refined by U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

These chunks are drawn from `.project/phases/02-fixture-native-dogfood-runner.md`. Later provider-dependency work remains approval-gated and is not the immediate implementation queue.

### 1. U6 native fixture error envelope refinement

- **Why it matters:** Provider adapter work needs native dogfood errors that can represent failure, malformed stream, cancellation, and backpressure without provider-specific leakage.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs`, `crates/yach-cli/src/main.rs`, possibly `docs/protocol/yach-proto-v0.md` if UI-visible types change.
- **Max scope:** Narrow error variants/copy/tests needed by fixture runner only; no full provider taxonomy and no credential/setup errors.
- **Dependencies/blockers:** Prefer after or alongside bounded queue/backpressure policy so any backpressure error shape is fixture-proven.
- **Validation command:** `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings` and `just dev cargo test -p yach-backend -p yach-cli`; if protocol/UI changes, also run `just dev cargo clippy -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli --all-targets -- -D warnings` and `just dev cargo test -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli`.
- **Risk level:** Medium.
- **Stop/ask condition:** If error taxonomy expands into a full provider matrix, auth/credential setup, or provider-specific extension policy.
- **Human approval needed:** No for fixture-scoped refinement; yes before provider dependency/credential work.

### 2. U6 native dogfood smoke/evidence checkpoint

- **Why it matters:** The phase should produce evidence that future provider work can trust the native runner lifecycle.
- **Expected files/areas:** `docs/protocol/yach-proto-v0.md`, `docs/project-os/next-work.md`, possibly a focused evidence/status doc if repo convention requires it, plus `.project/now.md` at wrap.
- **Max scope:** Factual status/evidence update after implementation passes; no priority reorder and no declaration that native mode is production-ready/default.
- **Dependencies/blockers:** Do after chunks 1–2 are validated, or after a smaller clean implementation slice if that slice is worth checkpointing.
- **Validation command:** `git diff --check`; code validation should already have passed for the implementation slice being documented.
- **Risk level:** Low.
- **Stop/ask condition:** If docs would change committed priority order, alter default backend policy, or claim native mode supersedes Pi.
- **Human approval needed:** No for factual updates; yes for priority/default-backend decisions.
