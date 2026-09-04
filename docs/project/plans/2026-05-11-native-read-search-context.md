# Native Read Search Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add native backend read-only project inspection primitives for path metadata, explicit text reads, bounded search, and local-only context packaging.

**Architecture:** Keep read/search/context behavior in `yach-backend` behind the existing resource and tool seams. `resource.rs` owns project-root policy, metadata, explicit reads, search traversal, and context packages. `tools.rs` gets only the read-only metadata tool in this slice; file content and search results remain local-only backend primitives until provider-visible policy and continuation are separately approved.

**Tech Stack:** Rust 2024, std filesystem APIs, Serde JSON, existing yach native session/tool/resource types, `just` recipes.

---

## Scope

This plan implements the next Native MVP read/search/context slice without crossing into file edits, process execution, network access, protocol UI, or provider-visible local file content.

In scope:

- project-relative path metadata through canonical project-root policy;
- explicit local-only text reads with existing size and UTF-8 policy;
- bounded project search that skips generated/heavy directories;
- context packaging for local provider-request assembly later;
- one non-fixture backend tool, `project_path_info`, that returns metadata only;

Out of scope:

- sending file contents or search results to a real provider;
- integrating read/search tools into `--backend native-provider`;
- file edit/create/delete/rename tools;
- shell/process/network tools;
- user approval UI or `yach-proto` tool/resource events;
- raw file content persistence in native session logs.

## Files

- Modify `crates/yach-backend/src/resource.rs`: add metadata, search, context package types and helpers.
- Modify `crates/yach-backend/src/tools.rs`: add `project_path_info` definition, read-only metadata permission, and executor.
- Modify `crates/yach-backend/src/lib.rs`: add focused backend tests and import new types.
- Modify `crates/yach-backend/src/native_runner.rs`: update native status text after backend primitives exist, without wiring provider-visible tools.
- Modify `crates/yach-bench/Cargo.toml`: add the `native_resource` bench target.
- Create `crates/yach-bench/benches/native_resource.rs`: benchmark path metadata, text read, and bounded search over fixture trees.
- Modify `docs/project/state.md` and `docs/project/next.md`: update active state after implementation.

## Task 1: Project Path Metadata

**Files:**

