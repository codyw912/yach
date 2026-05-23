# Extension Local Install Records Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add durable local-path extension install records and make `yach extension list/doctor` inspect installed local packages without spawning extension hosts.

**Architecture:** Add a focused backend install-record module beside the existing extension manifest/runtime module. The CLI owns command parsing and store path selection, while the backend owns ref parsing, settings JSON, record mutation, and conversion from enabled local-path records to `ExtensionPackageRoot` values. Runtime startup may consume enabled user-scope records after first paint, but this slice must not add real host activation, npm/git materialization, or package-manager execution.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, existing `ExtensionInstallScope`, `ExtensionPackageRoot`, `ExtensionManifestIndex`, `just dev cargo test`, `just lint`, `jj` checkpoints.

---

## File Structure

- Create `crates/yach-backend/src/extension_install.rs`
  - Owns install ref parsing, install settings JSON, store load/save, record mutation, and conversion to package roots.
  - Does not parse manifests, spawn hosts, fetch network refs, or know about CLI rendering.
- Modify `crates/yach-backend/src/lib.rs`
  - Adds `mod extension_install;` and `pub use extension_install::*;`.
- Modify `crates/yach-cli/src/main.rs`
  - Parses `yach install`, `yach extension install/remove/enable/disable/list/doctor`.
  - Selects user/project store paths with testable environment overrides.
  - Combines enabled install records with `YACH_EXTENSION_PACKAGE_ROOTS` for diagnostics and native startup package-root collection.
- Modify `docs/project/state.md` and `docs/project/next.md`
  - Only after implementation lands, mark local-path install records as implemented and point next work at persistent process-host transport.

## Scope Boundaries

This plan implements only local-path install records.

In scope:

- `yach install <local-path>` alias.
- `yach extension install <local-path> [--user|--project] [--disabled]`.
- `yach extension remove|enable|disable <id-or-ref> [--user|--project]`.
- `yach extension list` and `doctor` showing install-record state plus manifest scan state.
- Parsed but unavailable npm/git/http/ssh refs with categorical errors.
- Environment overrides for tests:
  - `YACH_EXTENSION_USER_STORE`
  - `YACH_EXTENSION_PROJECT_STORE`
- Existing `YACH_EXTENSION_PACKAGE_ROOTS` remains supported as ephemeral package roots.

Out of scope:

- Npm/git package materialization.
- Dependency install.
- Real persistent process host launch.
- Activation state beyond `enabled`, `disabled`, `discovered`, and scan failures.
- Project trust enforcement beyond keeping project records explicit and visible.
- Startup benchmarks unless the implementation unexpectedly moves record loading before first paint.

## Task 1: Backend Install Record Model

**Files:**
- Create: `crates/yach-backend/src/extension_install.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Test: `crates/yach-backend/src/extension_install.rs`

- [ ] **Step 1: Add the module export**

In `crates/yach-backend/src/lib.rs`, add the module beside `mod extension;`:

```rust
mod extension;
mod extension_install;
```

Add the public re-export beside `pub use extension::*;`:

```rust
pub use extension::*;
pub use extension_install::*;
```

- [ ] **Step 2: Create failing ref parser tests**

Create `crates/yach-backend/src/extension_install.rs` with imports and tests first:

```rust
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{ExtensionInstallScope, ExtensionPackageRoot};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_install_ref_parses_local_paths() {
        let relative = parse_extension_install_ref("./extensions/fff").unwrap();
        assert_eq!(relative.kind, ExtensionInstallRefKind::LocalPath);
        assert_eq!(relative.normalized, "./extensions/fff");

        let absolute = parse_extension_install_ref("/tmp/yach-extension").unwrap();
        assert_eq!(absolute.kind, ExtensionInstallRefKind::LocalPath);
        assert_eq!(absolute.normalized, "/tmp/yach-extension");
    }

    #[test]
    fn extension_install_ref_parses_future_remote_refs() {
        let npm = parse_extension_install_ref("npm:@scope/pkg@1.2.3").unwrap();
        assert_eq!(npm.kind, ExtensionInstallRefKind::Npm);
        assert_eq!(npm.normalized, "npm:@scope/pkg@1.2.3");

        let git = parse_extension_install_ref("git:github.com/example/tools@v1").unwrap();
        assert_eq!(git.kind, ExtensionInstallRefKind::Git);
        assert_eq!(git.normalized, "git:github.com/example/tools@v1");

        let https = parse_extension_install_ref("https://github.com/example/tools").unwrap();
        assert_eq!(https.kind, ExtensionInstallRefKind::Git);
        assert_eq!(https.normalized, "https://github.com/example/tools");
    }

    #[test]
    fn extension_install_ref_rejects_empty_ref() {
        assert_eq!(
            parse_extension_install_ref(""),
            Err(ExtensionInstallError::EmptyRef)
        );
    }
}
```

- [ ] **Step 3: Run parser tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend extension_install_ref_ -- --nocapture
```

