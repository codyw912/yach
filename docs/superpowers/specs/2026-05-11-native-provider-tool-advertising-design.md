# Native Provider Tool Advertising Design

Date: 2026-05-11
Status: accepted

## Context

The native backend can now execute a completed provider tool call for the safe read-only `project_path_info` tool, map the metadata-only result into a continuation provider request, and finish exactly one continuation round. The remaining gap is that the initial `--backend native-provider` request still does not advertise any tool schema, so real provider dogfooding usually behaves like one-shot chat unless a provider emits tool-call events without being told about available tools.

This slice should advertise only the metadata-safe `project_path_info` schema to providers behind the existing explicit native-provider opt-in. Execution must remain yach-owned: the provider may request a tool, but Rig or any other provider adapter must not execute it directly.

Rig has two relevant paths:

- `AgentBuilder::tool(...)` / `ToolSet` registers executable Rig tools and may auto-call them during prompt handling. That violates yach-owned execution authority and should not be used for `project_path_info`.
- `CompletionRequestBuilder::tools(...)` attaches `rig::completion::ToolDefinition` schemas to a provider request. This is the right shape for schema-only advertising, as long as stream events are still collected and handed back to `native_runner`.

## Goal

Advertise exactly one provider-callable schema, `project_path_info`, on explicit native-provider initial requests so models can request the existing yach-owned metadata tool. Keep execution, validation, permission checks, result shaping, session evidence, and continuation handling in yach.

## Non-Goals

- No new tools beyond `project_path_info`.
- No file contents, search results, absolute host paths, raw tool arguments, or command output sent to providers.
- No provider-native SDK tool-result block mapping.
- No Rig `Tool`, `ToolSet`, or adapter-owned execution path for project tools.
- No `yach-proto` changes, UI approval surface, or advertised backend capability changes.
- No default backend change.
- No file mutation, process execution, network tools, or extension runtime.
- No broad tool registry exposure to providers.
- No multi-round autonomous loop.

## Recommended Shape

Keep the common `ProviderRequest` shape unchanged, including `extensions: Vec<ProviderExtension>`, but stop treating provider extensions as arbitrary adapter-owned JSON at the advertising boundary. Add typed backend helpers around a yach-owned extension key, for example:

```text
yach.provider_tool_advertising.v1
```

The typed data should represent provider-advertised schemas, not executable tools:

```rust
pub struct ProviderAdvertisedToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct ProviderToolAdvertising {
    pub tools: Vec<ProviderAdvertisedToolSchema>,
}
```

Add helpers in `yach-backend`, most likely near the provider/tool seam:

- build an advertising extension for the explicitly allowed provider schemas;
- parse advertising from a `ProviderRequest`;
- project a `NativeToolDefinition` into a provider JSON schema;
- reject any explicitly requested advertised tool that is not exactly `project_path_info` with `NativeToolRisk::ReadsLocalMetadata`.

Advertising construction must fail closed if a caller explicitly asks to advertise an unsupported tool or risk class. Filtering or omission is acceptable only for a future path that starts from a larger candidate set after policy has already classified denied tools. This slice should use the fail-closed path so a registry or policy bug cannot silently downgrade the provider request to chat-only behavior.

The schema projector should be explicit rather than ad hoc freeform JSON. For the existing `NativeToolInputSchema::string_object(["path"], [], 1024)`, emit a JSON object schema with:

- `type: "object"`;
- `properties.path.type: "string"`;
- `properties.path.description: "Project-relative path to inspect."`;
- `required: ["path"]`;
- `additionalProperties: false`.

The schema may include provider-neutral descriptions, but must not include examples, a project root, file contents, previous arguments, current working directory values, usernames, discovered project paths, or local path examples that reveal machine state.

## Runtime Flow

Only the explicit native-provider runtime should attach the advertising extension. The native fixture backend and Pi backend remain unchanged.

In `native_runner.rs`, the first `ProviderRequest` built by `run_native_provider_one_readonly_tool_round` should include the typed advertising extension for `project_path_info`. Continuation requests should not advertise tools unless a future design explicitly needs that; this slice should avoid encouraging a second tool round. When building `ProviderContinuationRequest`, do not pass through the provider-tool-advertising extension from the initial request. For this slice, continuation request extensions must not contain `yach.provider_tool_advertising.v1`; other existing extension pass-through behavior may be preserved only if needed. The existing second-round `ToolCallCompleted` fail-closed behavior remains authoritative.

The runtime should still execute only through:

- `NativeToolRegistry::with_project_read_only_tools()`;
- `NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info")`;
- `build_project_readonly_provider_tool_results`;
- the existing continuation submission and Rig projection path.

## Rig Adapter Flow

