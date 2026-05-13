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

#[must_use]
pub fn assemble_project_static_context(
    project_root: impl AsRef<Path>,
    cwd: impl AsRef<Path>,
    policy: NativeStaticContextPolicy,
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
                title_for_path: agents_title,
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
            title_for_path: append_system_title,
        },
        policy.max_total_bytes,
    );

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
    title_for_path: fn(&str) -> String,
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
            NativeStaticContextOmissionReason::Io,
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

    let Ok(metadata) = std::fs::metadata(path) else {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::Io,
        ));
        return;
    };

    if metadata.len() > candidate.max_file_bytes {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::FileTooLarge,
        ));
        return;
    }

    let Ok(bytes) = std::fs::read(path) else {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::Io,
        ));
        return;
    };

    if assembly.bundle.total_bytes + bytes.len() > max_total_bytes {
        assembly.omissions.push(omission(
            relative_path,
            candidate.source.clone(),
            candidate.placement,
            NativeStaticContextOmissionReason::BundleTooLarge,
        ));
        return;
    }

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
    let title = (candidate.title_for_path)(&relative_path);
    assembly.bundle.items.push(NativeStaticContextItem {
        source: candidate.source.clone(),
        relative_path,
        placement: candidate.placement,
        title,
        content,
        byte_count,
        priority: candidate.priority,
    });
    assembly.bundle.total_bytes += byte_count;
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
}