Expected: compile failure for missing `ExtensionInstallRefKind`, `parse_extension_install_ref`, and `ExtensionInstallError`.

- [ ] **Step 4: Implement ref parsing types**

Add above the tests:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionInstallRefKind {
    LocalPath,
    Npm,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInstallRef {
    pub kind: ExtensionInstallRefKind,
    pub normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionInstallError {
    EmptyRef,
    UnsupportedRef { source: String },
    AdapterUnavailable { kind: ExtensionInstallRefKind },
    MissingLocalPath { path: PathBuf },
    StoreIo,
    StoreMalformed,
    RecordNotFound { selector: String },
}

pub fn parse_extension_install_ref(source: &str) -> Result<ExtensionInstallRef, ExtensionInstallError> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(ExtensionInstallError::EmptyRef);
    }
    let kind = if trimmed.starts_with("npm:") {
        ExtensionInstallRefKind::Npm
    } else if trimmed.starts_with("git:")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with("git@")
    {
        ExtensionInstallRefKind::Git
    } else if trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
        || trimmed == "."
        || trimmed == ".."
    {
        ExtensionInstallRefKind::LocalPath
    } else {
        return Err(ExtensionInstallError::UnsupportedRef {
            source: trimmed.to_owned(),
        });
    };
    Ok(ExtensionInstallRef {
        kind,
        normalized: trimmed.to_owned(),
    })
}
```

- [ ] **Step 5: Run parser tests and verify they pass**

Run:

```bash
just dev cargo test -p yach-backend extension_install_ref_ -- --nocapture
```

Expected: all `extension_install_ref_` tests pass.

- [ ] **Step 6: Checkpoint**

Run:

```bash
jj describe -m "feat: parse extension install refs"
jj new
```

## Task 2: Backend Store Load/Save And Local-Path Mutation

**Files:**
- Modify: `crates/yach-backend/src/extension_install.rs`
- Test: `crates/yach-backend/src/extension_install.rs`

- [ ] **Step 1: Add failing store tests**

Add these tests to the existing test module:

```rust
#[test]
fn extension_install_store_round_trips_records() {
    let root = TempDir::new("store-round-trip");
    let package = root.path().join("packages/fff");
    fs::create_dir_all(&package).unwrap();
    let store_path = root.path().join("extensions.json");

    let mut store = ExtensionInstallStore::default();
    store
        .install_local_path(
            "./packages/fff",
            &package,
            ExtensionInstallScope::User,
            true,
        )
        .unwrap();
    store.save_to_path(&store_path).unwrap();

    let loaded = ExtensionInstallStore::load_from_path(&store_path).unwrap();
    assert_eq!(loaded.records.len(), 1);
    assert_eq!(loaded.records[0].source, "./packages/fff");
    assert_eq!(loaded.records[0].kind, ExtensionInstallRefKind::LocalPath);
    assert_eq!(loaded.records[0].scope, ExtensionInstallScope::User);
    assert!(loaded.records[0].enabled);
    assert_eq!(loaded.records[0].package_root, package);
}

#[test]
fn extension_install_store_rejects_unavailable_remote_adapters() {
    let mut store = ExtensionInstallStore::default();
    let error = store.install_ref("npm:fff", ExtensionInstallScope::User, true);
    assert_eq!(
        error,
        Err(ExtensionInstallError::AdapterUnavailable {
            kind: ExtensionInstallRefKind::Npm
        })
    );
}

