# Native Provider One-Round Read-Only Tool Integration Design

Date: 2026-05-11
Status: proposed

## Context

The native backend now has the local pieces for safe read-only tool use:

- metadata-only `project_path_info`;
- a yach-owned validation, permission, execution, and session-evidence path;
- provider-bound `NativeProviderToolResult` shaping;
- adapter-ready continuation mapping into Rig `ProviderRequest` input.

The current `--backend native-provider` path still sends one provider request, streams text, records one assistant entry, and ignores provider tool calls. The next Native MVP slice should connect these pieces for exactly one safe read-only continuation round.

There is one important limitation: provider tool advertising is not wired yet. The current Rig adapter does not register `project_path_info` as a provider-callable tool. This slice should still add the runner orchestration so completed provider tool-call events are handled correctly when present, and so the behavior can be tested with an injected fake provider requester. Actual provider tool advertisement remains a follow-up.

## Goal

Add a guarded native-provider runtime path that can:

1. send the initial provider request;
2. collect completed provider tool calls from that response;
3. execute only `project_path_info` through the existing read-only tool loop;
4. project the resulting tool messages into one continuation `ProviderRequest`;
5. send exactly one continuation request;
6. stream and persist the final assistant response.

The implementation should be unit-testable without network by injecting a fake provider requester.

## Non-Goals

- No provider tool advertising or schema registration.
- No provider-native SDK tool-result block mapping.
- No default backend change.
- No new UI or `yach-proto` approval surface.
- No new tools beyond `project_path_info`.
- No file contents, search results, absolute host paths, raw tool arguments, or command output sent to the provider.
- No file mutation, process execution, network tools, or extension runtime.
- No multi-round autonomous loop.
- No partial success continuation after a tool validation/execution failure.

## Recommended Shape

Keep the integration in `crates/yach-backend/src/native_runner.rs`.

Add a runner-local provider requester abstraction over:

```rust
ProviderRequest -> Future<Output = Result<Vec<ProviderStreamEvent>, ProviderError>>
```

Production should delegate to `rig_adapter::run_provider_request`. Because the current production adapter config is consumed by `run_provider_request`, the production wrapper must clone or otherwise retain the adapter config so it can make the optional second request. Tests should use a fake async requester that records requests and returns canned provider events.

Add a small one-round orchestration helper that accepts:

- provider model/config;
- current `NativeSessionLog`;
- pending session events vector;
- current turn id;
- project resource root;
- provider requester.

It should build the first request from `native_provider_messages_from_log`, call the requester once, collect text/tool/completion events, and choose:

- no tool calls: preserve current one-shot behavior;
- one or more first-round tool calls: execute them through `build_project_readonly_provider_tool_results`, build `ProviderContinuationRequest`, build `ProviderContinuationSubmission`, project with `rig_adapter::project_provider_continuation_request`, call requester a second time, and use only the second response as the final assistant text;
- second-round tool calls: fail closed with no third request.

First-round text must be buffered, not immediately emitted to the UI, until the runner knows whether the first response contains tool calls. If there are no tool calls, buffered first-round text may be emitted and persisted as today's one-shot assistant response. If there are tool calls, first-round text is not emitted or persisted as assistant text; only the continuation response may produce UI deltas and the final assistant entry.

## Session Evidence

Tool execution already appends `ToolRequestRecorded` and `ToolExecutionFinished` to `NativeSessionLog`. The runner also persists a `pending_events` batch. The orchestration helper must ensure tool events added to `log` are also copied into `pending_events`, otherwise they will appear in memory but not in JSONL.

The final assistant `EntryAppended`, `native_prompt_total` metric, `TurnFinished`, and `PromptFinished` UI event should remain owned by the existing native runner finish path.

## Data And Policy Boundaries

Only `project_path_info` is allowed.

Use:

- `NativeToolRegistry::with_project_read_only_tools()`;
- `NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info")`;
- `NativeToolContinuationPolicy::fixture_default()` unless implementation finds an existing runtime policy;
- `NativeResourceRoot::project(std::env::current_dir())` for the runtime project root.

The provider-bound result remains metadata-only. It may include project-relative path, entry kind, byte size, and `provider_visibility:"never"`. It must not include file contents, absolute paths, command output, raw tool arguments, or raw provider payloads.

## Error Handling

The runner should fail closed and finish the turn as failed when:

- provider request fails;
- provider stream emits a failed event;
- provider stream is cancelled;
- first-round provider response ends without completion;
- first-round tool validation, permission, execution, result-size, or continuation mapping fails;
- continuation provider request fails;
- continuation provider stream emits a failed event;
- continuation provider stream is cancelled;
- continuation provider response emits any further `ToolCallCompleted` event;
- continuation provider response ends without completion.

Errors should be normalized into existing turn-failure surfaces and should not include raw local data. Stable reason strings are enough for tool-loop failures, for example `native_provider_tool_continuation_failed`.

## Runtime Exposure

This slice changes the explicit `--backend native-provider` path only. Pi remains the default backend.

Because provider tool advertising is not yet wired, real provider dogfooding may continue to behave as a one-shot chat path until a provider can be told about `project_path_info`. That is acceptable for this slice as long as unit tests prove the runner handles tool-call events and one continuation round correctly.

Do not advertise `tool_execution` in backend capabilities yet.

## Testing

Add backend tests with a fake async provider requester for:

- no-tool response preserves current one-shot behavior and makes one provider request;
- first response with `project_path_info` executes the read-only tool loop, persists tool session events into pending events, sends a second provider request with a `Tool` message, and returns final assistant text from the second response;
- first-round text preceding tool calls is not emitted or persisted when continuation happens;
- second-round tool call fails closed and no third request is made;
- unknown or malformed tool call fails before the second provider request;
- second provider failure maps to failed turn outcome.

Tests should not require network credentials and should not use real provider SDK calls.

## Acceptance Criteria

This slice is complete when backend tests prove that the native-provider runner can execute exactly one safe read-only `project_path_info` continuation round using yach-owned tool execution and Rig request projection, and the production native-provider path uses the same helper without changing defaults, capabilities, UI protocol, or provider tool advertising.

## Follow-Up

The next likely slice is provider tool advertising for `project_path_info` behind explicit native-provider opt-in:

- expose only the metadata tool schema to the selected provider;
- ensure provider adapters do not execute tools directly;
- keep yach-owned validation, policy, execution, and session records authoritative;
- keep capabilities conservative until user-visible semantics are stable.
