# Native Provider Tool Advertising Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Advertise the safe `project_path_info` schema on explicit native-provider initial requests while keeping tool execution and continuation control in yach.

**Architecture:** Add typed provider-tool-advertising helpers in `yach-backend` around the existing `ProviderRequest.extensions` seam. Wire only the native-provider runner to attach the advertising extension on the first request and strip it from continuation requests. Refactor the Rig adapter to project the typed extension into schema-only `rig::completion::ToolDefinition` values and collect completion-stream tool calls without registering executable Rig tools.

**Tech Stack:** Rust 2024, `yach-backend`, `serde_json`, Rig `0.36.0`, `just dev cargo test`.

---

## File Structure

- Modify `crates/yach-backend/src/tools.rs`: typed advertising structs, extension key, builder/parser/schema projection, advertising-strip helper.
- Modify `crates/yach-backend/src/native_runner.rs`: initial native-provider request extension wiring and continuation advertising stripping.
- Modify `crates/yach-backend/src/rig_adapter.rs`: parse advertising, convert to Rig `ToolDefinition`, route provider requests through low-level streaming completion, and make stream collection policy-aware.
- Modify `crates/yach-backend/src/lib.rs`: crate-level tests for tool advertising helper behavior and public re-exports already provided by `pub use tools::*`.
- Modify `docs/project/state.md` and `docs/project/next.md`: record the completed slice and next recommended step after implementation.

## Task 1: Provider Advertising Helpers

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing tests for builder, schema, parser, and rejection behavior**

Add tests in `crates/yach-backend/src/lib.rs` near the existing provider continuation tests:

```rust
#[test]
fn provider_tool_advertising_builder_emits_project_path_info_schema() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::project_path_info(),
    ])
    .expect("project_path_info should be advertisable");

    assert_eq!(extension.key, PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY);
    let advertising = parse_provider_tool_advertising_extensions(&[extension])
        .expect("advertising extension should parse")
        .expect("advertising should be present");
    assert_eq!(advertising.tools.len(), 1);
    let tool = &advertising.tools[0];
    assert_eq!(tool.name, "project_path_info");
    assert_eq!(
        tool.description,
        "Return local-only project path metadata without reading file contents."
    );
    assert_eq!(tool.parameters["type"], "object");
    assert_eq!(tool.parameters["required"], serde_json::json!(["path"]));
    assert_eq!(tool.parameters["additionalProperties"], false);
    assert_eq!(tool.parameters["properties"]["path"]["type"], "string");
    assert_eq!(
        tool.parameters["properties"]["path"]["description"],
        "Project-relative path to inspect."
    );
    assert_eq!(
        tool.parameters["properties"]
            .as_object()
            .map(serde_json::Map::len),
        Some(1)
    );
}

#[test]
fn provider_tool_advertising_rejects_unsupported_tools_and_risks() {
    let fixture_tool = NativeToolDefinition::fixture_echo_metadata();
    let content_tool = NativeToolDefinition {
        name: String::from("project_path_info"),
        description: String::from("wrong risk"),
        input_schema: NativeToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 1024),
        risk: NativeToolRisk::ReadsLocalContent,
    };
    let wrong_limit_tool = NativeToolDefinition {
        name: String::from("project_path_info"),
        description: String::from(
            "Return local-only project path metadata without reading file contents.",
        ),
        input_schema: NativeToolInputSchema::string_object(
            ["path"],
            std::iter::empty::<&str>(),
            2048,
        ),
        risk: NativeToolRisk::ReadsLocalMetadata,
    };

    assert_eq!(
        build_provider_tool_advertising_extension(&[fixture_tool]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedTool {
            name: String::from("fixture_echo_metadata"),
        })
    );
    assert_eq!(
        build_provider_tool_advertising_extension(&[content_tool]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedRisk {
            name: String::from("project_path_info"),
            risk: NativeToolRisk::ReadsLocalContent,
        })
    );
    assert_eq!(
        build_provider_tool_advertising_extension(&[wrong_limit_tool]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedSchema {
            name: String::from("project_path_info"),
        })
    );
}

#[test]
fn provider_tool_advertising_parser_fails_closed_for_malformed_known_data() {
    let malformed = ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({"tools":"not-an-array"}),
    };
    let empty = ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({"tools":[]}),
    };
    let duplicate_names = ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({
            "tools": [
                {"name":"project_path_info","description":"one","parameters":{"type":"object"}},
                {"name":"project_path_info","description":"two","parameters":{"type":"object"}}
            ]
        }),
    };
    let unsupported_name = ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({
            "tools": [
                {"name":"read","description":"read files","parameters":{"type":"object"}}
            ]
        }),
    };
    let unsupported_schema = ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({
            "tools": [
                {
                    "name":"project_path_info",
                    "description":"Return local-only project path metadata without reading file contents.",
                    "parameters":{
                        "type":"object",
                        "properties":{"path":{"type":"string"}},
                        "required":["path"],
                        "additionalProperties":true
                    }
                }
            ]
        }),
    };
    let valid = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::project_path_info(),
    ])
    .expect("fixture should build");

    assert_eq!(
        parse_provider_tool_advertising_extensions(&[malformed]).err(),
        Some(ProviderToolAdvertisingError::Malformed)
    );
    assert_eq!(
        parse_provider_tool_advertising_extensions(&[empty]).err(),
        Some(ProviderToolAdvertisingError::EmptyTools)
    );
    assert_eq!(
        parse_provider_tool_advertising_extensions(&[duplicate_names]).err(),
        Some(ProviderToolAdvertisingError::DuplicateToolName {
            name: String::from("project_path_info"),
        })
    );
    assert_eq!(
        parse_provider_tool_advertising_extensions(&[valid.clone(), valid]).err(),
        Some(ProviderToolAdvertisingError::DuplicateExtension)
    );
    assert_eq!(
        parse_provider_tool_advertising_extensions(&[unsupported_name]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedTool {
            name: String::from("read"),
        })
    );
    assert_eq!(
        parse_provider_tool_advertising_extensions(&[unsupported_schema]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedSchema {
            name: String::from("project_path_info"),
        })
    );
}

#[test]
fn provider_tool_advertising_parser_ignores_unrelated_extensions() {
    let unrelated = ProviderExtension {
        key: String::from("adapter.example"),
        value: serde_json::json!({"ok":true}),
    };

    assert_eq!(
        parse_provider_tool_advertising_extensions(&[unrelated]),
        Ok(None)
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
```