#[test]
fn extension_install_store_remove_enable_disable_by_source() {
    let root = TempDir::new("store-toggle");
    let package = root.path().join("fff");
    fs::create_dir_all(&package).unwrap();

    let mut store = ExtensionInstallStore::default();
    store
        .install_local_path("./fff", &package, ExtensionInstallScope::Project, true)
        .unwrap();

    store.set_enabled("./fff", false).unwrap();
    assert!(!store.records[0].enabled);

    store.set_enabled("./fff", true).unwrap();
    assert!(store.records[0].enabled);

    store.remove("./fff").unwrap();
    assert!(store.records.is_empty());
}
```

Also add a local test temp helper:

```rust
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "yach-extension-install-{name}-{}-{timestamp}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
```

- [ ] **Step 2: Run store tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend extension_install_store_ -- --nocapture
```

Expected: compile failure for missing store and record types.

- [ ] **Step 3: Implement store and record types**

Add above tests:

```rust
const EXTENSION_INSTALL_STORE_SCHEMA: &str = "yach.extensions.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionInstallStore {
    pub schema: String,
    pub records: Vec<ExtensionInstallRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionInstallRecord {
    pub source: String,
    pub kind: ExtensionInstallRefKind,
    pub scope: ExtensionInstallScope,
    pub enabled: bool,
    pub package_root: PathBuf,
}

impl Default for ExtensionInstallStore {
    fn default() -> Self {
        Self {
            schema: EXTENSION_INSTALL_STORE_SCHEMA.to_owned(),
            records: Vec::new(),
        }
    }
}

impl ExtensionInstallStore {
    pub fn load_from_path(path: &Path) -> Result<Self, ExtensionInstallError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(path).map_err(|_| ExtensionInstallError::StoreIo)?;
        let store: Self =
            serde_json::from_str(&contents).map_err(|_| ExtensionInstallError::StoreMalformed)?;
        if store.schema != EXTENSION_INSTALL_STORE_SCHEMA {
            return Err(ExtensionInstallError::StoreMalformed);
        }
        Ok(store)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<(), ExtensionInstallError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| ExtensionInstallError::StoreIo)?;
        }
        let contents =
            serde_json::to_string_pretty(self).map_err(|_| ExtensionInstallError::StoreIo)?;
        fs::write(path, contents).map_err(|_| ExtensionInstallError::StoreIo)
    }

    pub fn install_ref(
        &mut self,
        source: &str,
        scope: ExtensionInstallScope,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        let parsed = parse_extension_install_ref(source)?;
        match parsed.kind {
            ExtensionInstallRefKind::LocalPath => {
                let root = PathBuf::from(&parsed.normalized);
                self.install_local_path(&parsed.normalized, &root, scope, enabled)
            }
            ExtensionInstallRefKind::Npm | ExtensionInstallRefKind::Git => {
                Err(ExtensionInstallError::AdapterUnavailable { kind: parsed.kind })
            }
        }
    }

    pub fn install_local_path(
        &mut self,
        source: &str,
        package_root: &Path,
        scope: ExtensionInstallScope,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        if !package_root.is_dir() {
            return Err(ExtensionInstallError::MissingLocalPath {
                path: package_root.to_path_buf(),
            });
        }
        let record = ExtensionInstallRecord {
            source: source.to_owned(),
            kind: ExtensionInstallRefKind::LocalPath,
            scope,
            enabled,
            package_root: package_root.to_path_buf(),
        };
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|record| record.source == source || record.package_root == package_root)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
        self.records.sort_by(|left, right| left.source.cmp(&right.source));
        Ok(())
    }

    pub fn remove(&mut self, selector: &str) -> Result<(), ExtensionInstallError> {
        let before = self.records.len();
        self.records
            .retain(|record| record.source != selector && record.package_root != PathBuf::from(selector));
        if self.records.len() == before {
            return Err(ExtensionInstallError::RecordNotFound {
                selector: selector.to_owned(),
            });
        }
        Ok(())
    }

    pub fn set_enabled(
        &mut self,
        selector: &str,
        enabled: bool,
    ) -> Result<(), ExtensionInstallError> {
        let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.source == selector || record.package_root == PathBuf::from(selector))
        else {
            return Err(ExtensionInstallError::RecordNotFound {
                selector: selector.to_owned(),
            });
        };
        record.enabled = enabled;
        Ok(())
    }
}
```

- [ ] **Step 4: Run store tests and verify they pass**

Run:

```bash
just dev cargo test -p yach-backend extension_install_store_ -- --nocapture
```

Expected: all store tests pass.

- [ ] **Step 5: Run all backend install tests**

Run:

```bash
just dev cargo test -p yach-backend extension_install -- --nocapture
```

