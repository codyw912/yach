# Native Backend Completion Audit

Date: 2026-05-05
Status: not complete; blocked on owner approval for next implementation slice

## Objective audited

Get the native Rust backend fully implemented, including tool usage and other important backend aspects.

For this repo, that means more than chat streaming. The backend needs durable yach-owned sessions, provider streaming, cancellation, error handling, resources, tool validation/execution, tool-result continuation, security/redaction policy, and explicit dogfood evidence while keeping `yach-proto` as the UI/backend seam.

## Prompt-to-artifact checklist

| Requirement | Evidence inspected | Current status |
| --- | --- | --- |
| Native Rust backend crate and runner seam | `crates/yach-backend/src/lib.rs`; `.project/now.md` completed commits `eac5aba`, `1240615` | Partially complete. Backend session launch exists, but Pi process IO remains CLI-local by design. |
| UI/backend seam remains yach-owned | `.project/brief.md`; `.project/now.md`; `yach tui --backend native/native-provider` status in prior validation | Satisfied for current slices. `yach-ui` remains protocol-oriented; no provider SDK/UI coupling noted in cockpit evidence. |
| Native fixture dogfood runner | `.project/now.md` U6 evidence and validations | Implemented enough for fixture dogfood, including lifecycle/failure/cancel persistence. |
| Real provider native dogfood | `.project/now.md` Rig evidence; `crates/yach-cli/src/main.rs` references to `native-provider`; `crates/yach-backend/src/lib.rs` `run_provider_request` | Implemented as explicit opt-in for Anthropic and ChatGPT/Codex subscription paths. Pi remains default. |
| Provider errors and cancellation | `.project/now.md` provider-failure and cancellation evidence | Implemented for current native-provider prompt loop, with normalized errors and cancelled turn persistence. |
| Native resource roots and local reads | `crates/yach-backend/src/lib.rs` `NativeResourceRoot`, `NativeResourceReadPolicy`; `.project/now.md` validation entries | Backend-internal helpers implemented and tested. Provider-visible reads remain intentionally absent. |
| Native tool registry and validation | `crates/yach-backend/src/lib.rs` `NativeToolRegistry`, `PendingNativeToolRequest`, `NativeToolPermissionPolicy` | Backend-internal fixture-safe registry/validation exists. Real/non-fixture tool definitions are not implemented. |
| Native tool execution | `crates/yach-backend/src/lib.rs` `NativeToolExecutor`, `FixtureNativeToolExecutor`; `.project/now.md` | Fixture-only execution exists. No non-fixture tool executes yet. |
| Provider tool-call mapping | `crates/yach-backend/src/lib.rs` `ProviderToolCall`, `pending_tool_request_from_provider_call`; `.project/now.md` | Fixture/backend mapping exists. Real provider loop integration is not implemented. |
| Provider tool-result continuation | `crates/yach-backend/src/lib.rs` `NativeProviderToolResult`, `ProviderContinuationRequest`; docs plans `2026-05-05-004`, `2026-05-05-005` | Backend-only continuation primitives and validation skeleton exist. No real provider SDK continuation mapping or live native-provider integration. |
| Native session tool/resource records | `crates/yach-backend/src/lib.rs` `NativeSessionEvent::ToolRequestRecorded` / `ToolExecutionFinished`; `.project/now.md` | Provisional backend-internal tool records exist. JSONL is intentionally not stable. |
| Security/redaction policy | Plans `2026-05-05-001` through `006`; `.project/now.md` stop gates | Policy exists for current slices. Provider-visible resource/tool data, raw payload persistence, and non-fixture tools remain approval-gated. |
| Performance evidence | `docs/benchmarks/current-baseline-2026-05-05.md`; `.project/now.md` benchmark entries | Current yach-only benchmark baseline recorded. Tool/resource performance impact is not measured because tool/resource dogfood is not implemented. |
| Fully implemented native backend | Aggregate evidence above | Not complete. Missing non-fixture tools, provider tool-result continuation integration, approval/protocol UX for dangerous tools, and broader native session/resource workflow dogfood. |

## Missing or weakly verified deliverables

- Non-fixture native tool implementation. Recommended first candidate is backend-only `project_path_info` from `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`.
- Real provider SDK continuation mapping for tool results.
- Integration of tool execution and provider continuation into explicit `--backend native-provider`.
- Permission/approval protocol and UI for user-facing or dangerous tools.
- Provider-visible resource/tool-result policy and evidence.
- File/process/network/edit tools, if they are in scope for "fully implemented".
- Stable native session tree/fork/resource semantics and migration story.
- Performance evidence for tool/resource-heavy transcripts.

## Current blocker

`.project/now.md` explicitly says no ready implementation chunks are approved/scoped. It requires owner approval before:

- non-fixture tool implementation;
- real provider SDK mapping;
- native-provider tool integration;
- live provider calls beyond current approved paths;
- file/process/network tools;
- provider-visible resource reads;
- resource UI.

## Next approval request

Approve implementing backend-only `project_path_info` as the first non-fixture native tool:

- metadata only, no file contents;
- project-root path policy only;
- provider visibility fixed to `never`;
- deny by default unless an explicit test/dogfood policy allows exactly this tool;
- no `yach-proto`, `yach-ui`, native-provider loop, real provider SDK, file mutation, process execution, or network access.

Suggested validation after implementation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```
