# Native Static Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add native static context assembly for core `AGENTS.md` and project-root `.yach/APPEND_SYSTEM.md`, then inject that context into native provider requests with provenance, diagnostics, redacted session evidence, and benchmark coverage.

**Architecture:** Create a focused `static_context` backend module that owns context discovery, bounded UTF-8 reads, placement, ordering, summaries, and omission diagnostics. Wire the native provider runner to assemble context lazily per provider request, prepend it as system `ProviderMessage`s ahead of transcript messages, and record redacted evidence without putting full context bodies into session logs.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `std::fs`, `std::path`, `yach-backend`, `yach-bench`, `criterion`, `just dev cargo test`, `just test`.

---

## File Structure

- Create `crates/yach-backend/src/static_context.rs`: static context types, policies, discovery, bounded reads, context bundle construction, active-context summaries, and omission diagnostics.
- Modify `crates/yach-backend/src/lib.rs`: export `static_context`, add or relocate integration tests only when they need existing backend helpers.
- Modify `crates/yach-backend/src/session.rs`: add redacted static-context inclusion evidence event types and JSONL round-trip support.
- Modify `crates/yach-backend/src/native_runner.rs`: replace direct `native_provider_messages_from_log` use in native-provider dogfood path with static-context-aware message assembly and evidence recording.
- Modify `crates/yach-backend/src/rig_adapter.rs`: add tests proving existing system-message preamble projection remains stable with static context labels.
- Modify `crates/yach-bench/Cargo.toml`: add a `native_static_context` Criterion bench target.
- Create `crates/yach-bench/benches/native_static_context.rs`: benchmark no context, one root `AGENTS.md`, nested `AGENTS.md`, and `.yach/APPEND_SYSTEM.md` assembly.
- Modify `docs/project/state.md` and `docs/project/next.md`: update current project state after implementation.

## Task 1: Static Context Model And Core Discovery

**Files:**
- Create: `crates/yach-backend/src/static_context.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write failing tests for root/nested `AGENTS.md`, append-system discovery, and sibling exclusion**

Create `crates/yach-backend/src/static_context.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "yach-static-context-{name}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn static_context_discovers_agents_md_from_root_to_cwd() {
        let project = TempProject::new("agents-order");
        project.write("AGENTS.md", "root rules");
        project.write("crates/yach-backend/AGENTS.md", "backend rules");
        project.write("crates/yach-ui/AGENTS.md", "ui rules");
        let cwd = project.root().join("crates/yach-backend/src");
        std::fs::create_dir_all(&cwd).unwrap();

        let assembly = assemble_project_static_context(
            project.root(),
            &cwd,
            NativeStaticContextPolicy::test(),
        );

        assert_eq!(assembly.omissions, Vec::new());
        assert_eq!(
            assembly
                .bundle
                .items
                .iter()
                .map(|item| (&item.relative_path, item.placement, item.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    &String::from("AGENTS.md"),
                    NativeStaticContextPlacement::ProjectInstructions,
                    "root rules"
                ),
                (
                    &String::from("crates/yach-backend/AGENTS.md"),
                    NativeStaticContextPlacement::ProjectInstructions,
                    "backend rules"
                ),
            ]
        );
    }

    #[test]
    fn static_context_discovers_project_root_append_system_after_agents() {
        let project = TempProject::new("append-system");
        project.write("AGENTS.md", "ordinary project rules");
        project.write(".yach/APPEND_SYSTEM.md", "strong project system guidance");

        let assembly = assemble_project_static_context(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
        );

        assert_eq!(
            assembly
                .bundle
                .items
                .iter()
                .map(|item| (&item.relative_path, item.placement, item.content.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    &String::from("AGENTS.md"),
                    NativeStaticContextPlacement::ProjectInstructions,
                    "ordinary project rules"
                ),
                (
                    &String::from(".yach/APPEND_SYSTEM.md"),
                    NativeStaticContextPlacement::AppendSystem,
                    "strong project system guidance"
                ),
            ]
        );
    }
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend static_context_discovers_ -- --nocapture
```

Expected: compile failure because `assemble_project_static_context`, `NativeStaticContextPolicy`, and `NativeStaticContextPlacement` do not exist.

- [ ] **Step 3: Implement the static context module types and discovery path**

Add these concrete types and helpers in `crates/yach-backend/src/static_context.rs`:

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStaticContextPlacement {
    ProjectInstructions,
    AppendSystem,
    BackgroundContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStaticContextPriority {
    ProjectInstructions,
    AppendSystem,
    ExtensionBackground,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeStaticContextSource {
    AgentsMd,
    AppendSystemFile,
    ExtensionFile {
        extension_id: String,
        item_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStaticContextItem {
    pub source: NativeStaticContextSource,
    pub relative_path: String,
    pub placement: NativeStaticContextPlacement,
    pub title: String,
    pub content: String,
    pub byte_count: usize,
    pub priority: NativeStaticContextPriority,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStaticContextBundle {
    pub items: Vec<NativeStaticContextItem>,
    pub total_bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStaticContextAssembly {
    pub bundle: NativeStaticContextBundle,
    pub omissions: Vec<NativeStaticContextOmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStaticContextOmissionReason {
    PathOutsideRoot,
    FileMissing,
    FileNotUtf8,
    FileTooLarge,
    BundleTooLarge,
    SourceDisabled,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStaticContextOmission {
    pub relative_path: String,
    pub source: NativeStaticContextSource,
    pub placement: NativeStaticContextPlacement,
    pub reason: NativeStaticContextOmissionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeStaticContextPolicy {
    pub max_agents_file_bytes: u64,
    pub max_append_system_bytes: u64,
    pub max_total_bytes: usize,
}

impl NativeStaticContextPolicy {
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            max_agents_file_bytes: 32 * 1024,
            max_append_system_bytes: 8 * 1024,
            max_total_bytes: 48 * 1024,
        }
    }

    #[must_use]
    pub const fn test() -> Self {
        Self {
            max_agents_file_bytes: 1024,
            max_append_system_bytes: 256,
            max_total_bytes: 4096,
        }
    }
}
```

