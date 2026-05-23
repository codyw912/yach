use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStaticContextPlacement {
    ProjectInstructions,
    AppendSystem,
    BackgroundContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStaticContextPriority {
    ProjectInstructions,
    AppendSystem,
    ExtensionBackground,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// Raw UTF-8 content bytes, excluding provider-visible title/header bytes.
    pub byte_count: usize,
    pub priority: NativeStaticContextPriority,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStaticContextBundle {
    pub items: Vec<NativeStaticContextItem>,
    /// Provider-visible rendered bytes for all accepted items, including
    /// item headers and separators.
    pub total_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeStaticContextItemSummary {
    pub source: NativeStaticContextSource,
    pub relative_path: String,
    pub placement: NativeStaticContextPlacement,
    pub title: String,
    /// Raw UTF-8 content bytes, excluding provider-visible title/header bytes.
    pub byte_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeStaticContextSummary {
    pub items: Vec<NativeStaticContextItemSummary>,
    /// Provider-visible rendered bytes for all accepted items, including
    /// item headers and separators.
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeStaticContextAssembly {
    pub bundle: NativeStaticContextBundle,
    pub omissions: Vec<NativeStaticContextOmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStaticContextOmissionReason {
    PathOutsideRoot,
    FileMissing,
    FileNotUtf8,
    FileTooLarge,
    BundleTooLarge,
    SourceDisabled,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Maximum provider-visible rendered bytes for accepted static context.
    pub max_total_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeExtensionStaticContextFile {
    pub extension_id: String,
    pub item_id: String,
    pub package_root: PathBuf,
    pub relative_path: String,
    pub title: String,
    pub placement: NativeStaticContextPlacement,
    pub max_bytes: u64,
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

#[must_use]
pub fn assemble_project_static_context(
    project_root: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
    policy: NativeStaticContextPolicy,
) -> NativeStaticContextAssembly {
    assemble_project_static_context_with_extensions(
        project_root,
        cwd,
        policy,
        std::iter::empty::<NativeExtensionStaticContextFile>(),
    )
}

#[must_use]
pub fn assemble_project_static_context_with_extensions(
    project_root: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
    policy: NativeStaticContextPolicy,
    extension_files: impl IntoIterator<Item = NativeExtensionStaticContextFile>,
) -> NativeStaticContextAssembly {
    let Ok(project_root) = project_root.as_ref().canonicalize() else {
        return NativeStaticContextAssembly::default();
    };
    let cwd = cwd
        .as_ref()
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());

    if !cwd.starts_with(&project_root) {
        return NativeStaticContextAssembly {
            bundle: NativeStaticContextBundle::default(),
            omissions: vec![NativeStaticContextOmission {
                relative_path: ".".to_string(),
                source: NativeStaticContextSource::AgentsMd,
                placement: NativeStaticContextPlacement::ProjectInstructions,
                reason: NativeStaticContextOmissionReason::PathOutsideRoot,
            }],
        };
    }

    let mut assembly = NativeStaticContextAssembly::default();

    for directory in directories_from_root_to_cwd(&project_root, &cwd) {
        maybe_add_file(
            &mut assembly,
            &project_root,
            &ContextFileCandidate {
                path: directory.join("AGENTS.md"),
                source: NativeStaticContextSource::AgentsMd,
                placement: NativeStaticContextPlacement::ProjectInstructions,
                priority: NativeStaticContextPriority::ProjectInstructions,
                max_file_bytes: policy.max_agents_file_bytes,
                title: ContextFileTitle::FromPath(agents_title),
            },
            policy.max_total_bytes,
        );
    }

    maybe_add_file(
        &mut assembly,
        &project_root,
        &ContextFileCandidate {
            path: project_root.join(".yach/APPEND_SYSTEM.md"),
            source: NativeStaticContextSource::AppendSystemFile,
            placement: NativeStaticContextPlacement::AppendSystem,
            priority: NativeStaticContextPriority::AppendSystem,
            max_file_bytes: policy.max_append_system_bytes,
            title: ContextFileTitle::FromPath(append_system_title),
        },
        policy.max_total_bytes,
    );

    for extension_file in extension_files {
        maybe_add_extension_file(&mut assembly, extension_file, policy.max_total_bytes);
    }

    assembly
}

fn directories_from_root_to_cwd(project_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut directories = vec![project_root.to_path_buf()];
    let Ok(relative_cwd) = cwd.strip_prefix(project_root) else {
        return directories;
    };

    let mut current = project_root.to_path_buf();
    for component in relative_cwd {
        current.push(component);
        directories.push(current.clone());
    }

    directories
}

struct ContextFileCandidate {
    path: PathBuf,
    source: NativeStaticContextSource,
    placement: NativeStaticContextPlacement,
    priority: NativeStaticContextPriority,
    max_file_bytes: u64,
    title: ContextFileTitle,
}

enum ContextFileTitle {
    FromPath(fn(&str) -> String),
    Fixed(String),
}

impl ContextFileTitle {
    fn resolve(&self, relative_path: &str) -> String {
        match self {
            Self::FromPath(title_for_path) => title_for_path(relative_path),
            Self::Fixed(title) => title.clone(),
        }
    }
}

fn maybe_add_extension_file(
    assembly: &mut NativeStaticContextAssembly,
    extension_file: NativeExtensionStaticContextFile,
    max_total_bytes: usize,
) {
    let source = NativeStaticContextSource::ExtensionFile {
        extension_id: extension_file.extension_id,
        item_id: extension_file.item_id,
    };
    let package_relative_path = extension_file.relative_path.clone();
    if extension_file.placement != NativeStaticContextPlacement::BackgroundContext {
        assembly.omissions.push(omission(
            package_relative_path,
            source,
            extension_file.placement,
            NativeStaticContextOmissionReason::SourceDisabled,
        ));
        return;
    }
    let Ok(package_root) = extension_file.package_root.canonicalize() else {
        assembly.omissions.push(omission(
            package_relative_path,
            source,
            extension_file.placement,
            NativeStaticContextOmissionReason::FileMissing,
        ));
        return;
    };
    maybe_add_file(
        assembly,
        &package_root,
        &ContextFileCandidate {
            path: package_root.join(&extension_file.relative_path),
            source,
            placement: extension_file.placement,
            priority: NativeStaticContextPriority::ExtensionBackground,
            max_file_bytes: extension_file.max_bytes,
            title: ContextFileTitle::Fixed(format!(
                "Extension background context: {}",
                extension_file.title
            )),
        },
        max_total_bytes,
    );
}

fn maybe_add_file(
    assembly: &mut NativeStaticContextAssembly,
    project_root: &Path,
    candidate: &ContextFileCandidate,
    max_total_bytes: usize,
) {
    let path = &candidate.path;
    if !path.exists() {
        return;
    }

    let relative_path = project_relative_path(project_root, path);
    let Ok(canonical_path) = path.canonicalize() else {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileMissing,
        ));
        return;
    };

    if !canonical_path.starts_with(project_root) {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::PathOutsideRoot,
        ));
        return;
    }

    let Ok(metadata) = std::fs::metadata(&canonical_path) else {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileMissing,
        ));
        return;
    };

    if !metadata.is_file() {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileMissing,
        ));
        return;
    }

    if metadata.len() > candidate.max_file_bytes {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileTooLarge,
        ));
        return;
    }

    let bytes = match read_context_file_bytes(&canonical_path, candidate.max_file_bytes) {
        Ok(bytes) => bytes,
        Err(reason) => {
            assembly.omissions.push(omission(
                relative_path,
                candidate.source.clone(),
                candidate.placement,
                reason,
            ));
            return;
        }
    };

    let Ok(content) = String::from_utf8(bytes) else {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileNotUtf8,
        ));
        return;
    };

    let byte_count = content.len();
    let title = candidate.title.resolve(&relative_path);
    let rendered_byte_count = static_context_rendered_item_bytes(&title, byte_count)
        .saturating_add(if assembly.bundle.items.is_empty() {
            0
        } else {
            "\n\n".len()
        });

    if assembly
        .bundle
        .total_bytes
        .saturating_add(rendered_byte_count)
        > max_total_bytes
    {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::BundleTooLarge,
        ));
        return;
    }

    assembly.bundle.items.push(NativeStaticContextItem {
        source: candidate.source.clone(),
        relative_path,
        placement: candidate.placement,
        title,
        content,
        byte_count,
        priority: candidate.priority,
    });
    assembly.bundle.total_bytes = assembly
        .bundle
        .total_bytes
        .saturating_add(rendered_byte_count);
}

