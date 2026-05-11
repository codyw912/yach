use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

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
}

impl std::fmt::Display for NativeResourcePathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::RootUnavailable => "native resource root unavailable",
            Self::Missing => "native resource path missing",
            Self::EscapesRoot => "native resource path escapes root",
            Self::ExpectedFile => "native resource path is not a file",
            Self::ExpectedDirectory => "native resource path is not a directory",
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

/// Canonicalized native resource root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeResourceRoot {
    pub kind: NativeResourceRootKind,
    canonical_path: PathBuf,
}

impl NativeResourceRoot {
    /// Canonicalize a project root for backend-internal resource resolution.
    ///
    /// This does not make files provider-visible; it only records the root
    /// needed for later explicit, policy-bound reads.
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
        })
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
