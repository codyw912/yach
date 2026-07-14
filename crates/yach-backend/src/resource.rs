use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::sensitive_paths::NativeSensitivePathPolicy;

/// Native resource root classes owned by yach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourceRootKind {
    /// Project-local resources rooted at the current workspace/project.
    Project,
}

/// Errors produced while resolving native resource paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourcePathError {
    RootUnavailable,
    Missing,
    EscapesRoot,
    ExpectedFile,
    ExpectedDirectory,
    SensitiveDenied,
}

impl std::fmt::Display for NativeResourcePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RootUnavailable => "native resource root unavailable",
            Self::Missing => "native resource path missing",
            Self::EscapesRoot => "native resource path escapes root",
            Self::ExpectedFile => "native resource path is not a file",
            Self::ExpectedDirectory => "native resource path is not a directory",
            Self::SensitiveDenied => "native resource path matches the sensitive-file deny list",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NativeResourcePathError {}

/// Provider visibility policy for native resource reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeResourceProviderVisibility {
    Never,
}

/// Errors produced while reading native resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResourceReadError {
    Path(NativeResourcePathError),
    TooLarge { max_bytes: u64, actual_bytes: u64 },
    NotUtf8,
    Io,
}

impl From<NativeResourcePathError> for NativeResourceReadError {
    fn from(error: NativeResourcePathError) -> Self {
        Self::Path(error)
    }
}

/// Explicit read policy for backend-internal native resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeResourceReadPolicy {
    pub max_bytes: u64,
    pub provider_visibility: NativeResourceProviderVisibility,
}

impl NativeResourceReadPolicy {
    #[must_use]
    pub const fn local_only(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            provider_visibility: NativeResourceProviderVisibility::Never,
        }
    }
}

/// Text resource read through an explicit native resource policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceRead {
    pub path: PathBuf,
    pub text: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}

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
    /// True when at least one path was excluded by the sensitive-file deny
    /// list. Records that filtering occurred without naming what was
    /// filtered.
    pub denied_paths_excluded: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}

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
    /// True when at least one entry was excluded by the sensitive-file deny
    /// list.
    pub denied_paths_excluded: bool,
    pub provider_visibility: NativeResourceProviderVisibility,
}

/// Canonicalized native resource root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceRoot {
    pub kind: NativeResourceRootKind,
    canonical_path: PathBuf,
    sensitive_policy: NativeSensitivePathPolicy,
}

impl NativeResourceRoot {
    /// Canonicalize a project root for backend-internal resource resolution.
    ///
    /// This does not make files provider-visible; it only records the root
    /// needed for later explicit, policy-bound reads. The built-in
    /// sensitive-file deny policy applies by default.
    pub fn project(path: impl AsRef<Path>) -> Result<Self, NativeResourcePathError> {
        let canonical_path = path
            .as_ref()
            .canonicalize()
            .map_err(|_| NativeResourcePathError::RootUnavailable)?;
        if !canonical_path.is_dir() {
            return Err(NativeResourcePathError::RootUnavailable);
        }

        Ok(Self {
            kind: NativeResourceRootKind::Project,
            canonical_path,
            sensitive_policy: NativeSensitivePathPolicy::default(),
        })
    }

    /// Replace the sensitive-file policy, e.g. with a config-resolved one.
    #[must_use]
    pub fn with_sensitive_policy(mut self, policy: NativeSensitivePathPolicy) -> Self {
        self.sensitive_policy = policy;
        self
    }

    /// Whether the sensitive-file deny list refuses this relative path.
    /// The single chokepoint for read/search/list/edit decisions.
    #[must_use]
    pub fn sensitive_denies(&self, relative_path: impl AsRef<Path>) -> bool {
        self.sensitive_policy.denies(relative_path)
    }

