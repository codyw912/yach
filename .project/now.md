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
  - Added fixture-scoped provider error constructors for provider failure, malformed stream, backpressure, and cancellation; native fixture prompt `/native-fixture-malformed` now persists a failed malformed-stream turn.
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
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, and `just dev cargo test -p yach-backend -p yach-cli` passed after native fixture error envelope refinement slice.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on committed implementation/docs slices.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: mostly complete for current phase; CLI now launches through shared backend session state, while Pi process IO remains CLI-local by design for now.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: first committed P0 slice complete; no provider SDK dependencies added.
- U5 provider-library spike: fixture-backed seam pass in progress; no provider SDK dependency added.
- U6 native backend dogfood runner: first fake/fixture slice in progress; explicit CLI selection, limited status/model/session responses, fixture prompt streaming, native JSONL persistence, fixture-backed failed/cancelled/malformed turn persistence, native-only UI cancel emission, explicit prompt finish events, backend-owned bounded provider stream buffer policy, and fixture-scoped provider error constructors are implemented. Remaining follow-up: checkpoint native dogfood evidence.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Runner session launch exists but is intentionally small; avoid moving Pi process IO into `yach-backend` unless a later unit justifies it.
- Provider seam is P0-only and should be refined by U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

### 1. U5 real adapter dependency decision / fixture comparison

- **Why it matters:** Fixture-native dogfood lifecycle/backpressure/error semantics are now documented enough to support the next provider seam decision. The project needs to compare provider-library candidates before approving real dependencies.
- **Expected files/areas:** `docs/spikes/2026-04-28-rig-provider-evaluation.md`; possibly `.project/phases/03-provider-adapter-evidence.md` if deepening is useful; optional new provider crate/module only after approval; `Cargo.toml` only if adding a dependency.
- **Max scope:** Decide the next provider comparison/spike path and, only if approved, add one minimal provider-library adapter spike behind the existing seam; no native dogfood default switch, tools, resources, or broad protocol split.
- **Dependencies/blockers:** Requires human approval before adding Rig/Siumai/GenAI/direct SDK dependencies, network/API credential paths, or making a durable provider choice.
- **Validation command:** If code changes, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings` and `just dev cargo test -p yach-backend`; docs-only changes use `git diff --check`.
- **Risk level:** Medium/high due dependency choice.
- **Stop/ask condition:** Before adding provider SDK dependencies, credentials, network calls, or encoding provider-specific options into common core types.
- **Human approval needed:** Yes for dependencies/credentials/durable provider choice; no for docs-only comparison planning.

### 2. U7 project OS/native dogfood checkpoint follow-up

- **Why it matters:** Project OS should stay aligned with cockpit planning and the committed native fixture lifecycle/backpressure/error slices.
- **Expected files/areas:** `docs/project-os/roadmap.md`, `docs/project-os/next-work.md`, `docs/protocol/yach-proto-v0.md`, and `.project/now.md` at wrap.
- **Max scope:** Factual status/provenance update only; no broad doc rewrite, priority reorder, or default-backend policy change.
- **Dependencies/blockers:** Do after the current docs checkpoint commit if more repo-level docs are found stale.
- **Validation command:** `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** If updating committed priority order, declaring native mode production-ready/default, or changing compatibility policy.
- **Human approval needed:** No for factual updates; yes for priority/default-backend decisions.
