# Extension Tool Registration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first extension-owned tool registration path, proving a `toy_tool` extension can register a safe read-only metadata tool through yach-owned registry, policy, executor routing, result shaping, and provider advertising without slowing default TUI startup. Full process-backed `tool.invoke` execution remains a follow-up slice after this routing seam exists.

**Architecture:** Add a manifest-first extension module in `yach-backend`, then extend native tool definitions with owner and provider visibility so built-ins and extension tools share one catalog. Keep provider adapters schema-only; route extension execution through yach-owned executor boundaries and only advertise accepted, policy-approved, executable tools for future provider turns.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `tokio` where existing runtime code needs it, `yach-backend`, `yach-bench`, `just test`.

---

## File Structure

- Create `crates/yach-backend/src/extension.rs`: extension manifest parsing, activation metadata, conservative tool contribution schema, host protocol message structs, registration validation helpers, fake/test host helpers.
- Modify `crates/yach-backend/src/lib.rs`: add `mod extension; pub use extension::*;`, plus focused backend tests near existing native tool tests when the test needs multiple backend modules.
- Modify `crates/yach-backend/src/tools.rs`: add tool owner/provider visibility fields, extension registration API, catalog helpers, provider-advertisable projection for accepted extension tools, and executor routing seams.
- Modify `crates/yach-backend/src/native_runner.rs`: generalize the native-provider tool round to accept a tool catalog/router and advertise accepted provider-visible tools only on initial provider requests.
- Modify `crates/yach-backend/src/rig_adapter.rs`: keep schema-only projection; tests should prove extension tool schemas are still `ToolDefinition` only and never `rig::Tool`/`ToolSet`.
- Modify `crates/yach-cli/src/main.rs`: only if needed for deferred extension discovery wiring; do not spawn extension hosts before first render.
- Modify `crates/yach-bench/src/main.rs` and `docs/benchmarks/`: only in the startup/perf task, to measure installed inactive extensions.
- Modify `docs/project/state.md` and `docs/project/next.md`: update active status after implementation slices land.

## Task 1: Accept Design And Add Manifest Catalog

**Files:**
- Modify: `docs/project/specs/2026-05-12-extension-tool-registration-design.md`
- Create: `crates/yach-backend/src/extension.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Mark the design accepted**

Change the design header:

```markdown
Date: 2026-05-12
Status: accepted
```

- [ ] **Step 2: Write failing manifest parser tests**

Create `crates/yach-backend/src/extension.rs` with this initial test module and enough type references to drive the implementation:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn toy_tool_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.toy-tools",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "activation": {
                "events": ["onCommand:yach.extensions.activate.example.toy-tools"]
            },
            "contributes": {
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": false
                }]
            }
        })
    }

    #[test]
    fn extension_manifest_parses_toy_tool_without_executing_code() {
        let manifest = parse_extension_manifest(toy_tool_manifest_json());

        assert_eq!(
            manifest,
            Ok(ExtensionManifest {
                schema: ExtensionManifestSchema::V1,
                id: ExtensionId(String::from("example.toy-tools")),
                version: String::from("0.1.0"),
                main: ExtensionMain {
                    command: String::from("node"),
                    args: vec![String::from("./extension.js")],
                },
                activation: ExtensionActivation {
                    events: vec![ExtensionActivationEvent::Command(String::from(
                        "yach.extensions.activate.example.toy-tools"
                    ))],
                },
                contributes: ExtensionContributions {
                    tools: vec![ExtensionToolContribution {
                        name: String::from("toy_tool"),
                        description: String::from("Return static fixture metadata."),
                        risk: ExtensionToolRisk::ReadsLocalMetadata,
                        provider_visible: false,
                    }],
                },
            })
        );
    }

    #[test]
    fn extension_manifest_rejects_malformed_identity_and_tool_names() {
        let mut invalid_id = toy_tool_manifest_json();
        invalid_id["id"] = serde_json::json!("bad id with spaces");
        assert_eq!(
            parse_extension_manifest(invalid_id),
            Err(ExtensionManifestError::InvalidExtensionId)
        );

        let mut invalid_tool = toy_tool_manifest_json();
        invalid_tool["contributes"]["tools"][0]["name"] = serde_json::json!("project_path_info");
        assert_eq!(
            parse_extension_manifest(invalid_tool),
            Err(ExtensionManifestError::ReservedToolName {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn extension_catalog_discovery_is_manifest_only() {
        let manifest = parse_extension_manifest(toy_tool_manifest_json()).unwrap();
        let catalog = ExtensionCatalog::from_manifests(vec![manifest]).unwrap();

        assert_eq!(catalog.extensions().len(), 1);
        assert_eq!(catalog.host_start_count(), 0);
        assert_eq!(
            catalog.tool_candidates("toy_tool").map(|candidate| &candidate.extension_id),
            Some(&ExtensionId(String::from("example.toy-tools")))
        );
    }
}
```

