use std::collections::{BTreeSet, HashSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};

use super::{
    ExtensionCapability, ExtensionCapabilityGrant, requested_capabilities, utc_timestamp_now,
};
use crate::extension::ExtensionToolContribution;

const SCHEMA: &str = "yach.extension-authority.v1";
const GRANT_REASON: &str = "extension_capability_grant";
const REVOKE_REASON: &str = "extension_capability_revoke";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDecisionSurface {
    Cli,
    Lifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionAuthorityError {
    HomeUnavailable,
    InvalidId,
    InvalidDocument,
    UnsupportedSchema,
    UnsafePath,
    UnsafePermissions,
    Io,
    DurabilityUnknown,
}

#[derive(Debug, Clone)]
pub struct ExtensionAuthorityStore {
    home: PathBuf,
}

impl fmt::Display for ExtensionAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HomeUnavailable => "user home directory is unavailable",
            Self::InvalidId => "extension id is invalid",
            Self::InvalidDocument => "extension authority document is invalid",
            Self::UnsupportedSchema => "extension authority schema is unsupported",
            Self::UnsafePath => "extension authority path is unsafe",
            Self::UnsafePermissions => "extension authority permissions are unsafe",
            Self::Io => "extension authority storage is unavailable",
            Self::DurabilityUnknown => {
                "extension authority was updated but storage durability could not be confirmed"
            }
        })
    }
}