Expected: all extension install tests pass.

- [ ] **Step 6: Checkpoint**

Run:

```bash
jj describe -m "feat: add extension install record store"
jj new
```

## Task 3: Convert Enabled Records To Package Roots

**Files:**
- Modify: `crates/yach-backend/src/extension_install.rs`
- Test: `crates/yach-backend/src/extension_install.rs`

- [ ] **Step 1: Add failing package-root conversion tests**

Add tests:

```rust
#[test]
fn extension_install_store_enabled_package_roots_excludes_disabled_records() {
    let root = TempDir::new("package-roots");
    let enabled = root.path().join("enabled");
    let disabled = root.path().join("disabled");
    fs::create_dir_all(&enabled).unwrap();
    fs::create_dir_all(&disabled).unwrap();

    let mut store = ExtensionInstallStore::default();
    store
        .install_local_path("./enabled", &enabled, ExtensionInstallScope::User, true)
        .unwrap();
    store
        .install_local_path("./disabled", &disabled, ExtensionInstallScope::User, false)
        .unwrap();

    let roots = store.enabled_package_roots();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].root, enabled);
    assert_eq!(roots[0].scope, ExtensionInstallScope::User);
    assert_eq!(roots[0].source_ref.as_deref(), Some("./enabled"));
}

#[test]
fn extension_install_store_enabled_package_roots_preserves_project_scope() {
    let root = TempDir::new("project-roots");
    let package = root.path().join("project-package");
    fs::create_dir_all(&package).unwrap();

    let mut store = ExtensionInstallStore::default();
    store
        .install_local_path("./project-package", &package, ExtensionInstallScope::Project, true)
        .unwrap();

    let roots = store.enabled_package_roots();
    assert_eq!(roots[0].scope, ExtensionInstallScope::Project);
}
```

- [ ] **Step 2: Run package-root conversion tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend enabled_package_roots -- --nocapture
```

Expected: compile failure for missing `enabled_package_roots`.

- [ ] **Step 3: Implement package-root conversion**

Add to `impl ExtensionInstallStore`:

```rust
pub fn enabled_package_roots(&self) -> Vec<ExtensionPackageRoot> {
    self.records
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| record.kind == ExtensionInstallRefKind::LocalPath)
        .map(|record| ExtensionPackageRoot {
            root: record.package_root.clone(),
            scope: record.scope,
            source_ref: Some(record.source.clone()),
        })
        .collect()
}
```

- [ ] **Step 4: Run package-root conversion tests and verify they pass**

Run:

```bash
just dev cargo test -p yach-backend enabled_package_roots -- --nocapture
```

Expected: all package-root conversion tests pass.

- [ ] **Step 5: Checkpoint**

Run:

```bash
jj describe -m "feat: collect installed extension package roots"
jj new
```

## Task 4: CLI Install Management Commands

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Test: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Add failing CLI parser tests**

In `crates/yach-cli/src/main.rs`, extend the test module with parser coverage:

```rust
#[test]
fn cli_parses_extension_install_management_commands() {
    assert_eq!(
        CliArgs::from_args(["install", "./ext"].into_iter().map(String::from)).command,
        Command::ExtensionInstall {
            source: String::from("./ext"),
            scope: ExtensionInstallScope::User,
            enabled: true,
        }
    );
    assert_eq!(
        CliArgs::from_args(
            ["extension", "install", "./ext", "--project", "--disabled"]
                .into_iter()
                .map(String::from),
        )
        .command,
        Command::ExtensionInstall {
            source: String::from("./ext"),
            scope: ExtensionInstallScope::Project,
            enabled: false,
        }
    );
    assert_eq!(
        CliArgs::from_args(["extension", "disable", "./ext"].into_iter().map(String::from))
            .command,
        Command::ExtensionSetEnabled {
            selector: String::from("./ext"),
            scope: ExtensionInstallScope::User,
            enabled: false,
        }
    );
}
```

- [ ] **Step 2: Run parser test and verify it fails**

Run:

```bash
just dev cargo test -p yach-cli cli_parses_extension_install_management_commands -- --nocapture
```

Expected: compile failure for missing command variants.

- [ ] **Step 3: Add command variants and parser helpers**

Extend `Command`:

```rust
ExtensionInstall {
    source: String,
    scope: ExtensionInstallScope,
    enabled: bool,
},
ExtensionRemove {
    selector: String,
    scope: ExtensionInstallScope,
},
ExtensionSetEnabled {
    selector: String,
    scope: ExtensionInstallScope,
    enabled: bool,
},
```

Update `CliArgs::from_args`:

```rust
Some("install") => extension_install_command_from_args(&positional[1..]),
Some("extension") => extension_command_from_args(&positional[1..]),
```

Replace `extension_command_from_args` with:

```rust
fn extension_command_from_args(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("install") => extension_install_command_from_args(&args[1..]),
        Some("remove") => extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Remove),
        Some("enable") => extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Enable),
        Some("disable") => extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Disable),
        Some("doctor") => Command::ExtensionDoctor {
            extension_id: args.get(1).cloned(),
        },
        _ => Command::ExtensionList,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionSelectorAction {
    Remove,
    Enable,
    Disable,
}