Implement:

```rust
pub fn assemble_project_static_context(
    project_root: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
    policy: NativeStaticContextPolicy,
) -> NativeStaticContextAssembly {
    let project_root = match canonical_directory(project_root.as_ref()) {
        Some(path) => path,
        None => return NativeStaticContextAssembly::default(),
    };
    let cwd = match canonical_directory(cwd.as_ref()) {
        Some(path) => path,
        None => project_root.clone(),
    };
    if !cwd.starts_with(&project_root) {
        return NativeStaticContextAssembly {
            bundle: NativeStaticContextBundle::default(),
            omissions: vec![NativeStaticContextOmission {
                relative_path: String::from("."),
                source: NativeStaticContextSource::AgentsMd,
                placement: NativeStaticContextPlacement::ProjectInstructions,
                reason: NativeStaticContextOmissionReason::PathOutsideRoot,
            }],
        };
    }

    let mut assembly = NativeStaticContextAssembly::default();
    for directory in root_to_cwd_directories(&project_root, &cwd) {
        let path = directory.join("AGENTS.md");
        read_context_file_into_assembly(
            &project_root,
            &path,
            NativeStaticContextSource::AgentsMd,
            NativeStaticContextPlacement::ProjectInstructions,
            NativeStaticContextPriority::ProjectInstructions,
            policy.max_agents_file_bytes,
            policy.max_total_bytes,
            &mut assembly,
        );
    }
    read_context_file_into_assembly(
        &project_root,
        &project_root.join(".yach").join("APPEND_SYSTEM.md"),
        NativeStaticContextSource::AppendSystemFile,
        NativeStaticContextPlacement::AppendSystem,
        NativeStaticContextPriority::AppendSystem,
        policy.max_append_system_bytes,
        policy.max_total_bytes,
        &mut assembly,
    );
    assembly
}
```

The helper `read_context_file_into_assembly` should initially skip missing files without an omission record. It must call `std::fs::metadata`, reject directories, reject files larger than the supplied per-file limit, read bytes, reject invalid UTF-8, derive a normalized project-relative path with `/` separators, and push accepted items while updating `total_bytes`.

- [ ] **Step 4: Export the module**

Modify `crates/yach-backend/src/lib.rs`:

```rust
mod static_context;
pub use static_context::*;
```

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend static_context_discovers_ -- --nocapture
```

Expected: the two discovery tests pass.

Commit:

```bash
git add crates/yach-backend/src/static_context.rs crates/yach-backend/src/lib.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Add native static context discovery"
```

## Task 2: Context Policy, Budgets, Summaries, And Omissions

**Files:**
- Modify: `crates/yach-backend/src/static_context.rs`

- [ ] **Step 1: Add failing tests for path policy, UTF-8, budgets, and active summaries**

Append these tests to `crates/yach-backend/src/static_context.rs`:

```rust
#[test]
fn static_context_rejects_cwd_outside_project_root() {
    let project = TempProject::new("outside-root-project");
    let outside = TempProject::new("outside-root-cwd");
    let assembly = assemble_project_static_context(
        project.root(),
        outside.root(),
        NativeStaticContextPolicy::test(),
    );

    assert!(assembly.bundle.items.is_empty());
    assert_eq!(
        assembly.omissions,
        vec![NativeStaticContextOmission {
            relative_path: String::from("."),
            source: NativeStaticContextSource::AgentsMd,
            placement: NativeStaticContextPlacement::ProjectInstructions,
            reason: NativeStaticContextOmissionReason::PathOutsideRoot,
        }]
    );
}