Expected: compile/test failure because the advertising types and helpers are not implemented.

- [ ] **Step 3: Implement typed advertising helpers**

In `crates/yach-backend/src/tools.rs`, add near the provider continuation structs:

```rust
pub const PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY: &str = "yach.provider_tool_advertising.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAdvertisedToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolAdvertising {
    pub tools: Vec<ProviderAdvertisedToolSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderToolAdvertisingError {
    Malformed,
    EmptyTools,
    DuplicateExtension,
    DuplicateToolName { name: String },
    UnsupportedTool { name: String },
    UnsupportedRisk { name: String, risk: NativeToolRisk },
    UnsupportedSchema { name: String },
}

pub fn build_provider_tool_advertising_extension(
    tools: &[NativeToolDefinition],
) -> Result<ProviderExtension, ProviderToolAdvertisingError> {
    let advertising = ProviderToolAdvertising {
        tools: tools
            .iter()
            .map(provider_advertised_schema_from_native_tool)
            .collect::<Result<Vec<_>, _>>()?,
    };
    validate_provider_tool_advertising(&advertising)?;
    let value = serde_json::to_value(advertising)
        .map_err(|_| ProviderToolAdvertisingError::Malformed)?;
    Ok(ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value,
    })
}

pub fn build_project_path_info_provider_tool_advertising_extension(
) -> Result<ProviderExtension, ProviderToolAdvertisingError> {
    build_provider_tool_advertising_extension(&[NativeToolDefinition::project_path_info()])
}

pub fn parse_provider_tool_advertising_extensions(
    extensions: &[ProviderExtension],
) -> Result<Option<ProviderToolAdvertising>, ProviderToolAdvertisingError> {
    let mut matching = extensions
        .iter()
        .filter(|extension| extension.key == PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY);
    let Some(extension) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(ProviderToolAdvertisingError::DuplicateExtension);
    }
    let advertising = serde_json::from_value::<ProviderToolAdvertising>(extension.value.clone())
        .map_err(|_| ProviderToolAdvertisingError::Malformed)?;
    validate_provider_tool_advertising(&advertising)?;
    Ok(Some(advertising))
}

pub fn strip_provider_tool_advertising_extensions(
    extensions: Vec<ProviderExtension>,
) -> Vec<ProviderExtension> {
    extensions
        .into_iter()
        .filter(|extension| extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        .collect()
}

fn validate_provider_tool_advertising(
    advertising: &ProviderToolAdvertising,
) -> Result<(), ProviderToolAdvertisingError> {
    if advertising.tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }
    let mut names = BTreeSet::new();
    for tool in &advertising.tools {
        if !names.insert(tool.name.clone()) {
            return Err(ProviderToolAdvertisingError::DuplicateToolName {
                name: tool.name.clone(),
            });
        }
        validate_provider_advertised_tool_schema(tool)?;
    }
    Ok(())
}

fn validate_provider_advertised_tool_schema(
    tool: &ProviderAdvertisedToolSchema,
) -> Result<(), ProviderToolAdvertisingError> {
    if tool.name != "project_path_info" {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }
    let expected = provider_advertised_schema_from_native_tool(
        &NativeToolDefinition::project_path_info(),
    )?;
    if tool.description != expected.description || tool.parameters != expected.parameters {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }
    Ok(())
}

fn provider_advertised_schema_from_native_tool(
    tool: &NativeToolDefinition,
) -> Result<ProviderAdvertisedToolSchema, ProviderToolAdvertisingError> {
    if tool.name != "project_path_info" {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }
    if tool.risk != NativeToolRisk::ReadsLocalMetadata {
        return Err(ProviderToolAdvertisingError::UnsupportedRisk {
            name: tool.name.clone(),
            risk: tool.risk,
        });
    }
    if tool.input_schema.required_string_fields != BTreeSet::from([String::from("path")])
        || !tool.input_schema.optional_string_fields.is_empty()
        || tool.input_schema.max_serialized_bytes != 1024
    {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }
    Ok(ProviderAdvertisedToolSchema {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Project-relative path to inspect."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    })
}
```

