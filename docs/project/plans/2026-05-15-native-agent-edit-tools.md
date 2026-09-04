# Native Agent Edit Tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first native-provider agent-selected edit tools: policy-gated `edit_text_file` and `create_text_file` schemas that route through `NativeEditAccess`, preserve review/evidence boundaries, and return bounded provider continuation results.

**Architecture:** Keep provider advertising schema-only, but extend the allowlist to canonical built-in edit schemas when the session policy enables them. Split schema validation from mutation authorization so mutating tool calls can enter `NativePermissionDecisionEngine` and `NativeEditAccess`. Reuse the existing local edit review UI for agent edits through generic tool review protocol events and an internal provider-task decision bridge.

**Tech Stack:** Rust workspace, `yach-backend`, `yach-proto`, `yach-ui`, JSONL native session evidence, existing `just dev cargo test ...` recipes.

---

## Scope Notes

This plan intentionally implements only the canonical exact/create edit surface from `docs/project/specs/2026-05-15-native-agent-edit-tool-surface-design.md`.

It does not implement broad `write`, patch, delete, rename, shell/process, network, extension-owned mutation, a sandbox, or a real auto-review reviewer. The `edit_text_file` schema assumes the model already knows the text to replace; broader provider-visible read/search content tools remain a later design.

## File Structure

- `crates/yach-backend/src/tools.rs`: canonical edit tool definitions, schema-only validation, mutation advertising allowlist, provider result shape helpers.
- `crates/yach-backend/src/rig_adapter.rs`: adapter-side approval for the canonical edit schemas so advertised tools become Rig tool definitions only when explicitly approved.
- `crates/yach-backend/src/edit_access.rs`: carry `NativeToolRequestId` through prepared and finished edit evidence.
- `crates/yach-backend/src/agent_edit_tools.rs`: new focused module for provider-originated edit tool normalization, execution, review routing, and bounded results.
- `crates/yach-backend/src/native_runner.rs`: wire agent edit tools into the native provider path and bridge review decisions between the provider task and the main event loop.
- `crates/yach-backend/src/lib.rs`: export the new module and add backend tests that do not require live provider calls.
- `crates/yach-proto/src/lib.rs`: add generic tool review protocol events while keeping existing local edit events intact.
- `crates/yach-ui/src/app.rs`: render generic tool review requests with a local edit preview payload and send the matching tool review decision event.
- `docs/project/state.md` and `docs/project/next.md`: update active handoff after implementation lands.

---

### Task 1: Add Canonical Agent Edit Tool Definitions And Schema-Only Validation

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Test: `crates/yach-backend/src/lib.rs`
- Test: `crates/yach-backend/src/rig_adapter.rs`

- [ ] **Step 1: Write failing backend tests for canonical edit definitions**

Add these tests near the existing provider/tool registry tests in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_tool_registry_exposes_canonical_agent_edit_tools() {
    let registry = NativeToolRegistry::with_agent_edit_tools();

    let edit = registry.get("edit_text_file");
    assert!(edit.is_some());
    let Some(edit) = edit else {
        return;
    };
    assert_eq!(edit.risk, NativeToolRisk::MutatesLocalState);
    assert_eq!(edit.owner, NativeToolOwner::BuiltIn);
    assert_eq!(edit.provider_visibility, ProviderToolVisibility::Visible);

    let create = registry.get("create_text_file");
    assert!(create.is_some());
    let Some(create) = create else {
        return;
    };
    assert_eq!(create.risk, NativeToolRisk::MutatesLocalState);
    assert_eq!(create.owner, NativeToolOwner::BuiltIn);
    assert_eq!(create.provider_visibility, ProviderToolVisibility::Visible);
}

#[test]
fn agent_edit_tool_schema_rejects_expected_sha256_from_provider() {
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "expected_sha256": "provider-must-not-supply-this",
            "find": "old",
            "replace": "new"
        }),
    };

    assert_eq!(
        registry.validate_request_schema_only(&request).err(),
        Some(NativeToolError::UnexpectedField {
            field: String::from("expected_sha256")
        })
    );
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_tool_registry_exposes_canonical_agent_edit_tools -- --exact
just dev cargo test -p yach-backend agent_edit_tool_schema_rejects_expected_sha256_from_provider -- --exact
```

Expected: both fail because `with_agent_edit_tools`, canonical edit definitions, and `validate_request_schema_only` do not exist yet.

- [ ] **Step 3: Add canonical definitions in `tools.rs`**

Add these associated constructors to `impl NativeToolDefinition`:

```rust
#[must_use]
pub fn edit_text_file() -> Self {
    Self {
        name: String::from("edit_text_file"),
        description: String::from(
            "Replace exact text in an existing UTF-8 project file. Yach computes the current file hash before applying.",
        ),
        input_schema: NativeToolInputSchema::string_object(
            ["path", "find", "replace"],
            std::iter::empty::<&str>(),
            16 * 1024,
        ),
        risk: NativeToolRisk::MutatesLocalState,
        owner: NativeToolOwner::BuiltIn,
        provider_visibility: ProviderToolVisibility::Visible,
    }
}

#[must_use]
pub fn create_text_file() -> Self {
    Self {
        name: String::from("create_text_file"),
        description: String::from(
            "Create a new UTF-8 project file. Fails if the target already exists.",
        ),
        input_schema: NativeToolInputSchema::string_object(
            ["path", "content"],
            std::iter::empty::<&str>(),
            128 * 1024,
        ),
        risk: NativeToolRisk::MutatesLocalState,
        owner: NativeToolOwner::BuiltIn,
        provider_visibility: ProviderToolVisibility::Visible,
    }
}
```

Update `provider_string_field_description` to return specific descriptions for:

```rust
("edit_text_file", "path") => "Project-relative UTF-8 text file path to edit.",
("edit_text_file", "find") => "Exact text to replace. The match must be unique.",
("edit_text_file", "replace") => "Replacement text.",
("create_text_file", "path") => "Project-relative UTF-8 text file path to create.",
("create_text_file", "content") => "Full content for the new UTF-8 text file.",
```

- [ ] **Step 4: Add registry constructors and schema-only validation**

Add to `impl NativeToolRegistry`:

```rust
#[must_use]
pub fn with_agent_edit_tools() -> Self {
    Self {
        definitions: vec![
            NativeToolDefinition::edit_text_file(),
            NativeToolDefinition::create_text_file(),
        ],
    }
}

#[must_use]
pub fn with_project_read_only_and_agent_edit_tools() -> Self {
    Self {
        definitions: vec![
            NativeToolDefinition::project_path_info(),
            NativeToolDefinition::edit_text_file(),
            NativeToolDefinition::create_text_file(),
        ],
    }
}