impl std::error::Error for ExtensionAuthorityError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDocument {
    schema: String,
    extension_id: String,
    current: Option<ExtensionCapabilityGrant>,
    legacy_baseline: Option<ExtensionCapabilityGrant>,
    history: Vec<AuthorityDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthorityAction {
    Grant,
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDecision {
    operation_id: String,
    recorded_at: String,
    action: AuthorityAction,
    reason: String,
    surface: ExtensionDecisionSurface,
    before: Option<ExtensionCapabilityGrant>,
    after: Option<ExtensionCapabilityGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyGrant {
    approved: BTreeSet<ExtensionCapability>,
    version_at_grant: String,
    granted_at: String,
}

enum LoadedDocument {
    Missing,
    Legacy(ExtensionCapabilityGrant),
    Versioned(AuthorityDocument),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitPhase {
    BeforeRename,
    AfterRename,
}

impl ExtensionAuthorityStore {
    pub fn for_current_user() -> Result<Self, ExtensionAuthorityError> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or(ExtensionAuthorityError::HomeUnavailable)?;
        Ok(Self::in_home(Path::new(&home)))
    }

    #[must_use]
    pub fn in_home(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }

    pub fn load_grant(
        &self,
        id: &str,
    ) -> Result<Option<ExtensionCapabilityGrant>, ExtensionAuthorityError> {
        match self.load_document(id)? {
            LoadedDocument::Missing => Ok(None),
            LoadedDocument::Legacy(grant) => Ok(Some(grant)),
            LoadedDocument::Versioned(document) => Ok(document.current),
        }
    }

    pub fn grant_requested(
        &self,
        id: &str,
        version: &str,
        tools: &[ExtensionToolContribution],
        surface: ExtensionDecisionSurface,
    ) -> Result<Option<ExtensionCapabilityGrant>, ExtensionAuthorityError> {
        let approved = requested_capabilities(tools);
        if approved.is_empty() {
            let _ = self.document_path(id)?;
            return Ok(None);
        }
        self.mutate(id, |loaded| {
            let mut document = loaded.into_versioned(id);
            let before = document.current.clone();
            let grant = ExtensionCapabilityGrant {
                approved,
                version_at_grant: version.to_owned(),
                granted_at: utc_timestamp_now(),
            };
            document.history.push(AuthorityDecision {
                operation_id: uuid::Uuid::new_v4().to_string(),
                recorded_at: utc_timestamp_now(),
                action: AuthorityAction::Grant,
                reason: String::from(GRANT_REASON),
                surface,
                before,
                after: Some(grant.clone()),
            });
            document.current = Some(grant.clone());
            Ok((document, Some(grant)))
        })
    }

    pub fn revoke_grant(
        &self,
        id: &str,
        surface: ExtensionDecisionSurface,
    ) -> Result<bool, ExtensionAuthorityError> {
        self.mutate(id, |loaded| {
            let mut document = loaded.into_versioned(id);
            let before = document.current.clone();
            let had_grant = before.is_some();
            document.history.push(AuthorityDecision {
                operation_id: uuid::Uuid::new_v4().to_string(),
                recorded_at: utc_timestamp_now(),
                action: AuthorityAction::Revoke,
                reason: String::from(REVOKE_REASON),
                surface,
                before,
                after: None,
            });
            document.current = None;
            Ok((document, had_grant))
        })
    }

    fn yach_dir(&self) -> PathBuf {
        self.home.join(".yach")
    }

    fn grants_dir(&self) -> PathBuf {
        self.yach_dir().join("extensions")
    }

    fn document_path(&self, id: &str) -> Result<PathBuf, ExtensionAuthorityError> {
        if !crate::is_valid_extension_id(id) {
            return Err(ExtensionAuthorityError::InvalidId);
        }
        Ok(self.grants_dir().join(format!("{id}.json")))
    }

    fn load_document(&self, id: &str) -> Result<LoadedDocument, ExtensionAuthorityError> {
        let path = self.document_path(id)?;
        inspect_existing_state_dirs(&self.yach_dir(), &self.grants_dir())?;
        let Some(bytes) = read_private_file(&path)? else {
            return Ok(LoadedDocument::Missing);
        };
        parse_authority_bytes(&bytes, id)
    }

    fn mutate<T>(
        &self,
        id: &str,
        update: impl FnOnce(LoadedDocument) -> Result<(AuthorityDocument, T), ExtensionAuthorityError>,
    ) -> Result<T, ExtensionAuthorityError> {
        let path = self.document_path(id)?;
        prepare_grants_dir(&self.yach_dir(), &self.grants_dir())?;
        ensure_regular_private_or_missing(&path)?;
        let lock_path = self.grants_dir().join(format!("{id}.json.lock"));
        ensure_regular_private_or_missing(&lock_path)?;
        let lock = open_private_lock(&lock_path)?;
        lock.lock_exclusive()
            .map_err(|_| ExtensionAuthorityError::Io)?;
        let loaded = self.load_document(id)?;
        let (document, result) = update(loaded)?;
        validate_document(&document, id)?;
        let bytes =
            serde_json::to_vec_pretty(&document).map_err(|_| ExtensionAuthorityError::Io)?;
        commit_document(&path, &bytes, |_| Ok(()))?;
        Ok(result)
    }
}

impl LoadedDocument {
    fn into_versioned(self, id: &str) -> AuthorityDocument {
        match self {
            Self::Missing => AuthorityDocument {
                schema: String::from(SCHEMA),
                extension_id: String::from(id),
                current: None,
                legacy_baseline: None,
                history: Vec::new(),
            },
            Self::Legacy(grant) => AuthorityDocument {
                schema: String::from(SCHEMA),
                extension_id: String::from(id),
                current: Some(grant.clone()),
                legacy_baseline: Some(grant),
                history: Vec::new(),
            },
            Self::Versioned(document) => document,
        }
    }
}

fn parse_authority_bytes(
    bytes: &[u8],
    expected_id: &str,
) -> Result<LoadedDocument, ExtensionAuthorityError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ExtensionAuthorityError::InvalidDocument)?;
    let Some(object) = value.as_object() else {
        return Err(ExtensionAuthorityError::InvalidDocument);
    };
    match object.get("schema") {
        None => parse_legacy(value),
        Some(schema) if schema.is_null() => Err(ExtensionAuthorityError::InvalidDocument),
        Some(schema) => {
            let Some(name) = schema.as_str() else {
                return Err(ExtensionAuthorityError::InvalidDocument);
            };
            if name != SCHEMA {
                return Err(ExtensionAuthorityError::UnsupportedSchema);
            }
            for key in [
                "schema",
                "extension_id",
                "current",
                "legacy_baseline",
                "history",
            ] {
                if !object.contains_key(key) {
                    return Err(ExtensionAuthorityError::InvalidDocument);
                }
            }
            parse_versioned(value, expected_id)
        }
    }
}

fn parse_legacy(value: serde_json::Value) -> Result<LoadedDocument, ExtensionAuthorityError> {
    let grant: LegacyGrant =
        serde_json::from_value(value).map_err(|_| ExtensionAuthorityError::InvalidDocument)?;
    if grant.approved.is_empty() {
        return Err(ExtensionAuthorityError::InvalidDocument);
    }
    Ok(LoadedDocument::Legacy(ExtensionCapabilityGrant {
        approved: grant.approved,
        version_at_grant: grant.version_at_grant,
        granted_at: grant.granted_at,
    }))
}

fn parse_versioned(
    value: serde_json::Value,
    expected_id: &str,
) -> Result<LoadedDocument, ExtensionAuthorityError> {
    require_decision_keys(&value)?;
    let document: AuthorityDocument =
        serde_json::from_value(value).map_err(|_| ExtensionAuthorityError::InvalidDocument)?;
    validate_document(&document, expected_id)?;
    Ok(LoadedDocument::Versioned(document))
}

fn require_decision_keys(value: &serde_json::Value) -> Result<(), ExtensionAuthorityError> {
    let Some(history) = value.get("history").and_then(serde_json::Value::as_array) else {
        return Err(ExtensionAuthorityError::InvalidDocument);
    };
    for entry in history {
        let Some(object) = entry.as_object() else {
            return Err(ExtensionAuthorityError::InvalidDocument);
        };
        for key in [
            "operation_id",
            "recorded_at",
            "action",
            "reason",
            "surface",
            "before",
            "after",
        ] {
            if !object.contains_key(key) {
                return Err(ExtensionAuthorityError::InvalidDocument);
            }
        }
    }
    Ok(())
}

fn validate_document(
    document: &AuthorityDocument,
    expected_id: &str,
) -> Result<(), ExtensionAuthorityError> {
    if document.schema != SCHEMA {
        return Err(ExtensionAuthorityError::UnsupportedSchema);
    }
    if document.extension_id != expected_id {
        return Err(ExtensionAuthorityError::InvalidDocument);
    }
    if grant_is_empty(document.current.as_ref())
        || grant_is_empty(document.legacy_baseline.as_ref())
    {
        return Err(ExtensionAuthorityError::InvalidDocument);
    }
    let mut seen = HashSet::new();
    let mut previous = document.legacy_baseline.as_ref();
    for entry in &document.history {
        let Ok(operation_id) = uuid::Uuid::parse_str(&entry.operation_id) else {
            return Err(ExtensionAuthorityError::InvalidDocument);
        };
        if !seen.insert(operation_id) {
            return Err(ExtensionAuthorityError::InvalidDocument);
        }
        if entry.recorded_at.is_empty() {
            return Err(ExtensionAuthorityError::InvalidDocument);
        }
        match entry.action {
            AuthorityAction::Grant => {
                if entry.reason != GRANT_REASON
                    || entry
                        .after
                        .as_ref()
                        .is_none_or(|grant| grant.approved.is_empty())
                {
                    return Err(ExtensionAuthorityError::InvalidDocument);
                }
            }
            AuthorityAction::Revoke => {
                if entry.reason != REVOKE_REASON || entry.after.is_some() {
                    return Err(ExtensionAuthorityError::InvalidDocument);
                }
            }
        }
        if entry.before.as_ref() != previous {
            return Err(ExtensionAuthorityError::InvalidDocument);
        }
        previous = entry.after.as_ref();
    }
    if document.current.as_ref() != previous {
        return Err(ExtensionAuthorityError::InvalidDocument);
    }
    Ok(())
}

fn grant_is_empty(grant: Option<&ExtensionCapabilityGrant>) -> bool {
    grant.is_some_and(|grant| grant.approved.is_empty())
}

fn inspect_existing_state_dirs(yach: &Path, grants: &Path) -> Result<(), ExtensionAuthorityError> {
    match fs::symlink_metadata(yach) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ExtensionAuthorityError::Io),
        Ok(_) => {
            ensure_private_directory(yach)?;
            match fs::symlink_metadata(grants) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(ExtensionAuthorityError::Io),
                Ok(_) => ensure_private_directory(grants),
            }
        }
    }
}

