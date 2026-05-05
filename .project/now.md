# Project Now

Last updated: 2026-05-03

## Current objective

Continue the native backend path from `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md` after the merged native-provider dogfood checkpoint.

## Current branch

`main` (PR #12 merged; local main fast-forwarded to `origin/main`)

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
  - No runtime provider SDK path, network/API credential path, or provider dogfood path added for native fixture mode; a separate compile-time `rig-core` mapping spike exists under U5.
- Rig real-provider smoke diagnostics:
  - Added `smoke-rig-openai-compatible`, direct-control `smoke-openai-compatible-http`, and stock-provider `smoke-rig-anthropic` CLI commands.
  - OpenCode Zen direct curl and direct Rust HTTP control succeeded; OpenRouter direct Rust HTTP control succeeded via same OpenAI-compatible env shape.
  - Rig OpenAI-compatible smoke failed against OpenCode Zen and OpenRouter with zero events and collapsed HTTP client error.
  - Stock Rig Anthropic smoke succeeded: `event_count=5`, `text_delta_count=2`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.
  - Rig ChatGPT/Codex subscription OAuth smoke succeeded after device login: `event_count=4`, `text_delta_count=1`, `completed=true`, `matched_expected_text=true`, `response_chars=17`.
  - Added backend-internal `RigProviderAdapterConfig` / `RigProviderConfig` skeleton and `run_provider_request(...)` entry point for the working Anthropic and ChatGPT/Codex subscription paths. This consumes yach-owned `ProviderRequest` and emits yach-owned `ProviderStreamEvent`; no TUI/default backend integration.
  - Added diagnostic `smoke-rig-provider-request` CLI command to exercise the new `ProviderRequest -> run_provider_request(...) -> ProviderStreamEvent` seam for `YACH_RIG_PROVIDER=anthropic` or `chatgpt-subscription`.
  - Manual provider-request diagnostics succeeded for both working providers: Anthropic (`event_count=5`, `text_delta_count=2`, `completed=true`, `matched_expected_text=true`, `response_chars=17`) and ChatGPT/Codex subscription (`event_count=4`, `text_delta_count=1`, `completed=true`, `matched_expected_text=true`, `response_chars=17`).
  - Added explicit non-default `yach tui --backend native-provider` boundary. It uses `YACH_RIG_PROVIDER=anthropic|chatgpt-subscription` and existing provider env/token-dir config to route native prompt submissions through `ProviderRequest -> run_provider_request(...) -> ProviderStreamEvent`; Pi remains default and fixture native remains `--backend native`.
  - Human dogfood confirmed `yach tui --backend native-provider` launched with native provider backend and completed a chat/response turn successfully.
  - Inspected `.yach/native-sessions/default.jsonl`: persisted user entry, assistant entry, completed turn; assistant provider metadata included `provider=chatgpt-subscription`, `model=gpt-5.3-codex-spark`, `response_id=null`. Added JSONL roundtrip test coverage for provider metadata preservation.
  - Polished native-provider initial state/model list/status so explicit `--backend native-provider` advertises the selected provider/model instead of fixture echo while preserving fixture native behavior.
  - Added first active-turn guard for native-provider mode: provider prompts run in an abortable task, concurrent prompts are rejected while active, and Ctrl+C/`PromptCancelled` aborts the task and persists a cancelled turn marker.
  - Added opt-in `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` (clamped to 30s) to make native-provider cancellation dogfood deterministic; user entries are flushed before provider calls so cancelled delayed turns leave inspectable log evidence.
  - Fixed native/native-provider capability negotiation to advertise `PromptCancellation`; without this, the UI waited for provider completion instead of sending `PromptCancelled` on Ctrl+C. Human retest confirmed cancellation now works with `YACH_NATIVE_PROVIDER_TEST_DELAY_MS`.

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
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `just dev cargo tree -p yach-backend -e normal --depth 2` passed after minimal Rig adapter spike.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, and `just dev cargo test -p yach-backend` passed after Rig adapter lifecycle accumulator follow-up.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-proto -p yach-adapter-pi-rpc -p yach-ui -p yach-cli -p yach-backend`, and `git diff --check` passed after adversarial review fixes.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and no-env `smoke-rig-openai-compatible` validation passed after opt-in Rig smoke command implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and no-env `smoke-openai-compatible-http` validation passed after direct HTTP control smoke implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and no-env `smoke-rig-anthropic` validation passed after stock Anthropic Rig smoke implementation.
- Human-run real smokes: direct HTTP OpenAI-compatible control succeeded (`status=200`, `matched_expected_text=true`); stock Rig Anthropic smoke succeeded (`completed=true`, `matched_expected_text=true`); Rig ChatGPT/Codex subscription OAuth smoke succeeded (`completed=true`, `matched_expected_text=true`); provider-request seam diagnostics succeeded for both Anthropic and ChatGPT/Codex subscription; `yach tui --backend native-provider` launched and completed a chat/response dogfood turn; Rig OpenAI-compatible smoke failed against OpenCode Zen and OpenRouter with zero events.
- Commit hooks `cargo-clippy` and `cargo-fmt` passed on committed implementation/docs slices.
- `just dev cargo clippy --workspace --all-targets -- -D warnings` passed after confirming/applying rust-magic-linter standard preset configuration.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and no-env `tui --backend native-provider` validation passed after native-provider state/model/status polish.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and no-env `tui --backend native-provider` validation passed after native-provider active-turn/cancel guard.
- Checkpoint validation passed before PR merge: `just dev cargo fmt`, `just dev cargo clippy --workspace --all-targets -- -D warnings`, and `just dev cargo test --workspace`.
- PR #12 (`Add native provider dogfood path behind Rig adapter seam`) merged, and local `main` was fast-forwarded to `origin/main`.

## Active plan status

- U1 minimal backend crate / runner seam groundwork: completed enough for first committed slice.
- U2 extract backend runner seam from CLI Pi orchestration: mostly complete for current phase; CLI now launches through shared backend session state, while Pi process IO remains CLI-local by design for now.
- U3 minimal native session/event skeleton: first committed slice complete; richer append/reload semantics can still evolve with U6 needs.
- U4 provider request/event/error seam: first committed P0 slice complete; no provider SDK dependencies added.
- U5 provider-library spike: checkpointed for current branch. Rig is viable below the yach-owned seam for Anthropic API-key and ChatGPT/Codex subscription OAuth paths; provider-request seam diagnostics and non-default native-provider dogfood passed. Rig OpenAI-compatible streaming is deferred/non-blocking after failures against Zen/OpenRouter with direct HTTP controls succeeding.
- U6 native backend dogfood runner: fixture native path and explicit native-provider dogfood path are implemented enough for checkpoint. Native-provider supports real prompt streaming, provider metadata persistence, and cancellation via negotiated `PromptCancellation`; Pi remains default.
- U7 project OS/protocol update gate: partially touched via `docs/protocol/yach-proto-v0.md`; broader OS updates likely at wrap/checkpoint.

## Blockers / open questions

- Runner session launch exists but is intentionally small; avoid moving Pi process IO into `yach-backend` unless a later unit justifies it.
- Provider seam is P0-only and should be refined by U5 fixture pressure.
- Native session file format is intentionally provisional and should not be treated as stable.

## Ready next chunks

### 1. U6/U5 provider error dogfood

- **Why it matters:** Happy-path and cancellation dogfood now work. The next reliability gap is provider failure behavior: auth failure, unavailable/invalid model, timeout/network classification, and redacted failed-turn persistence.
- **Expected files/areas:** `crates/yach-backend/src/lib.rs`, `crates/yach-cli/src/main.rs`, docs in `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `.project/now.md`.
- **Max scope:** Add/execute diagnostic paths for invalid key/model or controlled timeout; ensure failures map to `ProviderErrorKind` where possible and persist redacted failed turns. Preserve existing smoke commands. No default backend change, tools/resources, credential persistence beyond explicit token dir, raw payload persistence, or retry loop.
- **Dependencies/blockers:** Manual real-provider failure tests require approved provider env; no secrets in commits/logs.
- **Validation command:** `just dev cargo fmt && just dev cargo clippy --workspace --all-targets -- -D warnings && just dev cargo test --workspace`.
- **Risk level:** Medium due credentials/network/provider behavior.
- **Stop/ask condition:** Before persisting credentials, broad TUI provider UX, changing default backend, retry policy, or provider-specific protocol changes.
- **Human approval needed:** Yes for manual real-provider failure runs; no for fixture/unit tests.

### 2. U7 project OS/native dogfood checkpoint follow-up

- **Why it matters:** Project OS should stay aligned with cockpit planning and the committed native fixture lifecycle/backpressure/error slices.
- **Expected files/areas:** `docs/project-os/roadmap.md`, `docs/project-os/next-work.md`, `docs/protocol/yach-proto-v0.md`, and `.project/now.md` at wrap.
- **Max scope:** Factual status/provenance update only; no broad doc rewrite, priority reorder, or default-backend policy change.
- **Dependencies/blockers:** Do after the current docs checkpoint commit if more repo-level docs are found stale.
- **Validation command:** `git diff --check`.
- **Risk level:** Low.
- **Stop/ask condition:** If updating committed priority order, declaring native mode production-ready/default, or changing compatibility policy.
- **Human approval needed:** No for factual updates; yes for priority/default-backend decisions.