    #[must_use]
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn resolve_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let path = self.resolve_existing(relative_path)?;
        if !path.is_file() {
            return Err(NativeResourcePathError::ExpectedFile);
        }
        Ok(path)
    }

    pub fn read_text_file(
        &self,
        relative_path: impl AsRef<Path>,
        policy: NativeResourceReadPolicy,
    ) -> Result<NativeResourceRead, NativeResourceReadError> {
        let path = self.resolve_file(relative_path)?;
        if self.sensitive_denies(self.normalized_relative_path(&path)?) {
            return Err(NativeResourceReadError::Path(
                NativeResourcePathError::SensitiveDenied,
            ));
        }
        let metadata = fs::metadata(&path).map_err(|_| NativeResourceReadError::Io)?;
        if metadata.len() > policy.max_bytes {
            return Err(NativeResourceReadError::TooLarge {
                max_bytes: policy.max_bytes,
                actual_bytes: metadata.len(),
            });
        }

        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|_| NativeResourceReadError::Io)?;
        let byte_count = bytes.len();
        if u64::try_from(byte_count).map_or(true, |actual| actual > policy.max_bytes) {
            return Err(NativeResourceReadError::TooLarge {
                max_bytes: policy.max_bytes,
                actual_bytes: u64::try_from(byte_count).unwrap_or(u64::MAX),
            });
        }
        let text = String::from_utf8(bytes).map_err(|_| NativeResourceReadError::NotUtf8)?;

        Ok(NativeResourceRead {
            path,
            text,
            byte_count,
            redacted: false,
            truncated: false,
            provider_visibility: policy.provider_visibility,
        })
    }

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
                .read_text_file(
                    &path,
                    NativeResourceReadPolicy::local_only(policy.max_file_bytes),
                )
                .map_err(|error| NativeResourceContextError::Read {
                    relative_path: path.to_string_lossy().into_owned(),
                    error,
                })?;
            items.push(NativeResourceContextItem {
                relative_path: self.normalized_relative_path(&read.path).map_err(|error| {
                    NativeResourceContextError::Read {
                        relative_path: path.to_string_lossy().into_owned(),
                        error: NativeResourceReadError::Path(error),
                    }
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

    pub fn search_text(
        &self,
        query: &str,
        policy: NativeResourceSearchPolicy,
    ) -> Result<NativeResourceSearchResult, NativeResourcePathError> {
        let mut result = NativeResourceSearchResult {
            matches: Vec::new(),
            searched_files: 0,
            truncated: false,
            denied_paths_excluded: false,
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
        let mut entries = entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeResourcePathError::Missing)?;
        entries.sort_by_key(|entry| {
            self.normalized_relative_path(&entry.path())
                .unwrap_or_else(|_| entry.file_name().to_string_lossy().into_owned())
        });

        for entry in entries {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                if generated_or_heavy_resource_entry(&file_name) {
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

        let relative_path = self.normalized_relative_path(path)?;
        if self.sensitive_denies(&relative_path) {
            result.denied_paths_excluded = true;
            return Ok(());
        }

        let Ok(metadata) = fs::metadata(path) else {
            return Ok(());
        };
        if metadata.len() > policy.max_file_bytes {
            return Ok(());
        }

        result.searched_files = result.searched_files.saturating_add(1);
        let read = self.read_text_file(
            Path::new(&relative_path),
            NativeResourceReadPolicy::local_only(policy.max_file_bytes),
        );
        let Ok(read) = read else {
            return Ok(());
        };

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
        let mut denied_paths_excluded = false;
        for entry in entries {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if generated_or_heavy_resource_entry(&file_name) {
                continue;
            }
            if self.sensitive_denies(self.normalized_relative_path(&entry.path())?) {
                denied_paths_excluded = true;
                continue;
            }
            if result_entries.len() >= policy.max_entries {
                truncated = true;
                break;
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
            let byte_size = if metadata.is_file() {
                Some(metadata.len())
            } else {
                None
            };

            result_entries.push(NativeResourceListEntry {
                relative_path: self.normalized_relative_path(&entry.path())?,
                kind,
                byte_size,
            });
        }

        Ok(NativeResourceListResult {
            relative_path,
            entries: result_entries,
            truncated,
            denied_paths_excluded,
            provider_visibility: NativeResourceProviderVisibility::Never,
        })
    }

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

    pub fn resolve_directory(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let path = self.resolve_existing(relative_path)?;
        if !path.is_dir() {
            return Err(NativeResourcePathError::ExpectedDirectory);
        }
        Ok(path)
    }

    fn resolve_existing(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<PathBuf, NativeResourcePathError> {
        let requested = relative_path.as_ref();
        if requested.is_absolute() {
            return Err(NativeResourcePathError::EscapesRoot);
        }

        let canonical = self
            .canonical_path
            .join(requested)
            .canonicalize()
            .map_err(|_| NativeResourcePathError::Missing)?;
        if !canonical.starts_with(&self.canonical_path) {
            return Err(NativeResourcePathError::EscapesRoot);
        }
        Ok(canonical)
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
}

fn generated_or_heavy_resource_entry(file_name: &str) -> bool {
    matches!(
        file_name,
        ".git"
            | ".yach"
            | "target"
            | ".jj"
            | ".hg"
            | ".svn"
            | ".devenv"
            | ".direnv"
            | ".worktrees"
            | ".cache"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
    )
}