`rig_adapter::run_provider_request` should parse the typed advertising extension and convert the advertised schemas into `rig::completion::ToolDefinition` values. It should then build the stream through Rig's completion request path with `.tools(...)`, not through `AgentBuilder::tool(...)` or `ToolSet`.

Add an adapter-local projection seam that can be unit-tested without network calls and compile-proves construction of a Rig completion request with `ToolDefinition` values. Do not introduce any `rig::Tool` implementation or `ToolSet` registration.

The existing prompt and preamble projection can remain provider-neutral:

- system messages become preamble/system context;
- non-system messages become the prompt/chat text;
- tool result messages in continuation requests stay ordinary transcript messages as currently projected.

The refactor should preserve current no-tool behavior. A request with no advertising extension should produce the same prompt, preamble, max token, timeout, and stream collection behavior as before.

If an advertising extension is malformed, contains unsupported tools, or cannot be projected into Rig definitions, the adapter should fail closed with a redacted `ProviderErrorKind::InvalidRequest`. Unknown extension keys may be ignored. Known malformed yach advertising data should not be silently ignored because that would make real dogfooding look like a chat-only success.

Parsing must fail closed when more than one `yach.provider_tool_advertising.v1` extension is present, when the known advertising extension has an empty tool list, or when duplicate advertised tool names are present. It must not use last-wins or merge semantics for duplicate yach advertising extensions.

## Data And Policy Boundaries

`project_path_info` is metadata-only. Advertising it tells the provider the name, description, and input shape. It does not grant execution authority.

The provider-visible schema must not contain:

- file contents;
- search result snippets;
- absolute paths;
- command output;
- previous raw tool arguments;
- local project root;
- extension/runtime metadata.

The provider may emit raw tool arguments in the response. Those remain untrusted provider input and must continue through the existing validation, permission, redaction, and session-evidence workflow before execution.

## Extension Ecosystem Considerations

This slice should not hard-code a design that prevents future extensions from registering tools, but it should also not build the extension runtime now. The safe boundary is a typed provider-advertising representation that can later be populated from extension-owned tool definitions after those definitions pass yach policy checks.

The future extension path should be:

1. extension registers a yach-owned tool definition;
2. yach classifies risk and policy;
3. only approved provider-visible schemas are projected into provider advertising;
4. provider requests still come back through yach validation and execution.

In this slice, the only producer of provider advertising is core yach code for `project_path_info`.

## Error Handling

Fail closed when:

- yach advertising extension data is malformed;
- more than one yach advertising extension is present;
- a known yach advertising extension has an empty tool list;
- advertised tool names are duplicated;
- a requested advertised tool cannot be projected;
- a requested advertised tool is not exactly `project_path_info`;
- the tool risk is not `ReadsLocalMetadata`;
- Rig rejects a completion request with tool schemas;
- the provider emits unsupported or malformed tool-call payloads.

Errors should use existing provider error surfaces with redacted debug labels. Do not include schema payloads if they could contain future local data. Stable labels like `provider_tool_advertising_malformed` or `provider_tool_advertising_unsupported_tool` are sufficient.

## Testing

Add backend tests for:

- provider advertising extension builder emits exactly one `project_path_info` schema;
- the schema has only a `path` string field, a project-relative `path` description, `required: ["path"]`, and `additionalProperties: false`;
- unsupported native tools and non-metadata risks are rejected before provider projection;
- initial native-provider requests include the advertising extension;
- continuation requests do not advertise tools and still fail closed on second-round tool calls;
- Rig adapter maps the advertising extension to exactly one schema-only Rig `ToolDefinition`;
- Rig request construction compiles with schema-only `ToolDefinition` values and no executable `rig::Tool` or `ToolSet` registration;
- Rig no-advertising requests preserve current prompt/preamble behavior;
- malformed yach advertising extension, duplicate advertising extensions, empty tool lists, and duplicate tool names fail closed with `InvalidRequest`;
- backend handshake/capabilities remain unchanged and do not advertise `tool_execution`.

Tests should not require network credentials or real provider calls. Use direct projection tests and fake provider requester tests for runner behavior.

## Acceptance Criteria

This slice is complete when backend tests prove that explicit native-provider initial requests advertise exactly the metadata-safe `project_path_info` schema, Rig receives schema-only tool definitions without registering executable Rig tools, yach-owned execution remains authoritative, continuation behavior remains one-round and fail-closed, and no UI/proto/capability/default-backend surface changes are introduced.

## Follow-Up

After this slice, the next likely work is either real-provider smoke validation for tool-call emission or the first extension-oriented tool registration design. Extension work should build on the typed advertising representation from this slice instead of bypassing yach-owned policy and execution.