- [ ] **Step 4: Run targeted tests to verify they pass**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
```

Expected: all `provider_tool_advertising_*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: add provider tool advertising helpers"
```

## Task 2: Native Runner Advertising Wiring

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add failing runner assertions for initial advertising and continuation stripping**

In `crates/yach-backend/src/native_runner.rs`, update the test imports to include:

```rust
send_native_initial_state,
PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY,
parse_provider_tool_advertising_extensions,
```

Also add:

```rust
use yach_proto::{BackendEvent, Capability, ServerEvent};
```

In `native_provider_one_round_without_tools_preserves_one_shot_response`, after `assert_eq!(requester.requests.len(), 1);`, add:

```rust
let advertising = parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
    .expect("initial advertising should parse");
assert!(advertising.is_some());
assert_eq!(
    advertising.and_then(|advertising| advertising.tools.into_iter().next()).map(|tool| tool.name),
    Some(String::from("project_path_info"))
);
```

In `native_provider_one_round_executes_project_path_info_and_continues`, after `assert_eq!(requester.requests.len(), 2);`, add:

```rust
assert!(
    parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        .expect("initial advertising should parse")
        .is_some()
);
assert!(
    requester.requests[1]
        .extensions
        .iter()
        .all(|extension| extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
);
assert_eq!(
    parse_provider_tool_advertising_extensions(&requester.requests[1].extensions),
    Ok(None)
);
```

In `native_provider_one_round_rejects_second_round_tool_calls`, after `assert_eq!(requester.requests.len(), 2);`, add the same continuation-stripping assertions.

Add this regression test in the same native runner test module to prove the protocol handshake did not grow a tool capability:

```rust
#[test]
fn native_initial_state_handshake_remains_streaming_and_cancellation_only() {
    let root_guard = temp_native_provider_root("native-provider-handshake-capabilities");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    send_native_initial_state(&tx, root_guard.path(), None);

    let ready = rx.try_recv().ok();
    assert!(matches!(
        ready,
        Some(BackendEvent::Server(ServerEvent::Ready { handshake }))
            if handshake.capabilities
                == vec![Capability::PromptStreaming, Capability::PromptCancellation]
    ));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
```

Expected: tests fail because initial requests still have empty extensions.

- [ ] **Step 3: Attach advertising on first request and strip it for continuation**

In `run_native_provider_one_readonly_tool_round`, replace `extensions: Vec::new(),` in the initial request with:

```rust
extensions: vec![
    crate::build_project_path_info_provider_tool_advertising_extension()
        .map_err(|error| NativeProviderRoundError::ToolContinuation(
            native_provider_tool_advertising_error_label(&error),
        ))?,
],
```

Replace continuation extension pass-through:

```rust
extensions: initial_request.extensions,
```

with:

```rust
extensions: crate::strip_provider_tool_advertising_extensions(initial_request.extensions),
```

Add a redacted label helper near `native_provider_mapping_error_label`:

```rust
fn native_provider_tool_advertising_error_label(
    error: &ProviderToolAdvertisingError,
) -> String {
    match error {
        ProviderToolAdvertisingError::Malformed => String::from("provider_tool_advertising_malformed"),
        ProviderToolAdvertisingError::EmptyTools => String::from("provider_tool_advertising_empty"),
        ProviderToolAdvertisingError::DuplicateExtension => {
            String::from("provider_tool_advertising_duplicate_extension")
        }
        ProviderToolAdvertisingError::DuplicateToolName { .. } => {
            String::from("provider_tool_advertising_duplicate_tool")
        }
        ProviderToolAdvertisingError::UnsupportedTool { .. } => {
            String::from("provider_tool_advertising_unsupported_tool")
        }
        ProviderToolAdvertisingError::UnsupportedRisk { .. } => {
            String::from("provider_tool_advertising_unsupported_risk")
        }
        ProviderToolAdvertisingError::UnsupportedSchema { .. } => {
            String::from("provider_tool_advertising_unsupported_schema")
        }
    }
}
```

Import `ProviderToolAdvertisingError` at the top of `native_runner.rs`.

- [ ] **Step 4: Run targeted runner tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
```

Expected: all native provider one-round tests pass, including second-round fail-closed behavior.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: advertise project path tool in native provider requests"
```

## Task 3: Rig Adapter Schema-Only ToolDefinitions

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing Rig adapter tests for projection and stream policy**

In `crates/yach-backend/src/rig_adapter.rs` test module, expand imports:

```rust
use super::{
    apply_rig_tool_definitions, collect_rig_stream_item, preamble_from_request, prompt_from_request,
    rig_tool_definitions_from_request, RigToolCallCollection, RigToolCallPolicy,
};
use rig::client::CompletionClient;
use rig::completion::{CompletionModel, ToolDefinition};
use rig::completion::message::{ToolCall, ToolFunction};
use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};
```

Add tests:

```rust
#[test]
fn rig_adapter_projects_advertising_to_schema_only_tool_definition() {
    let mut request = provider_request(vec![ProviderMessage {
        role: NativeRole::User,
        content: String::from("inspect cargo"),
    }]);
    request.extensions = vec![
        crate::build_project_path_info_provider_tool_advertising_extension()
            .expect("advertising extension should build"),
    ];

    let tools = rig_tool_definitions_from_request(&request)
        .expect("advertising should project to rig tools");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "project_path_info");
    assert_eq!(tools[0].parameters["properties"]["path"]["type"], "string");
}