- [ ] **Step 3: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend extension_manifest_ -- --nocapture
```

Expected: compile failure because `parse_extension_manifest`, manifest types, and catalog types do not exist.

- [ ] **Step 4: Implement the minimal manifest/catalog module**

In `crates/yach-backend/src/extension.rs`, add:

```rust
use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtensionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionManifestSchema {
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub schema: ExtensionManifestSchema,
    pub id: ExtensionId,
    pub version: String,
    pub main: ExtensionMain,
    pub activation: ExtensionActivation,
    pub contributes: ExtensionContributions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMain {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionActivation {
    pub events: Vec<ExtensionActivationEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionActivationEvent {
    Command(String),
    PostFirstPaint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionContributions {
    pub tools: Vec<ExtensionToolContribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolContribution {
    pub name: String,
    pub description: String,
    pub risk: ExtensionToolRisk,
    pub provider_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionToolRisk {
    ReadsLocalMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionManifestError {
    Malformed,
    UnsupportedSchema,
    InvalidExtensionId,
    InvalidCommand,
    InvalidActivationEvent { event: String },
    InvalidToolName { name: String },
    ReservedToolName { name: String },
    UnsupportedToolRisk { risk: String },
    DuplicateToolName { name: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    schema: String,
    id: String,
    version: String,
    main: RawMain,
    activation: RawActivation,
    #[serde(default)]
    contributes: RawContributions,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMain {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawActivation {
    #[serde(default)]
    events: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawContributions {
    #[serde(default)]
    tools: Vec<RawToolContribution>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawToolContribution {
    name: String,
    description: String,
    risk: String,
    provider_visible: bool,
}

pub fn parse_extension_manifest(
    value: serde_json::Value,
) -> Result<ExtensionManifest, ExtensionManifestError> {
    let raw: RawManifest =
        serde_json::from_value(value).map_err(|_| ExtensionManifestError::Malformed)?;
    let schema = match raw.schema.as_str() {
        "yach.extension.v1" => ExtensionManifestSchema::V1,
        _ => return Err(ExtensionManifestError::UnsupportedSchema),
    };
    validate_extension_id(&raw.id)?;
    if raw.main.command.trim().is_empty() {
        return Err(ExtensionManifestError::InvalidCommand);
    }

    let mut events = Vec::with_capacity(raw.activation.events.len());
    for event in raw.activation.events {
        if let Some(command) = event.strip_prefix("onCommand:") {
            if command.trim().is_empty() {
                return Err(ExtensionManifestError::InvalidActivationEvent { event });
            }
            events.push(ExtensionActivationEvent::Command(command.to_string()));
        } else if event == "postFirstPaint" {
            events.push(ExtensionActivationEvent::PostFirstPaint);
        } else {
            return Err(ExtensionManifestError::InvalidActivationEvent { event });
        }
    }

    let mut names = BTreeSet::new();
    let mut tools = Vec::with_capacity(raw.contributes.tools.len());
    for tool in raw.contributes.tools {
        validate_tool_name(&tool.name)?;
        if !names.insert(tool.name.clone()) {
            return Err(ExtensionManifestError::DuplicateToolName { name: tool.name });
        }
        let risk = match tool.risk.as_str() {
            "reads_local_metadata" => ExtensionToolRisk::ReadsLocalMetadata,
            _ => {
                return Err(ExtensionManifestError::UnsupportedToolRisk {
                    risk: tool.risk,
                });
            }
        };
        tools.push(ExtensionToolContribution {
            name: tool.name,
            description: tool.description,
            risk,
            provider_visible: tool.provider_visible,
        });
    }

    Ok(ExtensionManifest {
        schema,
        id: ExtensionId(raw.id),
        version: raw.version,
        main: ExtensionMain {
            command: raw.main.command,
            args: raw.main.args,
        },
        activation: ExtensionActivation { events },
        contributes: ExtensionContributions { tools },
    })
}

fn validate_extension_id(id: &str) -> Result<(), ExtensionManifestError> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '.' || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(ExtensionManifestError::InvalidExtensionId)
    }
}

fn validate_tool_name(name: &str) -> Result<(), ExtensionManifestError> {
    if matches!(name, "project_path_info" | "fixture_echo_metadata") {
        return Err(ExtensionManifestError::ReservedToolName {
            name: name.to_string(),
        });
    }
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(ExtensionManifestError::InvalidToolName {
            name: name.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolCandidate {
    pub extension_id: ExtensionId,
    pub tool: ExtensionToolContribution,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionCatalog {
    manifests: Vec<ExtensionManifest>,
    tools_by_name: BTreeMap<String, ExtensionToolCandidate>,
    host_start_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionCatalogError {
    DuplicateToolName { name: String },
}

impl ExtensionCatalog {
    pub fn from_manifests(
        manifests: Vec<ExtensionManifest>,
    ) -> Result<Self, ExtensionCatalogError> {
        let mut tools_by_name = BTreeMap::new();
        for manifest in &manifests {
            for tool in &manifest.contributes.tools {
                if tools_by_name
                    .insert(
                        tool.name.clone(),
                        ExtensionToolCandidate {
                            extension_id: manifest.id.clone(),
                            tool: tool.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(ExtensionCatalogError::DuplicateToolName {
                        name: tool.name.clone(),
                    });
                }
            }
        }
        Ok(Self {
            manifests,
            tools_by_name,
            host_start_count: 0,
        })
    }

    #[must_use]
    pub fn extensions(&self) -> &[ExtensionManifest] {
        &self.manifests
    }

    #[must_use]
    pub const fn host_start_count(&self) -> usize {
        self.host_start_count
    }

    #[must_use]
    pub fn tool_candidates(&self, name: &str) -> Option<&ExtensionToolCandidate> {
        self.tools_by_name.get(name)
    }
}
```

In `crates/yach-backend/src/lib.rs`, add:

```rust
mod extension;
pub use extension::*;
```

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend extension_ -- --nocapture
```

Expected: manifest/catalog tests pass.

Commit:

```bash
git add crates/yach-backend/src/extension.rs crates/yach-backend/src/lib.rs docs/project/specs/2026-05-12-extension-tool-registration-design.md
git commit -m "Add extension manifest catalog"
```

## Task 2: Add Tool Ownership, Visibility, And Extension Registration

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/extension.rs`

- [ ] **Step 1: Write failing registry tests**

Add tests near the existing native registry tests in `crates/yach-backend/src/lib.rs`.
Also update the explicit `use super::{ ... }` list in that test module to include:

```rust
NativeToolOwner, NativeToolRegistrationError, ProviderToolVisibility,
```

```rust
#[test]
fn native_tool_registry_registers_extension_owned_metadata_tool() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    let definition = NativeToolDefinition::extension_metadata_tool(
        "example.toy-tools",
        "toy_tool",
        "Return static fixture metadata.",
        NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
        ProviderToolVisibility::Hidden,
    );

    let result = registry.register_extension_tool(definition.clone());

    assert_eq!(result, Ok(()));
    assert_eq!(registry.get("toy_tool"), Some(&definition));
    assert_eq!(
        registry.get("toy_tool").map(|tool| &tool.owner),
        Some(&NativeToolOwner::Extension {
            extension_id: String::from("example.toy-tools")
        })
    );
}

#[test]
fn native_tool_registry_rejects_extension_tool_collisions() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    let colliding = NativeToolDefinition::extension_metadata_tool(
        "example.toy-tools",
        "project_path_info",
        "shadow built-in",
        NativeToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 512),
        ProviderToolVisibility::Hidden,
    );

    assert_eq!(
        registry.register_extension_tool(colliding),
        Err(NativeToolRegistrationError::DuplicateToolName {
            name: String::from("project_path_info")
        })
    );
}

#[test]
fn provider_advertising_candidates_include_only_visible_allowed_routable_tools() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    registry
        .register_extension_tool(NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        ))
        .unwrap();

    let policy = NativeToolPermissionPolicy::allow_project_metadata_tools([
        "project_path_info",
        "toy_tool",
    ]);
    let tools = registry.provider_advertising_candidates(&policy, ["toy_tool"]);

    assert_eq!(tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(), vec!["toy_tool"]);
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
just dev cargo test -p yach-backend native_tool_registry_registers_extension -- --nocapture
just dev cargo test -p yach-backend provider_advertising_candidates -- --nocapture
```

Expected: compile failure for `NativeToolOwner`, `ProviderToolVisibility`, registration error, and new registry methods.

- [ ] **Step 3: Extend native tool model**

In `crates/yach-backend/src/tools.rs`, update `NativeToolDefinition` and add supporting types:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolOwner {
    BuiltIn,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderToolVisibility {
    Hidden,
    Visible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: NativeToolInputSchema,
    pub risk: NativeToolRisk,
    pub owner: NativeToolOwner,
    pub provider_visibility: ProviderToolVisibility,
}
```

Update built-in constructors to set:

```rust
owner: NativeToolOwner::BuiltIn,
provider_visibility: ProviderToolVisibility::Hidden,
```

except `project_path_info`, which should use:

```rust
provider_visibility: ProviderToolVisibility::Visible,
```

Add:

```rust
impl NativeToolDefinition {
    #[must_use]
    pub fn extension_metadata_tool(
        extension_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: NativeToolInputSchema,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::Extension {
                extension_id: extension_id.into(),
            },
            provider_visibility,
        }
    }
}
```

Add registry APIs:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolRegistrationError {
    DuplicateToolName { name: String },
    UnsupportedRisk { name: String, risk: NativeToolRisk },
}

impl NativeToolRegistry {
    pub fn register_extension_tool(
        &mut self,
        definition: NativeToolDefinition,
    ) -> Result<(), NativeToolRegistrationError> {
        if self.get(&definition.name).is_some() {
            return Err(NativeToolRegistrationError::DuplicateToolName {
                name: definition.name,
            });
        }
        if definition.risk != NativeToolRisk::ReadsLocalMetadata {
            return Err(NativeToolRegistrationError::UnsupportedRisk {
                name: definition.name,
                risk: definition.risk,
            });
        }
        self.definitions.push(definition);
        Ok(())
    }

    #[must_use]
    pub fn provider_advertising_candidates<'a>(
        &'a self,
        policy: &NativeToolPermissionPolicy,
        routable_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<NativeToolDefinition> {
        let routable: BTreeSet<&str> = routable_tools.into_iter().collect();
        self.definitions
            .iter()
            .filter(|definition| {
                definition.provider_visibility == ProviderToolVisibility::Visible
                    && routable.contains(definition.name.as_str())
                    && policy.authorize(definition) == NativeToolPermissionState::Allowed
            })
            .cloned()
            .collect()
    }
}
```

Extend policy:

```rust
impl NativeToolPermissionPolicy {
    #[must_use]
    pub fn allow_project_metadata_tools(
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            allowed_fixture_tools: BTreeSet::new(),
            allowed_project_metadata_tools: names.into_iter().map(Into::into).collect(),
        }
    }
}
```

- [ ] **Step 4: Convert manifest tool candidates into native definitions**

In `crates/yach-backend/src/extension.rs`, add:

```rust
use crate::{NativeToolDefinition, NativeToolInputSchema, ProviderToolVisibility};

impl ExtensionToolCandidate {
    #[must_use]
    pub fn to_native_definition(&self) -> NativeToolDefinition {
        NativeToolDefinition::extension_metadata_tool(
            self.extension_id.0.clone(),
            self.tool.name.clone(),
            self.tool.description.clone(),
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            if self.tool.provider_visible {
                ProviderToolVisibility::Visible
            } else {
                ProviderToolVisibility::Hidden
            },
        )
    }
}
```

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_tool_registry_ -- --nocapture
just dev cargo test -p yach-backend provider_advertising_candidates -- --nocapture
just dev cargo test -p yach-backend extension_catalog_ -- --nocapture
```

Expected: focused tests pass.

Commit:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/extension.rs crates/yach-backend/src/lib.rs
git commit -m "Add extension tool registration model"
```

## Task 3: Add Host Protocol Message Validation

**Files:**
- Modify: `crates/yach-backend/src/extension.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing protocol registration tests**

Add to `crates/yach-backend/src/extension.rs` tests:

```rust
#[test]
fn extension_host_registers_toy_tool_after_ready_handshake() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    let messages = vec![
        serde_json::json!({
            "type": "extension.ready",
            "protocol": "yach.extension-host.v1",
            "extension_id": "example.toy-tools"
        }),
        serde_json::json!({
            "type": "tool.register",
            "name": "toy_tool",
            "description": "Return static fixture metadata.",
            "risk": "reads_local_metadata",
            "provider_visible": true,
            "input_schema": {
                "type": "object",
                "properties": {
                    "label": {"type": "string"}
                },
                "required": ["label"],
                "additionalProperties": false,
                "maxSerializedBytes": 512
            }
        }),
    ];

    let registrations = process_extension_registration_messages(
        ExtensionId(String::from("example.toy-tools")),
        messages,
        &mut registry,
    );

    assert_eq!(registrations, Ok(vec![String::from("toy_tool")]));
    assert!(registry.get("toy_tool").is_some());
}

#[test]
fn extension_host_registration_rejects_unsupported_schema_features() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    let messages = vec![
        serde_json::json!({
            "type": "extension.ready",
            "protocol": "yach.extension-host.v1",
            "extension_id": "example.toy-tools"
        }),
        serde_json::json!({
            "type": "tool.register",
            "name": "toy_tool",
            "description": "Return static fixture metadata.",
            "risk": "reads_local_metadata",
            "provider_visible": false,
            "input_schema": {
                "type": "object",
                "properties": {
                    "count": {"type": "number"}
                },
                "required": ["count"],
                "additionalProperties": false,
                "maxSerializedBytes": 512
            }
        }),
    ];

    assert_eq!(
        process_extension_registration_messages(
            ExtensionId(String::from("example.toy-tools")),
            messages,
            &mut registry,
        ),
        Err(ExtensionHostProtocolError::UnsupportedSchema)
    );
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
just dev cargo test -p yach-backend extension_host_ -- --nocapture
```

Expected: compile failure for host protocol types/functions.

- [ ] **Step 3: Implement JSON registration protocol helpers**

In `crates/yach-backend/src/extension.rs`, add:

```rust
use crate::{NativeToolRegistrationError, NativeToolRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionHostProtocolError {
    Malformed,
    MissingReady,
    UnsupportedProtocol,
    ExtensionIdMismatch,
    UnsupportedRisk,
    UnsupportedSchema,
    ToolRegistration(NativeToolRegistrationError),
}

pub fn process_extension_registration_messages(
    expected_extension_id: ExtensionId,
    messages: Vec<serde_json::Value>,
    registry: &mut NativeToolRegistry,
) -> Result<Vec<String>, ExtensionHostProtocolError> {
    let mut ready = false;
    let mut registered = Vec::new();
    for message in messages {
        let message_type = message
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or(ExtensionHostProtocolError::Malformed)?;
        match message_type {
            "extension.ready" => {
                let protocol = message
                    .get("protocol")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                if protocol != "yach.extension-host.v1" {
                    return Err(ExtensionHostProtocolError::UnsupportedProtocol);
                }
                let extension_id = message
                    .get("extension_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                if extension_id != expected_extension_id.0 {
                    return Err(ExtensionHostProtocolError::ExtensionIdMismatch);
                }
                ready = true;
            }
            "tool.register" => {
                if !ready {
                    return Err(ExtensionHostProtocolError::MissingReady);
                }
                let name = message
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                let description = message
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                let risk = message
                    .get("risk")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                if risk != "reads_local_metadata" {
                    return Err(ExtensionHostProtocolError::UnsupportedRisk);
                }
                let provider_visible = message
                    .get("provider_visible")
                    .and_then(serde_json::Value::as_bool)
                    .ok_or(ExtensionHostProtocolError::Malformed)?;
                let schema = parse_extension_string_object_schema(
                    message
                        .get("input_schema")
                        .ok_or(ExtensionHostProtocolError::Malformed)?,
                )?;
                let definition = NativeToolDefinition::extension_metadata_tool(
                    expected_extension_id.0.clone(),
                    name,
                    description,
                    schema,
                    if provider_visible {
                        ProviderToolVisibility::Visible
                    } else {
                        ProviderToolVisibility::Hidden
                    },
                );
                registry
                    .register_extension_tool(definition)
                    .map_err(ExtensionHostProtocolError::ToolRegistration)?;
                registered.push(name.to_string());
            }
            _ => return Err(ExtensionHostProtocolError::Malformed),
        }
    }
    if !ready {
        return Err(ExtensionHostProtocolError::MissingReady);
    }
    Ok(registered)
}

fn parse_extension_string_object_schema(
    value: &serde_json::Value,
) -> Result<NativeToolInputSchema, ExtensionHostProtocolError> {
    let object = value
        .as_object()
        .ok_or(ExtensionHostProtocolError::UnsupportedSchema)?;
    if object.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(ExtensionHostProtocolError::UnsupportedSchema);
    }
    if object
        .get("additionalProperties")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        return Err(ExtensionHostProtocolError::UnsupportedSchema);
    }
    let max_serialized_bytes = object
        .get("maxSerializedBytes")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ExtensionHostProtocolError::UnsupportedSchema)?
        as usize;
    let properties = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .ok_or(ExtensionHostProtocolError::UnsupportedSchema)?;
    let required_values = object
        .get("required")
        .and_then(serde_json::Value::as_array)
        .ok_or(ExtensionHostProtocolError::UnsupportedSchema)?;
    let mut required = Vec::new();
    for field in required_values {
        let Some(field) = field.as_str() else {
            return Err(ExtensionHostProtocolError::UnsupportedSchema);
        };
        let Some(property) = properties.get(field) else {
            return Err(ExtensionHostProtocolError::UnsupportedSchema);
        };
        if property.get("type").and_then(serde_json::Value::as_str) != Some("string") {
            return Err(ExtensionHostProtocolError::UnsupportedSchema);
        }
        required.push(field.to_string());
    }
    if properties.len() != required.len() {
        return Err(ExtensionHostProtocolError::UnsupportedSchema);
    }
    Ok(NativeToolInputSchema::string_object(
        required,
        std::iter::empty::<String>(),
        max_serialized_bytes,
    ))
}
```

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend extension_host_ -- --nocapture
```

Expected: host protocol tests pass.

Commit:

```bash
git add crates/yach-backend/src/extension.rs crates/yach-backend/src/tools.rs
git commit -m "Add extension host registration protocol"
```

## Task 4: Add Process Host Handshake And Failure Categorization

**Files:**
- Modify: `crates/yach-backend/src/extension.rs`

- [ ] **Step 1: Write failing process-host tests**

Add Unix-gated tests in `crates/yach-backend/src/extension.rs`. These tests use `sh` as a fixture host so the implementation proves an actual child process boundary without requiring Node, npm, or a compiled extension SDK:

```rust
#[cfg(unix)]
#[test]
fn extension_process_host_registers_toy_tool_from_stdout_jsonl() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    let script = r#"printf '%s\n' \
'{"type":"extension.ready","protocol":"yach.extension-host.v1","extension_id":"example.toy-tools"}' \
'{"type":"tool.register","name":"toy_tool","description":"Return static fixture metadata.","risk":"reads_local_metadata","provider_visible":true,"input_schema":{"type":"object","properties":{"label":{"type":"string"}},"required":["label"],"additionalProperties":false,"maxSerializedBytes":512}}'"#;

    let result = run_extension_host_registration_command(
        ExtensionId(String::from("example.toy-tools")),
        ExtensionHostCommand {
            command: String::from("sh"),
            args: vec![String::from("-c"), String::from(script)],
            timeout: std::time::Duration::from_secs(2),
            max_stdout_bytes: 16 * 1024,
        },
        &mut registry,
    );

    assert_eq!(result, Ok(vec![String::from("toy_tool")]));
    assert!(registry.get("toy_tool").is_some());
}

#[cfg(unix)]
#[test]
fn extension_process_host_reports_exit_timeout_and_malformed_output() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();

    let exited = run_extension_host_registration_command(
        ExtensionId(String::from("example.toy-tools")),
        ExtensionHostCommand {
            command: String::from("sh"),
            args: vec![String::from("-c"), String::from("exit 7")],
            timeout: std::time::Duration::from_secs(2),
            max_stdout_bytes: 16 * 1024,
        },
        &mut registry,
    );
    assert_eq!(
        exited,
        Err(ExtensionHostProtocolError::HostExited { status: Some(7) })
    );

    let timed_out = run_extension_host_registration_command(
        ExtensionId(String::from("example.toy-tools")),
        ExtensionHostCommand {
            command: String::from("sh"),
            args: vec![String::from("-c"), String::from("sleep 5")],
            timeout: std::time::Duration::from_millis(10),
            max_stdout_bytes: 16 * 1024,
        },
        &mut registry,
    );
    assert_eq!(timed_out, Err(ExtensionHostProtocolError::TimedOut));

    let malformed = run_extension_host_registration_command(
        ExtensionId(String::from("example.toy-tools")),
        ExtensionHostCommand {
            command: String::from("sh"),
            args: vec![String::from("-c"), String::from("printf 'not-json\\n'")],
            timeout: std::time::Duration::from_secs(2),
            max_stdout_bytes: 16 * 1024,
        },
        &mut registry,
    );
    assert_eq!(malformed, Err(ExtensionHostProtocolError::Malformed));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
just dev cargo test -p yach-backend extension_process_host_ -- --nocapture
```

Expected: compile failure for `ExtensionHostCommand` and `run_extension_host_registration_command`.

- [ ] **Step 3: Implement the synchronous process boundary**

In `crates/yach-backend/src/extension.rs`, extend `ExtensionHostProtocolError`:

```rust
    SpawnFailed,
    HostExited { status: Option<i32> },
    TimedOut,
    OutputTooLarge { max_bytes: usize },
```

Then add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHostCommand {
    pub command: String,
    pub args: Vec<String>,
    pub timeout: std::time::Duration,
    pub max_stdout_bytes: usize,
}

pub fn run_extension_host_registration_command(
    extension_id: ExtensionId,
    command: ExtensionHostCommand,
    registry: &mut NativeToolRegistry,
) -> Result<Vec<String>, ExtensionHostProtocolError> {
    let mut child = std::process::Command::new(&command.command)
        .args(&command.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?;
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() >= command.timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ExtensionHostProtocolError::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                if let Some(mut pipe) = child.stdout.take() {
                    use std::io::Read;
                    pipe.read_to_end(&mut stdout)
                        .map_err(|_| ExtensionHostProtocolError::Malformed)?;
                }
                if !status.success() {
                    return Err(ExtensionHostProtocolError::HostExited {
                        status: status.code(),
                    });
                }
                if stdout.len() > command.max_stdout_bytes {
                    return Err(ExtensionHostProtocolError::OutputTooLarge {
                        max_bytes: command.max_stdout_bytes,
                    });
                }
                let mut messages = Vec::new();
                for line in String::from_utf8(stdout)
                    .map_err(|_| ExtensionHostProtocolError::Malformed)?
                    .lines()
                {
                    messages.push(
                        serde_json::from_str(line)
                            .map_err(|_| ExtensionHostProtocolError::Malformed)?,
                    );
                }
                return process_extension_registration_messages(extension_id, messages, registry);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(1)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExtensionHostProtocolError::HostExited { status: None });
            }
        }
    }
}
```

This is deliberately synchronous and backend-local for the first slice. Do not wire it into TUI startup.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend extension_process_host_ -- --nocapture
```

Expected: process-host handshake and failure categorization tests pass.

Commit:

```bash
git add crates/yach-backend/src/extension.rs
git commit -m "Add extension process host registration"
```

## Task 5: Route Extension Execution Through Native Tool Workflow

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing executor router tests**

Add tests in `crates/yach-backend/src/lib.rs` near existing provider tool result tests.
Also update the explicit `use super::{ ... }` list in that test module to include:

```rust
ExtensionToolExecutorRouter, ExtensionToolHandler, NativeToolContinuationWorkflow,
```

Add this local test helper near the existing provider tool-call setup helpers:

```rust
fn provider_tool_call(
    call_id: impl Into<String>,
    name: impl Into<String>,
    arguments_json: serde_json::Value,
) -> ProviderToolCall {
    ProviderToolCall {
        call_id: call_id.into(),
        name: name.into(),
        arguments_json,
    }
}
```

```rust
#[test]
fn extension_executor_routes_through_native_tool_workflow_and_records_evidence() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    registry
        .register_extension_tool(NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        ))
        .unwrap();
    let mut log = NativeSessionLog::default();
    let context = fixture_continuation_context();
    let executor = ExtensionToolExecutorRouter::from_handlers([(
        "toy_tool",
        ExtensionToolHandler::static_metadata("{\"ok\":true}"),
    )]);

    let result = NativeToolContinuationWorkflow {
        registry: &registry,
        permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]),
        executor: &executor,
        continuation_policy: NativeToolContinuationPolicy::fixture_default(),
    }
    .build_provider_tool_results(
        &mut log,
        &context,
        vec![provider_tool_call("provider-call-1", "toy_tool", serde_json::json!({"label":"demo"}))],
    );

    assert_eq!(result.as_ref().map(Vec::len), Ok(1));
    assert_eq!(result.unwrap()[0].content, "{\"ok\":true}");
    assert!(log
        .events
        .iter()
        .any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded { tool_name, .. } if tool_name == "toy_tool"
        )));
}

#[test]
fn extension_executor_failure_modes_are_categorized() {
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    registry
        .register_extension_tool(NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        ))
        .unwrap();
    let context = fixture_continuation_context();

    let denied = NativeToolContinuationWorkflow {
        registry: &registry,
        permission_policy: &NativeToolPermissionPolicy::deny_all(),
        executor: &ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("{\"ok\":true}"),
        )]),
        continuation_policy: NativeToolContinuationPolicy::fixture_default(),
    }
    .build_provider_tool_results(
        &mut NativeSessionLog::default(),
        &context,
        vec![provider_tool_call("provider-call-1", "toy_tool", serde_json::json!({"label":"demo"}))],
    );
    assert_eq!(
        denied,
        Err(NativeToolContinuationError::Validation(
            NativeToolError::PermissionDenied
        ))
    );

    let malformed = NativeToolContinuationWorkflow {
        registry: &registry,
        permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]),
        executor: &ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::malformed_result(),
        )]),
        continuation_policy: NativeToolContinuationPolicy::fixture_default(),
    }
    .build_provider_tool_results(
        &mut NativeSessionLog::default(),
        &context,
        vec![provider_tool_call("provider-call-1", "toy_tool", serde_json::json!({"label":"demo"}))],
    );
    assert_eq!(
        malformed,
        Err(NativeToolContinuationError::Execution(
            NativeToolExecutionError::MalformedResult
        ))
    );

    let oversized = NativeToolContinuationWorkflow {
        registry: &registry,
        permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]),
        executor: &ExtensionToolExecutorRouter::from_handlers([(
            "toy_tool",
            ExtensionToolHandler::static_metadata("this result is too large"),
        )]),
        continuation_policy: NativeToolContinuationPolicy {
            max_tool_calls: 1,
            max_result_bytes: 4,
        },
    }
    .build_provider_tool_results(
        &mut NativeSessionLog::default(),
        &context,
        vec![provider_tool_call("provider-call-1", "toy_tool", serde_json::json!({"label":"demo"}))],
    );
    assert!(matches!(
        oversized,
        Err(NativeToolContinuationError::ResultTooLarge { .. })
    ));
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
just dev cargo test -p yach-backend extension_executor_routes -- --nocapture
```

Expected: compile failure for `ExtensionToolExecutorRouter`, `ExtensionToolHandler`, and helper visibility if needed.

- [ ] **Step 3: Implement router using existing executor trait**

In `crates/yach-backend/src/tools.rs`, change the existing import from:

```rust
use std::collections::BTreeSet;
```

to:

```rust
use std::collections::{BTreeMap, BTreeSet};
```

Then add:

```rust

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolHandler {
    response: String,
    malformed: bool,
}

