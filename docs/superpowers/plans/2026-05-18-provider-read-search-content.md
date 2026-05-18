# Provider-Visible Read/Search Content Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add provider-visible `read_text_file`, `search_project`, and `list_project_paths` tools so native-provider agents can inspect bounded project content before using existing edit tools.

**Architecture:** Reuse the existing yach-owned native tool registry, schema-only provider advertising, `ProjectReadOnlyToolExecutor`, `NativeResourceRoot`, redacted session evidence, and one-round provider continuation path. Add a separate `ReadsLocalContent` policy allowlist and bounded provider-content result shaping; do not add mutation, shell/process, network, extension-owned content, indexing, or a multi-round tool loop.

**Tech Stack:** Rust workspace, `yach-backend`, Serde JSON, native JSONL session evidence, native provider fake requester tests, existing `just dev cargo test ...` recipes.

---

## Scope Notes

This plan implements `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`.

In scope:

- canonical built-in content tool definitions for:
  - `read_text_file`;
  - `search_project`;
  - `list_project_paths`;
- a distinct `ReadsLocalContent` provider-advertising and execution allowlist;
- schema-only provider advertising for the new content tools;
- yach-owned execution through `ProjectReadOnlyToolExecutor`;
- bounded provider results that may contain file text/search lines/path lists;
- redacted durable session evidence that must not persist file bodies, search match lines, directory dumps, or raw queries;
- explicit native-provider routing for content tools in the same one-round path as `project_path_info` and agent edit tools.

Out of scope:

- broad `write`, patch, delete, rename, move, chmod, binary edit, or multi-operation mutation tools;
- shell/process, network, web fetch, LSP, MCP, or extension-owned content tools;
- background indexing or caches;
- provider adapter tool execution;
- multi-round autonomous provider tool loops;
- TUI protocol/UI changes.

## File Structure

- `crates/yach-backend/src/resource.rs`
  - Add bounded immediate directory listing primitives.
  - Keep project-root, symlink, generated-directory, and stable ordering policy near existing read/search primitives.
- `crates/yach-backend/src/tools.rs`
  - Add canonical tool definitions, schema descriptions, provider advertising allowlist, content permission policy, provider-content result shaping, and content execution dispatch in `ProjectReadOnlyToolExecutor`.
- `crates/yach-backend/src/rig_adapter.rs`
  - Extend schema-only advertising projection tests/allowlists so Rig can receive the new content schemas without executable Rig tools.
- `crates/yach-backend/src/native_runner.rs`
  - Advertise content tools in the explicit native-provider agent tool path and route first-round content tool calls through the read-only executor.
- `crates/yach-backend/src/lib.rs`
  - Add backend tests for resource listing, registry/policy/advertising, executor result shaping/evidence, and provider continuation behavior.
- `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`
  - Mark accepted after implementation planning.
- `docs/project/state.md` and `docs/project/next.md`
  - Update the active handoff after implementation lands.

---

### Task 1: Add Bounded Project Path Listing Primitive

**Files:**
- Modify: `crates/yach-backend/src/resource.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing list primitive tests**

Add these tests near the existing `native_project_path_metadata_*` and `native_project_search_*` tests in `crates/yach-backend/src/lib.rs`.

Also add these imports to the test module `use super::{ ... }` list if missing:

```rust
NativeResourceListPolicy, NativeResourceProviderVisibility,
```

Add tests:

```rust
#[test]
fn native_project_list_paths_returns_sorted_bounded_immediate_entries() {
    let root_path = temp_resource_dir("native-resource-list");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    assert!(std::fs::create_dir_all(root_path.join("target")).is_ok());
    assert!(std::fs::write(root_path.join("src/lib.rs"), "lib").is_ok());
    assert!(std::fs::write(root_path.join("src/main.rs"), "main").is_ok());
    assert!(std::fs::write(root_path.join("src/README.md"), "readme").is_ok());
    assert!(std::fs::write(root_path.join("target/generated.rs"), "skip").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };

    let result = root.list_paths(
        "src",
        NativeResourceListPolicy {
            max_entries: 2,
        },
    );

    assert!(result.is_ok());
    let Some(result) = result.ok() else {
        return;
    };
    assert_eq!(result.provider_visibility, NativeResourceProviderVisibility::Never);
    assert_eq!(result.relative_path, "src");
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].relative_path, "src/README.md");
    assert_eq!(result.entries[1].relative_path, "src/lib.rs");
    assert!(result.truncated);
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn native_project_list_paths_reuses_directory_and_root_escape_policy() {
    let base_path = temp_resource_dir("native-resource-list-policy");
    let root_path = base_path.join("project");
    let outside_path = base_path.join("outside");
    assert!(std::fs::create_dir_all(&root_path).is_ok());
    assert!(std::fs::create_dir_all(&outside_path).is_ok());
    assert!(std::fs::write(root_path.join("file.txt"), "file").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };

    let file_result = root.list_paths("file.txt", NativeResourceListPolicy { max_entries: 8 });
    let escape_result = root.list_paths("../outside", NativeResourceListPolicy { max_entries: 8 });

    assert_eq!(file_result, Err(NativeResourcePathError::ExpectedDirectory));
    assert_eq!(escape_result, Err(NativeResourcePathError::EscapesRoot));
    assert!(std::fs::remove_dir_all(base_path).is_ok());
}
```

- [ ] **Step 2: Run the list tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_project_list_paths -- --nocapture
```

Expected: compile fails because `NativeResourceListPolicy`, list result types, and `NativeResourceRoot::list_paths` do not exist.

- [ ] **Step 3: Add list types in `resource.rs`**

Add these types after `NativeResourceSearchResult`:

```rust
/// Bounded immediate listing policy for project-local resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeResourceListPolicy {
    pub max_entries: usize,
}

impl NativeResourceListPolicy {
    #[must_use]
    pub const fn small() -> Self {
        Self { max_entries: 200 }
    }
}

/// One listed project-root entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceListEntry {
    pub relative_path: String,
    pub kind: NativeResourceEntryKind,
    pub byte_size: Option<u64>,
}

/// Bounded immediate path listing result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceListResult {
    pub relative_path: String,
    pub entries: Vec<NativeResourceListEntry>,
    pub truncated: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}
```

- [ ] **Step 4: Add list helper in `impl NativeResourceRoot`**

Add this method near `search_text` and `path_metadata`:

```rust
    pub fn list_paths(
        &self,
        relative_path: impl AsRef<Path>,
        policy: NativeResourceListPolicy,
    ) -> Result<NativeResourceListResult, NativeResourcePathError> {
        let directory = self.resolve_directory(relative_path)?;
        let relative_path = self.normalized_relative_path(&directory)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|_| NativeResourcePathError::Missing)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeResourcePathError::Missing)?;
        entries.sort_by_key(|entry| {
            self.normalized_relative_path(&entry.path())
                .unwrap_or_else(|_| entry.file_name().to_string_lossy().into_owned())
        });

        let mut result_entries = Vec::new();
        let mut truncated = false;
        for entry in entries {
            if result_entries.len() >= policy.max_entries {
                truncated = true;
                break;
            }
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if generated_or_heavy_resource_entry(&file_name) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let kind = if metadata.is_file() {
                NativeResourceEntryKind::File
            } else if metadata.is_dir() {
                NativeResourceEntryKind::Directory
            } else {
                NativeResourceEntryKind::Other
            };
            result_entries.push(NativeResourceListEntry {
                relative_path: self.normalized_relative_path(&entry.path())?,
                kind,
                byte_size: metadata.is_file().then_some(metadata.len()),
            });
        }

        Ok(NativeResourceListResult {
            relative_path,
            entries: result_entries,
            truncated,
            provider_visibility: NativeResourceProviderVisibility::Never,
        })
    }
```

Add this private helper near search traversal helpers and update `search_directory` to use it instead of an inline `matches!`:

```rust
fn generated_or_heavy_resource_entry(file_name: &str) -> bool {
    matches!(file_name, ".git" | ".yach" | "target")
}
```

In `search_directory`, replace:

```rust
if matches!(file_name.as_str(), ".git" | ".yach" | "target") {
    continue;
}
```

with:

```rust
if generated_or_heavy_resource_entry(&file_name) {
    continue;
}
```

- [ ] **Step 5: Run the list tests and verify they pass**

Run:

```bash
just dev cargo test -p yach-backend native_project_list_paths -- --nocapture
```

Expected: 2 tests pass.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yach-backend/src/resource.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add native project path listing"
```

---

### Task 2: Add Content Tool Definitions, Policy, And Schema Advertising

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Modify/test: `crates/yach-backend/src/lib.rs`
- Modify/test: `crates/yach-backend/src/rig_adapter.rs`

- [ ] **Step 1: Write failing registry and advertising tests**

In `crates/yach-backend/src/lib.rs`, add tests near existing provider advertising and registry tests:

```rust
#[test]
fn native_tool_registry_exposes_provider_content_tools() {
    let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();

    assert_eq!(
        registry.get("read_text_file").map(|definition| definition.risk),
        Some(NativeToolRisk::ReadsLocalContent)
    );
    assert_eq!(
        registry.get("search_project").map(|definition| definition.risk),
        Some(NativeToolRisk::ReadsLocalContent)
    );
    assert_eq!(
        registry.get("list_project_paths").map(|definition| definition.risk),
        Some(NativeToolRisk::ReadsLocalContent)
    );
}

#[test]
fn provider_advertising_candidates_require_explicit_content_policy() {
    let registry = NativeToolRegistry::with_project_read_only_and_agent_edit_tools();
    let metadata_only = NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(
        ["project_path_info"],
        ["edit_text_file", "create_text_file"],
    );
    let content_policy =
        NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            ["project_path_info"],
            ["read_text_file", "search_project", "list_project_paths"],
            ["edit_text_file", "create_text_file"],
        );
    let routable = [
        "project_path_info",
        "read_text_file",
        "search_project",
        "list_project_paths",
        "edit_text_file",
        "create_text_file",
    ];

    let metadata_only_names = registry
        .provider_advertising_candidates(&metadata_only, routable)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    let content_names = registry
        .provider_advertising_candidates(&content_policy, routable)
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();

    assert_eq!(
        metadata_only_names,
        vec!["project_path_info", "edit_text_file", "create_text_file"]
    );
    assert_eq!(
        content_names,
        vec![
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
        ]
    );
}

#[test]
fn provider_tool_advertising_builder_emits_canonical_content_schemas() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::read_text_file(),
        NativeToolDefinition::search_project(),
        NativeToolDefinition::list_project_paths(),
    ]);

    assert!(extension.is_ok());
    let Some(extension) = extension.ok() else {
        return;
    };
    let advertising = serde_json::from_value::<ProviderToolAdvertising>(extension.value);
    assert!(advertising.is_ok());
    let Some(advertising) = advertising.ok() else {
        return;
    };
    let names = advertising
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["read_text_file", "search_project", "list_project_paths"]);
    for tool in &advertising.tools {
        assert_eq!(tool.parameters["type"], "object");
        assert_eq!(tool.parameters["additionalProperties"], false);
    }
    assert!(advertising.tools.iter().any(|tool| {
        tool.name == "read_text_file"
            && tool.parameters["properties"]["path"]["type"] == "string"
            && tool.parameters["required"] == serde_json::json!(["path"])
    }));
    assert!(advertising.tools.iter().any(|tool| {
        tool.name == "search_project"
            && tool.parameters["properties"]["query"]["type"] == "string"
            && tool.parameters["required"] == serde_json::json!(["query"])
    }));
    assert!(advertising.tools.iter().any(|tool| {
        tool.name == "list_project_paths"
            && tool.parameters["properties"]["path"]["type"] == "string"
            && tool.parameters["required"] == serde_json::json!(["path"])
    }));
}