pub fn validate_request_schema_only(
    &self,
    request: &PendingNativeToolRequest,
) -> Result<&NativeToolDefinition, NativeToolError> {
    let definition = self
        .get(&request.tool_name)
        .ok_or(NativeToolError::UnknownTool)?;
    definition.input_schema.validate(&request.arguments)?;
    Ok(definition)
}
```

Do not change `validate_request` to allow mutation. That method should keep enforcing `NativeToolPermissionPolicy` for the existing read-only workflow.

- [ ] **Step 5: Run the focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_tool_registry_exposes_canonical_agent_edit_tools -- --exact
just dev cargo test -p yach-backend agent_edit_tool_schema_rejects_expected_sha256_from_provider -- --exact
```

Expected: both pass.

Commit:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: define native agent edit tools"
```

---

### Task 2: Gate Provider Advertising For Canonical Edit Schemas

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing tests for policy-gated advertising**

Add tests near the existing provider advertising tests:

```rust
#[test]
fn provider_tool_advertising_builder_emits_canonical_agent_edit_schemas() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::edit_text_file(),
        NativeToolDefinition::create_text_file(),
    ])
    .unwrap();
    let advertising = parse_provider_tool_advertising_extensions(&[extension])
        .unwrap()
        .unwrap();

    let names = advertising
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["edit_text_file", "create_text_file"]);

    let edit = &advertising.tools[0];
    assert_eq!(
        edit.parameters["required"],
        serde_json::json!(["find", "path", "replace"])
    );
    assert!(edit.parameters["properties"].get("expected_sha256").is_none());
}

#[test]
fn provider_advertising_candidates_require_explicit_agent_edit_policy() {
    let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
    let no_edit_policy = NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
    let routable = ["project_path_info", "edit_text_file", "create_text_file"];

    let without_edits = registry.provider_advertising_candidates(&no_edit_policy, routable);
    assert_eq!(
        without_edits
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["project_path_info"]
    );

    let edit_policy = NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(
        ["project_path_info"],
        ["edit_text_file", "create_text_file"],
    );
    let with_edits = registry.provider_advertising_candidates(&edit_policy, routable);
    assert_eq!(
        with_edits
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["project_path_info", "edit_text_file", "create_text_file"]
    );
}