impl ExtensionToolHandler {
    #[must_use]
    pub fn static_metadata(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
            malformed: false,
        }
    }

    #[must_use]
    pub fn malformed_result() -> Self {
        Self {
            response: String::new(),
            malformed: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionToolExecutorRouter {
    handlers: BTreeMap<String, ExtensionToolHandler>,
}

impl ExtensionToolExecutorRouter {
    #[must_use]
    pub fn from_handlers(
        handlers: impl IntoIterator<Item = (impl Into<String>, ExtensionToolHandler)>,
    ) -> Self {
        Self {
            handlers: handlers
                .into_iter()
                .map(|(name, handler)| (name.into(), handler))
                .collect(),
        }
    }
}

impl NativeToolExecutor for ExtensionToolExecutorRouter {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnknownTool);
        };
        if validation.permission != NativeToolPermissionState::Allowed {
            return Err(NativeToolExecutionError::PermissionDenied);
        }
        if !matches!(definition.owner, NativeToolOwner::Extension { .. }) {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }
        let Some(handler) = self.handlers.get(&definition.name) else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        if handler.malformed {
            return Err(NativeToolExecutionError::MalformedResult);
        }
        Ok(NativeToolExecutionResult {
            request_id: request.request_id.clone(),
            summary: handler.response.clone(),
            byte_count: handler.response.len(),
            redacted: false,
            truncated: false,
        })
    }
}
```

Add a new execution error variant:

```rust
MalformedResult,
```

Update `native_tool_execution_error_label` to handle it with a stable categorical label:

```rust
NativeToolExecutionError::MalformedResult => "malformed_result",
```

This task intentionally uses an in-memory extension handler. Process-backed `tool.invoke` / `tool.result` handling, including invocation-time host crash and timeout categorization, remains out of scope for this first registration slice.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend extension_executor_routes -- --nocapture
just dev cargo test -p yach-backend project_readonly_provider_tool_results -- --nocapture
```