#[test]
fn static_context_records_not_utf8_and_oversized_omissions() {
    let project = TempProject::new("omissions");
    std::fs::write(project.root().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).unwrap();
    project.write(".yach/APPEND_SYSTEM.md", "0123456789");

    let assembly = assemble_project_static_context(
        project.root(),
        project.root(),
        NativeStaticContextPolicy {
            max_agents_file_bytes: 1024,
            max_append_system_bytes: 4,
            max_total_bytes: 1024,
        },
    );

    assert!(assembly.bundle.items.is_empty());
    assert_eq!(
        assembly
            .omissions
            .iter()
            .map(|omission| (&omission.relative_path, omission.reason))
            .collect::<Vec<_>>(),
        vec![
            (
                &String::from("AGENTS.md"),
                NativeStaticContextOmissionReason::FileNotUtf8
            ),
            (
                &String::from(".yach/APPEND_SYSTEM.md"),
                NativeStaticContextOmissionReason::FileTooLarge
            ),
        ]
    );
}

#[test]
fn static_context_enforces_total_bundle_budget_without_partial_content() {
    let project = TempProject::new("total-budget");
    project.write("AGENTS.md", "12345");
    project.write(".yach/APPEND_SYSTEM.md", "67890");

    let assembly = assemble_project_static_context(
        project.root(),
        project.root(),
        NativeStaticContextPolicy {
            max_agents_file_bytes: 1024,
            max_append_system_bytes: 1024,
            max_total_bytes: 5,
        },
    );

    assert_eq!(assembly.bundle.items.len(), 1);
    assert_eq!(assembly.bundle.items[0].content, "12345");
    assert_eq!(
        assembly.omissions,
        vec![NativeStaticContextOmission {
            relative_path: String::from(".yach/APPEND_SYSTEM.md"),
            source: NativeStaticContextSource::AppendSystemFile,
            placement: NativeStaticContextPlacement::AppendSystem,
            reason: NativeStaticContextOmissionReason::BundleTooLarge,
        }]
    );
}

#[test]
fn static_context_summary_is_attributable_without_content_body() {
    let project = TempProject::new("summary");
    project.write("AGENTS.md", "root rules");
    project.write(".yach/APPEND_SYSTEM.md", "system rules");

    let assembly = assemble_project_static_context(
        project.root(),
        project.root(),
        NativeStaticContextPolicy::test(),
    );

    assert_eq!(
        assembly.bundle.summary().items,
        vec![
            NativeStaticContextItemSummary {
                source: NativeStaticContextSource::AgentsMd,
                relative_path: String::from("AGENTS.md"),
                placement: NativeStaticContextPlacement::ProjectInstructions,
                title: String::from("AGENTS.md instructions for ."),
                byte_count: "root rules".len(),
            },
            NativeStaticContextItemSummary {
                source: NativeStaticContextSource::AppendSystemFile,
                relative_path: String::from(".yach/APPEND_SYSTEM.md"),
                placement: NativeStaticContextPlacement::AppendSystem,
                title: String::from(".yach/APPEND_SYSTEM.md"),
                byte_count: "system rules".len(),
            },
        ]
    );
}
```

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend static_context_ -- --nocapture
```

Expected: compile failure for `NativeStaticContextItemSummary` and `summary`, or assertion failures for missing omission behavior if Task 1 implemented more than the minimum.