#[test]
fn provider_tool_advertising_rejects_mutated_builtin_content_tool() {
    let mut tool = NativeToolDefinition::read_text_file();
    tool.description = String::from("changed");

    assert_eq!(
        build_provider_tool_advertising_extension(&[tool]),
        Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: String::from("read_text_file")
        })
    );
}
```

In `crates/yach-backend/src/rig_adapter.rs`, add a test near existing advertising projection tests:

```rust
#[test]
fn rig_adapter_emits_content_tool_definitions_when_approved() {
    let extension = build_provider_tool_advertising_extension(&[
        NativeToolDefinition::read_text_file(),
        NativeToolDefinition::search_project(),
        NativeToolDefinition::list_project_paths(),
    ]);
    assert!(extension.is_ok());
    let Some(extension) = extension.ok() else {
        return;
    };
    let request = ProviderRequest {
        turn_id: NativeTurnId(String::from("turn-1")),
        model: ProviderModel {
            provider: String::from("fixture"),
            model: String::from("fixture-model"),
        },
        messages: vec![ProviderMessage::user("inspect files")],
        extensions: vec![extension],
    };

    let definitions = rig_tool_definitions_from_request_with_approved_tools(
        &request,
        [
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
        ],
    );

    assert!(definitions.is_ok());
    let Some(definitions) = definitions.ok() else {
        return;
    };
    assert_eq!(
        definitions
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_text_file", "search_project", "list_project_paths"]
    );
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_tool_registry_exposes_provider_content_tools -- --exact
just dev cargo test -p yach-backend provider_advertising_candidates_require_explicit_content_policy -- --exact
just dev cargo test -p yach-backend provider_tool_advertising_builder_emits_canonical_content_schemas -- --exact
just dev cargo test -p yach-backend provider_tool_advertising_rejects_mutated_builtin_content_tool -- --exact
just dev cargo test -p yach-backend rig_adapter_emits_content_tool_definitions_when_approved -- --exact
```

Expected: compile failures because content definitions and content policy do not exist. Use module-qualified exact names if bare exact names run 0 tests.

- [ ] **Step 3: Add content tool definitions and field descriptions**

In `provider_string_field_description`, add:

```rust
        ("read_text_file", "path") => {
            String::from("Project-relative UTF-8 text file path to read.")
        }
        ("search_project", "query") => {
            String::from("Literal text to search for in project UTF-8 files.")
        }
        ("list_project_paths", "path") => {
            String::from("Project-relative directory path to list.")
        }
```

In `impl NativeToolDefinition`, add:

```rust
    #[must_use]
    pub fn read_text_file() -> Self {
        Self {
            name: String::from("read_text_file"),
            description: String::from(
                "Read a bounded UTF-8 project file through yach-owned resource policy.",
            ),
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn search_project() -> Self {
        Self {
            name: String::from("search_project"),
            description: String::from(
                "Search bounded UTF-8 project files for a literal query.",
            ),
            input_schema: NativeToolInputSchema::string_object(
                ["query"],
                std::iter::empty::<&str>(),
                4 * 1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn list_project_paths() -> Self {
        Self {
            name: String::from("list_project_paths"),
            description: String::from(
                "List bounded immediate project directory entries without file bodies.",
            ),
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }
```

- [ ] **Step 4: Extend provider advertising allowlists**

In `project_provider_advertised_tool`, add a built-in content branch:

```rust
            "read_text_file" | "search_project" | "list_project_paths" => {
                if tool.risk != NativeToolRisk::ReadsLocalContent {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
```

In `is_canonical_builtin_provider_tool`, add canonical checks:

```rust
        "read_text_file" => {
            let canonical = NativeToolDefinition::read_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "search_project" => {
            let canonical = NativeToolDefinition::search_project();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "list_project_paths" => {
            let canonical = NativeToolDefinition::list_project_paths();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
```

In `validate_provider_advertised_tool_schema`, include content tools in the canonical match:

```rust
        "read_text_file" => Some(NativeToolDefinition::read_text_file()),
        "search_project" => Some(NativeToolDefinition::search_project()),
        "list_project_paths" => Some(NativeToolDefinition::list_project_paths()),
```

- [ ] **Step 5: Extend permission policy and registry constructors**

Update `NativeToolPermissionPolicy`:

```rust
pub struct NativeToolPermissionPolicy {
    fixture_execution: BTreeSet<String>,
    metadata_advertising: BTreeSet<String>,
    content_advertising: BTreeSet<String>,
    agent_edit_advertising: BTreeSet<String>,
}
```

Update existing constructors to initialize `content_advertising: BTreeSet::new()`.

Add:

```rust
    #[must_use]
    pub fn allow_project_metadata_content_and_agent_edit_tools(
        metadata_names: impl IntoIterator<Item = impl Into<String>>,
        content_names: impl IntoIterator<Item = impl Into<String>>,
        edit_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            fixture_execution: BTreeSet::new(),
            metadata_advertising: metadata_names.into_iter().map(Into::into).collect(),
            content_advertising: content_names.into_iter().map(Into::into).collect(),
            agent_edit_advertising: edit_names.into_iter().map(Into::into).collect(),
        }
    }
```

Update `authorize`:

```rust
            NativeToolRisk::ReadsLocalContent => {
                self.content_advertising.contains(&definition.name)
            }
```

Update `allows_provider_advertising`:

```rust
            NativeToolRisk::ReadsLocalContent => {
                self.content_advertising.contains(&definition.name)
            }
```

Update `NativeToolRegistry::with_project_read_only_tools()` and `with_project_read_only_and_agent_edit_tools()` to include:

```rust
NativeToolDefinition::read_text_file(),
NativeToolDefinition::search_project(),
NativeToolDefinition::list_project_paths(),
```

Keep `with_agent_edit_tools()` unchanged.

- [ ] **Step 6: Extend Rig approved-tool tests if needed**

If `rig_tool_definitions_from_request_with_approved_tools` already accepts arbitrary approved names, no implementation change is needed beyond the new test. Do not widen the default `rig_tool_definitions_from_request`; it should still approve only `project_path_info` unless a specific native-provider path passes a wider list.

- [ ] **Step 7: Run focused tests**

Run the same commands from Step 2. Expected: all pass.

Also run:

```bash
just dev cargo test -p yach-backend provider_tool_advertising -- --nocapture
just dev cargo test -p yach-backend rig_adapter -- --nocapture
```

Expected: pass.

- [ ] **Step 8: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs crates/yach-backend/src/rig_adapter.rs
git commit -m "feat: define provider content tools"
```

---

### Task 3: Execute Content Tools Through `ProjectReadOnlyToolExecutor`

**Files:**
- Modify: `crates/yach-backend/src/tools.rs`
- Test: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing executor tests**

Add tests near existing `project_readonly_provider_tool_results_*` tests in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn project_readonly_provider_tool_results_read_text_file_returns_content_with_redacted_evidence() {
    let root_path = temp_resource_dir("provider-read-text-file");
    assert!(std::fs::write(root_path.join("notes.txt"), "alpha\nbeta\n").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let policy = NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        ["read_text_file"],
        std::iter::empty::<&str>(),
    );
    let mut log = NativeSessionLog::default();
    let context = NativeToolContinuationContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
    };

    let results = build_project_readonly_provider_tool_results(
        &mut log,
        &context,
        vec![ProviderToolCall {
            call_id: String::from("call-read-1"),
            name: String::from("read_text_file"),
            arguments_json: serde_json::json!({"path": "notes.txt"}),
        }],
        root,
        &registry,
        &policy,
        NativeToolContinuationPolicy {
            max_tool_calls: 4,
            max_result_bytes: 64 * 1024,
        },
    );

    assert!(results.is_ok());
    let Some(results) = results.ok() else {
        return;
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].provider_call_id.as_deref(), Some("call-read-1"));
    assert!(results[0].content.contains("\"text\":\"alpha\\nbeta\\n\""));
    assert!(!results[0].redacted);
    let raw_log = serde_json::to_string(&log.events);
    assert!(raw_log.is_ok());
    let Some(raw_log) = raw_log.ok() else {
        return;
    };
    assert!(raw_log.contains("read_text_file result redacted"));
    assert!(!raw_log.contains("alpha"));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn project_readonly_provider_tool_results_search_project_returns_bounded_matches_with_redacted_evidence() {
    let root_path = temp_resource_dir("provider-search-project");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    assert!(std::fs::write(root_path.join("src/lib.rs"), "needle one\nnone\nneedle two\n").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let policy = NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        ["search_project"],
        std::iter::empty::<&str>(),
    );
    let mut log = NativeSessionLog::default();
    let context = NativeToolContinuationContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
    };

    let results = build_project_readonly_provider_tool_results(
        &mut log,
        &context,
        vec![ProviderToolCall {
            call_id: String::from("call-search-1"),
            name: String::from("search_project"),
            arguments_json: serde_json::json!({"query": "needle"}),
        }],
        root,
        &registry,
        &policy,
        NativeToolContinuationPolicy {
            max_tool_calls: 4,
            max_result_bytes: 64 * 1024,
        },
    );

    assert!(results.is_ok());
    let Some(results) = results.ok() else {
        return;
    };
    assert!(results[0].content.contains("\"outcome\":\"search\""));
    assert!(results[0].content.contains("\"line_number\":1"));
    assert!(results[0].content.contains("needle one"));
    let raw_log = serde_json::to_string(&log.events);
    assert!(raw_log.is_ok());
    let Some(raw_log) = raw_log.ok() else {
        return;
    };
    assert!(raw_log.contains("search_project matches=2 truncated=false"));
    assert!(!raw_log.contains("needle one"));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn project_readonly_provider_tool_results_list_project_paths_returns_entries_with_redacted_evidence() {
    let root_path = temp_resource_dir("provider-list-project-paths");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    assert!(std::fs::write(root_path.join("src/lib.rs"), "lib").is_ok());
    assert!(std::fs::write(root_path.join("src/main.rs"), "main").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let policy = NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        ["list_project_paths"],
        std::iter::empty::<&str>(),
    );
    let mut log = NativeSessionLog::default();
    let context = NativeToolContinuationContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
    };

    let results = build_project_readonly_provider_tool_results(
        &mut log,
        &context,
        vec![ProviderToolCall {
            call_id: String::from("call-list-1"),
            name: String::from("list_project_paths"),
            arguments_json: serde_json::json!({"path": "src"}),
        }],
        root,
        &registry,
        &policy,
        NativeToolContinuationPolicy {
            max_tool_calls: 4,
            max_result_bytes: 64 * 1024,
        },
    );

    assert!(results.is_ok());
    let Some(results) = results.ok() else {
        return;
    };
    assert!(results[0].content.contains("\"outcome\":\"list\""));
    assert!(results[0].content.contains("\"path\":\"src/lib.rs\""));
    let raw_log = serde_json::to_string(&log.events);
    assert!(raw_log.is_ok());
    let Some(raw_log) = raw_log.ok() else {
        return;
    };
    assert!(raw_log.contains("list_project_paths entries=2 truncated=false"));
    assert!(!raw_log.contains("src/lib.rs"));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn project_readonly_provider_tool_results_content_requires_content_policy() {
    let root_path = temp_resource_dir("provider-content-policy");
    assert!(std::fs::write(root_path.join("notes.txt"), "secret").is_ok());
    let root = NativeResourceRoot::project(&root_path);
    assert!(root.is_ok());
    let Ok(root) = root else {
        unreachable!("asserted root creation succeeds");
    };
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let policy = NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info");
    let mut log = NativeSessionLog::default();
    let context = NativeToolContinuationContext {
        session_id: NativeSessionId(String::from("default")),
        turn_id: NativeTurnId(String::from("turn-1")),
    };

    let result = build_project_readonly_provider_tool_results(
        &mut log,
        &context,
        vec![ProviderToolCall {
            call_id: String::from("call-read-1"),
            name: String::from("read_text_file"),
            arguments_json: serde_json::json!({"path": "notes.txt"}),
        }],
        root,
        &registry,
        &policy,
        NativeToolContinuationPolicy::fixture_default(),
    );

    assert_eq!(
        result,
        Err(NativeToolContinuationError::Validation(
            NativeToolError::PermissionDenied
        ))
    );
    let raw_log = serde_json::to_string(&log.events);
    assert!(raw_log.is_ok());
    let Some(raw_log) = raw_log.ok() else {
        return;
    };
    assert!(!raw_log.contains("secret"));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run executor tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results_read_text_file_returns_content_with_redacted_evidence -- --exact
just dev cargo test -p yach-backend project_readonly_provider_tool_results_search_project_returns_bounded_matches_with_redacted_evidence -- --exact
just dev cargo test -p yach-backend project_readonly_provider_tool_results_list_project_paths_returns_entries_with_redacted_evidence -- --exact
just dev cargo test -p yach-backend project_readonly_provider_tool_results_content_requires_content_policy -- --exact
```

Expected: fail because `ProjectReadOnlyToolExecutor` still supports only `project_path_info`.

- [ ] **Step 3: Add provider content bounds and helpers**

In `tools.rs`, extend imports:

```rust
use crate::{
    NativeResourceListPolicy, NativeResourcePathError, NativeResourceReadError,
    NativeResourceReadPolicy, NativeResourceRoot, NativeResourceSearchPolicy, ...
};
```

Add constants near the executor:

```rust
const PROVIDER_READ_TEXT_MAX_BYTES: u64 = 32 * 1024;
const PROVIDER_SEARCH_MAX_FILE_BYTES: u64 = 64 * 1024;
const PROVIDER_SEARCH_MAX_FILES: usize = 512;
const PROVIDER_SEARCH_MAX_MATCHES: usize = 64;
const PROVIDER_SEARCH_LINE_MAX_BYTES: usize = 240;
const PROVIDER_LIST_MAX_ENTRIES: usize = 200;
```

Add helper functions:

```rust
fn required_string_argument(
    request: &PendingNativeToolRequest,
    field: &str,
) -> Result<String, NativeToolExecutionError> {
    request
        .arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(NativeToolExecutionError::MalformedResult)
}

fn resource_entry_kind_label(kind: crate::NativeResourceEntryKind) -> &'static str {
    match kind {
        crate::NativeResourceEntryKind::File => "file",
        crate::NativeResourceEntryKind::Directory => "directory",
        crate::NativeResourceEntryKind::Other => "other",
    }
}

fn bounded_provider_line(value: &str) -> (String, bool) {
    if value.len() <= PROVIDER_SEARCH_LINE_MAX_BYTES {
        return (value.to_owned(), false);
    }
    let mut end = 0;
    for (index, _) in value.char_indices() {
        if index > PROVIDER_SEARCH_LINE_MAX_BYTES {
            break;
        }
        end = index;
    }
    if end == 0 {
        return (String::new(), true);
    }
    (value[..end].to_owned(), true)
}
```

- [ ] **Step 4: Dispatch content execution by tool name**

Replace the current `ProjectReadOnlyToolExecutor::execute` body after validation with a dispatch:

```rust
        let Some(root) = &self.root else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        match definition.name.as_str() {
            "project_path_info" if definition.risk == NativeToolRisk::ReadsLocalMetadata => {
                execute_project_path_info(root, request)
            }
            "read_text_file" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_read_text_file(root, request)
            }
            "search_project" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_search_project(root, request)
            }
            "list_project_paths" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_list_project_paths(root, request)
            }
            _ => Err(NativeToolExecutionError::UnsupportedTool),
        }
```

Extract the existing metadata logic into:

```rust
fn execute_project_path_info(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let metadata = root
        .path_metadata(path)
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let summary = serde_json::json!({
        "relative_path": metadata.relative_path,
        "kind": resource_entry_kind_label(metadata.kind),
        "byte_size": metadata.byte_size,
        "provider_visibility": "never",
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: false,
    })
}
```

- [ ] **Step 5: Add `read_text_file` execution**

Add:

```rust
fn execute_read_text_file(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let read = root
        .read_text_file(
            &path,
            NativeResourceReadPolicy::local_only(PROVIDER_READ_TEXT_MAX_BYTES),
        )
        .map_err(native_read_error_to_execution_error)?;
    let relative_path = root
        .path_metadata(&path)
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?
        .relative_path;
    let content = serde_json::json!({
        "outcome": "read",
        "path": relative_path,
        "text": read.text,
        "byte_count": read.byte_count,
        "truncated": false,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: content.len(),
        summary: content,
        redacted: false,
        truncated: false,
    })
}

fn native_read_error_to_execution_error(error: NativeResourceReadError) -> NativeToolExecutionError {
    match error {
        NativeResourceReadError::Path(error) => NativeToolExecutionError::ResourcePath { error },
        NativeResourceReadError::TooLarge { .. } => NativeToolExecutionError::ResourceReadTooLarge,
        NativeResourceReadError::NotUtf8 => NativeToolExecutionError::ResourceReadNotUtf8,
        NativeResourceReadError::Io => NativeToolExecutionError::MalformedResult,
    }
}
```

Add `ResourceReadTooLarge` and `ResourceReadNotUtf8` variants to `NativeToolExecutionError`, update `native_tool_execution_error_label`:

```rust
NativeToolExecutionError::ResourceReadTooLarge => "resource_read_too_large",
NativeToolExecutionError::ResourceReadNotUtf8 => "resource_read_not_utf8",
```

- [ ] **Step 6: Add `search_project` execution**

Add:

```rust
fn execute_search_project(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let query = required_string_argument(request, "query")?;
    let result = root
        .search_text(
            &query,
            NativeResourceSearchPolicy {
                max_file_bytes: PROVIDER_SEARCH_MAX_FILE_BYTES,
                max_files: PROVIDER_SEARCH_MAX_FILES,
                max_matches: PROVIDER_SEARCH_MAX_MATCHES,
            },
        )
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let mut line_truncated = false;
    let matches = result
        .matches
        .into_iter()
        .map(|matched| {
            let (line, truncated) = bounded_provider_line(&matched.line);
            line_truncated |= truncated;
            serde_json::json!({
                "path": matched.relative_path,
                "line_number": matched.line_number,
                "line": line,
                "line_truncated": truncated,
            })
        })
        .collect::<Vec<_>>();
    let content = serde_json::json!({
        "outcome": "search",
        "matches": matches,
        "searched_files": result.searched_files,
        "truncated": result.truncated || line_truncated,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: content.len(),
        summary: content,
        redacted: false,
        truncated: result.truncated || line_truncated,
    })
}
```

Do not include the raw query in the provider result or session evidence in this first implementation. The provider already knows its own query from the tool call.

- [ ] **Step 7: Add `list_project_paths` execution**

Add:

```rust
fn execute_list_project_paths(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let result = root
        .list_paths(
            &path,
            NativeResourceListPolicy {
                max_entries: PROVIDER_LIST_MAX_ENTRIES,
            },
        )
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let entries = result
        .entries
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.relative_path,
                "kind": resource_entry_kind_label(entry.kind),
                "byte_size": entry.byte_size,
            })
        })
        .collect::<Vec<_>>();
    let content = serde_json::json!({
        "outcome": "list",
        "path": result.relative_path,
        "entries": entries,
        "truncated": result.truncated,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: content.len(),
        summary: content,
        redacted: false,
        truncated: result.truncated,
    })
}
```

- [ ] **Step 8: Redact durable summaries for content tools**

Add a helper:

```rust
fn provider_tool_result_summary(
    tool_name: &str,
    execution: &NativeToolExecutionResult,
) -> NativeToolPayloadSummary {
    let summary = match tool_name {
        "read_text_file" => String::from("read_text_file result redacted"),
        "search_project" => content_result_count_summary("search_project", &execution.summary)
            .unwrap_or_else(|| String::from("search_project result redacted")),
        "list_project_paths" => content_result_count_summary("list_project_paths", &execution.summary)
            .unwrap_or_else(|| String::from("list_project_paths result redacted")),
        _ => execution.summary.clone(),
    };
    NativeToolPayloadSummary {
        summary,
        byte_count: execution.byte_count,
        redacted: matches!(
            tool_name,
            "read_text_file" | "search_project" | "list_project_paths"
        ),
        truncated: execution.truncated,
    }
}

fn content_result_count_summary(tool_name: &str, content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    match tool_name {
        "search_project" => Some(format!(
            "search_project matches={} truncated={}",
            value.get("matches")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        "list_project_paths" => Some(format!(
            "list_project_paths entries={} truncated={}",
            value.get("entries")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        _ => None,
    }
}
```

In `NativeToolContinuationWorkflow::build_provider_tool_results`, replace construction of `result_summary` with:

```rust
let result_summary = provider_tool_result_summary(&request.tool_name, &execution);
```

Keep `NativeProviderToolResult.content = execution.summary`; provider content still goes to the provider. Only session evidence is redacted.

- [ ] **Step 9: Run executor tests**

Run the commands from Step 2. Expected: pass.

Also run:

```bash
just dev cargo test -p yach-backend project_readonly_provider_tool_results -- --nocapture
```

Expected: pass.

- [ ] **Step 10: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: execute provider content tools"
```

---

### Task 4: Advertise And Route Content Tools In Native Provider

**Files:**
- Modify/test: `crates/yach-backend/src/native_runner.rs`
- Modify/test as needed: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing native-provider tests**

In `crates/yach-backend/src/native_runner.rs`, add tests near existing native provider one-round and agent edit tests:

```rust
#[test]
fn native_provider_initial_request_advertises_content_tools_for_agent_edit_context() {
    let mut requester = FakeProviderRequester::with_responses([Ok(vec![
        ProviderStreamEvent::Started {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
        },
        ProviderStreamEvent::Completed {
            turn_id: NativeTurnId(String::from("turn-1")),
            finish_reason: Some(ProviderFinishReason::Stop),
            usage: None,
            provider_response_id: None,
        },
    ])]);
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    let root_guard = temp_native_provider_root("content-advertising");
    let resource_root = NativeResourceRoot::project(root_guard.path());
    assert!(resource_root.is_ok());
    let Ok(resource_root) = resource_root else {
        unreachable!("asserted root creation succeeds");
    };
    let (_review_tx, review_rx) = mpsc::unbounded_channel();
    let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
    let turn_id = NativeTurnId(String::from("turn-1"));

    let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
        &mut requester,
        NativeProviderAgentToolRound {
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
            log: &mut log,
            pending_events: &mut pending_events,
            turn_id: &turn_id,
            project_context: Some(NativeLaunchProjectContext {
                project_root: resource_root,
                cwd: root_guard.path().to_path_buf(),
            }),
            tool_event_store: None,
            review_tx: backend_tx,
            review_decisions: review_rx,
        },
    ));

    assert!(result.is_ok());
    assert_eq!(requester.requests.len(), 1);
    let advertising = parse_provider_tool_advertising_extensions(&requester.requests[0].extensions);
    assert!(advertising.is_ok());
    let Some(Some(advertising)) = advertising.ok() else {
        return;
    };
    let names = advertising
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "project_path_info",
            "read_text_file",
            "search_project",
            "list_project_paths",
            "edit_text_file",
            "create_text_file",
        ]
    );
}

#[test]
fn native_provider_one_round_executes_read_search_list_and_continues_with_redacted_evidence() {
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
                    call_id: String::from("call-read-1"),
                    name: String::from("read_text_file"),
                    arguments_json: serde_json::json!({"path": "notes.txt"}),
                },
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_call: ProviderToolCall {
                    call_id: String::from("call-search-1"),
                    name: String::from("search_project"),
                    arguments_json: serde_json::json!({"query": "needle"}),
                },
            },
            ProviderStreamEvent::ToolCallCompleted {
                turn_id: NativeTurnId(String::from("turn-1")),
                tool_call: ProviderToolCall {
                    call_id: String::from("call-list-1"),
                    name: String::from("list_project_paths"),
                    arguments_json: serde_json::json!({"path": "."}),
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
                delta: String::from("inspected"),
            },
            ProviderStreamEvent::Completed {
                turn_id: NativeTurnId(String::from("turn-1")),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: None,
            },
        ]),
    ]);
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    let root_guard = temp_native_provider_root("content-tools-round");
    assert!(std::fs::write(root_guard.path().join("notes.txt"), "needle\nbody\n").is_ok());
    let resource_root = NativeResourceRoot::project(root_guard.path());
    assert!(resource_root.is_ok());
    let Ok(resource_root) = resource_root else {
        unreachable!("asserted root creation succeeds");
    };
    let (_review_tx, review_rx) = mpsc::unbounded_channel();
    let (backend_tx, _backend_rx) = mpsc::unbounded_channel();
    let turn_id = NativeTurnId(String::from("turn-1"));

    let result = futures::executor::block_on(run_native_provider_one_agent_tool_round(
        &mut requester,
        NativeProviderAgentToolRound {
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
            log: &mut log,
            pending_events: &mut pending_events,
            turn_id: &turn_id,
            project_context: Some(NativeLaunchProjectContext {
                project_root: resource_root,
                cwd: root_guard.path().to_path_buf(),
            }),
            tool_event_store: None,
            review_tx: backend_tx,
            review_decisions: review_rx,
        },
    ));

    assert!(result.is_ok());
    assert_eq!(requester.requests.len(), 2);
    assert_eq!(requester.requests[1].extensions.len(), 0);
    let raw_continuation = serde_json::to_string(&requester.requests[1].messages);
    assert!(raw_continuation.is_ok());
    let Some(raw_continuation) = raw_continuation.ok() else {
        return;
    };
    assert!(raw_continuation.contains("needle"));
    let raw_log = serde_json::to_string(&log.events);
    assert!(raw_log.is_ok());
    let Some(raw_log) = raw_log.ok() else {
        return;
    };
    assert!(raw_log.contains("read_text_file result redacted"));
    assert!(raw_log.contains("search_project matches=1 truncated=false"));
    assert!(raw_log.contains("list_project_paths entries="));
    assert!(!raw_log.contains("needle\\nbody"));
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_provider_initial_request_advertises_content_tools_for_agent_edit_context -- --exact
just dev cargo test -p yach-backend native_provider_one_round_executes_read_search_list_and_continues_with_redacted_evidence -- --exact
```

Expected: fail because native-provider advertising/routing does not include content tools yet. Use module-qualified exact names if bare exact names run 0 tests.

- [ ] **Step 3: Advertise content tools in agent provider path**

In `run_native_provider_one_agent_tool_round`, replace:

```rust
let permission_policy = NativeToolPermissionPolicy::allow_project_metadata_and_agent_edit_tools(
    ["project_path_info"],
    ["edit_text_file", "create_text_file"],
);
let routable_tool_names = ["project_path_info", "edit_text_file", "create_text_file"];
```

with:

```rust
let permission_policy =
    NativeToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
        ["project_path_info"],
        ["read_text_file", "search_project", "list_project_paths"],
        ["edit_text_file", "create_text_file"],
    );
let routable_tool_names = [
    "project_path_info",
    "read_text_file",
    "search_project",
    "list_project_paths",
    "edit_text_file",
    "create_text_file",
];
```

- [ ] **Step 4: Route content tools through the read-only branch**

In the tool-call `match`, replace:

```rust
"project_path_info" => {
```

with:

```rust
"project_path_info" | "read_text_file" | "search_project" | "list_project_paths" => {
```

Keep the same validation, execution, result-size, evidence, and `tool_results.push(...)` path. This ensures content tools use `ProjectReadOnlyToolExecutor` and the same one-round continuation result flow.

- [ ] **Step 5: Ensure Rig approved tools include content names for live native-provider**

Find the live `RigProviderRequester` construction for the explicit agent tool path. Update its approved tool list from:

```rust
vec![
    String::from("project_path_info"),
    String::from("edit_text_file"),
    String::from("create_text_file"),
]
```

to:

```rust
vec![
    String::from("project_path_info"),
    String::from("read_text_file"),
    String::from("search_project"),
    String::from("list_project_paths"),
    String::from("edit_text_file"),
    String::from("create_text_file"),
]
```

Do not widen the default Rig request path.

- [ ] **Step 6: Run focused native-provider tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_initial_request_advertises_content_tools_for_agent_edit_context -- --exact
just dev cargo test -p yach-backend native_provider_one_round_executes_read_search_list_and_continues_with_redacted_evidence -- --exact
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
just dev cargo test -p yach-backend native_provider_agent_edit -- --nocapture
```

Expected: pass.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-backend/src/lib.rs
git commit -m "feat: route provider content tools"
```

---

### Task 5: Project Handoff And Final Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Mark design accepted**

In `docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md`, change:

```markdown
Status: proposed
```

to:

```markdown
Status: accepted
```

- [ ] **Step 2: Update project state**

In `docs/project/state.md`, replace the proposed-design paragraph with:

```markdown
The provider-visible read/search content implementation now adds canonical
`read_text_file`, `search_project`, and `list_project_paths` built-ins for the
explicit native-provider path. These use a separate `ReadsLocalContent`
risk/policy path, yach-owned project-root resolution, bounded provider results,
redacted durable session evidence, and the existing one-round provider
continuation boundary. `project_path_info` remains metadata-only, and content
tool evidence does not persist file bodies, search match lines, directory dumps,
or raw queries. This is not sufficient for shell/process tools, broad mutation,
network tools, extension-owned content tools, indexing, LSP, MCP integration,
or multi-round autonomous tool loops.
```

- [ ] **Step 3: Update next work**

In `docs/project/next.md`, update the recommended next move to dogfood/tune the newly implemented content tools:

```markdown
Recommended next move: dogfood the native-provider edit loop with provider-visible read/search/list content tools enabled.

Why: exact/create edit tools now have the minimum content acquisition surface
needed for practical file edits. The next useful evidence is real-session
behavior: whether bounds are too small, whether result summaries are readable,
and whether the model reliably uses read/search/list before edit calls.
```

Keep broader mutation, shell/process, network, extension-owned content tools, indexing, LSP, MCP, and multi-round loops in the “not ready without a new spec” list.

- [ ] **Step 4: Run focused verification**

Run:

```bash
just dev cargo test -p yach-backend native_project_list_paths -- --nocapture
just dev cargo test -p yach-backend provider_advertising_candidates_require_explicit_content_policy -- --exact
just dev cargo test -p yach-backend provider_tool_advertising_builder_emits_canonical_content_schemas -- --exact
just dev cargo test -p yach-backend project_readonly_provider_tool_results -- --nocapture
just dev cargo test -p yach-backend native_provider_one_round -- --nocapture
just dev cargo test -p yach-backend native_provider_agent_edit -- --nocapture
```

Use module-qualified exact names if bare exact names run 0 tests.

- [ ] **Step 5: Run workspace gates**

Run:

```bash
just test
just lint
```

Expected: both pass. If sandboxing blocks the shared `.devenv/state/target` cargo lock, rerun the same command with approval rather than changing the command.

- [ ] **Step 6: Commit docs**

Run:

```bash
git add docs/superpowers/specs/2026-05-18-provider-read-search-content-design.md docs/project/state.md docs/project/next.md
git commit -m "docs: update provider content handoff"
```

---

## Implementation Notes

- Keep provider content tools out of Pi/default adapter behavior except through the existing explicit native-provider path.
- Do not persist file bodies, search result line bodies, directory listings, raw queries, or raw provider arguments in `NativeSessionEvent` summaries.
- Do not introduce provider SDK executable tools. Provider adapters get schemas only.
- Do not widen `run_provider_request` default approved tools; use the explicit approved-tool seam for native-provider agent tools.
- Treat `NativeToolContinuationPolicy::fixture_default()` as existing behavior, but tests that need larger content results should pass an explicit policy with a larger `max_result_bytes`.
- Keep content result truncation and continuation max bytes separate: a tool may shape bounded content, but continuation still rejects an oversized final provider result.
- If strict clippy rejects `expect` in tests, use the existing `assert!(value.is_ok()); let Some(value) = value.ok() else { return; };` pattern.

## Final Review Checklist

- [ ] `read_text_file` sends bounded text to provider results but not to durable session evidence.
- [ ] `search_project` sends bounded match lines to provider results but not to durable session evidence.
- [ ] `list_project_paths` sends bounded path entries to provider results but not to durable session evidence.
- [ ] `project_path_info` remains metadata-only.
- [ ] Content tools require explicit `ReadsLocalContent` policy and are not enabled by metadata-only policy.
- [ ] Canonical content schemas are projected into provider advertising only when policy and route allow them.
- [ ] Rig receives schema-only tool definitions and does not execute tools.
- [ ] Native provider continuation remains one-round and strips advertising on continuation.
- [ ] Existing agent edit tests still pass.
- [ ] `just test` and `just lint` pass.