#[test]
fn rig_adapter_rejects_malformed_known_advertising_extension() {
    let mut request = provider_request(vec![ProviderMessage {
        role: NativeRole::User,
        content: String::from("inspect cargo"),
    }]);
    request.extensions = vec![ProviderExtension {
        key: String::from(crate::PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({"tools":[]}),
    }];

    let error = rig_tool_definitions_from_request(&request).err();

    assert_eq!(
        error.as_ref().map(|error| error.kind),
        Some(ProviderErrorKind::InvalidRequest)
    );
}

#[test]
fn rig_adapter_rejects_unsupported_advertised_tool_projection() {
    let mut request = provider_request(vec![ProviderMessage {
        role: NativeRole::User,
        content: String::from("inspect cargo"),
    }]);
    request.extensions = vec![ProviderExtension {
        key: String::from(crate::PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value: serde_json::json!({
            "tools": [
                {"name":"read","description":"read files","parameters":{"type":"object"}}
            ]
        }),
    }];

    let error = rig_tool_definitions_from_request(&request).err();

    assert_eq!(
        error.as_ref().map(|error| error.kind),
        Some(ProviderErrorKind::InvalidRequest)
    );
}

#[test]
fn rig_adapter_applies_schema_tools_to_completion_request_builder_without_network() {
    let client = rig::providers::anthropic::Client::builder()
        .api_key("test-key")
        .build()
        .expect("anthropic client should build without network");
    let model = client.completion_model("claude-test");
    let request = apply_rig_tool_definitions(
        model.completion_request("inspect cargo"),
        vec![ToolDefinition {
            name: String::from("project_path_info"),
            description: String::from(
                "Return local-only project path metadata without reading file contents.",
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Project-relative path to inspect."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        }],
    )
    .build();

    assert_eq!(request.tools.len(), 1);
    assert_eq!(request.tools[0].name, "project_path_info");
}

#[test]
fn rig_adapter_no_advertising_preserves_prompt_preamble_and_omits_tools() {
    let request = provider_request(vec![
        ProviderMessage {
            role: NativeRole::System,
            content: String::from("system guidance"),
        },
        ProviderMessage {
            role: NativeRole::User,
            content: String::from("visible prompt"),
        },
    ]);
    let prompt = prompt_from_request(&request).expect("prompt should project");
    let preamble = preamble_from_request(&request);
    let tools = rig_tool_definitions_from_request(&request)
        .expect("request without advertising should still parse");

    assert_eq!(prompt, "User:\nvisible prompt");
    assert_eq!(preamble, "system guidance");
    assert!(tools.is_empty());

    let client = rig::providers::anthropic::Client::builder()
        .api_key("test-key")
        .build()
        .expect("anthropic client should build without network");
    let model = client.completion_model("claude-test");
    let completion = apply_rig_tool_definitions(
        model
            .completion_request(prompt.clone())
            .preamble(preamble.clone())
            .max_tokens(64),
        tools,
    )
    .build();
    let serialized = serde_json::to_value(&completion)
        .expect("completion request should serialize for inspection");

    assert!(completion.tools.is_empty());
    assert_eq!(completion.max_tokens, Some(64));
    assert!(serialized.to_string().contains("system guidance"));
    assert!(serialized.to_string().contains("visible prompt"));
}

#[test]
fn rig_adapter_collects_advertised_tool_call_without_failure() {
    let mut collection = RigToolCallCollection::new(
        NativeTurnId(String::from("turn-1")),
        String::from("fixture-provider"),
        String::from("fixture-model"),
        RigToolCallPolicy::Advertised,
    );
    let events = collect_rig_stream_item(
        &mut collection,
        StreamedAssistantContent::<()>::ToolCall {
            internal_call_id: String::from("internal-1"),
            tool_call: ToolCall::new(
                String::from("provider-call-1"),
                ToolFunction::new(
                    String::from("project_path_info"),
                    serde_json::json!({"path":"Cargo.toml"}),
                ),
            )
            .with_call_id(String::from("call-1")),
        },
    );

    assert!(matches!(
        events.first(),
        Some(ProviderStreamEvent::ToolCallCompleted { tool_call, .. })
            if tool_call.call_id == "call-1"
                && tool_call.name == "project_path_info"
                && tool_call.arguments_json == serde_json::json!({"path":"Cargo.toml"})
    ));
    assert!(!events.iter().any(|event| matches!(event, ProviderStreamEvent::Failed { .. })));
    assert!(collection.saw_tool_call());
}

#[test]
fn rig_adapter_fails_unadvertised_tool_call() {
    let mut collection = RigToolCallCollection::new(
        NativeTurnId(String::from("turn-1")),
        String::from("fixture-provider"),
        String::from("fixture-model"),
        RigToolCallPolicy::Unexpected,
    );
    let events = collect_rig_stream_item(
        &mut collection,
        StreamedAssistantContent::<()>::ToolCall {
            internal_call_id: String::from("internal-1"),
            tool_call: ToolCall::new(
                String::from("provider-call-1"),
                ToolFunction::new(
                    String::from("project_path_info"),
                    serde_json::json!({"path":"Cargo.toml"}),
                ),
            ),
        },
    );

    assert!(events.iter().any(|event| matches!(
        event,
        ProviderStreamEvent::Failed { error, .. }
            if error.kind == ProviderErrorKind::InvalidRequest
    )));
}

#[test]
fn rig_adapter_finish_reason_tracks_advertised_tool_calls() {
    let mut collection = RigToolCallCollection::new(
        NativeTurnId(String::from("turn-1")),
        String::from("fixture-provider"),
        String::from("fixture-model"),
        RigToolCallPolicy::Advertised,
    );
    collection.record_tool_call();

    let completed = collection.completed_event(None);

    assert!(matches!(
        completed,
        ProviderStreamEvent::Completed {
            finish_reason: Some(crate::ProviderFinishReason::ToolCalls),
            ..
        }
    ));
}
```

In `crates/yach-backend/src/lib.rs`, add one crate-level compile-behavior test near the existing Rig adapter tests:

```rust
#[test]
fn rig_adapter_schema_tool_definition_is_not_executable_rig_tool() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::project_path_info(),
    ])
    .expect("extension should build");
    let request = ProviderRequest {
        turn_id: NativeTurnId(String::from("turn-1")),
        model: ProviderModel {
            provider: String::from("fixture-provider"),
            model: String::from("fixture-model"),
        },
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: String::from("inspect cargo"),
        }],
        extensions: vec![extension],
    };

    let tools = rig_adapter::rig_tool_definitions_from_request(&request)
        .expect("tool definitions should project");
    let tool = tools.first();

    assert_eq!(tools.len(), 1);
    assert_eq!(tool.map(|tool| tool.name.as_str()), Some("project_path_info"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend rig_adapter -- --nocapture
```

Expected: compile/test failure because adapter projection and collection seams do not exist.

- [ ] **Step 3: Implement adapter projection and error mapping**

In `crates/yach-backend/src/rig_adapter.rs`, change imports:

```rust
use rig::completion::{CompletionError, GetTokenUsage, Message, ToolDefinition};
use rig::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamedAssistantContent, StreamingCompletion,
    StreamingCompletionResponse, StreamingPrompt, ToolCallDeltaContent,
};
```

Add:

```rust
pub fn rig_tool_definitions_from_request(
    request: &ProviderRequest,
) -> Result<Vec<ToolDefinition>, ProviderError> {
    let Some(advertising) = crate::parse_provider_tool_advertising_extensions(&request.extensions)
        .map_err(provider_tool_advertising_error)?
    else {
        return Ok(Vec::new());
    };
    Ok(advertising
        .tools
        .into_iter()
        .map(|tool| ToolDefinition {
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        })
        .collect())
}

pub(crate) fn apply_rig_tool_definitions<M: rig::completion::CompletionModel>(
    builder: rig::completion::CompletionRequestBuilder<M>,
    tools: Vec<ToolDefinition>,
) -> rig::completion::CompletionRequestBuilder<M> {
    if tools.is_empty() {
        builder
    } else {
        builder.tools(tools)
    }
}

fn provider_tool_advertising_error(
    error: crate::ProviderToolAdvertisingError,
) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::InvalidRequest,
        message: String::from("Rig provider tool advertising is invalid"),
        redacted_debug: Some(match error {
            crate::ProviderToolAdvertisingError::Malformed => {
                String::from("provider_tool_advertising_malformed")
            }
            crate::ProviderToolAdvertisingError::EmptyTools => {
                String::from("provider_tool_advertising_empty")
            }
            crate::ProviderToolAdvertisingError::DuplicateExtension => {
                String::from("provider_tool_advertising_duplicate_extension")
            }
            crate::ProviderToolAdvertisingError::DuplicateToolName { .. } => {
                String::from("provider_tool_advertising_duplicate_tool")
            }
            crate::ProviderToolAdvertisingError::UnsupportedTool { .. } => {
                String::from("provider_tool_advertising_unsupported_tool")
            }
            crate::ProviderToolAdvertisingError::UnsupportedRisk { .. } => {
                String::from("provider_tool_advertising_unsupported_risk")
            }
            crate::ProviderToolAdvertisingError::UnsupportedSchema { .. } => {
                String::from("provider_tool_advertising_unsupported_schema")
            }
        }),
    }
}