- [ ] **Step 3: Implement summaries and omission behavior**

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeStaticContextItemSummary {
    pub source: NativeStaticContextSource,
    pub relative_path: String,
    pub placement: NativeStaticContextPlacement,
    pub title: String,
    pub byte_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStaticContextSummary {
    pub items: Vec<NativeStaticContextItemSummary>,
    pub total_bytes: usize,
}

impl NativeStaticContextBundle {
    #[must_use]
    pub fn summary(&self) -> NativeStaticContextSummary {
        NativeStaticContextSummary {
            items: self
                .items
                .iter()
                .map(|item| NativeStaticContextItemSummary {
                    source: item.source.clone(),
                    relative_path: item.relative_path.clone(),
                    placement: item.placement,
                    title: item.title.clone(),
                    byte_count: item.byte_count,
                })
                .collect(),
            total_bytes: self.total_bytes,
        }
    }
}
```

Ensure `read_context_file_into_assembly` does all of the following:

```rust
fn record_omission(
    assembly: &mut NativeStaticContextAssembly,
    relative_path: String,
    source: NativeStaticContextSource,
    placement: NativeStaticContextPlacement,
    reason: NativeStaticContextOmissionReason,
) {
    assembly.omissions.push(NativeStaticContextOmission {
        relative_path,
        source,
        placement,
        reason,
    });
}

let relative_path = relative_project_path(project_root, path);
if !path.exists() {
    return;
}
if !path.starts_with(project_root) {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::PathOutsideRoot,
    );
    return;
}
let Ok(metadata) = std::fs::metadata(path) else {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::FileMissing,
    );
    return;
};
if !metadata.is_file() {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::FileMissing,
    );
    return;
}
if metadata.len() > max_file_bytes {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::FileTooLarge,
    );
    return;
}
let Ok(bytes) = std::fs::read(path) else {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::Io,
    );
    return;
};
let Ok(content) = String::from_utf8(bytes) else {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::FileNotUtf8,
    );
    return;
};
if assembly.bundle.total_bytes.saturating_add(content.len()) > max_total_bytes {
    record_omission(
        assembly,
        relative_path,
        source.clone(),
        placement,
        NativeStaticContextOmissionReason::BundleTooLarge,
    );
    return;
}
```

The accepted-item path should use the same `relative_path`, `source`, and `placement` values that the omission branch would have recorded.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend static_context_ -- --nocapture
```

Expected: all static context unit tests pass.

Commit:

```bash
git add crates/yach-backend/src/static_context.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Enforce static context policy"
```

## Task 3: Redacted Session Evidence For Static Context

