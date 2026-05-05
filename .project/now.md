# Project Now

Last updated: 2026-05-05

## Current objective

Continue the native backend path from `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`. Phase 5 native-owned tools/resources/session-model hardening is now deepened in `.project/phases/05-native-tools-resources-session-hardening.md`; resource/config root policy planning is complete and next work should plan the native tool lifecycle/permission model before implementation.

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
  - Provider-failure dogfood follow-up added unit/fixture coverage for auth failure, unavailable/invalid model, timeout, and network classification; provider stream timeouts now map to `ProviderErrorKind::Timeout`; failed native turns persist normalized provider error kind plus redacted debug context without raw payloads or credentials.
- Phase 4 minimal real native dogfood path has been deepened into `.project/phases/04-minimal-real-native-dogfood-path.md` with ready chunks for native-provider error UX planning, narrow status/error copy polish, and factual evidence checkpoint.
- Native-provider error UX plan added at `docs/plans/2026-05-04-001-feat-native-provider-error-ux-plan.md`; it recommends existing `StatusUpdated`/`PromptFinished` events for the first polish slice and defers typed protocol errors until dogfood proves status-only UX insufficient.
- Narrow native-provider status/error copy polish implemented with existing protocol events only: setup failures are prefixed as native-provider setup failures, runtime provider failures include snake_case normalized error kind plus concise hints, and native session failed-turn reasons continue to persist normalized kind plus redacted debug context.
- Native-provider evidence checkpoint updated project OS/protocol docs to record that native-provider failure UX remains on existing `StatusUpdated` / `PromptFinished` events; no typed protocol error event, default backend change, retry loop, raw payload persistence, or broad provider UX was added.
- Phase 4 was rechunked after status-only UX completion: next ready planning chunks are typed protocol error event design and native-provider smoke harness feasibility plan; later implementation remains approval-gated.
- Typed protocol error event design added at `docs/plans/2026-05-04-002-design-typed-protocol-error-event.md`. Recommendation: keep status-only for now; if approved later, prefer a general `ServerEvent::ErrorRaised(ProtocolError)` over prompt-only error details.
- Native-provider smoke harness feasibility plan added at `docs/plans/2026-05-04-003-plan-native-provider-smoke-harness-feasibility.md`. Recommendation: do not add a broad harness yet; if approved later, start with missing-config smoke coverage and a narrow fake provider runtime path for no-secret success/failure/cancel tests.
- Roadmap reconciliation updated `.project/roadmap.md` to remove stale Phase 2/3/4 gates, added native-provider opt-in decision entries to both `.project/decisions.md` and `docs/project-os/decisions.md`, and identified Phase 5 deepening as the next major planning need.
- Phase 5 native tools/resources/session hardening plan added at `.project/phases/05-native-tools-resources-session-hardening.md`, with workstreams for resource roots, tool lifecycle/permissions, provider tool-call mapping, native session branch records, redaction/debug policy, and evidence checkpoints.
- Resource/config root policy plan added at `docs/plans/2026-05-05-001-plan-resource-config-root-policy.md`. Recommendation: start implementation later with backend-internal project-root canonicalization/read helpers and tests only; defer provider-visible reads, user/global config roots, compatibility imports, reload/discovery semantics, and broad resource UI until approved.

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
- `just dev cargo fmt`, `just dev cargo clippy --workspace --all-targets -- -D warnings`, `just dev cargo test --workspace`, and `git diff --check` passed after provider-failure dogfood follow-up.
- `git diff --check` passed after the U7 factual project OS/native dogfood checkpoint doc follow-up.
- Owner-run manual provider-request evidence passed Anthropic and ChatGPT/Codex subscription happy-path controls and produced invalid-model failures for both providers; classifier follow-up now treats provider `not_found` / `not supported` model-shaped failures as unavailable-model failures and CLI failure summaries include normalized provider error kind.
- `just dev cargo fmt`, `just dev cargo test -p yach-backend -p yach-cli`, `just dev cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` passed after manual-evidence classifier/doc follow-up.
- `git diff --check` passed after Phase 4 chunking/planning update.
- `git diff --check` passed after native-provider error UX plan.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-cli -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-cli -p yach-backend`, and `git diff --check` passed after narrow native-provider status/error copy polish.
- `git diff --check` passed after native-provider evidence checkpoint docs.
- `git diff --check` passed after Phase 4 rechunking update.
- `git diff --check` passed after typed protocol error event design.
- `git diff --check` passed after native-provider smoke harness feasibility plan.
- `git diff --check` passed after roadmap/decision-log reconciliation.
- `git diff --check` passed after Phase 5 deepening.
- `git diff --check` passed after resource/config root policy planning.

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

### 1. Native tool lifecycle and permission plan

- **Why it matters:** Provider tool calls and native tools are high-trust boundaries; yach needs an owned lifecycle before execution.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, references to provider seam docs and `docs/project-os/architecture-invariants.md`.
- **Max scope:** Planning/design only. Define tool registry shape, schema validation, permission defaults, execution boundary options, result redaction/size policy, and first safe tool candidate. No code changes.
- **Dependencies/blockers:** Can follow or run after Chunk 1; no provider env needed.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium-high due security implications, but planning-only.
- **Stop/ask condition:** Stop before committing to default permission behavior, executing tools, provider tool-result continuation, or process/network/file mutation policy.
- **Human approval needed:** No for planning; yes before implementing permission/security behavior.

### 2. Native session branch/tool record shape plan

- **Why it matters:** Tool/resource work will add richer records. The native session model should represent parent links, branches, tool calls/results, provider metadata, and outcomes without copying provider/Pi-owned sessions.
- **Expected files/areas:** `docs/plans/`, `.project/now.md`, possibly `docs/protocol/yach-proto-v0.md` if UI-visible implications are documented.
- **Max scope:** Planning/design only. Propose provisional backend-internal record additions and migration cautions. No code changes, no stable format promise.
- **Dependencies/blockers:** Can follow or run after Chunks 1/2; no provider env needed.
- **Validation command:** `git diff --check`.
- **Risk level:** Medium due session model coupling.
- **Stop/ask condition:** Stop before declaring native JSONL stable, adding migration tooling, or changing user-visible session tree policy.
- **Human approval needed:** No for planning; yes before stable format/migration decisions.

## Candidate next chunks

- Implement typed protocol error event only after owner approval of `docs/plans/2026-05-04-002-design-typed-protocol-error-event.md`.
- Add native-provider missing-config smoke assertion after approval/selection from `docs/plans/2026-05-04-003-plan-native-provider-smoke-harness-feasibility.md`.
- Add narrow fake provider runtime path for no-secret native-provider runtime tests after approval/selection from the feasibility plan.
- Additional approved real-provider failure runs for auth/rate-limit/network timeout.