fn static_context_rendered_item_bytes(title: &str, content_byte_count: usize) -> usize {
    "# ".len()
        .saturating_add(title.len())
        .saturating_add("\n\n".len())
        .saturating_add(content_byte_count)
}

fn read_context_file_bytes(
    path: &Path,
    max_file_bytes: u64,
) -> Result<Vec<u8>, NativeStaticContextOmissionReason> {
    let file = File::open(path).map_err(|_| NativeStaticContextOmissionReason::Io)?;
    let limit = max_file_bytes.saturating_add(1);
    let mut bytes = Vec::new();
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|_| NativeStaticContextOmissionReason::Io)?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(NativeStaticContextOmissionReason::FileTooLarge);
    }
    Ok(bytes)
}

fn omission(
    relative_path: String,
    source: NativeStaticContextSource,
    placement: NativeStaticContextPlacement,
    reason: NativeStaticContextOmissionReason,
) -> NativeStaticContextOmission {
    NativeStaticContextOmission {
        relative_path,
        source,
        placement,
        reason,
    }
}

fn project_relative_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn agents_title(relative_path: &str) -> String {
    let directory = relative_path.strip_suffix("/AGENTS.md").unwrap_or(".");
    if directory == "AGENTS.md" {
        "AGENTS.md instructions for .".to_string()
    } else {
        format!("AGENTS.md instructions for {directory}")
    }
}