**Files:**
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/static_context.rs`
- Modify: `crates/yach-backend/src/lib.rs` tests if the existing integration test style is easier there

- [ ] **Step 1: Write failing session evidence tests**

Add a test near existing session-log JSONL tests in `crates/yach-backend/src/lib.rs`:

```rust
#[test]
fn native_session_log_preserves_static_context_evidence_without_content_body() {
    let root_path = temp_resource_dir("native-static-context-evidence");
    assert!(std::fs::write(root_path.join("AGENTS.md"), "do not persist this body").is_ok());
    let assembly = assemble_project_static_context(
        &root_path,
        &root_path,
        NativeStaticContextPolicy::test(),
    );
    let summary = assembly.bundle.summary();
    let path = root_path.join("session.jsonl");
    let mut log = NativeSessionLog::default();

    log.record_static_context_included(
        NativeSessionId(String::from("session-static-context")),
        NativeTurnId(String::from("turn-static-context")),
        summary.clone(),
        assembly.omissions.clone(),
    );
    assert!(log.write_to_file(&path).is_ok());
    let raw = std::fs::read_to_string(&path).unwrap();
    let loaded = NativeSessionLog::load_from_file(&path).unwrap();

    assert!(raw.contains("static_context_included"));
    assert!(raw.contains("AGENTS.md"));
    assert!(!raw.contains("do not persist this body"));
    assert_eq!(loaded.events, log.events);
    assert!(std::fs::remove_dir_all(root_path).is_ok());
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_static_context_evidence -- --nocapture
```

Expected: compile failure because `record_static_context_included` and the `NativeSessionEvent` variant do not exist.

- [ ] **Step 3: Add serializable evidence structs and event variant**

In `crates/yach-backend/src/static_context.rs`, derive `Serialize` and `Deserialize` for summary and omission structs/enums:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStaticContextPlacement {
    ProjectInstructions,
    AppendSystem,
    BackgroundContext,
}
```

Use the same serde pattern for `NativeStaticContextPriority`, `NativeStaticContextSource`, `NativeStaticContextOmissionReason`, `NativeStaticContextOmission`, `NativeStaticContextItemSummary`, and `NativeStaticContextSummary`. Do not serialize `NativeStaticContextItem.content` into evidence.

In `crates/yach-backend/src/session.rs`, import the summary and omission types and add:

```rust
StaticContextIncluded {
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    summary: NativeStaticContextSummary,
    omissions: Vec<NativeStaticContextOmission>,
},
```

Update every `match NativeSessionEvent` arm in `session.rs`, `native_runner.rs`, and backend tests that enumerates variants so `StaticContextIncluded { .. }` is ignored for transcript projection and turn-id extraction unless a test specifically reads evidence.

Add this helper:

```rust
pub fn record_static_context_included(
    &mut self,
    session_id: NativeSessionId,
    turn_id: NativeTurnId,
    summary: NativeStaticContextSummary,
    omissions: Vec<NativeStaticContextOmission>,
) {
    self.push(NativeSessionEvent::StaticContextIncluded {
        session_id,
        turn_id,
        summary,
        omissions,
    });
}
```

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend static_context_ -- --nocapture
just dev cargo test -p yach-backend native_session_log_preserves_static_context_evidence -- --nocapture
```

Expected: static context tests and the session evidence test pass.

Commit:

```bash
git add crates/yach-backend/src/static_context.rs crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs crates/yach-backend/src/native_runner.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Record static context evidence"
```

## Task 4: Provider Request Assembly Integration

**Files:**
- Modify: `crates/yach-backend/src/resource.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-backend/src/rig_adapter.rs`

- [ ] **Step 1: Write failing native runner tests for static context injection and transcript ordering**

In `crates/yach-backend/src/native_runner.rs`, add tests near `native_provider_messages_include_resumed_transcript`:

```rust
#[test]
fn native_provider_messages_prepend_static_context_before_transcript() {
    let mut log = NativeSessionLog::default();
    let session_id = NativeSessionId(String::from("session-static-context"));
    let turn_id = NativeTurnId(String::from("turn-static-context"));
    log.push(NativeSessionEvent::EntryAppended {
        session_id: session_id.clone(),
        entry_id: NativeEntryId(String::from("entry-user")),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: NativeRole::User,
        text: String::from("hello"),
        provider: None,
    });

    let bundle = NativeStaticContextBundle {
        total_bytes: "root rules".len(),
        items: vec![NativeStaticContextItem {
            source: NativeStaticContextSource::AgentsMd,
            relative_path: String::from("AGENTS.md"),
            placement: NativeStaticContextPlacement::ProjectInstructions,
            title: String::from("AGENTS.md instructions for ."),
            content: String::from("root rules"),
            byte_count: "root rules".len(),
            priority: NativeStaticContextPriority::ProjectInstructions,
        }],
    };

    let messages = native_provider_messages_from_log_with_static_context(
        &log,
        &turn_id,
        &bundle,
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, NativeRole::System);
    assert!(messages[0].content.contains("# AGENTS.md instructions for ."));
    assert!(messages[0].content.contains("root rules"));
    assert_eq!(messages[1].role, NativeRole::User);
    assert_eq!(messages[1].content, "hello");
}

#[tokio::test]
async fn native_provider_request_includes_project_static_context_and_records_evidence() {
    let root = TempProject::new("provider-static-context");
    root.write("AGENTS.md", "root rules");
    root.write(".yach/APPEND_SYSTEM.md", "system rules");
    let project_root = NativeResourceRoot::project(root.root()).ok();
    let mut log = NativeSessionLog::default();
    let mut pending_events = Vec::new();
    let turn_id = NativeTurnId(String::from("turn-static-context-provider"));
    log.push(NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("default")),
        entry_id: NativeEntryId(String::from("entry-user")),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: NativeRole::User,
        text: String::from("hello"),
        provider: None,
    });
    let mut requester = FakeProviderRequester::with_responses([Ok(vec![
        ProviderStreamEvent::Started {
            turn_id: turn_id.clone(),
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
        },
        ProviderStreamEvent::TextDelta {
            turn_id: turn_id.clone(),
            delta: String::from("ok"),
        },
        ProviderStreamEvent::Completed {
            turn_id: turn_id.clone(),
            finish_reason: Some(ProviderFinishReason::Stop),
            usage: None,
            provider_response_id: None,
        },
    ])]);

    let result = run_native_provider_one_tool_round_with_registry(
        &mut requester,
        NativeProviderToolRoundContext {
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
            log: &mut log,
            pending_events: &mut pending_events,
            turn_id: &turn_id,
            project_root,
            static_context_cwd: Some(root.root().to_path_buf()),
            tool_event_store: None,
            registry: &NativeToolRegistry::with_project_read_only_tools(),
            permission_policy: &NativeToolPermissionPolicy::allow_project_metadata_tool(
                "project_path_info",
            ),
            executor: &ProjectReadOnlyToolExecutor::new(
                NativeResourceRoot::project(root.root()).unwrap(),
            ),
            routable_tool_names: &["project_path_info"],
            require_project_root_for_tools: true,
        },
    )
    .await;

    assert!(result.is_ok());
    let request = &requester.requests[0];
    assert_eq!(request.messages[0].role, NativeRole::System);
    assert!(request.messages[0].content.contains("root rules"));
    assert!(request.messages[0].content.contains("system rules"));
    assert!(pending_events.iter().any(|event| {
        matches!(event, NativeSessionEvent::StaticContextIncluded { summary, .. }
            if summary.items.len() == 2)
    }));
}
```

If `TempProject` is only available in `static_context.rs`, move a small test-only temp project helper into `native_runner.rs` tests instead of making production code depend on a test helper.

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_prepend_static_context -- --nocapture
just dev cargo test -p yach-backend native_provider_request_includes_project_static_context -- --nocapture
```