fn map_completion_error(error: &CompletionError) -> ProviderError {
    let debug = error_chain(error);
    ProviderError {
        kind: classify_provider_error_debug(&debug),
        message: String::from("Rig provider call failed"),
        redacted_debug: Some(redact_secrets(&debug)),
    }
}
```

- [ ] **Step 4: Implement policy-aware completion-stream collection**

In `crates/yach-backend/src/rig_adapter.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RigToolCallPolicy {
    Advertised,
    Unexpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RigToolCallCollection {
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    policy: RigToolCallPolicy,
    text: String,
    saw_tool_call: bool,
}

impl RigToolCallCollection {
    pub(crate) fn new(
        turn_id: NativeTurnId,
        provider_label: String,
        model: String,
        policy: RigToolCallPolicy,
    ) -> Self {
        Self {
            turn_id,
            provider_label,
            model,
            policy,
            text: String::new(),
            saw_tool_call: false,
        }
    }

    pub(crate) fn saw_tool_call(&self) -> bool {
        self.saw_tool_call
    }

    pub(crate) fn record_tool_call(&mut self) {
        self.saw_tool_call = true;
    }

    pub(crate) fn completed_event(
        &self,
        provider_response_id: Option<String>,
    ) -> ProviderStreamEvent {
        ProviderStreamEvent::Completed {
            turn_id: self.turn_id.clone(),
            finish_reason: Some(if self.saw_tool_call {
                ProviderFinishReason::ToolCalls
            } else {
                ProviderFinishReason::Stop
            }),
            usage: None,
            provider_response_id,
        }
    }
}

pub(crate) fn collect_rig_stream_item<R: Clone + Unpin>(
    collection: &mut RigToolCallCollection,
    item: StreamedAssistantContent<R>,
) -> Vec<ProviderStreamEvent> {
    match item {
        StreamedAssistantContent::Text(delta) => {
            collection.text.push_str(&delta.text);
            vec![ProviderStreamEvent::TextDelta {
                turn_id: collection.turn_id.clone(),
                delta: delta.text,
            }]
        }
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id,
        } => {
            if collection.policy == RigToolCallPolicy::Unexpected {
                return vec![unexpected_rig_tool_call_failure(
                    &collection.turn_id,
                    internal_call_id,
                )];
            }
            collection.record_tool_call();
            vec![ProviderStreamEvent::ToolCallCompleted {
                turn_id: collection.turn_id.clone(),
                tool_call: ProviderToolCall {
                    call_id: tool_call.call_id.unwrap_or(tool_call.id),
                    name: tool_call.function.name,
                    arguments_json: tool_call.function.arguments,
                },
            }]
        }
        StreamedAssistantContent::ToolCallDelta {
            id,
            internal_call_id,
            content,
        } => {
            if collection.policy == RigToolCallPolicy::Unexpected {
                return vec![unexpected_rig_tool_call_failure(
                    &collection.turn_id,
                    internal_call_id,
                )];
            }
            collection.record_tool_call();
            vec![map_tool_call_delta(
                &collection.turn_id,
                id,
                internal_call_id,
                content,
            )]
        }
        StreamedAssistantContent::Final(_) => {
            vec![collection.completed_event(None)]
        }
        StreamedAssistantContent::Reasoning(_) | StreamedAssistantContent::ReasoningDelta { .. } => {
            Vec::new()
        }
    }
}