fn extension_install_command_from_args(args: &[String]) -> Command {
    let scope = extension_scope_from_args(args);
    let enabled = !args.iter().any(|arg| arg == "--disabled");
    let source = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    Command::ExtensionInstall {
        source,
        scope,
        enabled,
    }
}

fn extension_selector_command_from_args(args: &[String], action: ExtensionSelectorAction) -> Command {
    let scope = extension_scope_from_args(args);
    let selector = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    match action {
        ExtensionSelectorAction::Remove => Command::ExtensionRemove { selector, scope },
        ExtensionSelectorAction::Enable => Command::ExtensionSetEnabled {
            selector,
            scope,
            enabled: true,
        },
        ExtensionSelectorAction::Disable => Command::ExtensionSetEnabled {
            selector,
            scope,
            enabled: false,
        },
    }
}

fn extension_scope_from_args(args: &[String]) -> ExtensionInstallScope {
    if args.iter().any(|arg| arg == "--project") {
        ExtensionInstallScope::Project
    } else {
        ExtensionInstallScope::User
    }
}
```

- [ ] **Step 4: Run parser test and verify it passes**

Run:

```bash
just dev cargo test -p yach-cli cli_parses_extension_install_management_commands -- --nocapture
```

Expected: parser test passes.

- [ ] **Step 5: Add failing command rendering tests**

Add tests:

```rust
#[test]
fn extension_install_management_renders_stable_lines() {
    let result = CommandResult::ExtensionManagement {
        action: ExtensionManagementAction::Install,
        outcome: ExtensionManagementOutcome::Completed,
        scope: ExtensionInstallScope::User,
        message: Some(String::from("installed ./ext")),
    };
    assert_eq!(
        result.render_lines(),
        vec![
            "extension_action=install",
            "extension_outcome=Completed",
            "extension_scope=user",
            "message=installed ./ext",
        ]
    );
}
```

- [ ] **Step 6: Implement management render types**

Add enums near existing extension diagnostics enums:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionManagementAction {
    Install,
    Remove,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionManagementOutcome {
    Completed,
    Failed,
}
```

Add a `CommandResult` variant:

```rust
ExtensionManagement {
    action: ExtensionManagementAction,
    outcome: ExtensionManagementOutcome,
    scope: ExtensionInstallScope,
    message: Option<String>,
},
```

Render it:

```rust
Self::ExtensionManagement {
    action,
    outcome,
    scope,
    message,
} => {
    let mut lines = vec![
        format!("extension_action={}", extension_management_action_label(*action)),
        format!("extension_outcome={outcome:?}"),
        format!("extension_scope={}", extension_install_scope_label(*scope)),
    ];
    if let Some(message) = message {
        lines.push(format!("message={message}"));
    }
    lines
}
```

Add:

```rust
const fn extension_management_action_label(action: ExtensionManagementAction) -> &'static str {
    match action {
        ExtensionManagementAction::Install => "install",
        ExtensionManagementAction::Remove => "remove",
        ExtensionManagementAction::Enable => "enable",
        ExtensionManagementAction::Disable => "disable",
    }
}
```

- [ ] **Step 7: Run CLI tests**

Run:

```bash
just dev cargo test -p yach-cli extension_install_management -- --nocapture
```

Expected: parser and rendering tests pass.

- [ ] **Step 8: Checkpoint**

Run:

```bash
jj describe -m "feat: parse extension install commands"
jj new
```

## Task 5: CLI Store Paths And Management Execution

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Test: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Add backend imports**