Expected: compile failure because `native_provider_messages_from_log_with_static_context` and `static_context_cwd` do not exist.

- [ ] **Step 3: Implement provider message assembly helpers**

Add to `crates/yach-backend/src/native_runner.rs`:

```rust
fn native_provider_messages_from_log_with_static_context(
    log: &NativeSessionLog,
    current_turn_id: &NativeTurnId,
    context: &NativeStaticContextBundle,
) -> Vec<ProviderMessage> {
    let mut messages = Vec::new();
    let context_message = provider_message_from_static_context(context);
    if let Some(message) = context_message {
        messages.push(message);
    }
    messages.extend(native_provider_messages_from_log(log, current_turn_id));
    messages
}

fn provider_message_from_static_context(
    context: &NativeStaticContextBundle,
) -> Option<ProviderMessage> {
    if context.items.is_empty() {
        return None;
    }
    let content = context
        .items
        .iter()
        .map(|item| format!("# {}\n\n{}", item.title, item.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(ProviderMessage {
        role: NativeRole::System,
        content,
    })
}
```

Extend `NativeProviderToolRoundContext` with:

```rust
static_context_cwd: Option<PathBuf>,
```

In `run_native_provider_one_tool_round_with_registry`, before building `initial_request`, assemble context only when a project root exists:

```rust
let static_context_assembly = project_root
    .as_ref()
    .map(|root| {
        assemble_project_static_context(
            root.canonical_path(),
            static_context_cwd.as_deref().unwrap_or_else(|| root.canonical_path()),
            NativeStaticContextPolicy::conservative(),
        )
    })
    .unwrap_or_default();
if !static_context_assembly.bundle.items.is_empty()
    || !static_context_assembly.omissions.is_empty()
{
    log.record_static_context_included(
        NativeSessionId(String::from("default")),
        turn_id.clone(),
        static_context_assembly.bundle.summary(),
        static_context_assembly.omissions.clone(),
    );
    pending_events.push(log.events.last().cloned().expect("event just pushed"));
}
```

Then set:

```rust
messages: native_provider_messages_from_log_with_static_context(
    log,
    turn_id,
    &static_context_assembly.bundle,
),
```

Add this accessor to `NativeResourceRoot` in `resource.rs` if it does not already exist:

```rust
#[must_use]
pub fn canonical_path(&self) -> &Path {
    &self.canonical_path
}
```

Update all existing `NativeProviderToolRoundContext` construction sites in tests with `static_context_cwd: None`.

- [ ] **Step 4: Add Rig adapter stability test**

In `crates/yach-backend/src/rig_adapter.rs`, add a test near `rig_provider_prompt_keeps_system_messages_in_preamble_only`:

```rust
#[test]
fn rig_provider_preamble_preserves_static_context_system_message() {
    let request = provider_request(vec![
        ProviderMessage {
            role: NativeRole::System,
            content: String::from("# AGENTS.md instructions for .\n\nroot rules"),
        },
        ProviderMessage {
            role: NativeRole::User,
            content: String::from("hello"),
        },
    ]);

    assert_eq!(
        preamble_from_request(&request),
        "# AGENTS.md instructions for .\n\nroot rules"
    );
    assert_eq!(prompt_from_request(&request).unwrap(), "User:\nhello");
}
```

- [ ] **Step 5: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_prepend_static_context -- --nocapture
just dev cargo test -p yach-backend native_provider_request_includes_project_static_context -- --nocapture
just dev cargo test -p yach-backend rig_provider_preamble_preserves_static_context -- --nocapture
```

Expected: all three tests pass.

Commit:

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-backend/src/resource.rs crates/yach-backend/src/rig_adapter.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Inject static context into provider requests"
```

## Task 5: Extension Manifest Placeholder For Static Context Contributions

**Files:**
- Modify: `crates/yach-backend/src/extension.rs`
- Modify: `crates/yach-backend/src/static_context.rs`

- [ ] **Step 1: Write failing manifest tests for extension-packaged background context and rejected append-system placement**

In `crates/yach-backend/src/extension.rs`, add:

```rust
#[test]
fn extension_manifest_accepts_packaged_background_static_context_contribution() {
    let manifest = parse_extension_manifest(serde_json::json!({
        "schema": "yach.extension.v1",
        "id": "example.context-pack",
        "version": "0.1.0",
        "main": {
            "command": "node",
            "args": ["./extension.js"]
        },
        "contributes": {
            "static_context": [{
                "id": "rust-style-guide",
                "title": "Rust style guide",
                "source": {
                    "type": "extension_file",
                    "path": "context/rust.md"
                },
                "placement": "background_context",
                "max_bytes": 12000
            }]
        }
    }))
    .unwrap();

    assert_eq!(
        manifest.contributes.static_context,
        vec![ExtensionStaticContextContribution {
            id: String::from("rust-style-guide"),
            title: String::from("Rust style guide"),
            source: ExtensionStaticContextSource::ExtensionFile {
                path: String::from("context/rust.md"),
            },
            placement: ExtensionStaticContextPlacement::BackgroundContext,
            max_bytes: 12000,
        }]
    );
}

#[test]
fn extension_manifest_rejects_static_context_append_system_placement_for_now() {
    let error = parse_extension_manifest(serde_json::json!({
        "schema": "yach.extension.v1",
        "id": "example.context-pack",
        "version": "0.1.0",
        "main": {
            "command": "node",
            "args": ["./extension.js"]
        },
        "contributes": {
            "static_context": [{
                "id": "system-guide",
                "title": "System guide",
                "source": {
                    "type": "extension_file",
                    "path": "context/system.md"
                },
                "placement": "append_system",
                "max_bytes": 1024
            }]
        }
    }));

    assert_eq!(
        error,
        Err(ExtensionManifestError::UnsupportedStaticContextPlacement {
            placement: String::from("append_system")
        })
    );
}
```

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend extension_manifest_accepts_packaged_background_static_context -- --nocapture
just dev cargo test -p yach-backend extension_manifest_rejects_static_context_append_system -- --nocapture
```

Expected: compile failure because extension static-context contribution types do not exist.

- [ ] **Step 3: Extend manifest parsing with static context contribution metadata only**

In `crates/yach-backend/src/extension.rs`, extend `ExtensionContributions`:

```rust
pub struct ExtensionContributions {
    pub tools: Vec<ExtensionToolContribution>,
    pub static_context: Vec<ExtensionStaticContextContribution>,
}
```

Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionStaticContextContribution {
    pub id: String,
    pub title: String,
    pub source: ExtensionStaticContextSource,
    pub placement: ExtensionStaticContextPlacement,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionStaticContextSource {
    ExtensionFile { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionStaticContextPlacement {
    BackgroundContext,
}
```

Extend `ExtensionManifestError`:

```rust
UnsupportedStaticContextPlacement { placement: String },
InvalidStaticContextId { id: String },
InvalidStaticContextPath { path: String },
```

Parse only:

```json
{"source":{"type":"extension_file","path":"context/rust.md"},"placement":"background_context"}
```

Reject `append_system` for extensions in this slice. This task only stores manifest metadata; it must not read extension files or spawn extension hosts.

- [ ] **Step 4: Run focused tests and commit**

Run:

```bash
just dev cargo test -p yach-backend extension_manifest_accepts_packaged_background_static_context -- --nocapture
just dev cargo test -p yach-backend extension_manifest_rejects_static_context_append_system -- --nocapture
```

Expected: both manifest tests pass, and existing extension manifest defaults still pass.

Commit:

```bash
git add crates/yach-backend/src/extension.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Add extension static context manifest metadata"
```

## Task 6: Static Context Assembly Benchmarks

**Files:**
- Modify: `crates/yach-bench/Cargo.toml`
- Create: `crates/yach-bench/benches/native_static_context.rs`

- [ ] **Step 1: Write the benchmark target**

Add to `crates/yach-bench/Cargo.toml`:

```toml
[[bench]]
name = "native_static_context"
harness = false
```

Create `crates/yach-bench/benches/native_static_context.rs`:

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::{NativeStaticContextPolicy, assemble_project_static_context};