fn unexpected_rig_tool_call_failure(
    turn_id: &NativeTurnId,
    internal_call_id: String,
) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id: turn_id.clone(),
        error: ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider received an unexpected tool call"),
            redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
        },
    }
}

async fn collect_rig_completion_stream<R>(
    mut stream: StreamingCompletionResponse<R>,
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    timeout: Duration,
    policy: RigToolCallPolicy,
) -> Result<Vec<ProviderStreamEvent>, ProviderError>
where
    R: Clone + Unpin + GetTokenUsage,
{
    let mut collection = RigToolCallCollection::new(
        turn_id.clone(),
        provider_label.clone(),
        model.clone(),
        policy,
    );
    let mut events = vec![ProviderStreamEvent::Started {
        turn_id,
        model: crate::ProviderModel {
            provider: provider_label,
            model,
        },
    }];

    loop {
        let next = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| ProviderError {
                kind: ProviderErrorKind::Timeout,
                message: String::from("Rig provider stream timed out"),
                redacted_debug: Some(String::from("timeout while awaiting next stream event")),
            })?;
        let Some(item) = next else {
            break;
        };
        let item = item.map_err(|error| map_completion_error(&error))?;
        events.extend(collect_rig_stream_item(&mut collection, item));
        if events.last().is_some_and(|event| matches!(event, ProviderStreamEvent::Failed { .. })) {
            break;
        }
    }

    Ok(events)
}
```

- [ ] **Step 5: Route provider requests through schema-only completion streaming**

In `run_provider_request`, compute prompt and tools before the provider match:

```rust
let prompt = prompt_from_request(&request)?;
let rig_tools = rig_tool_definitions_from_request(&request)?;
let tool_policy = if rig_tools.is_empty() {
    RigToolCallPolicy::Unexpected
} else {
    RigToolCallPolicy::Advertised
};
```

For each provider branch, replace the current `agent.stream_prompt(prompt).await` path with:

```rust
let preamble = preamble_from_request(&request);
let agent = client
    .agent(request.model.model.clone())
    .preamble(&preamble)
    .max_tokens(config.max_tokens)
    .build();