- Modify: `crates/yach-backend/src/resource.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing metadata tests**

Add these tests near the existing `native_project_resource_root_*` tests in `crates/yach-backend/src/lib.rs`:

Also add `NativeResourceEntryKind` to the `use super::{ ... }` list at the top of the test module.

```rust
#[test]
fn native_project_path_metadata_returns_normalized_file_and_directory_info() {
    let root_path = temp_resource_dir("native-resource-metadata");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    assert!(std::fs::write(root_path.join("src/lib.rs"), "pub fn demo() {}\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let file = root
        .as_ref()
        .and_then(|root| root.path_metadata("src/lib.rs").ok());
    let directory = root
        .as_ref()
        .and_then(|root| root.path_metadata("src").ok());

    assert_eq!(file.as_ref().map(|metadata| metadata.relative_path.as_str()), Some("src/lib.rs"));
    assert_eq!(file.as_ref().map(|metadata| metadata.kind), Some(NativeResourceEntryKind::File));
    assert_eq!(file.as_ref().and_then(|metadata| metadata.byte_size), Some(17));
    assert_eq!(
        file.as_ref().map(|metadata| metadata.provider_visibility),
        Some(NativeResourceProviderVisibility::Never)
    );
    assert_eq!(directory.as_ref().map(|metadata| metadata.relative_path.as_str()), Some("src"));
    assert_eq!(
        directory.as_ref().map(|metadata| metadata.kind),
        Some(NativeResourceEntryKind::Directory)
    );
    assert_eq!(directory.as_ref().and_then(|metadata| metadata.byte_size), None);
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn native_project_path_metadata_reuses_root_escape_policy() {
    let base_path = temp_resource_dir("native-resource-metadata-escape");
    let root_path = base_path.join("project");
    let outside_path = base_path.join("outside");
    assert!(std::fs::create_dir_all(&root_path).is_ok());
    assert!(std::fs::create_dir_all(&outside_path).is_ok());
    assert!(std::fs::write(outside_path.join("secret.txt"), "secret").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let error = root
        .as_ref()
        .map(|root| root.path_metadata("../outside/secret.txt"));

    assert_eq!(error, Some(Err(NativeResourcePathError::EscapesRoot)));
    assert!(std::fs::remove_dir_all(base_path).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_project_path_metadata
```

Expected: FAIL because `NativeResourceEntryKind`, `NativeResourcePathMetadata`, and `NativeResourceRoot::path_metadata` do not exist.

- [ ] **Step 3: Implement metadata types and helper**

Add this code to `crates/yach-backend/src/resource.rs` after `NativeResourceRead`:

```rust
/// Project-root entry kind returned by read-only path metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourceEntryKind {
    File,
    Directory,
    Other,
}

/// Normalized path metadata scoped to a native resource root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourcePathMetadata {
    pub relative_path: String,
    pub kind: NativeResourceEntryKind,
    pub byte_size: Option<u64>,
    pub provider_visibility: NativeResourceProviderVisibility,
}
```

Add these methods inside `impl NativeResourceRoot`:

```rust
    pub fn path_metadata(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<NativeResourcePathMetadata, NativeResourcePathError> {
        let path = self.resolve_existing(relative_path)?;
        let metadata = fs::metadata(&path).map_err(|_| NativeResourcePathError::Missing)?;
        let kind = if metadata.is_file() {
            NativeResourceEntryKind::File
        } else if metadata.is_dir() {
            NativeResourceEntryKind::Directory
        } else {
            NativeResourceEntryKind::Other
        };
        let byte_size = if metadata.is_file() {
            Some(metadata.len())
        } else {
            None
        };

        Ok(NativeResourcePathMetadata {
            relative_path: self.normalized_relative_path(&path)?,
            kind,
            byte_size,
            provider_visibility: NativeResourceProviderVisibility::Never,
        })
    }

    fn normalized_relative_path(
        &self,
        canonical_path: &Path,
    ) -> Result<String, NativeResourcePathError> {
        let relative = canonical_path
            .strip_prefix(&self.canonical_path)
            .map_err(|_| NativeResourcePathError::EscapesRoot)?;
        let normalized = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok(normalized)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
just dev cargo test -p yach-backend native_project_path_metadata
```

Expected: PASS for both new metadata tests.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/resource.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add native project path metadata"
```

## Task 2: Local-Only Context Package Reads

**Files:**

- Modify: `crates/yach-backend/src/resource.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing context package tests**

Add these tests near the resource read tests in `crates/yach-backend/src/lib.rs`:

Also add `NativeResourceContextError` and `NativeResourceContextPolicy` to the `use super::{ ... }` list at the top of the test module.

```rust
#[test]
fn native_project_context_package_reads_explicit_text_files_local_only() {
    let root_path = temp_resource_dir("native-resource-context");
    assert!(std::fs::create_dir_all(root_path.join("docs")).is_ok());
    assert!(std::fs::write(root_path.join("docs/one.md"), "one").is_ok());
    assert!(std::fs::write(root_path.join("docs/two.md"), "two").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let package = root.as_ref().and_then(|root| {
        root.read_context_package(
            ["docs/one.md", "docs/two.md"],
            NativeResourceContextPolicy {
                max_file_bytes: 16,
                max_files: 4,
            },
        )
        .ok()
    });

    assert_eq!(package.as_ref().map(|package| package.items.len()), Some(2));
    assert_eq!(
        package.as_ref().map(|package| package.provider_visibility),
        Some(NativeResourceProviderVisibility::Never)
    );
    assert_eq!(
        package.as_ref().map(|package| package.items[0].relative_path.as_str()),
        Some("docs/one.md")
    );
    assert_eq!(
        package.as_ref().map(|package| package.items[0].text.as_str()),
        Some("one")
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn native_project_context_package_enforces_file_count_limit() {
    let root_path = temp_resource_dir("native-resource-context-limit");
    assert!(std::fs::write(root_path.join("one.txt"), "one").is_ok());
    assert!(std::fs::write(root_path.join("two.txt"), "two").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let result = root.as_ref().map(|root| {
        root.read_context_package(
            ["one.txt", "two.txt"],
            NativeResourceContextPolicy {
                max_file_bytes: 16,
                max_files: 1,
            },
        )
    });

    assert_eq!(
        result,
        Some(Err(NativeResourceContextError::TooManyFiles {
            max_files: 1,
            actual_files: 2,
        }))
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_project_context_package
```

Expected: FAIL because context package types and `read_context_package` do not exist.

- [ ] **Step 3: Implement context package types and helper**

Add this code to `crates/yach-backend/src/resource.rs` after `NativeResourcePathMetadata`:

```rust
/// Explicit local-only context read policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeResourceContextPolicy {
    pub max_file_bytes: u64,
    pub max_files: usize,
}

/// Errors produced while packaging local-only context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResourceContextError {
    TooManyFiles {
        max_files: usize,
        actual_files: usize,
    },
    Read {
        relative_path: String,
        error: NativeResourceReadError,
    },
}

/// One text file in a local-only native context package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceContextItem {
    pub relative_path: String,
    pub text: String,
    pub byte_count: usize,
    pub provider_visibility: NativeResourceProviderVisibility,
}

/// Local-only context package assembled from explicit project paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceContextPackage {
    pub items: Vec<NativeResourceContextItem>,
    pub provider_visibility: NativeResourceProviderVisibility,
}
```

Add this method inside `impl NativeResourceRoot`:

```rust
    pub fn read_context_package(
        &self,
        relative_paths: impl IntoIterator<Item = impl AsRef<Path>>,
        policy: NativeResourceContextPolicy,
    ) -> Result<NativeResourceContextPackage, NativeResourceContextError> {
        let paths = relative_paths
            .into_iter()
            .map(|path| path.as_ref().to_path_buf())
            .collect::<Vec<_>>();
        if paths.len() > policy.max_files {
            return Err(NativeResourceContextError::TooManyFiles {
                max_files: policy.max_files,
                actual_files: paths.len(),
            });
        }

        let mut items = Vec::with_capacity(paths.len());
        for path in paths {
            let read = self
                .read_text_file(&path, NativeResourceReadPolicy::local_only(policy.max_file_bytes))
                .map_err(|error| NativeResourceContextError::Read {
                    relative_path: path.to_string_lossy().into_owned(),
                    error,
                })?;
            items.push(NativeResourceContextItem {
                relative_path: self
                    .normalized_relative_path(&read.path)
                    .map_err(|error| NativeResourceContextError::Read {
                        relative_path: path.to_string_lossy().into_owned(),
                        error: NativeResourceReadError::Path(error),
                    })?,
                text: read.text,
                byte_count: read.byte_count,
                provider_visibility: NativeResourceProviderVisibility::Never,
            });
        }

        Ok(NativeResourceContextPackage {
            items,
            provider_visibility: NativeResourceProviderVisibility::Never,
        })
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
just dev cargo test -p yach-backend native_project_context_package
```

Expected: PASS for both context package tests.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/resource.rs crates/yach-backend/src/lib.rs
git commit -m "feat: package local native project context"
```

## Task 3: Bounded Project Search

**Files:**

- Modify: `crates/yach-backend/src/resource.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing search tests**

Add these tests near the resource context tests in `crates/yach-backend/src/lib.rs`:

Also add `NativeResourceSearchPolicy` to the `use super::{ ... }` list at the top of the test module.

```rust
#[test]
fn native_project_search_returns_bounded_local_only_matches() {
    let root_path = temp_resource_dir("native-resource-search");
    assert!(std::fs::create_dir_all(root_path.join("src")).is_ok());
    assert!(std::fs::write(root_path.join("src/lib.rs"), "alpha\nneedle one\n").is_ok());
    assert!(std::fs::write(root_path.join("src/main.rs"), "needle two\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let results = root
        .as_ref()
        .and_then(|root| root.search_text("needle", NativeResourceSearchPolicy::small()).ok());

    assert_eq!(results.as_ref().map(|results| results.matches.len()), Some(2));
    assert_eq!(
        results.as_ref().map(|results| results.matches[0].relative_path.as_str()),
        Some("src/lib.rs")
    );
    assert_eq!(results.as_ref().map(|results| results.matches[0].line_number), Some(2));
    assert_eq!(
        results.as_ref().map(|results| results.matches[0].line.as_str()),
        Some("needle one")
    );
    assert_eq!(
        results.as_ref().map(|results| results.provider_visibility),
        Some(NativeResourceProviderVisibility::Never)
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}

#[test]
fn native_project_search_skips_excluded_and_oversized_files() {
    let root_path = temp_resource_dir("native-resource-search-skip");
    assert!(std::fs::create_dir_all(root_path.join("target")).is_ok());
    assert!(std::fs::write(root_path.join("target/generated.txt"), "needle generated").is_ok());
    assert!(std::fs::write(root_path.join("big.txt"), "needle but too large").is_ok());
    assert!(std::fs::write(root_path.join("ok.txt"), "needle ok").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());

    let results = root.as_ref().and_then(|root| {
        root.search_text(
            "needle",
            NativeResourceSearchPolicy {
                max_file_bytes: 12,
                max_files: 64,
                max_matches: 8,
            },
        )
        .ok()
    });

    assert_eq!(results.as_ref().map(|results| results.matches.len()), Some(1));
    assert_eq!(
        results.as_ref().map(|results| results.matches[0].relative_path.as_str()),
        Some("ok.txt")
    );
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_project_search
```

Expected: FAIL because search types and `search_text` do not exist.

- [ ] **Step 3: Implement search types**

Add this code to `crates/yach-backend/src/resource.rs` after the context package types:

```rust
/// Bounded text search policy for project-local resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeResourceSearchPolicy {
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub max_matches: usize,
}

impl NativeResourceSearchPolicy {
    #[must_use]
    pub const fn small() -> Self {
        Self {
            max_file_bytes: 64 * 1024,
            max_files: 512,
            max_matches: 64,
        }
    }
}

/// One local-only text search match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceSearchMatch {
    pub relative_path: String,
    pub line_number: usize,
    pub line: String,
}

/// Bounded local-only text search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceSearchResult {
    pub matches: Vec<NativeResourceSearchMatch>,
    pub searched_files: usize,
    pub truncated: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}
```

- [ ] **Step 4: Implement recursive bounded search helper**

Add these methods inside `impl NativeResourceRoot`:

```rust
    pub fn search_text(
        &self,
        query: &str,
        policy: NativeResourceSearchPolicy,
    ) -> Result<NativeResourceSearchResult, NativeResourcePathError> {
        let mut result = NativeResourceSearchResult {
            matches: Vec::new(),
            searched_files: 0,
            truncated: false,
            provider_visibility: NativeResourceProviderVisibility::Never,
        };
        if query.is_empty() {
            return Ok(result);
        }

        self.search_directory(self.canonical_path(), query, policy, &mut result)?;
        Ok(result)
    }

    fn search_directory(
        &self,
        directory: &Path,
        query: &str,
        policy: NativeResourceSearchPolicy,
        result: &mut NativeResourceSearchResult,
    ) -> Result<(), NativeResourcePathError> {
        if result.truncated {
            return Ok(());
        }

        let entries = fs::read_dir(directory).map_err(|_| NativeResourcePathError::Missing)?;
        for entry in entries {
            let entry = entry.map_err(|_| NativeResourcePathError::Missing)?;
            let file_type = entry.file_type().map_err(|_| NativeResourcePathError::Missing)?;
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if matches!(file_name.as_str(), ".git" | ".yach" | "target") {
                    continue;
                }
                self.search_directory(&entry.path(), query, policy, result)?;
            } else if file_type.is_file() {
                self.search_file(&entry.path(), query, policy, result)?;
            }
            if result.truncated {
                break;
            }
        }
        Ok(())
    }

    fn search_file(
        &self,
        path: &Path,
        query: &str,
        policy: NativeResourceSearchPolicy,
        result: &mut NativeResourceSearchResult,
    ) -> Result<(), NativeResourcePathError> {
        if result.searched_files >= policy.max_files || result.matches.len() >= policy.max_matches {
            result.truncated = true;
            return Ok(());
        }

        let metadata = fs::metadata(path).map_err(|_| NativeResourcePathError::Missing)?;
        if metadata.len() > policy.max_file_bytes {
            return Ok(());
        }

        let relative_path = self.normalized_relative_path(path)?;
        let read = self.read_text_file(
            Path::new(&relative_path),
            NativeResourceReadPolicy::local_only(policy.max_file_bytes),
        );
        let Ok(read) = read else {
            return Ok(());
        };
        result.searched_files = result.searched_files.saturating_add(1);

        for (line_index, line) in read.text.lines().enumerate() {
            if line.contains(query) {
                result.matches.push(NativeResourceSearchMatch {
                    relative_path: relative_path.clone(),
                    line_number: line_index.saturating_add(1),
                    line: line.to_owned(),
                });
                if result.matches.len() >= policy.max_matches {
                    result.truncated = true;
                    break;
                }
            }
        }
        Ok(())
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
just dev cargo test -p yach-backend native_project_search
```

Expected: PASS for both search tests.

- [ ] **Step 6: Commit**

Run:

```bash
git add crates/yach-backend/src/resource.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add bounded native project search"
```

## Task 4: `project_path_info` Metadata Tool

**Files:**

- Modify: `crates/yach-backend/src/tools.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing tool tests**

Add these tests near the native tool registry/executor tests in `crates/yach-backend/src/lib.rs`:

Also add `ProjectReadOnlyToolExecutor` to the `use super::{ ... }` list at the top of the test module.

```rust
#[test]
fn project_path_info_tool_requires_explicit_metadata_policy() {
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let request = fixture_tool_request(
        "project_path_info",
        serde_json::json!({"path":"Cargo.toml"}),
    );

    let denied = registry.validate_request(&request, &NativeToolPermissionPolicy::deny_all());
    let allowed = registry.validate_request(
        &request,
        &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
    );

    assert_eq!(denied, Err(NativeToolError::PermissionDenied));
    assert_eq!(
        allowed,
        Ok(super::NativeToolValidation {
            request_id: String::from("tool-request-1"),
            tool_name: String::from("project_path_info"),
            permission: NativeToolPermissionState::Allowed,
        })
    );
}

#[test]
fn project_path_info_tool_executes_metadata_without_file_content() {
    let root_path = temp_resource_dir("native-project-path-info-tool");
    assert!(std::fs::write(root_path.join("Cargo.toml"), "[package]\n").is_ok());
    let root = NativeResourceRoot::project(&root_path).ok();
    assert!(root.is_some());
    let registry = NativeToolRegistry::with_project_read_only_tools();
    let request = fixture_tool_request(
        "project_path_info",
        serde_json::json!({"path":"Cargo.toml"}),
    );
    let validation = registry
        .validate_request(
            &request,
            &NativeToolPermissionPolicy::allow_project_metadata_tool("project_path_info"),
        )
        .ok();
    assert!(validation.is_some());

    let Some(root) = root else { return; };
    let executor = ProjectReadOnlyToolExecutor::new(root);
    let result = validation
        .as_ref()
        .map(|validation| executor.execute(&registry, &request, validation));

    assert_eq!(result.as_ref().and_then(|result| result.as_ref().ok()).map(|result| result.redacted), Some(false));
    assert!(result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|result| result.summary.contains("\"relative_path\":\"Cargo.toml\"")));
    assert!(result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|result| !result.summary.contains("[package]")));
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
just dev cargo test -p yach-backend project_path_info_tool
```

Expected: FAIL because `with_project_read_only_tools`, `allow_project_metadata_tool`, and `ProjectReadOnlyToolExecutor` do not exist.

- [ ] **Step 3: Add the tool definition**

Add this associated function to `impl NativeToolDefinition` in `crates/yach-backend/src/tools.rs`:

```rust
    #[must_use]
    pub fn project_path_info() -> Self {
        Self {
            name: String::from("project_path_info"),
            description: String::from(
                "Return local-only project path metadata without reading file contents.",
            ),
            input_schema: NativeToolInputSchema::string_object(["path"], [], 1024),
            risk: NativeToolRisk::ReadsLocalMetadata,
        }
    }
```

Add this constructor to `impl NativeToolRegistry`:

```rust
    #[must_use]
    pub fn with_project_read_only_tools() -> Self {
        Self {
            definitions: vec![NativeToolDefinition::project_path_info()],
        }
    }
```

- [ ] **Step 4: Add metadata permission policy**

Change `NativeToolPermissionPolicy` to:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolPermissionPolicy {
    allowed_fixture_tools: BTreeSet<String>,
    allowed_project_metadata_tools: BTreeSet<String>,
}
```

Update `allow_fixture_tool`:

```rust
    #[must_use]
    pub fn allow_fixture_tool(name: impl Into<String>) -> Self {
        Self {
            allowed_fixture_tools: BTreeSet::from([name.into()]),
            allowed_project_metadata_tools: BTreeSet::new(),
        }
    }
```

Add this method:

```rust
    #[must_use]
    pub fn allow_project_metadata_tool(name: impl Into<String>) -> Self {
        Self {
            allowed_fixture_tools: BTreeSet::new(),
            allowed_project_metadata_tools: BTreeSet::from([name.into()]),
        }
    }
```

Update `authorize`:

```rust
    #[must_use]
    pub fn authorize(&self, definition: &NativeToolDefinition) -> NativeToolPermissionState {
        let allowed = match definition.risk {
            NativeToolRisk::FixtureSafe => self.allowed_fixture_tools.contains(&definition.name),
            NativeToolRisk::ReadsLocalMetadata => {
                self.allowed_project_metadata_tools.contains(&definition.name)
            }
            NativeToolRisk::ReadsLocalContent
            | NativeToolRisk::MutatesLocalState
            | NativeToolRisk::UsesNetwork
            | NativeToolRisk::RunsProcess => false,
        };

        if allowed {
            NativeToolPermissionState::Allowed
        } else {
            NativeToolPermissionState::Denied
        }
    }
```

- [ ] **Step 5: Implement metadata executor**

Add imports at the top of `crates/yach-backend/src/tools.rs`:

```rust
use crate::{NativeResourceRoot, NativeResourcePathError};
```

If that duplicates the existing `crate::{ ... }` import, merge the names into the existing grouped import.

Add this code after `FixtureNativeToolExecutor`:

```rust
/// Read-only project tool executor for local metadata-only tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReadOnlyToolExecutor {
    root: NativeResourceRoot,
}

impl ProjectReadOnlyToolExecutor {
    #[must_use]
    pub fn new(root: NativeResourceRoot) -> Self {
        Self { root }
    }
}

impl NativeToolExecutor for ProjectReadOnlyToolExecutor {
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
        if definition.name != "project_path_info"
            || definition.risk != NativeToolRisk::ReadsLocalMetadata
        {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }

        let Some(path) = request.arguments.get("path").and_then(serde_json::Value::as_str) else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        let metadata = self.root.path_metadata(path).map_err(|error| {
            NativeToolExecutionError::ResourcePath {
                error,
            }
        })?;
        let summary = serde_json::json!({
            "relative_path": metadata.relative_path,
            "kind": match metadata.kind {
                crate::NativeResourceEntryKind::File => "file",
                crate::NativeResourceEntryKind::Directory => "directory",
                crate::NativeResourceEntryKind::Other => "other",
            },
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
}
```

Extend `NativeToolExecutionError` with:

```rust
    ResourcePath {
        error: NativeResourcePathError,
    },
```

Update exhaustive matches if the compiler points to any.

- [ ] **Step 6: Run tests to verify they pass**

Run:

```bash
just dev cargo test -p yach-backend project_path_info_tool
```

Expected: PASS for both `project_path_info_tool_*` tests.

- [ ] **Step 7: Commit**

Run:

```bash
git add crates/yach-backend/src/tools.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add native project path info tool"
```

## Task 5: Resource Metrics and Native Status

**Files:**

- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Add failing status test**

Add this test to `crates/yach-backend/src/native_runner.rs` in the existing `#[cfg(test)]` module:

```rust
#[test]
fn native_status_reports_local_read_only_resources_available() {
    let status = native_status_message(None);

    assert_eq!(
        status,
        "backend: native dogfood; local read-only project inspection available; provider tools unavailable"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_status_reports_local_read_only_resources_available -- --exact
```

Expected: FAIL because the current status still says tools/resources are unavailable.

- [ ] **Step 3: Update status text only**

Change the fixture-native branch in `native_status_message` to:

```rust
        String::from(
            "backend: native dogfood; local read-only project inspection available; provider tools unavailable",
        )
```

Leave the native-provider status conservative unless this slice wires read-only primitives into that runtime.

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
just dev cargo test -p yach-backend native_status_reports_local_read_only_resources_available -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "docs: report native read-only resource availability"
```

## Task 6: Native Resource Benchmark Baseline

**Files:**

- Modify: `crates/yach-bench/Cargo.toml`
- Create: `crates/yach-bench/benches/native_resource.rs`

- [ ] **Step 1: Add bench target**

Add this bench target to `crates/yach-bench/Cargo.toml`:

```toml
[[bench]]
name = "native_resource"
harness = false
```

- [ ] **Step 2: Create benchmark file**

Create `crates/yach-bench/benches/native_resource.rs`:

```rust
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use yach_backend::{
    NativeResourceContextPolicy, NativeResourceRoot, NativeResourceSearchPolicy,
};

fn bench_native_resource(c: &mut Criterion) {
    let root_path = fixture_project();
    let Ok(root) = NativeResourceRoot::project(&root_path) else {
        process::abort();
    };

    c.bench_function("native_resource_path_metadata", |b| {
        b.iter(|| root.path_metadata("src/file-010.rs"))
    });
    c.bench_function("native_resource_context_10_files", |b| {
        b.iter(|| {
            root.read_context_package(
                (0..10).map(|index| format!("src/file-{index:03}.rs")),
                NativeResourceContextPolicy {
                    max_file_bytes: 4096,
                    max_files: 16,
                },
            )
        })
    });
    c.bench_function("native_resource_search_100_files", |b| {
        b.iter(|| root.search_text("needle", NativeResourceSearchPolicy::small()))
    });

    let _ = fs::remove_dir_all(root_path);
}

fn fixture_project() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root = std::env::temp_dir().join(format!("yach-native-resource-bench-{unique}"));
    let src = root.join("src");
    if fs::create_dir_all(&src).is_err() {
        process::abort();
    }
    for index in 0..100 {
        let content = if index % 10 == 0 {
            format!("pub fn file_{index}() {{}}\n// needle\n")
        } else {
            format!("pub fn file_{index}() {{}}\n")
        };
        if fs::write(src.join(format!("file-{index:03}.rs")), content).is_err() {
            process::abort();
        }
    }
    root
}

criterion_group!(benches, bench_native_resource);
criterion_main!(benches);
```

- [ ] **Step 3: Run benchmark compile check**

Run:

```bash
just dev cargo test -p yach-bench --bench native_resource --no-run
```

Expected: PASS compile check for the benchmark.

- [ ] **Step 4: Commit**

Run:

```bash
git add crates/yach-bench/Cargo.toml crates/yach-bench/benches/native_resource.rs
git commit -m "bench: add native resource baseline"
```

## Task 7: Full Verification and Planning Docs

**Files:**

- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run formatting**

Run:

```bash
just dev cargo fmt --all
```

Expected: no formatting failures.

- [ ] **Step 2: Run backend lint**

Run:

```bash
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
```

Expected: PASS with no warnings.

- [ ] **Step 3: Run backend tests**

Run:

```bash
just dev cargo test -p yach-backend
```

Expected: PASS.

- [ ] **Step 4: Run bench compile check**

Run:

```bash
just dev cargo test -p yach-bench --bench native_resource --no-run
```

Expected: PASS.

- [ ] **Step 5: Check whitespace**

Run:

```bash
git diff --check
```

Expected: no output.

- [ ] **Step 6: Update project planning docs**

In `docs/project/state.md`, update `Current Posture` to mention that native read-only project inspection now has backend primitives for path metadata, explicit local-only text context packages, bounded search, and a metadata-only project path tool.

In `docs/project/next.md`, replace the recommended first slice with the next Native MVP blocker after this work. Use this wording unless implementation reveals a different blocker:

```markdown
Recommended next move: continue Native MVP implementation with autonomous tool-call loop integration for safe read-only tools.

Why: read-only project inspection primitives now exist locally, but real dogfooding still needs model-requested tool execution, policy checks, result shaping, and provider continuation before file edits.
```

- [ ] **Step 7: Commit planning docs**

Run:

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update native read search context status"
```

## Final Verification

Run:

```bash
just dev cargo fmt --all
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
just dev cargo test -p yach-bench --bench native_resource --no-run
git diff --check
```

Expected:

- formatting completes successfully;
- clippy reports no warnings;
- backend tests pass;
- native resource bench compile check passes;
- `git diff --check` prints no whitespace errors.

## Stop Gates

Stop and ask before:

- sending file contents or search results to a real provider;
- integrating these helpers into `--backend native-provider`;
- adding file mutation tools;
- adding process, shell, or network tools;
- persisting raw file contents in native session logs;
- changing `yach-proto` for resource/tool UI;
- defaulting to the native backend.