Expected: extension router and existing project read-only tool workflow tests pass.

Commit:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
git commit -m "Route extension tools through native workflow"
```

## Task 6: Generalize Provider Advertising For Accepted Extension Tools

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing advertising tests**

Add tests:

```rust
#[test]
fn provider_tool_advertising_builder_emits_approved_extension_schema() {
    let tool = NativeToolDefinition::extension_metadata_tool(
        "example.toy-tools",
        "toy_tool",
        "Return static fixture metadata.",
        NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
        ProviderToolVisibility::Visible,
    );

    let extension = build_provider_tool_advertising_extension(&[tool]).unwrap();
    let advertising = parse_provider_tool_advertising_extensions(&[extension])
        .unwrap()
        .unwrap();

    assert_eq!(advertising.tools[0].name, "toy_tool");
    assert_eq!(advertising.tools[0].parameters["required"], serde_json::json!(["label"]));
    assert_eq!(
        advertising.tools[0].parameters["properties"]["label"]["type"],
        "string"
    );
}
```

Add a `rig_adapter.rs` test near existing advertising projection tests:
Also update the `crate::{ ... }` imports in the `rig_adapter.rs` test module to include:

```rust
NativeToolDefinition, NativeToolInputSchema, ProviderToolVisibility,
build_provider_tool_advertising_extension,
```

```rust
#[test]
fn rig_adapter_projects_extension_advertising_to_schema_only_tool_definition() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        ),
    ])
    .expect("extension tool should advertise");
    let request = provider_request_with_extensions(vec![extension]);

    let tools = rig_tool_definitions_from_request(&request)
        .expect("advertising should project to rig tools");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "toy_tool");
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising_builder_emits_approved_extension -- --nocapture
just dev cargo test -p yach-backend rig_adapter_projects_extension -- --nocapture
```

Expected: current hard-coded `project_path_info` projector rejects `toy_tool`.

- [ ] **Step 3: Generalize schema projection while preserving fail-closed validation**

In `crates/yach-backend/src/tools.rs`, replace the hard-coded `project_provider_advertised_tool` path with a conservative projector:

```rust
fn project_provider_advertised_tool(
    tool: &NativeToolDefinition,
) -> Result<ProviderAdvertisedToolSchema, ProviderToolAdvertisingError> {
    if tool.provider_visibility != ProviderToolVisibility::Visible {
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
    Ok(ProviderAdvertisedToolSchema {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.to_provider_json_schema(&tool.name)?,
    })
}
```

Add to `NativeToolInputSchema`:

```rust
pub fn to_provider_json_schema(
    &self,
    name: &str,
) -> Result<serde_json::Value, ProviderToolAdvertisingError> {
    let mut properties = serde_json::Map::new();
    for field in &self.required_string_fields {
        properties.insert(
            field.clone(),
            serde_json::json!({
                "type": "string",
                "description": format!("{field} argument for {name}.")
            }),
        );
    }
    for field in &self.optional_string_fields {
        properties.insert(
            field.clone(),
            serde_json::json!({
                "type": "string",
                "description": format!("{field} argument for {name}.")
            }),
        );
    }
    Ok(serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": self.required_string_fields.iter().cloned().collect::<Vec<_>>(),
        "additionalProperties": false
    }))
}
```

Keep `parse_provider_tool_advertising_extensions` fail-closed by validating each parsed schema shape generically: object type, string properties only, required fields present in properties, and `additionalProperties: false`.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising_ -- --nocapture
just dev cargo test -p yach-backend rig_adapter_projects_ -- --nocapture
```