let mut builder = agent
    .stream_completion(prompt, std::iter::empty::<Message>())
    .await
    .map_err(|error| map_completion_error(&error))?;
builder = apply_rig_tool_definitions(builder, rig_tools);
let stream = builder
    .stream()
    .await
    .map_err(|error| map_completion_error(&error))?;
collect_rig_completion_stream(
    stream,
    request.turn_id,
    request.model.provider,
    request.model.model,
    config.timeout,
    tool_policy,
)
.await
```

Because `rig_tools` is moved into the first matched provider branch, no clone should be needed. If the compiler requires reuse across match arms, derive the tools inside each arm from `&request` instead.

Leave smoke functions on `agent.stream_prompt(...)` and the existing multi-turn smoke collector. The no-advertising provider-request behavior remains fail-closed for unexpected tool calls via `RigToolCallPolicy::Unexpected`.

- [ ] **Step 6: Run targeted Rig adapter tests**

Run:

```bash
just dev cargo test -p yach-backend rig_adapter -- --nocapture
```

Expected: Rig adapter tests pass without network calls.

- [ ] **Step 7: Commit**

```bash
git add crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "feat: map advertised tools to rig schemas"
```

## Task 4: Project Docs And Full Verification

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run full backend verification**

Run:

```bash
just dev cargo test -p yach-backend
```

Expected: all `yach-backend` tests pass.

- [ ] **Step 2: Run workspace tests**

Run:

```bash
just dev cargo test --workspace
```

Expected: all workspace tests pass.

- [ ] **Step 3: Update project state**

In `docs/project/state.md`, add an entry stating that native provider tool advertising is implemented:

```markdown
- Native-provider initial requests now advertise the schema-only `project_path_info` tool through `yach.provider_tool_advertising.v1`; continuation requests strip that advertising so the one-round/fail-closed boundary remains intact.
```

Do not mark UI/proto/capabilities as changed.

- [ ] **Step 4: Update next slice**

In `docs/project/next.md`, replace the native-provider tool-advertising recommendation with the next likely slice:

```markdown
Recommended next move: run real-provider smoke validation for native-provider tool-call emission, or begin the extension-owned tool registration design that can populate the typed provider-advertising representation after yach policy approval.
```

- [ ] **Step 5: Verify docs diff is scoped**

Run:

```bash
git diff -- docs/project/state.md docs/project/next.md
```

Expected: only project planning text changes for this completed slice and next recommendation.

- [ ] **Step 6: Commit docs**

```bash
git add docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "docs: update native provider advertising project state"
```

## Final Verification

- [ ] Run:

```bash
just dev cargo test --workspace
```

- [ ] Run:

```bash
git status --short --branch
```

- [ ] Confirm `git status` is clean except for expected branch-ahead state.
