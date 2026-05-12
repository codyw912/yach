# Native Read-Only Provider Continuation Mapping Design

Date: 2026-05-11
Status: accepted

## Context

PR #19 added the backend-only read-only tool loop. Yach can now take provider-style `project_path_info` tool calls, validate and authorize them through yach-owned registry and policy seams, execute metadata lookup locally, record redacted session evidence, and return `NativeProviderToolResult` values without contacting a provider.

The next Native MVP blocker is continuation mapping. The native provider path can receive model tool calls, and yach can execute safe read-only metadata tools, but there is not yet an adapter-facing mapping that turns yach-owned tool results into the next provider request shape.

Some continuation primitives already exist on `main`:

- `ProviderContinuationRequest`;
- `ProviderContinuationValidationPolicy`;
- `validate_provider_continuation_request`;
- `NativeProviderToolResult`;
- `ProviderMessage` and `NativeRole::Tool`;
- Rig prompt mapping that already preserves `Tool` role messages as text.

This slice should use those primitives instead of replacing them.

## Goal

Add a backend-only continuation mapping skeleton for safe read-only tool results.

The mapping should take a validated `ProviderContinuationRequest` containing `NativeProviderToolResult` values and produce adapter-ready continuation input without executing tools, mutating sessions, or making a provider network call.

For the current Rig adapter, adapter-ready means a deterministic `ProviderRequest` projection that appends tool-result messages after the prior transcript. This gives yach a tested bridge from local tool execution to the existing provider request seam while keeping live provider continuation as a later explicit integration slice.

## Non-Goals

- No live provider continuation call.
- No `--backend native-provider` tool-loop integration.
- No default backend change.
- No provider SDK native tool-result block implementation yet.
- No file contents, search results, command output, or absolute host paths sent to a provider.
- No read, grep, find, ls, edit, write, bash, process, or network tools.
- No extension runtime implementation.
- No `yach-proto` or UI approval surface changes.
- No raw tool arguments, raw provider payloads, or raw local result persistence.

## Recommended Shape

Use two layers.

### 1. Provider-independent continuation submission

Add a small yach-owned normalized submission shape that is built from `ProviderContinuationRequest` only after validation.

It should:

- preserve `turn_id`, `model`, `prior_messages`, and `extensions`;
- preserve tool-result order;
- preserve `tool_request_id` for traceability and error reporting;
- require every tool result to have a provider call id;
- require at least one tool result;
- reject non-completed tool results for this first safe-read-only continuation path;
- keep `NativeProviderToolResult.content` as already-shaped provider-bound content;
- preserve byte count, redaction, truncation, and reason metadata for adapter policy checks.

This keeps the future extension ecosystem on the same registry/executor/result-shaping path. Built-in and extension tools should both produce yach-owned `NativeProviderToolResult` values before any adapter sees them.

### 2. Rig prompt projection

Add a Rig adapter helper that projects a validated continuation submission into the current `ProviderRequest` shape. It should not call `run_provider_request`.

The projected request should:

- preserve the original `turn_id`, `model`, and `extensions`;
- copy prior messages in order;
- append one `NativeRole::Tool` message per tool result, in result order;
- render each tool message as compact JSON with stable keys:
  - `provider_call_id`;
  - `status`;
  - `content`;
  - `byte_count`;
  - `redacted`;
  - `truncated`;
  - `reason`.

The JSON envelope intentionally includes provider call id because adapters need it to pair results with provider tool requests. It must not include raw arguments, absolute paths, command output, or session internals.

This is a prompt projection for the current Rig adapter, not a claim that every provider accepts plain text tool-result continuation. Provider-native tool-result block mapping remains a later adapter-specific slice after the current seam is tested.

## Data Boundaries

For this slice, the only expected successful tool result is metadata-only `project_path_info` content from the previous branch. The provider-bound content may include project-relative path metadata, kind, byte size, and conservative visibility markers. It must not include local file contents or absolute paths.

The continuation mapper should not inspect local files or re-run tools. It only consumes already-shaped `NativeProviderToolResult` values.

If later provider-specific SDKs require raw arguments, raw local contents, or provider-owned thread objects to continue, the adapter should reject that continuation rather than widening this seam.

## Error Handling

Mapping should fail closed before any adapter submission when:

- provider call id is missing;
- provider-bound content exceeds the configured size policy;
- redaction or truncation conflicts with policy;
- there are zero tool results;
- a tool result status is not `Completed`.

Existing `ProviderContinuationValidationError` can continue to cover existing validation failures. If empty or non-completed results do not fit that enum cleanly, add a small mapping-specific error enum rather than overloading provider errors or panicking.

Errors should identify yach tool request ids and stable reasons only. They should not include raw content, raw arguments, absolute paths, or provider payloads.

## Session And Adapter Boundaries

The mapper must not mutate `NativeSessionLog`.

Session evidence remains owned by the tool loop and native runner. Provider adapters should translate yach-owned continuation input into provider-specific request forms and emit provider stream events only. They should not execute tools, validate schemas, decide permissions, or write session records.

## Testing

Add focused backend tests for:

- successful mapping from a `project_path_info`-style `NativeProviderToolResult` into a continuation submission that preserves turn/model/prior messages/tool result metadata;
- Rig prompt projection preserving prior message order and appending a `Tool` message;
- projected tool message JSON preserving provider call id and content while excluding raw arguments;
- missing provider call id rejection before projection;
- empty tool result rejection before projection;
- non-completed tool result rejection before projection;
- existing continuation validation behavior still passing.

Tests should use synthetic provider/tool values and temporary roots only where needed to reuse real `project_path_info` output. No network, no provider credentials, and no provider SDK continuation calls.

## Metrics And Benchmarks

No benchmark is required. This is a small deterministic mapping path. Add timing metrics only when the mapper is wired into a runtime turn loop where per-turn evidence is meaningful.

## Acceptance Criteria

This slice is complete when backend tests prove that yach can take safe read-only `NativeProviderToolResult` values, validate them for continuation submission, and project them into the current Rig `ProviderRequest` seam as ordered tool messages without running tools, mutating sessions, or contacting a provider.

## Follow-Up

After this slice, the next likely work is explicit native-provider integration for one continuation round:

- collect provider tool calls from a live native-provider turn;
- execute the safe read-only local loop;
- map results through this continuation seam;
- call the provider only behind explicit opt-in native-provider mode;
- record final turn outcome with clear loop limits and cancellation semantics.

Provider-native SDK tool-result block support can also be planned separately once the exact Rig/provider APIs are inspected and can be tested without leaking local data.
