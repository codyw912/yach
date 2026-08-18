//! Authentication error shared by the OAuth-capable providers (ChatGPT,
//! Copilot). Re-exported from each provider's `auth` module as `AuthError`.

use std::path::PathBuf;
use std::time::SystemTime;

/// Kind of unsafe final auth-file entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeEntryKind {
    /// The final path component is a symlink.
    Symlink,
    /// The final path component is a directory or other non-regular file.
    NonRegular,
}

/// Classified present-entry repair cause. The original Io/Json error is discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairKind {
    /// The file exists but is not valid JSON / auth record.
    Corrupt,
    /// The file exists but cannot be read.
    Unreadable,
    /// The entry is unsafe in a way that is still fence-deletable.
    UnsafeEntry,
    /// A valid token has no parseable account claim (yach mapping layer).
    MissingIdentity,
}

/// File-type recorded in an entry token (metadata only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFileType {
    /// Regular file.
    Regular,
    /// Symbolic link (token may still be used to fenced-unlink).
    Symlink,
}

/// Platform file identity used for fencing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    /// Device / volume identifier.
    pub device: u64,
    /// Inode / file index.
    pub inode: u64,
}

/// Metadata-only identity of an auth-file entry. Never contains token bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthEntryToken {
    /// Resolved physical path at the moment of observation.
    pub resolved_path: PathBuf,
    /// Observed file type.
    pub file_type: AuthFileType,
    /// Platform identity.
    pub identity: FileIdentity,
    /// Last modification time.
    pub mtime: Option<SystemTime>,
    /// Status-change time (Unix ctime). May be unavailable.
    pub ctime: Option<SystemTime>,
    /// Observed size in bytes.
    pub size: u64,
}

impl std::fmt::Debug for AuthEntryToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthEntryToken")
            .field("file_type", &self.file_type)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Expected auth-file state for a fenced write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedAuthEntry {
    /// No regular auth file may exist.
    Absent,
    /// The present entry must still match this token.
    Present(AuthEntryToken),
}

#[derive(thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    /// Device flow is disabled and no usable cached token exists.
    #[error("ChatGPT sign-in required")]
    DeviceFlowDisabled,
    /// Another process holds the auth-file lock.
    #[error("ChatGPT auth file is busy")]
    AuthBusy,
    /// The auth-file state changed under a fenced operation.
    #[error("ChatGPT auth file changed")]
    AuthConflict,
    /// The cached account does not match the expected account.
    #[error("ChatGPT account mismatch")]
    AccountMismatch {
        expected: Option<String>,
        actual: Option<String>,
        entry: AuthEntryToken,
    },
    /// The auth file path is a symlink or non-regular entry.
    #[error("ChatGPT auth file is unsafe")]
    UnsafeAuthFile {
        kind: UnsafeEntryKind,
        entry: Option<AuthEntryToken>,
    },
    /// The lock file itself is a symlink or non-regular entry.
    #[error("ChatGPT auth lock file is unsafe")]
    UnsafeLockFile,
    /// A present auth file cannot be used and must be repaired.
    #[error("ChatGPT auth file requires repair: {detail}")]
    RepairRequired {
        kind: RepairKind,
        detail: String,
        entry: AuthEntryToken,
    },
}

impl std::fmt::Debug for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(_) => f.write_str("Message(<redacted>)"),
            Self::Io(_) => f.write_str("Io(<redacted>)"),
            Self::Json(_) => f.write_str("Json(<redacted>)"),
            Self::Http(_) => f.write_str("Http(<redacted>)"),
            Self::DeviceFlowDisabled => f.write_str("DeviceFlowDisabled"),
            Self::AuthBusy => f.write_str("AuthBusy"),
            Self::AuthConflict => f.write_str("AuthConflict"),
            Self::AccountMismatch {
                expected, actual, ..
            } => f
                .debug_struct("AccountMismatch")
                .field("expected", expected)
                .field("actual", actual)
                .finish_non_exhaustive(),
            Self::UnsafeAuthFile { kind, .. } => f
                .debug_struct("UnsafeAuthFile")
                .field("kind", kind)
                .finish_non_exhaustive(),
            Self::UnsafeLockFile => f.write_str("UnsafeLockFile"),
            Self::RepairRequired { kind, detail, .. } => f
                .debug_struct("RepairRequired")
                .field("kind", kind)
                .field("detail", detail)
                .finish_non_exhaustive(),
        }
    }
}