Extend the `yach_backend` import:

```rust
ExtensionInstallError, ExtensionInstallStore,
```

- [ ] **Step 2: Add failing store path tests**

Add tests:

```rust
#[test]
fn extension_store_path_uses_environment_overrides() {
    let temp = std::env::temp_dir().join(format!(
        "yach-cli-extension-store-test-{}",
        std::process::id()
    ));
    let user = temp.join("user.json");
    let project = temp.join("project.json");

    unsafe {
        std::env::set_var("YACH_EXTENSION_USER_STORE", &user);
        std::env::set_var("YACH_EXTENSION_PROJECT_STORE", &project);
    }

    assert_eq!(extension_store_path(ExtensionInstallScope::User).unwrap(), user);
    assert_eq!(
        extension_store_path(ExtensionInstallScope::Project).unwrap(),
        project
    );

    unsafe {
        std::env::remove_var("YACH_EXTENSION_USER_STORE");
        std::env::remove_var("YACH_EXTENSION_PROJECT_STORE");
    }
}
```

- [ ] **Step 3: Implement store path selection**

Add:

```rust
fn extension_store_path(scope: ExtensionInstallScope) -> io::Result<PathBuf> {
    match scope {
        ExtensionInstallScope::User => {
            if let Some(path) = std::env::var_os("YACH_EXTENSION_USER_STORE").map(PathBuf::from) {
                return Ok(path);
            }
            let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set"));
            };
            Ok(home.join(".yach/extensions.json"))
        }
        ExtensionInstallScope::Project => {
            if let Some(path) = std::env::var_os("YACH_EXTENSION_PROJECT_STORE").map(PathBuf::from) {
                return Ok(path);
            }
            Ok(std::env::current_dir()?.join(".yach/extensions.json"))
        }
        ExtensionInstallScope::Ephemeral => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ephemeral extension store is runtime-only",
        )),
    }
}
```

- [ ] **Step 4: Add command execution functions**

Add match arms in `Command::run`:

```rust
Self::ExtensionInstall {
    source,
    scope,
    enabled,
} => run_extension_install_command(source, *scope, *enabled),
Self::ExtensionRemove { selector, scope } => run_extension_remove_command(selector, *scope),
Self::ExtensionSetEnabled {
    selector,
    scope,
    enabled,
} => run_extension_set_enabled_command(selector, *scope, *enabled),
```

Add helpers:

```rust
fn run_extension_install_command(
    source: &str,
    scope: ExtensionInstallScope,
    enabled: bool,
) -> CommandResult {
    let action = ExtensionManagementAction::Install;
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(extension_install_io_error)?;
        store
            .install_ref(source, scope, enabled)
            .map_err(extension_install_io_error)?;
        store.save_to_path(&path).map_err(extension_install_io_error)?;
        Ok(format!("installed {source}"))
    })();
    extension_management_result(action, scope, result)
}

fn run_extension_remove_command(selector: &str, scope: ExtensionInstallScope) -> CommandResult {
    let action = ExtensionManagementAction::Remove;
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(extension_install_io_error)?;
        store.remove(selector).map_err(extension_install_io_error)?;
        store.save_to_path(&path).map_err(extension_install_io_error)?;
        Ok(format!("removed {selector}"))
    })();
    extension_management_result(action, scope, result)
}

fn run_extension_set_enabled_command(
    selector: &str,
    scope: ExtensionInstallScope,
    enabled: bool,
) -> CommandResult {
    let action = if enabled {
        ExtensionManagementAction::Enable
    } else {
        ExtensionManagementAction::Disable
    };
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(extension_install_io_error)?;
        store
            .set_enabled(selector, enabled)
            .map_err(extension_install_io_error)?;
        store.save_to_path(&path).map_err(extension_install_io_error)?;
        Ok(format!(
            "{} {selector}",
            if enabled { "enabled" } else { "disabled" }
        ))
    })();
    extension_management_result(action, scope, result)
}

fn extension_management_result(
    action: ExtensionManagementAction,
    scope: ExtensionInstallScope,
    result: io::Result<String>,
) -> CommandResult {
    match result {
        Ok(message) => CommandResult::ExtensionManagement {
            action,
            outcome: ExtensionManagementOutcome::Completed,
            scope,
            message: Some(message),
        },
        Err(error) => CommandResult::ExtensionManagement {
            action,
            outcome: ExtensionManagementOutcome::Failed,
            scope,
            message: Some(error.to_string()),
        },
    }
}

fn extension_install_io_error(error: ExtensionInstallError) -> io::Error {
    io::Error::other(format!(
        "extension install failed: {}",
        extension_install_error_label(&error)
    ))
}

fn extension_install_error_label(error: &ExtensionInstallError) -> &'static str {
    match error {
        ExtensionInstallError::EmptyRef => "empty_ref",
        ExtensionInstallError::UnsupportedRef { .. } => "unsupported_ref",
        ExtensionInstallError::AdapterUnavailable { .. } => "adapter_unavailable",
        ExtensionInstallError::MissingLocalPath { .. } => "missing_local_path",
        ExtensionInstallError::StoreIo => "store_io",
        ExtensionInstallError::StoreMalformed => "store_malformed",
        ExtensionInstallError::RecordNotFound { .. } => "record_not_found",
    }
}
```