static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
    cwd: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let sequence = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yach-native-static-context-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            cwd: root.clone(),
            root,
        }
    }

    fn with_cwd(mut self, relative_path: &str) -> Self {
        self.cwd = self.root.join(relative_path);
        fs::create_dir_all(&self.cwd).unwrap();
        self
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn write(&self, relative_path: &str, content: &str) {
        let path = self.root.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn bench_static_context_empty(c: &mut Criterion) {
    c.bench_function("native_static_context_empty", |b| {
        b.iter_batched(
            || TempProject::new("empty"),
            |project| {
                black_box(assemble_project_static_context(
                    project.root(),
                    project.cwd(),
                    NativeStaticContextPolicy::conservative(),
                ));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_one_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_one_agents", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("one-agents");
                project.write("AGENTS.md", "root rules\n");
                project
            },
            |project| {
                black_box(assemble_project_static_context(
                    project.root(),
                    project.cwd(),
                    NativeStaticContextPolicy::conservative(),
                ));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_nested_agents(c: &mut Criterion) {
    c.bench_function("native_static_context_nested_agents", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("nested-agents").with_cwd("crates/backend/src");
                project.write("AGENTS.md", "root rules\n");
                project.write("crates/AGENTS.md", "crates rules\n");
                project.write("crates/backend/AGENTS.md", "backend rules\n");
                project
            },
            |project| {
                black_box(assemble_project_static_context(
                    project.root(),
                    project.cwd(),
                    NativeStaticContextPolicy::conservative(),
                ));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_static_context_append_system(c: &mut Criterion) {
    c.bench_function("native_static_context_append_system", |b| {
        b.iter_batched(
            || {
                let project = TempProject::new("append-system");
                project.write("AGENTS.md", "root rules\n");
                project.write(".yach/APPEND_SYSTEM.md", "system rules\n");
                project
            },
            |project| {
                black_box(assemble_project_static_context(
                    project.root(),
                    project.cwd(),
                    NativeStaticContextPolicy::conservative(),
                ));
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_static_context_empty,
    bench_static_context_one_agents,
    bench_static_context_nested_agents,
    bench_static_context_append_system,
);
criterion_main!(benches);
```

- [ ] **Step 2: Run the benchmark compile/test target and commit**

Run:

```bash
just dev cargo test -p yach-bench --bench native_static_context -- --nocapture
```

Expected: the Criterion bench target compiles and reports `0 tests` without compilation errors.

Commit:

```bash
git add crates/yach-bench/Cargo.toml crates/yach-bench/benches/native_static_context.rs
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Benchmark native static context assembly"
```

## Task 7: Project Planning Docs

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update project state**

In `docs/project/state.md`, add a current posture bullet:

```markdown
- Native static context assembly now supports core `AGENTS.md` discovery plus explicit project-root `.yach/APPEND_SYSTEM.md`, injects accepted context into native provider requests with redacted evidence, and keeps extension static context limited to manifest metadata for a later contribution slice.
```

Add the new spec and plan to the relevant records list:

```markdown
- `docs/superpowers/specs/2026-05-13-native-static-context-design.md`
- `docs/superpowers/plans/2026-05-13-native-static-context.md`
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, replace the recommended next move with:

```markdown
Recommended next move: execute the native static context implementation plan, adding core `AGENTS.md` discovery, explicit `.yach/APPEND_SYSTEM.md` append-system context, provider request assembly integration, redacted context evidence, and static-context assembly benchmarks.

Why: the static context design is accepted and fills a core harness UX gap before broader extension context providers. `AGENTS.md` should be yach core behavior, while extension-provided static context remains additive and attributable through the same yach-owned assembly pipeline.
```

Add the new spec and plan to relevant sources.

- [ ] **Step 3: Commit docs**

Run:

```bash
git diff -- docs/project/state.md docs/project/next.md
```

Expected: docs reflect the implemented static context slice and the next move no longer says to write this design.

Commit:

```bash
git add docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Update project state after static context implementation"
```

## Task 8: Final Verification

**Files:**
- No source edits unless verification exposes a defect.

- [ ] **Step 1: Run formatting**

Run:

```bash
just dev cargo fmt --check
```

Expected: exits 0.

- [ ] **Step 2: Run focused static context tests**

Run:

```bash
just dev cargo test -p yach-backend static_context -- --nocapture
```

Expected: static-context backend tests pass.

- [ ] **Step 3: Run full test suite**

Run:

```bash
just test
```

Expected: all workspace tests and doctests pass.

- [ ] **Step 4: Run benchmark compile target**

Run:

```bash
just dev cargo test -p yach-bench --bench native_static_context -- --nocapture
```

Expected: benchmark target compiles successfully.

- [ ] **Step 5: Run a short benchmark smoke**

Run:

```bash
just dev cargo bench -p yach-bench --bench native_static_context -- --sample-size 10
```

Expected: Criterion reports timings for `native_static_context_empty`, `native_static_context_one_agents`, `native_static_context_nested_agents`, and `native_static_context_append_system`.

- [ ] **Step 6: Check whitespace and status**

Run:

```bash
git diff --check
git status --short --branch
```

Expected: no whitespace errors; branch has committed implementation changes only.

## Self-Review Checklist

- Spec coverage: Tasks cover core `AGENTS.md`, project-root `.yach/APPEND_SYSTEM.md`, placement, ordering, bounded UTF-8 reads, summaries, redacted evidence, provider request injection, extension manifest metadata only, and benchmarks.
- Scope: arbitrary project-file selectors, dynamic extension context hooks, user-global instructions, prompt replay, approval UI, and install UX remain out of scope.
- TDD: every behavior-changing task starts with focused failing tests before implementation.
- Performance: startup path is not touched; context assembly is lazy at provider request time and has a dedicated Criterion benchmark.
- Safety: full static context bodies are provider-visible but not persisted in session evidence; diagnostics record source, placement, path, byte count, extension id where applicable, and omission category.