#[test]
fn provider_tool_advertising_rejects_noncanonical_mutation_tool() {
    let mut tool = NativeToolDefinition::edit_text_file();
    tool.name = String::from("write_text_file");

    assert_eq!(
        build_provider_tool_advertising_extension(&[tool]).err(),
        Some(ProviderToolAdvertisingError::UnsupportedTool {
            name: String::from("write_text_file")
        })
    );
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising_builder_emits_canonical_agent_edit_schemas -- --exact
just dev cargo test -p yach-backend provider_advertising_candidates_require_explicit_agent_edit_policy -- --exact
just dev cargo test -p yach-backend provider_tool_advertising_rejects_noncanonical_mutation_tool -- --exact
```

Expected: failures because mutation advertising and explicit edit policy are not implemented.

- [ ] **Step 3: Extend provider advertising eligibility without granting mutation execution**

Add an `allowed_agent_edit_tools: BTreeSet<String>` field and update constructors:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolPermissionPolicy {
    allowed_fixture_tools: BTreeSet<String>,
    allowed_project_metadata_tools: BTreeSet<String>,
    allowed_agent_edit_tools: BTreeSet<String>,
}
```

Add this constructor:

```rust
#[must_use]
pub fn allow_project_metadata_and_agent_edit_tools(
    metadata_names: impl IntoIterator<Item = impl Into<String>>,
    edit_names: impl IntoIterator<Item = impl Into<String>>,
) -> Self {
    Self {
        allowed_fixture_tools: BTreeSet::new(),
        allowed_project_metadata_tools: metadata_names.into_iter().map(Into::into).collect(),
        allowed_agent_edit_tools: edit_names.into_iter().map(Into::into).collect(),
    }
}
```

Do not update `authorize` to allow `NativeToolRisk::MutatesLocalState`. `authorize` is still the existing native tool execution gate and must keep returning `Denied` for mutating tools. The new `allowed_agent_edit_tools` set is only for provider advertising eligibility.

Add a separate helper used by `provider_advertising_candidates`:

```rust
pub fn allows_provider_advertising(&self, definition: &NativeToolDefinition) -> bool {
    match definition.risk {
        NativeToolRisk::ReadsLocalMetadata => {
            self.allowed_project_metadata_tools.contains(&definition.name)
        }
        NativeToolRisk::MutatesLocalState => {
            self.allowed_agent_edit_tools.contains(&definition.name)
        }
        NativeToolRisk::ReadsLocalContent
        | NativeToolRisk::RunsProcess
        | NativeToolRisk::UsesNetwork => false,
    }
}
```

Then update `provider_advertising_candidates` to call `allows_provider_advertising` instead of treating provider advertisement as runtime execution permission. Mutating tool calls must still enter through `validate_request_schema_only`, `NativePermissionDecisionEngine`, and `NativeEditAccess`.

- [ ] **Step 4: Extend provider advertising allowlist narrowly**

Change `project_provider_advertised_tool` so it accepts:

- canonical `project_path_info` with `ReadsLocalMetadata`;
- canonical built-in `edit_text_file` and `create_text_file` with `MutatesLocalState`;
- accepted extension metadata tools as they work today.

Use a helper:

```rust
fn is_canonical_builtin_provider_tool(tool: &NativeToolDefinition) -> bool {
    match tool.name.as_str() {
        "project_path_info" => {
            let canonical = NativeToolDefinition::project_path_info();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "edit_text_file" => {
            let canonical = NativeToolDefinition::edit_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "create_text_file" => {
            let canonical = NativeToolDefinition::create_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        _ => false,
    }
}
```

Keep duplicate-name and schema validation unchanged.

- [ ] **Step 5: Add Rig adapter approval tests**

In `crates/yach-backend/src/rig_adapter.rs`, add this test near the existing `rig_adapter_emits_project_path_info_tool_definition_from_advertising` test:

```rust
#[test]
fn rig_adapter_emits_agent_edit_tool_definitions_when_approved() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::edit_text_file(),
        NativeToolDefinition::create_text_file(),
    ])
    .unwrap();
    let request = provider_request_with_extensions(vec![extension]);

    let tools = rig_tool_definitions_from_request_with_approved_tools(
        &request,
        ["edit_text_file", "create_text_file"],
    )
    .unwrap();

    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        vec!["edit_text_file", "create_text_file"]
    );
    assert!(tools[0].parameters["properties"].get("expected_sha256").is_none());
}

#[test]
fn rig_adapter_default_approval_still_rejects_agent_edit_advertising() {
    let extension =
        build_provider_tool_advertising_extension(&[NativeToolDefinition::edit_text_file()])
            .unwrap();
    let request = provider_request_with_extensions(vec![extension]);

    let error = rig_tool_definitions_from_request(&request).err();

    assert!(matches!(
        error,
        Some(ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            ..
        })
    ));
}
```

- [ ] **Step 6: Add an explicit Rig approval seam and keep the default conservative**

Do not change `rig_tool_definitions_from_request`; it should keep approving only `project_path_info` by default.

Add a new helper in `rig_adapter.rs`:

```rust
pub async fn run_provider_request_with_approved_tools(
    config: RigProviderAdapterConfig,
    request: ProviderRequest,
    approved_tools: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<Vec<ProviderStreamEvent>, ProviderError>
```

This helper should share the existing `run_provider_request` body but call `rig_tool_definitions_from_request_with_approved_tools(&request, approved_tools)` instead of `rig_tool_definitions_from_request(&request)`. Then keep `run_provider_request(config, request)` as the default wrapper that passes `["project_path_info"]`.

This gives `native_runner.rs` a concrete way to opt the explicit agent-edit provider path into `["project_path_info", "edit_text_file", "create_text_file"]` without widening generic Rig requests.

- [ ] **Step 7: Run provider advertising and Rig adapter tests, then commit**

Run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
just dev cargo test -p yach-backend provider_advertising_candidates -- --nocapture
just dev cargo test -p yach-backend rig_adapter_emits_agent_edit_tool_definitions_when_approved -- --exact
just dev cargo test -p yach-backend rig_adapter_default_approval_still_rejects_agent_edit_advertising -- --exact
```

Expected: all provider advertising, candidate, and Rig approval tests pass.

Commit:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/rig_adapter.rs crates/yach-backend/src/lib.rs
git commit -m "feat: gate provider edit tool advertising"
```

---

### Task 3: Correlate Edit Evidence With Tool Request IDs

**Files:**
- Modify: `crates/yach-backend/src/edit_access.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write a failing edit access evidence test**

Add this helper near `temp_resource_dir` in `crates/yach-backend/src/lib.rs` so the edit access and agent edit tests have a writable project root:

```rust
struct TempNativeEditRoot {
    path: PathBuf,
}

impl TempNativeEditRoot {
    fn root(&self) -> &Path {
        &self.path
    }

    fn write(&self, relative_path: &str, content: &str) {
        let path = self.path.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TempNativeEditRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_native_edit_root(name: &str) -> TempNativeEditRoot {
    let path = std::env::temp_dir().join(format!(
        "yach-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    TempNativeEditRoot { path }
}
```

Then add this test near existing edit access tests:

```rust
#[test]
fn native_edit_access_records_tool_request_id_on_prepare_apply() {
    let root_guard = temp_native_edit_root("edit-access-tool-request-id");
    root_guard.write("notes.txt", "alpha\n");
    let resource_root = NativeResourceRoot::project(root_guard.root()).unwrap();
    let mut access = NativeEditAccess::default();
    let mut log = NativeSessionLog::default();
    let context = NativeEditAccessContext {
        session_id: NativeSessionId(String::from("session-1")),
        turn_id: NativeTurnId(String::from("turn-1")),
        permission_policy: NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        edit_policy: NativeEditPolicy::test(),
        tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
    };

    let preview = access
        .prepare(
            &resource_root,
            NativeEditTransactionRequest {
                operations: vec![NativeEditOperation::ModifyTextFile {
                    path: String::from("notes.txt"),
                    expected_sha256: sha256_hex_for_test("alpha\n"),
                    hunks: vec![NativeEditHunk {
                        find: String::from("alpha"),
                        replace: String::from("beta"),
                    }],
                }],
            },
            context,
            &mut log,
        )
        .unwrap();
    let _ = access
        .apply(&preview.preview_id, &preview.permission_decision_id, &mut log)
        .unwrap();

    assert!(log.events.iter().any(|event| matches!(
        event,
        NativeSessionEvent::EditTransactionPrepared {
            tool_request_id: Some(NativeToolRequestId(id)),
            ..
        } if id == "tool-request-1"
    )));
    assert!(log.events.iter().any(|event| matches!(
        event,
        NativeSessionEvent::EditTransactionFinished {
            tool_request_id: Some(NativeToolRequestId(id)),
            outcome: NativeEditEvidenceOutcome::Completed,
            ..
        } if id == "tool-request-1"
    )));
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_edit_access_records_tool_request_id_on_prepare_apply -- --exact
```

Expected: fails because `NativeEditAccessContext` has no `tool_request_id`.

- [ ] **Step 3: Add `tool_request_id` to `NativeEditAccessContext`**

Update the struct:

```rust
pub struct NativeEditAccessContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub permission_policy: NativePermissionPolicy,
    pub edit_policy: NativeEditPolicy,
    pub tool_request_id: Option<NativeToolRequestId>,
}
```

Replace all `tool_request_id: None::<NativeToolRequestId>` evidence writes in `edit_access.rs` with `context.tool_request_id.clone()` or `pending.context.tool_request_id.clone()`.

Update local edit call sites in `native_runner.rs` to set:

```rust
tool_request_id: None,
```

- [ ] **Step 4: Run local edit and edit access tests, then commit**

Run:

```bash
just dev cargo test -p yach-backend native_edit_access_records_tool_request_id_on_prepare_apply -- --exact
just dev cargo test -p yach-backend native_runner_prepares_and_applies_local_edit -- --exact
just dev cargo test -p yach-backend native_runner_does_not_apply_when_local_edit_evidence_preflight_fails -- --exact
```

Expected: all pass.

Commit:

```bash
git add crates/yach-backend/src/edit_access.rs crates/yach-backend/src/native_runner.rs crates/yach-backend/src/lib.rs
git commit -m "feat: correlate edit evidence with tool requests"
```

---

### Task 4: Add Agent Edit Tool Normalization And Bounded Results

**Files:**
- Create: `crates/yach-backend/src/agent_edit_tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/edit.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing normalization tests**

Add these tests in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn agent_edit_text_file_normalization_computes_expected_hash() {
    let root_guard = temp_native_edit_root("agent-edit-normalize-modify");
    root_guard.write("notes.txt", "alpha\n");
    let resource_root = NativeResourceRoot::project(root_guard.root()).unwrap();
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let normalized = normalize_agent_edit_tool_request(&registry, &resource_root, &request, NativeEditPolicy::test())
        .unwrap();

    assert!(matches!(
        normalized.transaction.operations.as_slice(),
        [NativeEditOperation::ModifyTextFile {
            path,
            expected_sha256,
            hunks
        }] if path == "notes.txt"
            && expected_sha256 == &sha256_hex_for_test("alpha\n")
            && hunks.len() == 1
            && hunks[0].find == "alpha"
            && hunks[0].replace == "beta"
    ));
}

#[test]
fn agent_create_text_file_normalization_builds_create_transaction() {
    let root_guard = temp_native_edit_root("agent-edit-normalize-create");
    let resource_root = NativeResourceRoot::project(root_guard.root()).unwrap();
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("create_text_file"),
        provider_call_id: Some(String::from("call-create-1")),
        arguments: serde_json::json!({
            "path": "new.txt",
            "content": "created\n"
        }),
    };

    let normalized = normalize_agent_edit_tool_request(&registry, &resource_root, &request, NativeEditPolicy::test())
        .unwrap();

    assert!(matches!(
        normalized.transaction.operations.as_slice(),
        [NativeEditOperation::CreateTextFile { path, content }]
            if path == "new.txt" && content == "created\n"
    ));
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_text_file_normalization_computes_expected_hash -- --exact
just dev cargo test -p yach-backend agent_create_text_file_normalization_builds_create_transaction -- --exact
```

Expected: fails because `agent_edit_tools.rs` and normalization do not exist.

- [ ] **Step 3: Create `agent_edit_tools.rs`**

Add a focused module with these public shapes:

```rust
use crate::{
    NativeEditHunk, NativeEditOperation, NativeEditPolicy, NativeEditTransactionRequest,
    NativeResourceReadPolicy, NativeResourceRoot, NativeToolError, NativeToolRegistry,
    PendingNativeToolRequest, native_edit_sha256_hex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAgentEditToolRequest {
    pub transaction: NativeEditTransactionRequest,
    pub path: String,
    pub operation: String,
}

pub fn normalize_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
    edit_policy: NativeEditPolicy,
) -> Result<NormalizedAgentEditToolRequest, NativeToolError> {
    let definition = registry.validate_request_schema_only(request)?;
    match definition.name.as_str() {
        "edit_text_file" => normalize_edit_text_file(root, request, edit_policy),
        "create_text_file" => normalize_create_text_file(request),
        _ => Err(NativeToolError::UnknownTool),
    }
}
```

Implement `normalize_edit_text_file` by reading the local file with:

```rust
let read = root
    .read_text_file(path, NativeResourceReadPolicy::local_only(edit_policy.max_file_bytes))
    .map_err(|_| NativeToolError::MalformedArguments)?;
let expected_sha256 = native_edit_sha256_hex(read.text.as_bytes());
```

Return a single-operation `NativeEditTransactionRequest`.

- [ ] **Step 4: Expose a crate-level hash helper without exposing apply**

In `crates/yach-backend/src/edit.rs`, rename or wrap the private hash helper:

```rust
pub(crate) fn native_edit_sha256_hex(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}
```

Keep `sha256_hex_for_test` unchanged for tests.

In `crates/yach-backend/src/lib.rs`, add:

```rust
mod agent_edit_tools;
pub use agent_edit_tools::*;
```

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_text_file_normalization_computes_expected_hash -- --exact
just dev cargo test -p yach-backend agent_create_text_file_normalization_builds_create_transaction -- --exact
```

Expected: both pass.

Commit:

```bash
git add crates/yach-backend/src/agent_edit_tools.rs crates/yach-backend/src/edit.rs crates/yach-backend/src/lib.rs
git commit -m "feat: normalize native agent edit requests"
```

---

### Task 5: Execute Agent Edit Tools Through NativeEditAccess

**Files:**
- Modify: `crates/yach-backend/src/agent_edit_tools.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing workflow tests for allow and ask modes**

Add backend tests:

```rust
#[test]
fn agent_edit_tool_allow_mode_applies_and_preserves_provider_call_id() {
    let root_guard = temp_native_edit_root("agent-edit-allow");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).unwrap();
    let store_path = root_guard.root().join("session.jsonl");
    let store = NativeJsonlSessionStore::new(store_path.clone());
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let result = execute_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    )
    .unwrap();

    assert_eq!(result.provider_call_id.as_deref(), Some("call-edit-1"));
    assert_eq!(result.status, NativeToolOutcome::Completed);
    assert_eq!(std::fs::read_to_string(root_guard.root().join("notes.txt")).unwrap(), "beta\n");

    let log = NativeJsonlSessionStore::new(store_path).load().unwrap();
    assert!(events_are_ordered_before_completed_apply(&log.events));
}

#[test]
fn agent_edit_tool_ask_mode_returns_review_without_applying() {
    let root_guard = temp_native_edit_root("agent-edit-ask");
    root_guard.write("notes.txt", "alpha\n");
    let root = NativeResourceRoot::project(root_guard.root()).unwrap();
    let store = NativeJsonlSessionStore::new(root_guard.root().join("session.jsonl"));
    let registry = NativeToolRegistry::with_agent_edit_tools();
    let mut access = NativeEditAccess::default();
    let request = PendingNativeToolRequest {
        request_id: String::from("tool-request-1"),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        arguments: serde_json::json!({
            "path": "notes.txt",
            "find": "alpha",
            "replace": "beta"
        }),
    };

    let outcome = prepare_agent_edit_tool_request(
        &registry,
        &root,
        &mut access,
        &store,
        NativeAgentEditToolContext {
            session_id: NativeSessionId(String::from("default")),
            turn_id: NativeTurnId(String::from("turn-1")),
            permission_policy: NativePermissionPolicy::default_local_edit(),
            edit_policy: NativeEditPolicy::test(),
        },
        request,
    )
    .unwrap();

    assert!(matches!(outcome, NativeAgentEditToolPrepared::NeedsUserReview { .. }));
    assert_eq!(std::fs::read_to_string(root_guard.root().join("notes.txt")).unwrap(), "alpha\n");
}
```

Use a local helper in the test module for ordering:

```rust
fn events_are_ordered_before_completed_apply(events: &[NativeSessionEvent]) -> bool {
    let tool_request = events.iter().position(|event| matches!(event, NativeSessionEvent::ToolRequestRecorded { .. }));
    let permission = events.iter().position(|event| matches!(event, NativeSessionEvent::PermissionDecisionRecorded { .. }));
    let prepared = events.iter().position(|event| matches!(event, NativeSessionEvent::EditTransactionPrepared { .. }));
    let apply_started = events.iter().position(|event| matches!(
        event,
        NativeSessionEvent::EditTransactionFinished {
            outcome: NativeEditEvidenceOutcome::ApplyStarted,
            ..
        }
    ));
    let completed = events.iter().position(|event| matches!(
        event,
        NativeSessionEvent::EditTransactionFinished {
            outcome: NativeEditEvidenceOutcome::Completed,
            ..
        }
    ));
    matches!(
        (tool_request, permission, prepared, apply_started, completed),
        (Some(a), Some(b), Some(c), Some(d), Some(e)) if a < b && b < c && c < d && d < e
    )
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_tool_allow_mode_applies_and_preserves_provider_call_id -- --exact
just dev cargo test -p yach-backend agent_edit_tool_ask_mode_returns_review_without_applying -- --exact
```

Expected: failures because execution workflow types do not exist.

- [ ] **Step 3: Add workflow types**

In `agent_edit_tools.rs`, add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentEditToolContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
    pub permission_policy: NativePermissionPolicy,
    pub edit_policy: NativeEditPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeAgentEditToolPrepared {
    Completed(NativeProviderToolResult),
    Denied(NativeProviderToolResult),
    NeedsUserReview {
        request_id: String,
        provider_call_id: String,
        preview: NativeEditPreview,
        path: String,
        operation: String,
    },
}
```

- [ ] **Step 3b: Add result finalization helpers for paused reviews**

Also add a `PendingAgentEditToolReview` shape that keeps everything needed to finish a paused provider tool call after the user decision:

```rust
#[derive(Debug)]
pub struct PendingAgentEditToolReview {
    pub request_id: String,
    pub provider_call_id: String,
    pub preview_id: NativeEditPreviewId,
    pub permission_decision_id: NativePermissionDecisionId,
    pub path: String,
    pub operation: String,
}
```

Add functions:

```rust
pub fn apply_agent_edit_tool_review(
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<NativeProviderToolResult, NativeToolContinuationError>

pub fn reject_agent_edit_tool_review(
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    pending: PendingAgentEditToolReview,
) -> Result<NativeProviderToolResult, NativeToolContinuationError>
```

These should preserve `provider_call_id`, append final tool execution evidence, and return bounded provider continuation results. Use `NativeToolOutcome::Completed` as the provider transport status for successfully handled tool calls, including user rejection, and put the semantic outcome in the bounded JSON content as `"outcome": "applied"`, `"outcome": "rejected"`, or `"outcome": "denied"`. Reserve `NativeToolOutcome::Denied` for policy denial before a provider continuation should be attempted.

Add a focused continuation mapping test in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn provider_continuation_accepts_agent_edit_rejection_as_completed_transport_result() {
    let content = serde_json::json!({
        "outcome": "rejected",
        "tool_request_id": "tool-request-1",
        "path": "notes.txt"
    })
    .to_string();
    let request = ProviderContinuationRequest {
        turn_id: NativeTurnId(String::from("turn-1")),
        model: ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        },
        prior_messages: Vec::new(),
        tool_results: vec![NativeProviderToolResult {
            tool_request_id: String::from("tool-request-1"),
            provider_call_id: Some(String::from("call-edit-1")),
            status: NativeToolOutcome::Completed,
            byte_count: content.len(),
            content,
            redacted: true,
            truncated: false,
            reason: Some(String::from("user_rejected")),
        }],
        extensions: Vec::new(),
    };

    let submission = build_provider_continuation_submission(
        &request,
        ProviderContinuationValidationPolicy::strict_tool_results(512),
    )
    .unwrap();

    assert_eq!(submission.tool_results[0].status, NativeToolOutcome::Completed);
    assert!(submission.tool_results[0].content.contains("\"outcome\":\"rejected\""));
}
```

- [ ] **Step 4: Implement prepare and allow-mode execution**

Implement:

```rust
pub fn prepare_agent_edit_tool_request(
    registry: &NativeToolRegistry,
    root: &NativeResourceRoot,
    edit_access: &mut NativeEditAccess,
    sink: &impl NativeSessionEventSink,
    context: NativeAgentEditToolContext,
    request: PendingNativeToolRequest,
) -> Result<NativeAgentEditToolPrepared, NativeToolContinuationError>
```

Required behavior:

- call `registry.validate_request_schema_only(&request)`;
- append `ToolRequestRecorded` with redacted argument summary;
- normalize into `NativeEditTransactionRequest`;
- call `NativeEditAccess::prepare` with `tool_request_id: Some(NativeToolRequestId(request.request_id.clone()))`;
- append all prepare-side events to `sink` before returning;
- if review state is `Allowed`, call `apply_with_evidence_sink` immediately and return a bounded `NativeProviderToolResult`;
- if review state is `NeedsUserApproval` or `AutoReviewUnavailable`, return `NeedsUserReview`;
- if permission denied before a provider continuation is valid, append `ToolExecutionFinished` with `Denied` and return `NativeAgentEditToolPrepared::Denied`;
- if the user rejects a prepared review, append final edit and tool evidence, then return a completed transport result whose content has `"outcome": "rejected"` and whose `reason` is `Some("user_rejected")`;
- preserve `provider_call_id`.

Use bounded JSON content like:

```rust
serde_json::json!({
    "outcome": "applied",
    "tool_request_id": request.request_id,
    "preview_id": preview.preview_id.0,
    "transaction_id": preview.transaction_id.0,
    "operation": operation,
    "path": path,
    "diff_summary_truncated": preview.diff_summary_truncated
})
.to_string()
```

Do not include full file content or raw arguments.

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend agent_edit_tool_allow_mode_applies_and_preserves_provider_call_id -- --exact
just dev cargo test -p yach-backend agent_edit_tool_ask_mode_returns_review_without_applying -- --exact
just dev cargo test -p yach-backend provider_continuation_accepts_agent_edit_rejection_as_completed_transport_result -- --exact
```

Expected: all pass.

Commit:

```bash
git add crates/yach-backend/src/agent_edit_tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: execute native agent edit tools"
```

---

### Task 6: Add Generic Tool Review Protocol And UI Handling

**Files:**
- Modify: `crates/yach-proto/src/lib.rs`
- Modify: `crates/yach-ui/src/app.rs`
- Test: `crates/yach-proto/src/lib.rs`
- Test: `crates/yach-ui/src/app.rs`

- [ ] **Step 1: Write failing protocol round-trip tests**

In `crates/yach-proto/src/lib.rs`, add:

```rust
#[test]
fn tool_review_events_round_trip_as_jsonl() {
    let review = ServerEvent::ToolReviewRequested {
        request_id: String::from("tool-review-request-1"),
        tool_name: String::from("edit_text_file"),
        payload: ToolReviewPayload::LocalEdit {
            preview: LocalEditPreviewSummary {
                preview_id: String::from("preview-1"),
                transaction_id: String::from("transaction-1"),
                permission_decision_id: String::from("permission-1"),
                path: String::from("notes.txt"),
                operation: String::from("modify_text_file"),
                review_state: LocalEditReviewState::NeedsUserApproval,
                diff_summary: String::from("--- notes.txt\n+++ notes.txt\n"),
                diff_summary_truncated: false,
            },
        },
    };
    let line = review.to_jsonl().unwrap();
    assert!(line.contains("\"type\":\"tool_review_requested\""));
    assert_eq!(ServerEvent::from_jsonl(&line).unwrap(), review);

    let decision = ClientEvent::ToolReviewDecisionSubmitted {
        request_id: String::from("tool-review-request-1"),
        preview_id: String::from("preview-1"),
        permission_decision_id: String::from("permission-1"),
        decision: LocalEditDecision::Apply,
    };
    let line = decision.to_jsonl().unwrap();
    assert!(line.contains("\"type\":\"tool_review_decision_submitted\""));
    assert_eq!(ClientEvent::from_jsonl(&line).unwrap(), decision);
}
```

- [ ] **Step 2: Add UI tests for unsolicited tool review**

In `crates/yach-ui/src/app.rs`, add:

```rust
#[test]
fn tool_review_enters_review_mode_without_local_request() {
    let (mut app, _rx) = test_app();

    app.handle_server_event(ServerEvent::ToolReviewRequested {
        request_id: String::from("tool-review-request-1"),
        tool_name: String::from("edit_text_file"),
        payload: ToolReviewPayload::LocalEdit {
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        },
    });

    assert!(matches!(app.mode, AppMode::LocalEditReview { .. }));
    assert_eq!(app.pending_tool_review_request_id.as_deref(), Some("tool-review-request-1"));
}

#[test]
fn tool_review_emits_tool_decision_event() {
    let (mut app, mut rx) = test_app();
    app.handle_server_event(ServerEvent::ToolReviewRequested {
        request_id: String::from("tool-review-request-1"),
        tool_name: String::from("edit_text_file"),
        payload: ToolReviewPayload::LocalEdit {
            preview: local_edit_preview(LocalEditReviewState::NeedsUserApproval),
        },
    });

    app.handle_local_edit_review_key(KeyCode::Char('a'), KeyModifiers::NONE);

    assert!(matches!(
        rx.try_recv(),
        Ok(ClientEvent::ToolReviewDecisionSubmitted {
            request_id,
            decision: LocalEditDecision::Apply,
            ..
        }) if request_id == "tool-review-request-1"
    ));
}
```

- [ ] **Step 3: Run tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-proto tool_review_events_round_trip_as_jsonl -- --exact
just dev cargo test -p yach-ui tool_review -- --nocapture
```

Expected: failures because events and UI state do not exist.

- [ ] **Step 4: Add protocol events**

Add variants:

```rust
ClientEvent::ToolReviewDecisionSubmitted {
    request_id: String,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
}

ServerEvent::ToolReviewRequested {
    request_id: String,
    tool_name: String,
    payload: ToolReviewPayload,
}

pub enum ToolReviewPayload {
    LocalEdit {
        preview: LocalEditPreviewSummary,
    },
}
```

Keep the existing `LocalEdit*` variants unchanged for `/debug-edit`.

- [ ] **Step 5: Reuse the local edit review UI for tool review payloads**

In `App`, add:

```rust
pending_tool_review_request_id: Option<String>,
active_tool_review_preview_id: Option<String>,
```

When receiving `ServerEvent::ToolReviewRequested` with `ToolReviewPayload::LocalEdit`, enter `AppMode::LocalEditReview` with the provided preview even if there is no pending local edit request. Set `pending_tool_review_request_id` and `active_tool_review_preview_id`.

When `submit_local_edit_review` runs:

- if `active_tool_review_preview_id` matches the preview, send `ClientEvent::ToolReviewDecisionSubmitted`;
- otherwise keep the current `ClientEvent::LocalEditDecisionSubmitted` behavior.

When any edit finishes or the decision is submitted, clear the relevant local or tool-review pending IDs.

- [ ] **Step 6: Run proto and UI tests, then commit**

Run:

```bash
just dev cargo test -p yach-proto tool_review_events_round_trip_as_jsonl -- --exact
just dev cargo test -p yach-ui tool_review -- --nocapture
just dev cargo test -p yach-ui local_edit -- --nocapture
```

Expected: all pass, and existing local edit tests still pass.

Commit:

```bash
git add crates/yach-proto/src/lib.rs crates/yach-ui/src/app.rs
git commit -m "feat: add tool review protocol"
```

---

### Task 7: Bridge Provider Tool Calls Through Agent Edit Review

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/agent_edit_tools.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`
- Test: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write a failing native runner test for ask-mode agent edit review**

Add a test near the native provider one-round tests in `crates/yach-backend/src/native_runner.rs`:

```rust
#[test]
fn native_provider_agent_edit_tool_pauses_for_user_review_and_continues() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = TempProject::new("native-provider-agent-edit-review");
        root.write("notes.txt", "alpha\n");
        let session_path = root.root().join("session.jsonl");
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let provider = FakeProviderRequester::with_responses([
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::ToolCallCompleted {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    tool_call: ProviderToolCall {
                        call_id: String::from("call-edit-1"),
                        name: String::from("edit_text_file"),
                        arguments_json: serde_json::json!({
                            "path": "notes.txt",
                            "find": "alpha",
                            "replace": "beta"
                        }),
                    },
                },
                ProviderStreamEvent::Completed {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::ToolCalls),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
            Ok(vec![
                ProviderStreamEvent::Started {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    model: ProviderModel {
                        provider: String::from("fixture"),
                        model: String::from("fixture-model"),
                    },
                },
                ProviderStreamEvent::TextDelta {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    delta: String::from("edit applied"),
                },
                ProviderStreamEvent::Completed {
                    turn_id: NativeTurnId(String::from("turn-1")),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    usage: None,
                    provider_response_id: None,
                },
            ]),
        ]);

        let handle = tokio::spawn(super::run_native_dogfood_loop_with_provider_requester(
            client_rx,
            backend_tx,
            super::NativeDogfoodRunnerConfig {
                session_path: session_path.clone(),
                project_root: Some(root.root().to_path_buf()),
                provider: Some(native_provider_test_config()),
            },
            provider,
        ));

        client_tx
            .send(ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: String::from("change alpha to beta"),
            })
            .unwrap();

        let review = recv_tool_review(&mut backend_rx).await.unwrap();
        let ToolReviewPayload::LocalEdit { preview } = review.payload;
        client_tx
            .send(ClientEvent::ToolReviewDecisionSubmitted {
                request_id: review.request_id,
                preview_id: preview.preview_id,
                permission_decision_id: preview.permission_decision_id,
                decision: LocalEditDecision::Apply,
            })
            .unwrap();

        let finished = recv_prompt_finished(&mut backend_rx).await.unwrap();
        assert_eq!(finished, PromptOutcome::Completed);
        assert_eq!(std::fs::read_to_string(root.root().join("notes.txt")).unwrap(), "beta\n");

        let log = NativeJsonlSessionStore::new(session_path).load().unwrap();
        assert!(log.events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::ToolRequestRecorded {
                provider_call_id: Some(id),
                tool_name,
                ..
            } if id == "call-edit-1" && tool_name == "edit_text_file"
        )));

        drop(client_tx);
        assert!(handle.await.is_ok());
    });
}
```

Also add a denial-path test next to it:

```rust
#[test]
fn native_provider_agent_edit_tool_denial_does_not_continue_provider_round() {
    let mut requester = FakeProviderRequester::with_responses([
        Ok(vec![
            ProviderStreamEvent::Started {
                turn_id: NativeTurnId(String::from("turn-1")),
                model: ProviderModel {
                    provider: String::from("fixture"),
                    model: String::from("fixture-model"),
                },
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_call: ProviderToolCall {
                    call_id: String::from("call-edit-1"),
                    name: String::from("edit_text_file"),
                    arguments_json: serde_json::json!({
                        "path": ".yach/APPEND_SYSTEM.md",
                        "find": "old",
                        "replace": "new"
                    }),
                },
            },
            ProviderStreamEvent::Completed {
                turn_id: NativeTurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                usage: None,
                provider_response_id: None,
            },
        ]),
    ]);
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    let root_guard = temp_native_edit_root("agent-edit-denied");
    let resource_root = NativeResourceRoot::project(root_guard.root()).unwrap();

    let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
        &mut requester,
        ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        },
        &mut log,
        &mut pending_events,
        &NativeTurnId(String::from("turn-1")),
        Some(NativeLaunchProjectContext::from_project_root(resource_root)),
        None,
        mpsc::unbounded_channel().0,
        mpsc::unbounded_channel().1,
    ));

    assert!(matches!(result, Err(NativeProviderRoundError::ToolExecutionDenied { .. })));
    assert_eq!(requester.requests.len(), 1);
}
```

Add a small test-only `run_native_dogfood_loop_with_provider_requester` seam if one does not already exist. It should mirror `run_native_dogfood_loop` but accept an injected `impl ProviderRequester`, matching the existing `FakeProviderRequester` test pattern used by native provider one-round tests.

Add this test-only config helper in the same module so the injected requester can bypass the adapter while the runner still has provider metadata:

```rust
fn native_provider_test_config() -> NativeProviderDogfoodConfig {
    NativeProviderDogfoodConfig {
        adapter: RigProviderAdapterConfig {
            provider: RigProviderConfig::Anthropic {
                api_key: String::from("test-key"),
            },
            timeout: Duration::from_secs(30),
            max_tokens: 1000,
        },
        model: String::from("fixture-model"),
        test_delay_ms: None,
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_agent_edit_tool_pauses_for_user_review_and_continues -- --exact
```

Expected: fails because the runner does not advertise edit tools or bridge agent edit decisions.

- [ ] **Step 3: Add an internal review bridge**

Add internal types in `native_runner.rs`:

```rust
#[derive(Debug)]
struct ActiveProviderTurn {
    handle: tokio::task::JoinHandle<()>,
    turn_id: NativeTurnId,
    prompt_started: Instant,
    review_decision_tx: mpsc::UnboundedSender<AgentEditReviewDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentEditReviewDecision {
    request_id: String,
    preview_id: String,
    permission_decision_id: String,
    decision: LocalEditDecision,
}

type AgentEditDecisionReceiver = mpsc::UnboundedReceiver<AgentEditReviewDecision>;
```

Replace `active_provider_turn: Option<(JoinHandle, NativeTurnId, Instant)>` with `Option<ActiveProviderTurn>`. When spawning the provider task, create an unbounded `AgentEditReviewDecision` channel, store the sender in `ActiveProviderTurn`, and pass the receiver to `handle_started_native_provider_prompt`.

The provider task should send `ServerEvent::ToolReviewRequested { payload: ToolReviewPayload::LocalEdit { .. } }` and then wait on `AgentEditDecisionReceiver` for a matching request ID, preview ID, and permission decision ID. The main event loop should handle `ClientEvent::ToolReviewDecisionSubmitted` by forwarding an `AgentEditReviewDecision` through `active_provider_turn.review_decision_tx`; if there is no active provider turn or the send fails, emit a bounded status update and ignore the stale decision.

On prompt cancellation or provider task completion, drop the stored sender with the active turn. That unblocks the provider helper with a cancelled/stale review error instead of leaving the task waiting.

Do not reuse the `/debug-edit` `NativeEditAccess` instance for provider edit calls. The provider task should own the `NativeEditAccess` used for provider-originated previews so apply/reject happens in the same task that can continue the provider round.

- [ ] **Step 4: Add a provider agent edit tool round helper**

Add a denial error variant to `NativeProviderRoundError`:

```rust
ToolExecutionDenied {
    tool_request_id: String,
    tool_name: String,
    reason: String,
}
```

Map it in `native_provider_round_error_to_provider_error` to `ProviderErrorKind::InvalidRequest` with redacted debug `Some(format!("tool_execution_denied:{tool_name}:{tool_request_id}:{reason}"))`.

Then add a new helper alongside `run_native_provider_one_readonly_tool_round`:

```rust
async fn run_native_provider_one_agent_tool_round(
    requester: &mut impl ProviderRequester,
    model: ProviderModel,
    log: &mut NativeSessionLog,
    pending_events: &mut Vec<NativeSessionEvent>,
    turn_id: &NativeTurnId,
    project_context: Option<NativeLaunchProjectContext>,
    tool_event_store: Option<&NativeJsonlSessionStore>,
    review_tx: mpsc::UnboundedSender<BackendEvent>,
    review_decisions: AgentEditDecisionReceiver,
) -> Result<NativeProviderRoundResult, NativeProviderRoundError>
```

Inside the helper:

- use `NativeToolRegistry::with_project_read_only_and_agent_edit_tools()`;
- use `NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(["project_path_info"], ["edit_text_file", "create_text_file"])`;
- advertise `["project_path_info", "edit_text_file", "create_text_file"]`;
- route `project_path_info` calls through the existing read-only workflow;
- route edit calls through `prepare_agent_edit_tool_request`;
- if review is needed, send `ServerEvent::ToolReviewRequested`, wait for the decision, apply or reject through `NativeEditAccess`, then build the bounded tool result;
- if `prepare_agent_edit_tool_request` returns `NativeAgentEditToolPrepared::Denied`, append the denied evidence and finish the prompt as failed or cancelled without attempting a provider continuation;
- append pending events before any provider continuation;
- strip advertising from continuation requests.

When applying Rig tool definitions for this provider path, use the `run_provider_request_with_approved_tools` seam from Task 2 so the live `RigProviderRequester` passes `["project_path_info", "edit_text_file", "create_text_file"]` to the adapter-side tool definition builder. Do not widen the default `rig_tool_definitions_from_request` approval list.

Keep `run_native_provider_one_readonly_tool_round` available for existing tests if that makes the refactor smaller.

- [ ] **Step 5: Wire `handle_native_provider_prompt`**

Change `handle_native_provider_prompt` to call the new agent tool round helper instead of `run_native_provider_one_readonly_tool_round`.

Update `RigProviderRequester` to carry the explicit approval list:

```rust
struct RigProviderRequester {
    adapter: RigProviderAdapterConfig,
    approved_tools: Vec<String>,
}

impl ProviderRequester for RigProviderRequester {
    fn request(
        &mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        let adapter = self.adapter.clone();
        let approved_tools = self.approved_tools.clone();
        Box::pin(async move {
            run_provider_request_with_approved_tools(adapter, request, approved_tools).await
        })
    }
}
```

Initialize it in the agent edit path with `approved_tools: vec!["project_path_info".into(), "edit_text_file".into(), "create_text_file".into()]`. Existing read-only tests can keep using fake requesters or construct `RigProviderRequester` with only `project_path_info`.

Use `NativePermissionPolicy::default_local_edit()` for edit permissions in the first implementation so provider-originated edits pause for user review by default. Do not default provider-originated edits to `Allow`.

- [ ] **Step 6: Run native provider tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_provider_one_round_executes_project_path_info_and_continues -- --exact
just dev cargo test -p yach-backend native_provider_agent_edit_tool_pauses_for_user_review_and_continues -- --exact
just dev cargo test -p yach-backend native_provider_agent_edit_tool_denial_does_not_continue_provider_round -- --exact
just dev cargo test -p yach-backend native_provider_one_round_rejects_second_round_tool_calls -- --exact
```

Expected: all pass.

Commit:

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-backend/src/agent_edit_tools.rs
git commit -m "feat: route native provider edit tools"
```

---

### Task 8: Preserve Provider Replay And Extension Boundaries

**Files:**
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Test: `crates/yach-backend/src/lib.rs`
- Test: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Add regression tests for transcript projection and extension mutation rejection**

Add tests:

```rust
#[test]
fn native_provider_messages_ignore_agent_edit_evidence() {
    let mut log = NativeSessionLog::default();
    log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_request_id: NativeToolRequestId(String::from("tool-request-1")),
        tool_name: String::from("edit_text_file"),
        provider_call_id: Some(String::from("call-edit-1")),
        validation: Ok(()),
        permission: NativeToolPermissionState::Allowed,
        argument_summary: NativeToolPayloadSummary {
            summary: String::from("tool payload redacted"),
            byte_count: 42,
            redacted: true,
            truncated: false,
        },
    });
    log.push(NativeSessionEvent::EditTransactionPrepared {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
        tool_request_id: Some(NativeToolRequestId(String::from("tool-request-1"))),
        transaction_id: NativeEditTransactionId(String::from("edit-1")),
        summary: native_edit_summary_fixture(),
    });

    let messages = native_provider_messages_from_log(&log);
    let rendered = format!("{messages:?}");
    assert!(!rendered.contains("edit_text_file"));
    assert!(!rendered.contains("call-edit-1"));
    assert!(!rendered.contains("tool-request-1"));
}

#[test]
fn extension_mutation_tool_registration_still_rejected() {
    let mut registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
    let mut extension_tool = NativeToolDefinition::extension_metadata_tool(
        "example.extension",
        "extension_edit_text_file",
        "tries to edit files",
        NativeToolInputSchema::string_object(["path"], std::iter::empty::<&str>(), 1024),
        ProviderToolVisibility::Visible,
    );
    extension_tool.risk = NativeToolRisk::MutatesLocalState;

    assert_eq!(
        registry.register_extension_tool(extension_tool).err(),
        Some(NativeToolRegistrationError::UnsupportedRisk {
            name: String::from("extension_edit_text_file"),
            risk: NativeToolRisk::MutatesLocalState
        })
    );
}
```

Use existing fixture helpers for `native_edit_summary_fixture()` or add a tiny local helper that returns a redacted `NativeEditEvidenceSummary`.

- [ ] **Step 2: Run tests and verify failures if coverage is missing**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_ignore_agent_edit_evidence -- --exact
just dev cargo test -p yach-backend extension_mutation_tool_registration_still_rejected -- --exact
```

Expected: either pass immediately if existing behavior already covers them, or fail until imports/helpers are added. If they pass immediately, keep the tests as regression coverage.

- [ ] **Step 3: Fix projections only if needed**

If transcript projection includes edit evidence, update `native_provider_messages_from_log` and related projections in `native_runner.rs` to ignore:

- `ToolRequestRecorded`;
- `ToolExecutionFinished`;
- `PermissionDecisionRecorded`;
- `EditTransactionPrepared`;
- `EditTransactionFinished`.

These events should stay durable evidence, not provider transcript content.

- [ ] **Step 4: Run regression tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_ignore_agent_edit_evidence -- --exact
just dev cargo test -p yach-backend extension_mutation_tool_registration_still_rejected -- --exact
```

Expected: both pass.

Commit:

```bash
git add crates/yach-backend/src/lib.rs crates/yach-backend/src/native_runner.rs
git commit -m "test: preserve agent edit safety boundaries"
```

---

### Task 9: Update Active Planning And Run Final Verification

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update active project state**

In `docs/project/state.md`, replace the paragraph about the proposed native agent edit tool surface with a merged/implemented summary:

```markdown
The native agent edit tool surface implementation now provides policy-gated
provider-visible canonical `edit_text_file` and `create_text_file` schemas for
the native-provider path. Provider-originated edit calls route through
yach-owned schema validation, permission routing, `NativeEditAccess`
preview/apply/reject, redacted tool/edit evidence with provider-call
correlation, and bounded provider continuation results. The temporary
`/debug-edit` harness remains a manual local test surface, not the product edit
surface.
```

Keep explicit not-sufficient language for broader mutation:

```markdown
This is not sufficient for broad write/patch/delete/rename tools,
extension-owned mutation, shell/process tools, network tools, sandboxing, or a
working auto-review runtime.
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, recommend the next slice as production edit tracing or read/search content exposure, depending on what the implementation reveals:

```markdown
Recommended next move: design production edit tracing for agent edit operations.

Why: provider-originated edit tools now have concrete request, review, apply,
and continuation states. Durable trace IDs and performance timings should be
designed around those real states before broader mutation or read/search content
tools expand the surface.
```

If implementation leaves provider-visible read/search content as the stronger blocker for real usefulness, name that explicitly as the near-term alternative.

- [ ] **Step 3: Run focused verification**

Run:

```bash
just dev cargo test -p yach-backend agent_edit -- --nocapture
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
just dev cargo test -p yach-proto tool_review -- --nocapture
just dev cargo test -p yach-ui tool_review -- --nocapture
just dev cargo test -p yach-ui local_edit -- --nocapture
```

Expected: all pass.

- [ ] **Step 4: Run workspace verification**

Run:

```bash
just test
just lint
```

Expected: both pass. If `just lint` reports Clippy diagnostics, fix them rather than bypassing hooks.

- [ ] **Step 5: Commit docs and final fixes**

Commit:

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update native agent edit next work"
```

If final verification forced code fixes after the docs commit, make a separate targeted fix commit with the smallest accurate message.

---

## Plan Self-Review

- Spec coverage: Tasks 1-2 cover canonical provider-visible schemas and policy gating. Tasks 3-5 cover `NativeEditAccess`, yach-computed `expected_sha256`, bounded results, provider call IDs, and pre-mutation evidence. Tasks 6-7 cover TUI/protocol review pause and continuation. Task 8 preserves provider replay and extension boundaries. Task 9 updates handoff.
- Open-slot scan: no unspecified implementation slots remain; every task has concrete files, tests, commands, and expected outcomes.
- Type consistency: this plan consistently uses `edit_text_file`, `create_text_file`, `NativeToolRisk::MutatesLocalState`, `NativePermissionCapability::EditTransaction`, `NativeEditAccess`, `NativeToolRequestId`, `ProviderToolVisibility::Visible`, and `NativeProviderToolResult`.