fn append_system_title(relative_path: &str) -> String {
    relative_path.to_string()
}

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
            assert!(
                std::fs::create_dir_all(&root).is_ok(),
                "failed to create temp project at {}",
                root.display()
            );
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn write(&self, relative_path: &str, content: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                assert!(
                    std::fs::create_dir_all(parent).is_ok(),
                    "failed to create parent directory at {}",
                    parent.display()
                );
            }
            assert!(
                std::fs::write(&path, content).is_ok(),
                "failed to write file at {}",
                path.display()
            );
        }

        #[cfg(unix)]
        fn symlink_file(&self, target: &Path, relative_path: &str) {
            let path = self.root.join(relative_path);
            if let Some(parent) = path.parent() {
                assert!(
                    std::fs::create_dir_all(parent).is_ok(),
                    "failed to create parent directory at {}",
                    parent.display()
                );
            }
            assert!(
                std::os::unix::fs::symlink(target, &path).is_ok(),
                "failed to symlink {} to {}",
                path.display(),
                target.display()
            );
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
        assert!(
            std::fs::create_dir_all(&cwd).is_ok(),
            "failed to create cwd at {}",
            cwd.display()
        );

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

    #[test]
    fn static_context_falls_back_to_project_root_when_cwd_missing() {
        let project = TempProject::new("missing-cwd");
        project.write("AGENTS.md", "root rules");
        project.write(".yach/APPEND_SYSTEM.md", "strong project system guidance");
        let missing_cwd = project.root().join("missing");

        let assembly = assemble_project_static_context(
            project.root(),
            &missing_cwd,
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
                    &String::from(".yach/APPEND_SYSTEM.md"),
                    NativeStaticContextPlacement::AppendSystem,
                    "strong project system guidance"
                ),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn static_context_omits_symlinked_context_files_outside_project_root() {
        let project = TempProject::new("symlink-project");
        let outside = TempProject::new("symlink-outside");
        outside.write("AGENTS.md", "outside project rules");
        outside.write("APPEND_SYSTEM.md", "outside append guidance");
        project.symlink_file(&outside.root().join("AGENTS.md"), "AGENTS.md");
        project.symlink_file(
            &outside.root().join("APPEND_SYSTEM.md"),
            ".yach/APPEND_SYSTEM.md",
        );

        let assembly = assemble_project_static_context(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
        );

        assert_eq!(assembly.bundle.items, Vec::new());
        assert_eq!(
            assembly.omissions,
            vec![
                NativeStaticContextOmission {
                    relative_path: "AGENTS.md".to_string(),
                    source: NativeStaticContextSource::AgentsMd,
                    placement: NativeStaticContextPlacement::ProjectInstructions,
                    reason: NativeStaticContextOmissionReason::PathOutsideRoot,
                },
                NativeStaticContextOmission {
                    relative_path: ".yach/APPEND_SYSTEM.md".to_string(),
                    source: NativeStaticContextSource::AppendSystemFile,
                    placement: NativeStaticContextPlacement::AppendSystem,
                    reason: NativeStaticContextOmissionReason::PathOutsideRoot,
                },
            ]
        );
    }

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
        assert!(std::fs::write(project.root().join("AGENTS.md"), [0xff, 0xfe, 0xfd]).is_ok());
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
    fn static_context_bounded_read_reports_actual_oversized_bytes_before_utf8() {
        let project = TempProject::new("actual-oversized-read");
        let path = project.root().join("AGENTS.md");
        assert!(std::fs::write(&path, [b'a', b'b', b'c', 0xff]).is_ok());

        let result = read_context_file_bytes(&path, 3);

        assert_eq!(result, Err(NativeStaticContextOmissionReason::FileTooLarge));
    }

    #[test]
    fn static_context_enforces_total_bundle_budget_without_partial_content() {
        let project = TempProject::new("total-budget");
        project.write("AGENTS.md", "12345");
        project.write(".yach/APPEND_SYSTEM.md", "67890");
        let accepted_rendered_bytes = "# AGENTS.md instructions for .\n\n12345".len();

        let assembly = assemble_project_static_context(
            project.root(),
            project.root(),
            NativeStaticContextPolicy {
                max_agents_file_bytes: 1024,
                max_append_system_bytes: 1024,
                max_total_bytes: accepted_rendered_bytes,
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
    fn static_context_enforces_total_bundle_budget_on_rendered_titles_and_separators() {
        let project = TempProject::new("rendered-total-budget");
        project.write("AGENTS.md", "");
        let mut cwd = project.root().to_path_buf();
        let mut nested_relative_path = PathBuf::new();
        for index in 0..8 {
            let component = format!("nested-{index}");
            cwd.push(&component);
            nested_relative_path.push(component);
            project.write(
                &format!("{}/AGENTS.md", nested_relative_path.to_string_lossy()),
                "",
            );
        }
        assert!(
            std::fs::create_dir_all(&cwd).is_ok(),
            "failed to create cwd at {}",
            cwd.display()
        );

        let root_rendered_bytes = "# AGENTS.md instructions for .\n\n".len();
        let first_nested_rendered_bytes = "\n\n# AGENTS.md instructions for nested-0\n\n".len();
        let assembly = assemble_project_static_context(
            project.root(),
            &cwd,
            NativeStaticContextPolicy {
                max_agents_file_bytes: 1024,
                max_append_system_bytes: 1024,
                max_total_bytes: root_rendered_bytes + first_nested_rendered_bytes,
            },
        );

        assert_eq!(
            assembly.bundle.items.len(),
            2,
            "root and first nested empty AGENTS.md should fit"
        );
        assert_eq!(
            assembly
                .bundle
                .items
                .iter()
                .map(|item| item.byte_count)
                .sum::<usize>(),
            0,
            "regression requires tiny raw content while rendered headers consume budget"
        );
        assert_eq!(
            assembly
                .omissions
                .iter()
                .map(|omission| (&omission.relative_path, omission.reason))
                .collect::<Vec<_>>(),
            vec![
                (
                    &String::from("nested-0/nested-1/AGENTS.md"),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from("nested-0/nested-1/nested-2/AGENTS.md"),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from("nested-0/nested-1/nested-2/nested-3/AGENTS.md"),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from("nested-0/nested-1/nested-2/nested-3/nested-4/AGENTS.md"),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from(
                        "nested-0/nested-1/nested-2/nested-3/nested-4/nested-5/AGENTS.md"
                    ),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from(
                        "nested-0/nested-1/nested-2/nested-3/nested-4/nested-5/nested-6/AGENTS.md"
                    ),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
                (
                    &String::from(
                        "nested-0/nested-1/nested-2/nested-3/nested-4/nested-5/nested-6/nested-7/AGENTS.md"
                    ),
                    NativeStaticContextOmissionReason::BundleTooLarge
                ),
            ]
        );
    }

    fn extension_context_file(
        package_root: &Path,
        relative_path: &str,
        max_bytes: u64,
    ) -> NativeExtensionStaticContextFile {
        NativeExtensionStaticContextFile {
            extension_id: String::from("example.context-pack"),
            item_id: String::from("rust-style-guide"),
            package_root: package_root.to_path_buf(),
            relative_path: relative_path.to_string(),
            title: String::from("Rust style guide"),
            placement: NativeStaticContextPlacement::BackgroundContext,
            max_bytes,
        }
    }

    #[test]
    fn static_context_extension_file_is_included_from_package_root_after_discovery() {
        let project = TempProject::new("extension-context-project");
        let package = TempProject::new("extension-context-package");
        package.write("context/rust.md", "prefer clear ownership boundaries");

        let assembly = assemble_project_static_context_with_extensions(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
            [extension_context_file(
                package.root(),
                "context/rust.md",
                1024,
            )],
        );

        assert_eq!(assembly.omissions, Vec::new());
        assert_eq!(assembly.bundle.items.len(), 1);
        assert_eq!(
            assembly.bundle.items[0].source,
            NativeStaticContextSource::ExtensionFile {
                extension_id: String::from("example.context-pack"),
                item_id: String::from("rust-style-guide"),
            }
        );
        assert_eq!(assembly.bundle.items[0].relative_path, "context/rust.md");
        assert_eq!(
            assembly.bundle.items[0].placement,
            NativeStaticContextPlacement::BackgroundContext
        );
        assert_eq!(
            assembly.bundle.items[0].priority,
            NativeStaticContextPriority::ExtensionBackground
        );
        assert_eq!(
            assembly.bundle.items[0].title,
            "Extension background context: Rust style guide"
        );
        assert_eq!(
            assembly.bundle.items[0].content,
            "prefer clear ownership boundaries"
        );
    }

    #[test]
    fn static_context_extension_file_path_must_stay_inside_package_root() {
        let project = TempProject::new("extension-context-escape-project");
        let package = TempProject::new("extension-context-escape-package");
        let outside = TempProject::new("extension-context-escape-outside");
        outside.write("context.md", "outside package context");
        let escaped_relative_path = format!(
            "../{}/context.md",
            outside
                .root()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("missing-outside")
        );

        let assembly = assemble_project_static_context_with_extensions(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
            [extension_context_file(
                package.root(),
                &escaped_relative_path,
                1024,
            )],
        );

        assert_eq!(assembly.bundle.items, Vec::new());
        assert_eq!(
            assembly.omissions,
            vec![NativeStaticContextOmission {
                relative_path: escaped_relative_path,
                source: NativeStaticContextSource::ExtensionFile {
                    extension_id: String::from("example.context-pack"),
                    item_id: String::from("rust-style-guide"),
                },
                placement: NativeStaticContextPlacement::BackgroundContext,
                reason: NativeStaticContextOmissionReason::PathOutsideRoot,
            }]
        );
    }

    #[test]
    fn static_context_extension_file_max_bytes_is_enforced() {
        let project = TempProject::new("extension-context-max-project");
        let package = TempProject::new("extension-context-max-package");
        package.write("context/rust.md", "too large");

        let assembly = assemble_project_static_context_with_extensions(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
            [extension_context_file(package.root(), "context/rust.md", 3)],
        );

        assert_eq!(assembly.bundle.items, Vec::new());
        assert_eq!(
            assembly.omissions,
            vec![NativeStaticContextOmission {
                relative_path: String::from("context/rust.md"),
                source: NativeStaticContextSource::ExtensionFile {
                    extension_id: String::from("example.context-pack"),
                    item_id: String::from("rust-style-guide"),
                },
                placement: NativeStaticContextPlacement::BackgroundContext,
                reason: NativeStaticContextOmissionReason::FileTooLarge,
            }]
        );
    }

    #[test]
    fn static_context_extension_file_rejects_non_background_placement() {
        let project = TempProject::new("extension-context-placement-project");
        let package = TempProject::new("extension-context-placement-package");
        package.write("context/system.md", "system mutation attempt");
        let mut file = extension_context_file(package.root(), "context/system.md", 1024);
        file.placement = NativeStaticContextPlacement::AppendSystem;

        let assembly = assemble_project_static_context_with_extensions(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
            [file],
        );

        assert_eq!(assembly.bundle.items, Vec::new());
        assert_eq!(
            assembly.omissions,
            vec![NativeStaticContextOmission {
                relative_path: String::from("context/system.md"),
                source: NativeStaticContextSource::ExtensionFile {
                    extension_id: String::from("example.context-pack"),
                    item_id: String::from("rust-style-guide"),
                },
                placement: NativeStaticContextPlacement::AppendSystem,
                reason: NativeStaticContextOmissionReason::SourceDisabled,
            }]
        );
    }

    #[test]
    fn static_context_extension_omission_excludes_raw_file_contents() {
        let project = TempProject::new("extension-context-redaction-project");
        let package = TempProject::new("extension-context-redaction-package");
        package.write("context/secret.md", "secret extension body");

        let assembly = assemble_project_static_context_with_extensions(
            project.root(),
            project.root(),
            NativeStaticContextPolicy::test(),
            [extension_context_file(
                package.root(),
                "context/secret.md",
                3,
            )],
        );
        let encoded_omissions = serde_json::to_string(&assembly.omissions);

        assert!(encoded_omissions.is_ok());
        assert!(
            !encoded_omissions
                .unwrap_or_default()
                .contains("secret extension body")
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
        let summary = assembly.bundle.summary();
        let accepted_item_bytes = assembly
            .bundle
            .items
            .iter()
            .map(|item| item.byte_count)
            .sum::<usize>();
        let rendered_provider_bytes =
            "# AGENTS.md instructions for .\n\nroot rules\n\n# .yach/APPEND_SYSTEM.md\n\nsystem rules"
                .len();

        assert_eq!(accepted_item_bytes, "root rulessystem rules".len());
        assert_eq!(summary.total_bytes, rendered_provider_bytes);
        assert_eq!(
            summary.items,
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
}