fn prepare_grants_dir(yach: &Path, grants: &Path) -> Result<(), ExtensionAuthorityError> {
    create_or_verify_private_dir(yach)?;
    create_or_verify_private_dir(grants)
}

fn create_or_verify_private_dir(path: &Path) -> Result<(), ExtensionAuthorityError> {
    create_or_verify_private_dir_with_sync(path, sync_directory)
}

fn create_or_verify_private_dir_with_sync(
    path: &Path,
    sync: impl Fn(&Path) -> io::Result<()>,
) -> Result<(), ExtensionAuthorityError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path).map_err(|_| ExtensionAuthorityError::Io)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ExtensionAuthorityError::UnsafePath);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
                if metadata.uid() != unsafe { libc::geteuid() } {
                    return Err(ExtensionAuthorityError::UnsafePermissions);
                }
                if metadata.mode() & 0o077 != 0 {
                    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                        .map_err(|_| ExtensionAuthorityError::Io)?;
                }
            }
        }
        Err(_) => return Err(ExtensionAuthorityError::Io),
    }
    ensure_private_directory(path)?;
    map_pre_commit_sync(sync(path))?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        map_pre_commit_sync(sync(parent))?;
    }
    Ok(())
}

fn commit_document(
    path: &Path,
    bytes: &[u8],
    phase_hook: impl Fn(CommitPhase) -> io::Result<()>,
) -> Result<(), ExtensionAuthorityError> {
    let Some(parent) = path.parent() else {
        return Err(ExtensionAuthorityError::UnsafePath);
    };
    if parent.as_os_str().is_empty() {
        return Err(ExtensionAuthorityError::UnsafePath);
    }
    ensure_private_directory(parent)?;
    ensure_regular_private_or_missing(path)?;
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(ExtensionAuthorityError::UnsafePath);
    };
    let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let Ok(mut file) = options.open(&temporary) else {
        return Err(ExtensionAuthorityError::Io);
    };
    let Ok(metadata) = file.metadata() else {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionAuthorityError::Io);
    };
    if let Err(error) = ensure_regular_private(&temporary, &metadata) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionAuthorityError::Io);
    }
    drop(file);
    if phase_hook(CommitPhase::BeforeRename).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionAuthorityError::Io);
    }
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(ExtensionAuthorityError::Io);
    }
    if phase_hook(CommitPhase::AfterRename).is_err() {
        return Err(ExtensionAuthorityError::DurabilityUnknown);
    }
    map_post_rename_sync(sync_directory(parent))
}

fn open_private_lock(path: &Path) -> Result<File, ExtensionAuthorityError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| map_open_error(&error))?;
    ensure_regular_private(
        path,
        &file.metadata().map_err(|_| ExtensionAuthorityError::Io)?,
    )?;
    Ok(file)
}

fn read_private_file(path: &Path) -> Result<Option<Vec<u8>>, ExtensionAuthorityError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(map_open_error(&error)),
    };
    ensure_regular_private(
        path,
        &file.metadata().map_err(|_| ExtensionAuthorityError::Io)?,
    )?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ExtensionAuthorityError::Io)?;
    Ok(Some(bytes))
}