Expected: provider advertising and Rig schema-only projection tests pass.

Commit:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
git commit -m "Advertise approved extension tool schemas"
```

## Task 7: Wire Extension Tools Into Native Provider Runner Future-Turn Advertising

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/tools.rs`

- [ ] **Step 1: Write failing native runner test with injected catalog/router**

Add a test in `crates/yach-backend/src/native_runner.rs`.
Also update the test module imports:

```rust
use super::{run_native_provider_one_tool_round_with_registry, ...};
use crate::{
    ExtensionToolExecutorRouter, ExtensionToolHandler, NativeToolDefinition,
    NativeToolInputSchema, NativeToolPermissionPolicy, NativeToolRegistry,
    ProviderToolVisibility, ...
};
```

```rust
#[test]
fn native_provider_initial_request_advertises_registered_extension_tool_for_future_turn() {
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    append_native_provider_test_entry(
        &mut log,
        &NativeSessionId(String::from("default")),
        "turn-0",
        "entry-0-user",
        NativeRole::User,
        "use toy tool if helpful",
    );
    let turn = NativeTurnId(String::from("turn-0"));
    let model = ProviderModel {
        provider: String::from("fixture-provider"),
        model: String::from("fixture-model"),
    };
    let mut registry = NativeToolRegistry::with_project_read_only_tools();
    registry
        .register_extension_tool(NativeToolDefinition::extension_metadata_tool(
            "example.toy-tools",
            "toy_tool",
            "Return static fixture metadata.",
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            ProviderToolVisibility::Visible,
        ))
        .unwrap();
    let executor = ExtensionToolExecutorRouter::from_handlers([(
        "toy_tool",
        ExtensionToolHandler::static_metadata("{\"ok\":true}"),
    )]);
    let mut requester = FakeProviderRequester::with_responses([Ok(vec![
        ProviderStreamEvent::Started {
            turn_id: turn.clone(),
            model: model.clone(),
        },
        ProviderStreamEvent::TextDelta {
            turn_id: turn.clone(),
            delta: String::from("done"),
        },
        ProviderStreamEvent::Completed {
            turn_id: turn.clone(),
            finish_reason: Some(crate::ProviderFinishReason::Stop),
            usage: None,
            provider_response_id: Some(String::from("response-1")),
        },
    ])]);

    let result = futures::executor::block_on(run_native_provider_one_tool_round_with_registry(
        &mut requester,
        model,
        &mut log,
        &mut pending_events,
        &turn,
        None,
        None,
        &registry,
        &NativeToolPermissionPolicy::allow_project_metadata_tools(["project_path_info", "toy_tool"]),
        &executor,
    ));

    assert_eq!(
        result,
        Ok(NativeProviderRoundResult {
            text: String::from("done"),
            provider_response_id: Some(String::from("response-1")),
        })
    );
    let advertising = parse_provider_tool_advertising_extensions(&requester.requests[0].extensions)
        .expect("advertising should parse")
        .expect("advertising should exist");
    assert!(advertising.tools.iter().any(|tool| tool.name == "toy_tool"));
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
just dev cargo test -p yach-backend native_provider_initial_request_advertises_registered_extension -- --nocapture
```