- [ ] **Step 5: Run CLI management tests**

Run:

```bash
just dev cargo test -p yach-cli extension_ -- --nocapture
```

Expected: extension CLI tests pass.

- [ ] **Step 6: Run a manual local install smoke**

Run:

```bash
tmp="$(mktemp -d)"
mkdir -p "$tmp/ext"
printf '%s\n' '{"schema":"yach.extension.v1","id":"example.local-install","version":"0.1.0","main":{"command":"node","args":["./extension.js"]},"contributes":{"tools":[]}}' > "$tmp/ext/yach.extension.json"
YACH_EXTENSION_USER_STORE="$tmp/extensions.json" just dev cargo run -p yach-cli -- extension install "$tmp/ext"
YACH_EXTENSION_USER_STORE="$tmp/extensions.json" just dev cargo run -p yach-cli -- extension list
```

Expected output includes:

```text
extension_action=install
extension_outcome=Completed
extension_command=list
extension_outcome=Completed
extension_count=1
```

- [ ] **Step 7: Checkpoint**

Run:

```bash
jj describe -m "feat: manage local extension install records"
jj new
```

## Task 6: Diagnostics Include Installed Records And Existing Env Roots

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Test: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Add diagnostic record state**

Extend `ExtensionDiagnosticRecord`:

```rust
id: Option<String>,
version: Option<String>,
scope: ExtensionInstallScope,
package_root: PathBuf,
manifest_path: Option<PathBuf>,
source_ref: Option<String>,
install_source: Option<String>,
install_enabled: bool,
discovered: bool,
```

Update `render_line` to include:

```rust
"extension id={} version={} scope={} package_root={} manifest_path={} source_ref={} install_source={} install_enabled={} discovered={}"
```

Use `as_deref().unwrap_or("none")` for absent `id`, `version`,
`manifest_path`, `source_ref`, and `install_source`. Existing discovered
manifest records should set `discovered=true`; disabled install-only records
should set `id=None`, `version=None`, `manifest_path=None`, and
`discovered=false`.

- [ ] **Step 2: Add failing diagnostics tests**

Add tests that create a temp store with one enabled local package and one disabled local package. Assert:

```rust
let lines = result.render_lines();
assert!(lines.iter().any(|line| line.contains("install_enabled=true")));
assert!(lines.iter().any(|line| line.contains("install_enabled=false")));
assert!(lines.iter().any(|line| line.contains("discovered=true")));
assert!(lines.iter().any(|line| line.contains("discovered=false")));
```

Use `YACH_EXTENSION_USER_STORE` override and a temp package with `yach.extension.json`.

- [ ] **Step 3: Collect installed package roots for diagnostics**

Add:

```rust
fn extension_install_store_for_scope(scope: ExtensionInstallScope) -> ExtensionInstallStore {
    extension_store_path(scope)
        .ok()
        .and_then(|path| ExtensionInstallStore::load_from_path(&path).ok())
        .unwrap_or_default()
}

fn installed_extension_package_roots() -> Vec<ExtensionPackageRoot> {
    let mut roots = extension_install_store_for_scope(ExtensionInstallScope::User)
        .enabled_package_roots();
    roots.extend(
        extension_install_store_for_scope(ExtensionInstallScope::Project)
            .enabled_package_roots(),
    );
    roots
}
```

Update `extension_package_roots_from_env` into a more general function:

```rust
fn extension_package_roots_from_env_and_installs() -> Vec<ExtensionPackageRoot> {
    let mut roots = installed_extension_package_roots();
    roots.extend(extension_package_roots_from_env());
    roots
}
```

Use `extension_package_roots_from_env_and_installs()` in diagnostics. Keep the native TUI config on env-only until Task 7 if the diagnostic change gets large.

- [ ] **Step 4: Render disabled install-only records**

Disabled records should be visible in `extension list` even though they are not scanned. The output should include:

```text
install_enabled=false
discovered=false
```

If a record is enabled but scan fails, command outcome may be `Failed`, but the message must be categorical and must not include raw command lines or host output.

- [ ] **Step 5: Run diagnostics tests**

Run:

```bash
just dev cargo test -p yach-cli extension_ -- --nocapture
```

Expected: extension CLI tests pass.

- [ ] **Step 6: Checkpoint**

Run:

```bash
jj describe -m "feat: list installed extension records"
jj new
```

## Task 7: Native Startup Reads Enabled User Install Records After First Paint

**Files:**
- Modify: `crates/yach-cli/src/main.rs`
- Test: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Add a unit test for package root collection**

Add a test that writes an enabled user store and asserts `native_dogfood_runner_config` or a smaller helper includes that root in `extension_package_roots`.

Prefer extracting:

```rust
fn native_extension_package_roots() -> Vec<ExtensionPackageRoot> {
    extension_package_roots_from_env_and_installs()
}
```

Test that:

- enabled user records are included;
- disabled records are excluded;
- env roots are still included with source ref `env:YACH_EXTENSION_PACKAGE_ROOTS`.

- [ ] **Step 2: Use installed roots in native backend config**

Change:

```rust
extension_package_roots: extension_package_roots_from_env(),
```

to:

```rust
extension_package_roots: native_extension_package_roots(),
```

This does not scan or spawn hosts before first paint. It only passes package root values to the existing post-first-paint scan path.

- [ ] **Step 3: Run native config tests**

Run:

```bash
just dev cargo test -p yach-cli native_backend_config_ extension_ -- --nocapture
```

Expected: native backend config and extension CLI tests pass.

- [ ] **Step 4: Run one startup smoke with an installed disabled extension**

Run:

```bash
tmp="$(mktemp -d)"
mkdir -p "$tmp/ext"
printf '%s\n' '{"schema":"yach.extension.v1","id":"example.disabled-install","version":"0.1.0","main":{"command":"node","args":["./extension.js"]},"contributes":{"tools":[]}}' > "$tmp/ext/yach.extension.json"
YACH_EXTENSION_USER_STORE="$tmp/extensions.json" just dev cargo run -p yach-cli -- extension install "$tmp/ext" --disabled
YACH_EXTENSION_USER_STORE="$tmp/extensions.json" just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 5
```

Expected: `samples_collected=5`. Disabled records should not cause manifest scan work.

- [ ] **Step 5: Checkpoint**

Run:

```bash
jj describe -m "feat: load installed extension roots after first paint"
jj new
```

## Task 8: Project Docs And Final Verification

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update project state**

In `docs/project/state.md`, update the extension install paragraph to say:

```text
Local-path extension install records are implemented for user/project scopes.
The CLI can install, remove, enable, disable, list, and doctor local records;
npm/git refs are parsed but remain unavailable adapters. Enabled records feed
the existing post-first-paint manifest scan path without spawning hosts before
first render.
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, set the recommended next move to:

```text
implement the persistent process-backed extension host transport and activation
manager.
```

Keep npm/git adapters, developer templates, and higher-risk tools listed as not
ready until later slices.

- [ ] **Step 3: Run formatting and focused tests**

Run:

```bash
just dev cargo fmt
just dev cargo test -p yach-backend extension_install -- --nocapture
just dev cargo test -p yach-cli extension_ -- --nocapture
git diff --check
```

Expected: all commands pass.

- [ ] **Step 4: Run full verification**

Run:

```bash
just lint
just test
```

Expected: all commands pass.

- [ ] **Step 5: Inspect jj stack**

Run:

```bash
jj status
jj log -r 'main..@'
```

Expected: one reviewable stack containing the local install records work, with no unrelated files.

- [ ] **Step 6: Final checkpoint**

Run:

```bash
jj describe -m "feat: add local extension install records"
```

Do not run `jj new` until after the PR is pushed, unless you are intentionally opening the next empty change.