fn ensure_regular_private_or_missing(path: &Path) -> Result<(), ExtensionAuthorityError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure_regular_private(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ExtensionAuthorityError::Io),
    }
}

fn ensure_regular_private(
    _path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), ExtensionAuthorityError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ExtensionAuthorityError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(ExtensionAuthorityError::UnsafePermissions);
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), ExtensionAuthorityError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ExtensionAuthorityError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ExtensionAuthorityError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(ExtensionAuthorityError::UnsafePermissions);
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn map_pre_commit_sync(result: io::Result<()>) -> Result<(), ExtensionAuthorityError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => Ok(()),
        Err(_) => Err(ExtensionAuthorityError::Io),
    }
}

fn map_post_rename_sync(result: io::Result<()>) -> Result<(), ExtensionAuthorityError> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => Ok(()),
        Err(_) => Err(ExtensionAuthorityError::DurabilityUnknown),
    }
}

fn map_open_error(error: &io::Error) -> ExtensionAuthorityError {
    if is_symlink_open_error(error) {
        ExtensionAuthorityError::UnsafePath
    } else {
        ExtensionAuthorityError::Io
    }
}

fn is_symlink_open_error(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CommitPhase, ExtensionAuthorityError, ExtensionAuthorityStore, ExtensionDecisionSurface,
        commit_document, create_or_verify_private_dir_with_sync,
    };
    use crate::extension::{ExtensionToolContribution, ExtensionToolRisk};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const CONCURRENCY_HOME_ENV: &str = "YACH_EXTENSION_AUTHORITY_TEST_HOME";
    const CONCURRENCY_ITERS_ENV: &str = "YACH_EXTENSION_AUTHORITY_TEST_ITERS";
    const CONCURRENCY_HELPER: &str =
        "extension_capability::store::tests::concurrent_authority_writer_helper";
    const CONCURRENCY_ITERS: usize = 8;
    const COMMIT_WRITE_PATH_ENV: &str = "YACH_EXTENSION_AUTHORITY_COMMIT_PATH";
    const COMMIT_WRITE_HELPER: &str =
        "extension_capability::store::tests::commit_document_write_limit_helper";

    struct TestHome {
        path: PathBuf,
    }

    impl TestHome {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("yach-authority-home-{}", uuid::Uuid::new_v4()));
            assert!(
                fs::create_dir_all(&path).is_ok(),
                "temporary home should be created: {path:?}"
            );
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn store(&self) -> ExtensionAuthorityStore {
            ExtensionAuthorityStore::in_home(&self.path)
        }

        fn document_path(&self, id: &str) -> PathBuf {
            self.path
                .join(".yach")
                .join("extensions")
                .join(format!("{id}.json"))
        }

        fn document(&self, id: &str) -> serde_json::Value {
            let bytes = fs::read(self.document_path(id));
            assert!(bytes.is_ok(), "authority document should exist: {bytes:?}");
            let Ok(bytes) = bytes else {
                return serde_json::Value::Null;
            };
            let parsed = serde_json::from_slice(&bytes);
            assert!(
                parsed.is_ok(),
                "authority document should parse: {parsed:?}"
            );
            let Ok(parsed) = parsed else {
                return serde_json::Value::Null;
            };
            parsed
        }

        fn write_raw(&self, id: &str, bytes: &[u8]) {
            let path = self.document_path(id);
            let Some(parent) = path.parent() else {
                return;
            };
            assert!(
                fs::create_dir_all(parent).is_ok(),
                "grants directory should be created: {parent:?}"
            );
            privatize_dir(&self.path.join(".yach"));
            privatize_dir(parent);
            assert!(
                fs::write(&path, bytes).is_ok(),
                "raw authority bytes should write: {path:?}"
            );
            privatize_file(&path);
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn privatize_dir(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).is_ok(),
                "directory should be private: {path:?}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    fn privatize_file(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).is_ok(),
                "file should be private: {path:?}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    fn tool(name: &str, risk: ExtensionToolRisk) -> ExtensionToolContribution {
        ExtensionToolContribution {
            name: String::from(name),
            description: String::from("fixture"),
            risk,
            provider_visible: true,
        }
    }

    #[test]
    fn private_dir_retry_syncs_existing_directory_and_parent() {
        let root = std::env::temp_dir().join(format!("yach-private-dir-{}", uuid::Uuid::new_v4()));
        assert!(fs::create_dir_all(&root).is_ok());
        let path = root.join("child");
        let Some(parent) = path.parent() else {
            let _ = fs::remove_dir_all(root);
            return;
        };
        let first = create_or_verify_private_dir_with_sync(&path, |candidate| {
            if candidate == parent {
                Err(io::Error::other("injected parent sync failure"))
            } else {
                Ok(())
            }
        });
        assert_eq!(first, Err(ExtensionAuthorityError::Io));
        let second = create_or_verify_private_dir_with_sync(&path, |_| {
            Err(io::Error::other("injected retry sync failure"))
        });
        assert_eq!(second, Err(ExtensionAuthorityError::Io));
        let third = create_or_verify_private_dir_with_sync(&path, |_| Ok(()));
        assert_eq!(third, Ok(()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn revoke_retains_evidence_but_denies_activation() {
        let home = TestHome::new();
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let granted =
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(granted, Ok(Some(_))));
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Ok(true)
        );
        assert_eq!(store.load_grant("example.net"), Ok(None));
        let doc = home.document("example.net");
        assert!(doc["current"].is_null());
        assert_eq!(
            doc["history"][0]["after"]["approved"],
            serde_json::json!(["uses_network"])
        );
        assert_eq!(doc["history"][1]["before"], doc["history"][0]["after"]);
        assert!(doc["history"][1]["after"].is_null());
        assert_eq!(doc["history"][1]["reason"], "extension_capability_revoke");
    }

    #[test]
    fn repeat_revoke_records_null_to_null_and_reports_absent() {
        let home = TestHome::new();
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let granted =
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(granted, Ok(Some(_))));
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Ok(true)
        );
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Ok(false)
        );
        let doc = home.document("example.net");
        assert!(doc["current"].is_null());
        assert_eq!(doc["history"][2]["action"], "revoke");
        assert!(doc["history"][2]["before"].is_null());
        assert!(doc["history"][2]["after"].is_null());
        assert_eq!(doc["history"][2]["reason"], "extension_capability_revoke");
    }

    #[test]
    fn repeat_grant_retains_both_decisions() {
        let home = TestHome::new();
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let first =
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(first, Ok(Some(_))));
        let second =
            store.grant_requested("example.net", "2.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(second, Ok(Some(_))));
        let doc = home.document("example.net");
        assert_eq!(doc["history"][0]["action"], "grant");
        assert_eq!(doc["history"][1]["action"], "grant");
        assert_eq!(doc["history"][1]["before"], doc["history"][0]["after"]);
        assert_eq!(doc["history"][1]["after"]["version_at_grant"], "2.0");
        assert_eq!(doc["current"], doc["history"][1]["after"]);
    }

    #[test]
    fn no_capability_trust_creates_no_document() {
        let home = TestHome::new();
        let store = home.store();
        let tools = vec![tool("read", ExtensionToolRisk::ReadsLocalContent)];
        let granted =
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
        assert_eq!(granted, Ok(None));
        assert!(!home.document_path("example.net").exists());
    }

    #[test]
    fn restart_reload_denies_a_retained_revoked_document() {
        let home = TestHome::new();
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let granted =
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(granted, Ok(Some(_))));
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Ok(true)
        );
        drop(store);
        let restarted = ExtensionAuthorityStore::in_home(home.path());
        assert_eq!(restarted.load_grant("example.net"), Ok(None));
        let doc = home.document("example.net");
        assert!(doc["current"].is_null());
        assert_eq!(doc["history"][1]["action"], "revoke");
    }

    #[test]
    fn malformed_document_mutation_returns_error_without_changing_bytes() {
        let home = TestHome::new();
        home.write_raw("example.net", b"{not-json");
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli,),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn unknown_schema_cannot_masquerade_as_legacy() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": "yach.extension-authority.v0",
                "approved": ["uses_network"],
                "version_at_grant": "1.0",
                "granted_at": "2026-09-16T00:00:00Z"
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::UnsupportedSchema)
        );
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli,),
            Err(ExtensionAuthorityError::UnsupportedSchema)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn invalid_ids_do_not_mutate_a_sentinel_outside_the_grants_directory() {
        let home = TestHome::new();
        let sentinel = home.path().join("keep.json");
        assert!(
            fs::write(&sentinel, b"keep").is_ok(),
            "sentinel should write: {sentinel:?}"
        );
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("../keep", "1.0", &tools, ExtensionDecisionSurface::Cli,),
            Err(ExtensionAuthorityError::InvalidId)
        );
        assert_eq!(
            store.revoke_grant("../keep", ExtensionDecisionSurface::Cli),
            Err(ExtensionAuthorityError::InvalidId)
        );
        assert_eq!(
            store.load_grant("/tmp/keep"),
            Err(ExtensionAuthorityError::InvalidId)
        );
        let leftover = fs::read(&sentinel);
        assert_eq!(leftover.ok().as_deref(), Some(b"keep".as_slice()));
        assert!(!home.document_path("../keep").exists());
    }

    #[test]
    fn explicit_null_schema_is_not_legacy() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": null,
                "approved": ["uses_network"],
                "version_at_grant": "1.0",
                "granted_at": "2026-09-16T00:00:00Z"
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli,),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_world_readable_grants_directory_without_mutation() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#,
        );
        let grants = home.path().join(".yach").join("extensions");
        assert!(fs::set_permissions(&grants, fs::Permissions::from_mode(0o755)).is_ok());
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::UnsafePermissions)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_world_readable_yach_directory_without_mutation() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#,
        );
        let yach = home.path().join(".yach");
        assert!(fs::set_permissions(&yach, fs::Permissions::from_mode(0o755)).is_ok());
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::UnsafePermissions)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn grant_decision_with_null_after_is_rejected_without_mutation() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": "yach.extension-authority.v1",
                "extension_id": "example.net",
                "current": null,
                "legacy_baseline": null,
                "history": [{
                    "operation_id": "550e8400-e29b-41d4-a716-446655440000",
                    "recorded_at": "2026-09-16T00:00:00Z",
                    "action": "grant",
                    "reason": "extension_capability_grant",
                    "surface": "cli",
                    "before": null,
                    "after": null
                }]
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn missing_decision_before_key_is_rejected_without_mutation() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": "yach.extension-authority.v1",
                "extension_id": "example.net",
                "current": {
                    "approved": ["uses_network"],
                    "version_at_grant": "1.0",
                    "granted_at": "2026-09-16T00:00:00Z"
                },
                "legacy_baseline": null,
                "history": [{
                    "operation_id": "550e8400-e29b-41d4-a716-446655440000",
                    "recorded_at": "2026-09-16T00:00:00Z",
                    "action": "grant",
                    "reason": "extension_capability_grant",
                    "surface": "cli",
                    "after": {
                        "approved": ["uses_network"],
                        "version_at_grant": "1.0",
                        "granted_at": "2026-09-16T00:00:00Z"
                    }
                }]
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        assert_eq!(
            store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn missing_decision_after_key_is_rejected_without_mutation() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": "yach.extension-authority.v1",
                "extension_id": "example.net",
                "current": null,
                "legacy_baseline": null,
                "history": [{
                    "operation_id": "550e8400-e29b-41d4-a716-446655440000",
                    "recorded_at": "2026-09-16T00:00:00Z",
                    "action": "revoke",
                    "reason": "extension_capability_revoke",
                    "surface": "cli",
                    "before": null
                }]
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn equivalent_uuid_spellings_are_not_unique_operations() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{
                "schema": "yach.extension-authority.v1",
                "extension_id": "example.net",
                "current": {
                    "approved": ["uses_network"],
                    "version_at_grant": "2.0",
                    "granted_at": "2026-09-16T00:00:00Z"
                },
                "legacy_baseline": null,
                "history": [
                    {
                        "operation_id": "550e8400-e29b-41d4-a716-446655440000",
                        "recorded_at": "2026-09-16T00:00:00Z",
                        "action": "grant",
                        "reason": "extension_capability_grant",
                        "surface": "cli",
                        "before": null,
                        "after": {
                            "approved": ["uses_network"],
                            "version_at_grant": "1.0",
                            "granted_at": "2026-09-16T00:00:00Z"
                        }
                    },
                    {
                        "operation_id": "550e8400e29b41d4a716446655440000",
                        "recorded_at": "2026-09-16T00:00:01Z",
                        "action": "grant",
                        "reason": "extension_capability_grant",
                        "surface": "cli",
                        "before": {
                            "approved": ["uses_network"],
                            "version_at_grant": "1.0",
                            "granted_at": "2026-09-16T00:00:00Z"
                        },
                        "after": {
                            "approved": ["uses_network"],
                            "version_at_grant": "2.0",
                            "granted_at": "2026-09-16T00:00:00Z"
                        }
                    }
                ]
            }"#,
        );
        let before = fs::read(home.document_path("example.net"));
        assert!(before.is_ok(), "planted bytes should read: {before:?}");
        let Ok(before) = before else {
            return;
        };
        let store = home.store();
        assert_eq!(
            store.load_grant("example.net"),
            Err(ExtensionAuthorityError::InvalidDocument)
        );
        let after = fs::read(home.document_path("example.net"));
        assert_eq!(after.ok().as_deref(), Some(before.as_slice()));
    }

    #[test]
    fn legacy_grant_load_is_read_only() {
        let home = TestHome::new();
        let legacy = br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#;
        home.write_raw("example.net", legacy);
        let store = home.store();
        let loaded = store.load_grant("example.net");
        assert!(matches!(loaded, Ok(Some(_))));
        let Ok(Some(grant)) = loaded else {
            return;
        };
        assert_eq!(grant.version_at_grant, "1.0");
        assert_eq!(grant.granted_at, "2026-09-16T00:00:00Z");
        let leftover = fs::read(home.document_path("example.net"));
        assert_eq!(leftover.ok().as_deref(), Some(legacy.as_slice()));
        assert!(
            !home
                .document_path("example.net")
                .with_extension("json.lock")
                .exists()
        );
    }

    #[test]
    fn legacy_grant_mutation_imports_baseline_and_appends_one_decision() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#,
        );
        let store = home.store();
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        let granted =
            store.grant_requested("example.net", "2.0", &tools, ExtensionDecisionSurface::Cli);
        assert!(matches!(granted, Ok(Some(_))));
        let doc = home.document("example.net");
        assert_eq!(doc["schema"], "yach.extension-authority.v1");
        assert_eq!(doc["legacy_baseline"]["granted_at"], "2026-09-16T00:00:00Z");
        assert_eq!(doc["legacy_baseline"]["version_at_grant"], "1.0");
        assert_eq!(doc["history"][0]["before"], doc["legacy_baseline"]);
        assert_eq!(doc["history"].as_array().map(Vec::len), Some(1));
        assert_eq!(doc["current"]["version_at_grant"], "2.0");
        assert_eq!(doc["current"], doc["history"][0]["after"]);
    }

    #[test]
    fn legacy_revoke_mutation_imports_baseline_and_appends_one_decision() {
        let home = TestHome::new();
        home.write_raw(
            "example.net",
            br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#,
        );
        let store = home.store();
        assert_eq!(
            store.revoke_grant("example.net", ExtensionDecisionSurface::Cli),
            Ok(true)
        );
        let doc = home.document("example.net");
        assert_eq!(doc["legacy_baseline"]["granted_at"], "2026-09-16T00:00:00Z");
        assert_eq!(doc["history"].as_array().map(Vec::len), Some(1));
        assert_eq!(doc["history"][0]["action"], "revoke");
        assert_eq!(doc["history"][0]["before"], doc["legacy_baseline"]);
        assert!(doc["history"][0]["after"].is_null());
        assert!(doc["current"].is_null());
    }

    #[test]
    fn before_rename_failure_removes_temporary_and_leaves_original_bytes() {
        let home = TestHome::new();
        let path = home.document_path("example.net");
        let Some(parent) = path.parent() else {
            return;
        };
        assert!(fs::create_dir_all(parent).is_ok());
        privatize_dir(&home.path().join(".yach"));
        privatize_dir(parent);
        let original = br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#;
        assert!(fs::write(&path, original).is_ok());
        privatize_file(&path);
        let result = commit_document(&path, b"{\"schema\":\"new\"}", |phase| {
            if phase == CommitPhase::BeforeRename {
                Err(io::Error::other("injected before rename"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(ExtensionAuthorityError::Io));
        let leftover = fs::read(&path);
        assert_eq!(leftover.ok().as_deref(), Some(original.as_slice()));
        let entries = fs::read_dir(parent);
        assert!(entries.is_ok(), "grants directory should remain readable");
        let Ok(entries) = entries else {
            return;
        };
        let stray_tmp = entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!stray_tmp, "failed commit must remove the temporary file");
    }

    #[test]
    fn after_rename_failure_reports_durability_unknown_with_new_document() {
        let home = TestHome::new();
        let path = home.document_path("example.net");
        let Some(parent) = path.parent() else {
            return;
        };
        assert!(fs::create_dir_all(parent).is_ok());
        privatize_dir(&home.path().join(".yach"));
        privatize_dir(parent);
        let original = br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#;
        assert!(fs::write(&path, original).is_ok());
        privatize_file(&path);
        let replacement = br#"{"schema":"yach.extension-authority.v1","extension_id":"example.net","current":null,"legacy_baseline":null,"history":[]}"#;
        let result = commit_document(&path, replacement, |phase| {
            if phase == CommitPhase::AfterRename {
                Err(io::Error::other("injected after rename"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(ExtensionAuthorityError::DurabilityUnknown));
        let leftover = fs::read(&path);
        assert_eq!(leftover.ok().as_deref(), Some(replacement.as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn commit_document_reports_io_when_the_parent_is_not_writable() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = TestHome::new();
        let path = home.document_path("example.net");
        let Some(parent) = path.parent() else {
            return;
        };
        assert!(fs::create_dir_all(parent).is_ok());
        privatize_dir(&home.path().join(".yach"));
        privatize_dir(parent);
        assert!(fs::set_permissions(parent, fs::Permissions::from_mode(0o500)).is_ok());
        let result = commit_document(&path, b"{}", |_| Ok(()));
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        assert_eq!(result, Err(ExtensionAuthorityError::Io));
    }

    #[cfg(unix)]
    #[test]
    fn commit_document_reports_io_when_rename_target_is_a_directory() {
        let home = TestHome::new();
        let path = home.document_path("example.net");
        let Some(parent) = path.parent() else {
            return;
        };
        assert!(fs::create_dir_all(parent).is_ok());
        privatize_dir(&home.path().join(".yach"));
        privatize_dir(parent);
        let original = br#"{"approved":["uses_network"],"version_at_grant":"1.0","granted_at":"2026-09-16T00:00:00Z"}"#;
        assert!(fs::write(&path, original).is_ok());
        privatize_file(&path);
        let result = commit_document(&path, b"replacement-document", |phase| {
            if phase == CommitPhase::BeforeRename {
                let _ = fs::remove_file(&path);
                assert!(fs::create_dir(&path).is_ok());
                privatize_dir(&path);
                Ok(())
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err(ExtensionAuthorityError::Io));
        assert!(path.is_dir(), "rename must fail against a directory target");
        let entries = fs::read_dir(parent);
        assert!(entries.is_ok(), "grants directory should remain readable");
        let Ok(entries) = entries else {
            return;
        };
        let stray_tmp = entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!stray_tmp, "failed rename must remove the temporary file");
    }

    #[cfg(unix)]
    #[test]
    fn commit_document_write_limit_helper() {
        let Ok(path) = std::env::var(COMMIT_WRITE_PATH_ENV) else {
            return;
        };
        // SAFETY: helper is an isolated child; ignore SIGXFSZ so a size-limit
        // write returns EFBIG instead of terminating the process.
        unsafe {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        }
        let limit = libc::rlimit {
            rlim_cur: 1,
            rlim_max: 1,
        };
        // SAFETY: the file-size limit applies only to this helper process.
        assert_eq!(
            unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &raw const limit) },
            0
        );
        let path = PathBuf::from(path);
        let payload = vec![b'x'; 4096];
        let result = commit_document(&path, &payload, |_| Ok(()));
        assert_eq!(result, Err(ExtensionAuthorityError::Io));
        let Some(parent) = path.parent() else {
            return;
        };
        let entries = fs::read_dir(parent);
        assert!(entries.is_ok(), "grants directory should remain readable");
        let Ok(entries) = entries else {
            return;
        };
        let stray_tmp = entries
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp"));
        assert!(!stray_tmp, "failed write must remove the temporary file");
        assert!(!path.is_file(), "failed write must leave no new document");
    }

    #[cfg(unix)]
    #[test]
    fn commit_document_reports_io_when_write_hits_a_file_size_limit() {
        if std::env::var(COMMIT_WRITE_PATH_ENV).is_ok() {
            return;
        }
        let home = TestHome::new();
        let path = home.document_path("example.net");
        let Some(parent) = path.parent() else {
            return;
        };
        assert!(fs::create_dir_all(parent).is_ok());
        privatize_dir(&home.path().join(".yach"));
        privatize_dir(parent);
        let executable = std::env::current_exe();
        assert!(
            executable.is_ok(),
            "test executable should resolve: {executable:?}"
        );
        let Ok(executable) = executable else {
            return;
        };
        let output = Command::new(&executable)
            .arg("--exact")
            .arg(COMMIT_WRITE_HELPER)
            .arg("--nocapture")
            .env(COMMIT_WRITE_PATH_ENV, &path)
            .output();
        assert!(
            output.is_ok(),
            "write-limit helper should spawn: {output:?}"
        );
        let Ok(output) = output else {
            return;
        };
        assert!(
            output.status.success(),
            "write-limit helper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn concurrent_authority_writer_helper() {
        let Ok(home) = std::env::var(CONCURRENCY_HOME_ENV) else {
            return;
        };
        let iters = std::env::var(CONCURRENCY_ITERS_ENV)
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(CONCURRENCY_ITERS);
        let store = ExtensionAuthorityStore::in_home(Path::new(&home));
        let tools = vec![tool("fetch", ExtensionToolRisk::UsesNetwork)];
        for _ in 0..iters {
            let granted =
                store.grant_requested("example.net", "1.0", &tools, ExtensionDecisionSurface::Cli);
            assert!(granted.is_ok(), "helper grant should commit: {granted:?}");
            let revoked = store.revoke_grant("example.net", ExtensionDecisionSurface::Cli);
            assert!(revoked.is_ok(), "helper revoke should commit: {revoked:?}");
        }
    }

    #[test]
    fn concurrent_writers_preserve_ordered_unique_decisions() {
        if std::env::var(CONCURRENCY_HOME_ENV).is_ok() {
            return;
        }
        let home = TestHome::new();
        let executable = std::env::current_exe();
        assert!(
            executable.is_ok(),
            "test executable should resolve: {executable:?}"
        );
        let Ok(executable) = executable else {
            return;
        };
        let spawn = |tag: &str| {
            Command::new(&executable)
                .arg("--exact")
                .arg(CONCURRENCY_HELPER)
                .arg("--nocapture")
                .env(CONCURRENCY_HOME_ENV, home.path())
                .env(CONCURRENCY_ITERS_ENV, CONCURRENCY_ITERS.to_string())
                .env("YACH_EXTENSION_AUTHORITY_TEST_TAG", tag)
                .spawn()
        };
        let first = spawn("a");
        let second = spawn("b");
        assert!(first.is_ok(), "first writer should spawn: {first:?}");
        assert!(second.is_ok(), "second writer should spawn: {second:?}");
        let Ok(first) = first else {
            return;
        };
        let Ok(second) = second else {
            return;
        };
        let first = first.wait_with_output();
        let second = second.wait_with_output();
        assert!(first.is_ok(), "first writer should exit: {first:?}");
        assert!(second.is_ok(), "second writer should exit: {second:?}");
        let Ok(first) = first else {
            return;
        };
        let Ok(second) = second else {
            return;
        };
        assert!(
            first.status.success(),
            "first writer failed: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert!(
            second.status.success(),
            "second writer failed: {}",
            String::from_utf8_lossy(&second.stderr)
        );
        let doc = home.document("example.net");
        let history = doc["history"].as_array();
        assert!(history.is_some(), "committed history should be an array");
        let Some(history) = history else {
            return;
        };
        assert_eq!(history.len(), CONCURRENCY_ITERS * 4);
        let mut ids = Vec::new();
        let mut previous = &serde_json::Value::Null;
        for entry in history {
            let id = entry["operation_id"].as_str();
            assert!(id.is_some(), "each decision needs an operation id");
            if let Some(id) = id {
                ids.push(id.to_owned());
            }
            assert_eq!(&entry["before"], previous);
            previous = &entry["after"];
        }
        ids.sort();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "operation ids must be unique");
        assert_eq!(doc["current"], *previous);
    }
}