Expected: compile failure for injected helper and missing test imports.

- [ ] **Step 3: Add injected runner helper without changing default behavior**

In `crates/yach-backend/src/native_runner.rs`, extract the fixed registry path into a helper that accepts registry/policy/executor:

```rust
async fn run_native_provider_one_tool_round_with_registry<Provider, Executor>(
    requester: &mut Provider,
    model: ProviderModel,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    turn_id: &NativeTurnId,
    project_root: Option<NativeResourceRoot>,
    tool_event_store: Option<&NativeJsonlSessionStore>,
    registry: &NativeToolRegistry,
    permission_policy: &NativeToolPermissionPolicy,
    executor: &Executor,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError>
where
    Provider: ProviderRequester,
    Executor: NativeToolExecutor,
{
    let routable_names = registry
        .definitions()
        .iter()
        .map(|definition| definition.name.as_str());
    let advertising_tools =
        registry.provider_advertising_candidates(&permission_policy, routable_names);
    let extensions = if advertising_tools.is_empty() {
        Vec::new()
    } else {
        vec![build_provider_tool_advertising_extension(&advertising_tools).map_err(|error| {
            NativeProviderRoundError::ToolContinuation(
                native_provider_tool_advertising_error_label(&error),
            )
        })?]
    };
    // Move the body of run_native_provider_one_readonly_tool_round into this helper:
    // - build ProviderRequest from native_provider_messages_from_log(log, turn_id)
    // - collect first round
    // - if tool calls exist, require project_root
    // - call NativeToolContinuationWorkflow with the injected registry, policy, and executor
    // - preserve pending_events and tool_event_store append behavior
    // - strip provider advertising from continuation extensions
    // - return NativeProviderRoundResult
}
```

