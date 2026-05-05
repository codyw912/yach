# Provider Tool-result Continuation Loop Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/plans/2026-05-05-002-plan-native-tool-lifecycle-permissions.md`, `docs/plans/2026-05-05-003-plan-native-session-branch-tool-records.md`, `docs/protocol/yach-proto-v0.md`

## Goal

Plan the minimum safe loop for provider tool-call request -> yach-owned validation/execution -> provider tool-result continuation.

The goal is not to implement provider continuation yet. The goal is to keep yach, not Rig/provider SDKs, in control of tool execution, session mutation, permission policy, redaction, and loop limits before any local tool result can be sent back to a provider.

## Current baseline

`yach-backend` now has backend-internal pieces for the first half of the loop:

- Provider stream events can represent `ToolCallStarted`, `ToolCallDelta`, and `ToolCallCompleted`.
- `ProviderToolCall` preserves provider call id, tool name, and JSON arguments.
- `pending_tool_request_from_provider_call(...)` maps provider call metadata into a yach-owned `PendingNativeToolRequest`.
- `NativeToolRegistry` validates fixture-safe tool arguments and deny-by-default permission policy.
- `record_native_tool_validation(...)` appends provisional tool request/session outcome records with redacted payload summaries.
- `FixtureNativeToolExecutor` proves an in-process execution trait boundary for fixture-safe tools only.
- Native JSONL tool records remain backend-internal/provisional.

Missing pieces:

- A yach-owned representation of tool results intended for provider continuation.
- A provider request/adapter API that can resume/continue a turn with tool results.
- Loop limits, cancellation semantics across tool execution + provider continuation, and transcript/session ordering policy.
- Tests that prove provider libraries never execute tools or mutate sessions directly.

## Proposed loop shape

### 1. Accumulate provider tool requests

Provider adapters surface tool-call events only. The native runner/loop should:

1. Accumulate `ToolCall*` events into completed `ProviderToolCall`s.
2. Convert each completed provider call into a yach-owned pending tool request.
3. Validate schema and permission using yach-owned registry/policy.
4. Record provisional session events for request/validation before any execution.

If a provider emits malformed, duplicate, or incomplete tool calls, yach records a normalized failure and ends or rejects continuation without executing anything.

### 2. Execute only yach-approved tools

Only validated and allowed yach requests can reach the execution boundary.

First implementation candidate should use fixture-safe tools only. Do not add file/process/network tools in the provider loop slice.

Execution output should become a yach-owned `NativeToolResultForProvider`-style value:

- yach tool request id
- provider call id metadata, if needed by the provider adapter
- status (`completed`, `failed`, `denied`, `cancelled`, `validation_failed`)
- compact content/summary intended for provider continuation
- byte count and truncation/redaction markers
- normalized error kind/reason when relevant

Raw tool args/results should not be persisted by default; provider-bound content must pass size/redaction policy first.

### 3. Continue provider turn through yach provider seam

Add a provider-seam operation that accepts yach-owned continuation input, not provider SDK-owned history mutation.

Possible shape:

- Extend `ProviderRequest` with prior messages/tool results, or
- Add `ProviderContinuationRequest` with original turn/model/context plus tool results.

Recommendation: start with a separate backend-internal continuation request type so the initial prompt request remains simple and the continuation semantics are explicit.

Provider adapters translate yach-owned tool result values into provider-specific SDK payloads. They must not execute tools, discover tools, or own session state.

### 4. Enforce loop limits

Minimum limits before implementation:

- max tool calls per turn
- max continuation rounds per turn
- max provider-bound tool result bytes per tool and per turn
- cancellation check before execution and before each provider continuation
- fail closed on unknown tool, validation failure, denied permission, malformed stream, backpressure, or result-size violation

Recommended first defaults for fixture tests:

- one continuation round
- one or small fixed number of fixture tool calls
- small byte caps to exercise truncation/rejection paths

### 5. Persist ordered session evidence

Session event ordering should be deterministic:

1. user entry / turn start evidence
2. provider tool-call request record
3. validation/permission outcome
4. execution result summary or rejected outcome
5. provider continuation assistant text/outcome, if continuation happens
6. final turn outcome

Keep records provisional and redacted. Do not add migration tooling or stable JSONL claims.

## Protocol/UI impact

No protocol change is needed for the first backend fixture continuation tests.

Protocol becomes necessary only when:

- UI must show pending tool approvals,
- UI must show tool progress/results in real time,
- users can approve/deny tool execution,
- resource/tool output is user-visible beyond existing transcript/status surfaces.

Until then, keep continuation tests in `yach-backend` and explicit native dogfood paths.

## Recommended first implementation slice

Add backend-internal fixture continuation primitives and tests only.

Expected scope:

- A yach-owned provider-bound tool result type with redaction/size metadata.
- A small loop helper that takes completed `ProviderToolCall`s, validates/executes fixture-safe tools, records provisional session events, and returns provider-bound tool results.
- Unit tests for success, validation failure, permission denial, oversized result rejection/truncation policy, and cancellation/loop-limit behavior if the helper models it.
- No real provider SDK continuation, no runtime native-provider integration, no file/process/network tools, no protocol/UI changes, no provider-visible local resource reads.

Suggested validation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

## Later implementation chunks

1. Add provider adapter continuation mapping for one real provider path behind explicit opt-in fixtures and no-secret tests if possible.
2. Integrate continuation into explicit `--backend native-provider` only after fixture loop tests pass and owner approves live provider behavior.
3. Add approval UI/protocol events only when non-fixture or user-impacting tools require them.
4. Add first non-fixture read-only metadata tool only after policy approval.

## Stop/approval gates

Stop before:

- real provider continuation in `native-provider`,
- sending local file/resource contents to a provider,
- adding file/process/network tools,
- changing default backend behavior,
- adding permission UI/protocol events,
- persisting raw tool args/results/provider payloads,
- declaring native JSONL stable.

## Recommendation

Proceed next with backend-only fixture continuation primitives if implementation is approved. Keep it deliberately non-user-facing and no-secret. This creates pressure-tested yach-owned loop semantics before any real provider SDK continuation or local data exposure.
