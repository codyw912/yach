# Project Now

Last updated: 2026-05-08

## Current objective

Continue the native backend path from `docs/plans/2026-04-27-004-feat-native-backend-path-plan.md`. Phase 5 native-owned tools/resources/session-model hardening is now deepened in `.project/phases/05-native-tools-resources-session-hardening.md`; resource/config root, native tool lifecycle/permission, native session branch/tool record, provider tool-result continuation, real provider continuation adapter mapping, and first non-fixture native tool candidate planning are complete. The first Phase 5 implementation slices added backend-internal project resource root/path helpers, explicit local-only resource text reads, a native tool registry/validation skeleton, fixture-only tool execution boundary, provisional native session tool record variants, and provider tool-call fixture-to-validation/session-record wiring. Provider tool-result continuation is planned and the backend-only fixture continuation primitive slice is implemented. Real provider continuation adapter mapping is planned and the backend-only continuation request validation/mapping skeleton is implemented. The recommended first non-fixture native tool candidate is backend-only `project_path_info`, pending owner approval before implementation. Next work should be owner-approved before any non-fixture tool implementation, real provider SDK mapping, native-provider integration, live provider calls, file/process/network tools, provider-visible resource reads, or resource UI.

## Current branch

`phase5-next-native-backend-chunk`

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
- Native tool lifecycle and permission plan added at `docs/plans/2026-05-05-002-plan-native-tool-lifecycle-permissions.md`. Recommendation: start later with a backend-internal registry/validation skeleton and fixture-safe tool only; defer user-facing permission defaults, file/process/network tools, provider tool-result continuation, raw arg/result persistence, protocol approval UI, and user-defined tool loading until approved.
- Native session branch/tool record shape plan added at `docs/plans/2026-05-05-003-plan-native-session-branch-tool-records.md`. Recommendation: add backend-internal record variants only under implementation pressure, likely after tool registry skeleton exists; keep native JSONL provisional and defer migration/import/user-visible tree policy until approved.
- Backend-internal project resource root/path helper slice implemented in `crates/yach-backend/src/lib.rs`: `NativeResourceRoot`, root kind, normalized path errors, project-root canonicalization, file/directory resolution, traversal/symlink escape rejection, and focused tests. No provider-visible reads, resource UI, Pi import, watcher/reload, or credential/config persistence was added.
- Backend-internal native tool registry/validation skeleton implemented in `crates/yach-backend/src/lib.rs`: fixture-safe tool definition, allowlisted object schema validation, pending request/validation types, deny-by-default permission policy with explicit fixture allowlist, normalized tool errors, and tests for unknown tools, malformed/schema/oversized args, default denial, and explicit fixture allowance. No real tool execution, provider continuation, file/process/network mutation, TUI permission UI, protocol change, raw arg/result persistence, or user-defined tool loading was added.
- Provisional native session tool record variants implemented in `crates/yach-backend/src/lib.rs`: yach-owned `NativeToolRequestId`, redacted `NativeToolPayloadSummary`, `NativeToolOutcome`, `ToolRequestRecorded`, and `ToolExecutionFinished` JSONL variants with roundtrip tests for completed tool records and validation-failure summaries without raw args. Native JSONL remains backend-internal/provisional; no migration/import tooling, user-visible tree policy, protocol events, provider-hosted session sync, or raw payload persistence was added.
- Provider tool-call fixture wiring implemented in `crates/yach-backend/src/lib.rs`: helper maps `ProviderToolCall` into a yach-owned `PendingNativeToolRequest`, validation records append redacted provisional session tool records, and validation failures append non-executed terminal tool outcomes. No real tool execution, provider tool-result continuation, provider loop integration, protocol/UI surface, raw argument persistence, or file/process/network mutation was added.
- Follow-up resource helper slice implemented in `crates/yach-backend/src/lib.rs`: explicit `NativeResourceReadPolicy::local_only(...)`, provider visibility fixed to `Never`, text read results with byte/redaction/truncation metadata, size-limit enforcement, UTF-8 rejection, and tests proving path policy reuse. No provider submission, resource UI, Pi import, watcher/reload, credential persistence, or automatic context injection was added.
- Fixture-only tool execution boundary implemented in `crates/yach-backend/src/lib.rs`: `NativeToolExecutor` trait, `FixtureNativeToolExecutor`, execution result/error shape, and tests proving only validated/allowed fixture-safe tools execute with redacted summaries. No file/process/network tool, provider continuation, provider loop integration, UI/protocol surface, or user-defined tool loading was added.
- Provider tool-result continuation loop plan added at `docs/plans/2026-05-05-004-plan-provider-tool-result-continuation.md`. Recommendation: next implementation, if approved, should add backend-only fixture continuation primitives/tests; defer real provider SDK continuation, native-provider integration, provider-visible local resources, file/process/network tools, protocol/UI approval surfaces, raw payload persistence, and stable JSONL claims.
- Backend-only fixture provider tool-result continuation primitives implemented in `crates/yach-backend/src/lib.rs`: yach-owned provider-bound tool result type, continuation policy/context/error types, fixture loop helper that validates/executes fixture-safe provider tool calls, records provisional session events, enforces tool-call/result-size limits, and tests success, validation failure, permission denial, oversized result rejection, and tool-call limit behavior. No real provider SDK continuation, native-provider integration, file/process/network tools, protocol/UI changes, or provider-visible resource reads were added.
- Real provider continuation adapter mapping plan added at `docs/plans/2026-05-05-005-plan-real-provider-continuation-adapter-mapping.md`. Recommendation: next implementation, if approved, should add backend-only `ProviderContinuationRequest`/validation mapping skeleton and tests; defer real provider SDK mapping, live calls, native-provider integration, provider-visible resources, protocol/UI surfaces, and raw payload persistence.
- Backend-only provider continuation request validation/mapping skeleton implemented in `crates/yach-backend/src/lib.rs`: `ProviderContinuationRequest`, validation policy/error types, validation helper, and tests for metadata preservation, missing provider call id, oversized content, and redacted/truncated policy. No real provider SDK mapping, native-provider integration, live calls, file/process/network tools, protocol/UI changes, or raw payload persistence was added.
- Current performance benchmark refresh documented in `docs/benchmarks/current-baseline-2026-05-05.md` and indexed from `docs/project-os/performance-evidence.md` / `docs/benchmarks/README.md`. It records yach-only headless replay, live Crossterm draw/flush proxies, transcript scroll, and synthetic-ready PTY first-output. `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` was unset and is not used by these yach-bench harnesses, so no provider-delay removal was needed for this benchmark update.
- First non-fixture native tool candidate plan added at `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`. Recommendation: ask for owner approval to implement backend-only `project_path_info` next; it should return project-relative metadata only, stay provider-visible `never`, preserve deny-by-default policy, and stop before native-provider integration, file contents, process/network/file mutation, protocol/UI approval surfaces, or stable JSONL claims.
- Native backend completion audit handoff added at `.project/handoffs/2026-05-05-native-backend-completion-audit.md`. It maps the full native-backend objective to current code/docs evidence and identifies the remaining approval-gated deliverables before the backend can be called fully implemented.
- Dev environment migrated to the trimmed Rust template shape: default shell no longer includes Zig/cargo-zigbuild, musl cross targets, Kani setup, or the full `languages.c` module; `prek` git hooks and `just run -p yach-cli` remain.
- Architecture deepening refactor completed after owner approval:
  - Split `yach-backend` crate root into focused runner, resource, tools, session, provider, Rig adapter, diagnostics, and native runner modules while preserving public re-exports.
  - Moved native dogfood runner behavior from `yach-cli` into backend-owned `run_native_dogfood_loop(...)` with explicit runner/provider config.
  - Added a deeper native tool continuation workflow module interface while preserving the existing fixture helper.
  - Added a provider diagnostics module for smoke-facing imports and moved CLI smoke callers to that seam.
  - Extracted UI lifecycle status classification into a dedicated reducer module.

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
- `git diff --check` passed after native tool lifecycle and permission planning.
- `git diff --check` passed after native session branch/tool record shape planning.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after backend-internal project resource root/path helper implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after backend-internal native tool registry/validation skeleton implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after provisional native session tool record implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli`, and `git diff --check` passed after provider tool-call fixture-to-validation/session-record wiring.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after local-only native resource text read helper implementation.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after fixture-only native tool execution boundary implementation.
- `git diff --check` passed after provider tool-result continuation loop planning.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after backend-only fixture provider tool-result continuation primitives.
- `git diff --check` passed after real provider continuation adapter mapping planning.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend --all-targets -- -D warnings`, `just dev cargo test -p yach-backend`, and `git diff --check` passed after backend-only provider continuation request validation/mapping skeleton.
- Native fixture-backend TUI PTY first-output benchmark passed via a temporary wrapper that launched `yach-cli tui --backend native`: `just dev cargo build -p yach-cli -p yach-bench --release` passed; 20-sample warm run collected 20/20 (`p50=6.113ms`, `p95=9.343ms`, `p99=9.503ms`, `max=9.503ms`); 100-sample run collected 100/100 (`p50=6.703ms`, `p95=10.640ms`, `p99=11.315ms`, `max=12.662ms`).
- Current performance benchmark refresh passed: `just dev cargo run -p yach-bench --release -- headless-report --samples 1000`; `script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-report --samples 500`; `terminal-keypress-report --samples 500`; `terminal-active-stream-report --samples 500`; `terminal-async-backlog-stress-report --samples 500`; `terminal-heavy-output-report --samples 500`; `terminal-transcript-scroll-report --samples 200`; `terminal-transcript-scroll-stress-report --samples 50`; `yach-tui-ready-startup-report --samples 100`. Results are recorded in `docs/benchmarks/current-baseline-2026-05-05.md`.
- `git diff --check` passed after first non-fixture native tool candidate planning.
- `git diff --check` passed after native backend completion audit handoff.
- `nix flake check --no-pure-eval`, `nix build --no-pure-eval --dry-run --no-link '.#devShells.aarch64-darwin.default^*'`, `nix develop --no-pure-eval -c just check`, `nix develop --no-pure-eval -c prek run --all-files`, and `git diff --check` passed after dev environment template migration.
- `just dev cargo fmt`, `just dev cargo clippy -p yach-backend -p yach-cli -p yach-ui --all-targets -- -D warnings`, `just dev cargo test -p yach-backend -p yach-cli -p yach-ui`, and `git diff --check` passed after the architecture deepening refactor.

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

No ready implementation chunks are currently approved/scoped. The planning set recommends backend-only `project_path_info` as the first non-fixture native tool candidate, but implementation affects security/session architecture and should be owner-approved before code changes.

## Candidate next chunks

- If approved, implement backend-only `project_path_info` native tool skeleton from `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`; stop before native-provider integration, provider-visible metadata/results, file contents, file/process/network mutation, protocol/UI approval surfaces, or stable JSONL claims.
- Inspect/provider-specific continuation mapping for a selected provider only after approval; stop before live calls or native-provider integration.
- Implement typed protocol error event only after owner approval of `docs/plans/2026-05-04-002-design-typed-protocol-error-event.md`.
- Add native-provider missing-config smoke assertion after approval/selection from `docs/plans/2026-05-04-003-plan-native-provider-smoke-harness-feasibility.md`.
- Add narrow fake provider runtime path for no-secret native-provider runtime tests after approval/selection from the feasibility plan.
- Additional approved real-provider failure runs for auth/rate-limit/network timeout.