Add `NativeToolRegistry::definitions(&self) -> &[NativeToolDefinition]`.

Keep `run_native_provider_one_readonly_tool_round` as the production wrapper. It should construct `NativeToolRegistry::with_project_read_only_tools()`, `NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info")`, and `ProjectReadOnlyToolExecutor`, then call the injected helper with the existing `log`, `pending_events`, `turn_id`, `project_root`, and `tool_event_store` arguments.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
just dev cargo test -p yach-backend native_provider_initial_request_advertises_registered_extension -- --nocapture
```

Expected: existing one-round native-provider behavior remains passing; injected extension advertising test passes.

Commit:

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-backend/src/tools.rs
git commit -m "Allow injected extension tools in native provider runner"
```

## Task 8: Add Deferred Startup Diagnostics For Installed Inactive Extensions

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-bench/src/main.rs`
- Modify: `docs/benchmarks/native-startup-profile-2026-05-12.md` or add a new dated benchmark report

- [ ] **Step 1: Write a startup benchmark expectation**

Add a yach-bench mode that sets a temporary `YACH_EXTENSION_MANIFEST_DIR` containing one valid manifest but does not activate it:

```rust
Some("yach-tui-startup-profile-with-inactive-extension-report") => {
    yach_tui_startup_profile_with_inactive_extension_report_lines(sample_count(&args))
}
```

The first testable expectation is not a unit test but a benchmark command:

```bash
just dev cargo build -p yach-cli --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 100
```

Expected after implementation: `tui_first_render_end_since_main` remains in the same sub-millisecond p95 envelope as the no-extension profile, and no trace marker named `extension_host_spawned_before_first_render` exists.

- [ ] **Step 2: Add post-first-paint-only discovery marks**

If CLI wiring is needed, ensure it happens after the TUI first render marker. Add trace labels only for manifest discovery, not host activation:

```text
extension_manifest_scan_scheduled
extension_manifest_scan_started
extension_manifest_scan_finished
```

Do not spawn extension hosts from the startup path in this task.

- [ ] **Step 3: Run startup profiles**

Run:

```bash
just dev cargo build -p yach-cli --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 100
```

Expected: inactive extension profile remains within the same broad envelope as baseline. If the p95 delta exceeds 5ms, stop and investigate before committing.

- [ ] **Step 4: Document evidence and commit**

Add a short benchmark report under `docs/benchmarks/extension-startup-profile-YYYY-MM-DD.md` with command output and limitations.

Commit:

```bash
git add crates/yach-cli/src/main.rs crates/yach-bench/src/main.rs docs/benchmarks/extension-startup-profile-*.md
git commit -m "Profile inactive extension startup path"
```

## Task 9: Update Active Project Docs

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update state**

In `docs/project/state.md`, add a concise posture bullet:

```markdown
- Extension-owned tool registration now has a manifest/catalog path, host registration protocol, extension-owned executor routing, and policy-gated schema-only provider advertising for safe read-only metadata tools. Extension hosts remain off the default first-frame path.
```

- [ ] **Step 2: Update next**

In `docs/project/next.md`, set the recommended next move to the next unimplemented extension slice, likely `static_context_provider` or install UX, depending on what remains after this plan execution.

- [ ] **Step 3: Run full verification**

Run:

```bash
just fmt
just test
git diff --check
```

Expected: formatting succeeds, all tests pass, diff check is clean.

- [ ] **Step 4: Commit docs**

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "Update project state after extension tool registration"
```

## Final Verification

Before opening the implementation PR, run:

```bash
just fmt
just test
just dev cargo build -p yach-cli --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 100
git diff --check
```

`just lint` is desirable if the repo's existing Clippy debt has been cleaned up; if it still fails on unrelated backend lint debt, report the exact pre-existing failures separately instead of broadening this implementation PR.
