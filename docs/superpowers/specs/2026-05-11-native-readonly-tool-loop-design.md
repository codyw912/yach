# Native Read-Only Tool Loop Design

Date: 2026-05-11
Status: accepted

## Context

Native yach now has local read-only project inspection primitives:

- project-root path metadata;
- explicit local-only text context packages;
- bounded local-only text search;
- a metadata-only `project_path_info` tool behind explicit permission policy.

The next Native MVP blocker is autonomous tool use. The model must be able to request a tool, but yach must own validation, policy, execution, result shaping, session evidence, and continuation boundaries. This slice should prove that loop with a safe read-only tool before any live provider continuation or provider-visible file contents are introduced.

## Goal

Add a backend-only autonomous tool-loop helper that executes safe read-only tool requests from completed provider-style tool calls and returns provider-bound result objects.

The helper should be shaped like runtime code, but it should remain test-driven and backend-local in this slice. It should be ready for later native-provider integration without making live provider calls now.

## Non-Goals

- No live `--backend native-provider` integration.
- No provider continuation network call.
- No text read/search provider exposure.
- No file contents sent to a provider.
- No file edits or mutations.
- No process, shell, or network tools.
- No approval UI or `yach-proto` changes.
- No raw tool arguments, raw file contents, or raw provider payload persistence.

## Recommended Shape

Add a backend-owned read-only tool-loop helper that accepts:

- session id and turn id;
- completed `ProviderToolCall` values;
- project resource root;
- tool registry and explicit permission policy;
- loop/result limits.

It should:

1. Enforce loop limits before execution.
2. Convert provider calls into `PendingNativeToolRequest` values.
3. Validate schema and permission through the existing registry/policy.
4. Execute only allowed read-only metadata tools through `ProjectReadOnlyToolExecutor`.
5. Record native session tool request and execution evidence using the existing redacted session event types.
6. Return `NativeProviderToolResult` values with provider call ids preserved.
7. Keep provider-bound content limited to metadata-only output from `project_path_info`.

The first implementation should support `project_path_info` only. `fixture_echo_metadata` remains useful for existing fixture tests, but the new helper should exercise the real project metadata executor.

## Extension Compatibility

Yach's tool loop should not assume that every tool is compiled into core forever. A future extension runtime must be able to register tools through the same definition, validation, permission, execution, result-shaping, and session-evidence path as built-ins.

This slice can use a built-in `project_path_info` executor, but the loop boundary should stay registry/executor-oriented rather than hard-coding a closed set of core tool names into provider handling. Built-ins and extension tools should differ by registration and policy, not by provider-loop semantics.

## Built-In Tool Roadmap

Existing harnesses are useful references for the eventual core tool set. Pi's built-ins include read, bash, edit, write, grep, find, and ls. That is a good starting comparison set for future yach built-ins, with room for yach-specific additions where the native architecture makes them clearer or safer.

This slice intentionally implements only the safe metadata subset before provider continuation and before file contents are provider-visible. Read, grep/search, find/list, edit/write, bash/process, and any additional core tools should each land behind explicit policy, result-shaping, session-evidence, and approval/safety decisions appropriate to their risk class.

## Data And Policy Boundaries

`project_path_info` returns:

- normalized project-relative path;
- entry kind;
- byte size for files;
- provider visibility marker `never`.

It must not return:

- file contents;
- absolute host paths;
- credentials;
- command output;
- raw provider payloads;
- text read/search results.

Provider visibility remains conservative. The result object may be shaped as provider-bound because later provider continuation needs that seam, but this slice does not actually submit it to a provider.

The permission policy remains explicit. A denied `project_path_info` request should produce session evidence and no execution.

## Error Handling

The loop should fail closed:

- unknown tool: validation failure, no execution;
- malformed or oversized arguments: validation failure, no execution;
- permission denied: denial record, no execution;
- too many tool calls: loop error before executing any call;
- oversized execution result: failure record and no provider-bound result for that call;
- resource path errors: execution failure with redacted reason and no absolute path leakage.

For this slice, stop on the first failing tool call. Later runtime integration can decide whether a turn may continue with partial tool results.

## Session Evidence

For each accepted tool call attempt, append existing backend-internal session events:

- `ToolRequestRecorded` with redacted argument summary;
- `ToolExecutionFinished` with completed, denied, validation-failed, or failed outcome;
- result summaries that include compact metadata only.

Do not add new JSONL stability claims or migration tooling. These records remain provisional backend-internal native session evidence.

## Testing

Add focused backend tests for:

- successful `project_path_info` provider-call execution and session evidence;
- permission denied by default;
- unknown tool rejection;
- traversal/resource path error without absolute path leakage;
- result size limit failure;
- tool-call count limit before execution;
- provider call id preservation in `NativeProviderToolResult`.

Tests should use temporary project roots and no network. They should prove the loop uses yach-owned validation/execution/session records, not provider SDK execution.

## Metrics And Benchmarks

No new benchmark is required in this slice. Existing resource benchmarks cover the read-only primitives. Add timing metrics only when the helper is wired into a native runtime path that can produce meaningful per-turn evidence.

## Acceptance Criteria

This slice is complete when backend tests prove that yach can take a completed provider-style `project_path_info` tool call, validate and authorize it, execute project metadata lookup, record redacted session evidence, and return a provider-bound metadata-only result without contacting a provider.

## Follow-Up

After this slice, the next likely work is provider continuation mapping:

- translate `NativeProviderToolResult` into one provider adapter's continuation input;
- validate provider-specific requirements around call ids and result content;
- keep provider adapters from executing tools or mutating sessions;
- still avoid file contents until provider-visible resource policy is explicitly approved.
