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

## Validation status

Latest full validation before cockpit init:

- `just dev cargo test --workspace` passed.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on both commits.
- Targeted `yach-backend` clippy/test passed for native session skeleton.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: partially complete; CLI is less monolithic, but a general runner trait/handle and clearer Pi runner ownership remain.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: not started.
- U5 provider-library spike: not started.
- U6 native backend dogfood runner: not started.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Exact runner trait/handle boundary is still provisional.
- Provider seam signatures should wait for U4/U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

### 1. U2 finish runner seam extraction

- **Why it matters:** Native dogfood mode needs CLI runner selection without more Pi-specific orchestration accumulating in `run_tui_command()`.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs`, `crates/yach-cli/src/main.rs`, `crates/yach-cli/Cargo.toml` if needed.
- **Max scope:** Introduce a small runner/handle abstraction or equivalent launch seam; keep Pi RPC behavior default; do not implement native provider/session streaming yet.
- **Dependencies/blockers:** Current U1/U2 commits on branch.
- **Validation command:** `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings && just dev cargo test -p yach-backend -p yach-cli` plus workspace tests before commit if cross-crate impact grows.
- **Risk level:** Medium.
- **Stop/ask condition:** If the abstraction wants to move Pi process IO into `yach-backend` or introduce async traits/dependency churn beyond the plan.
- **Human approval needed:** No, if scope stays within runner seam extraction.

### 2. U4 dogfood-minimum provider seam

- **Why it matters:** Provider-library evaluation needs yach-owned request/event/error types before Rig/Siumai/direct SDKs are considered.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs` initially; split only if concrete consumers justify it.
- **Max scope:** P0 text request/stream/error types and fixture-style tests; no provider SDK dependencies; no real API calls.
- **Dependencies/blockers:** Prefer finishing enough of U2 first, but can proceed after current session skeleton if runner work stalls.
- **Validation command:** `just dev cargo clippy -p yach-backend --all-targets -- -D warnings && just dev cargo test -p yach-backend`.
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
