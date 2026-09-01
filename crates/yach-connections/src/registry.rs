use std::{
    error::Error,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ConnectionId, ConnectionKey, ConnectionState, ProviderConnection, ProviderKind,
    ValidationError, normalize_label,
};

const LEGACY_REGISTRY_SCHEMA: &str = "yach.connections.v1";
const REGISTRY_SCHEMA: &str = "yach.connections.v2";
const MAX_CONNECTIONS: usize = 64;
const MAX_RETIRED_KEYS: usize = 1_024;

trait DirectorySync: Send + Sync + fmt::Debug {
    fn sync_parent(&self, parent: &Path) -> io::Result<()>;
}

#[derive(Debug)]
struct PlatformDirectorySync;

impl DirectorySync for PlatformDirectorySync {
    fn sync_parent(&self, parent: &Path) -> io::Result<()> {
        sync_parent_directory(parent)
    }
}

#[cfg(test)]
#[derive(Debug)]
struct FailingDirectorySync {
    error_kind: io::ErrorKind,
}

#[cfg(test)]
impl DirectorySync for FailingDirectorySync {
    fn sync_parent(&self, _parent: &Path) -> io::Result<()> {
        Err(io::Error::from(self.error_kind))
    }
}

/// Bounded errors emitted while loading or updating connection metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// The registry JSON cannot be trusted as a complete connection registry.
    Malformed,
    /// A metadata record violates persisted-connection validation.
    InvalidConnection,
    /// The registry exceeds its fixed connection capacity.
    CapacityExceeded,
    /// A filesystem operation failed without exposing its platform details.
    Io,
    /// The new registry file was renamed, but parent-directory durability could not be confirmed.
    DurabilityUnknown,
    /// The registry or one of its lock files is not a regular file.
    UnsafePath,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Malformed => "Connection registry is malformed.",
            Self::InvalidConnection => "Connection metadata is invalid.",
            Self::CapacityExceeded => "At most 64 provider connections can be stored.",
            Self::Io => "Connection metadata storage is unavailable.",
            Self::DurabilityUnknown => {
                "Connection metadata was updated but storage durability could not be confirmed."
            }
            Self::UnsafePath => "Connection metadata storage path is unsafe.",
        };
        formatter.write_str(message)
    }
}

impl Error for RegistryError {}

/// Metadata storage whose mutations are scoped by stable connection identity.
pub trait ConnectionMetadataStore: Send + Sync {
    /// Loads all persisted connections in deterministic stable-ID order.
    fn load(&self) -> Result<Vec<ProviderConnection>, RegistryError>;

    /// Acquires the per-connection lock required for a compound mutation.
    fn lock_connection(
        &self,
        id: &ConnectionId,
    ) -> Result<Box<dyn LockedConnectionMetadata>, RegistryError>;
}

/// A per-connection mutation scope.
pub trait LockedConnectionMetadata: Send {
    /// Reloads metadata after the per-connection lock has been acquired.
    fn load(&mut self) -> Result<Vec<ProviderConnection>, RegistryError>;

    /// Inserts one pending persisted connection.
    fn create_pending(&mut self, connection: ProviderConnection) -> Result<(), RegistryError>;

    /// Marks the locked pending connection ready.
    fn mark_ready(&mut self, id: &ConnectionId) -> Result<(), RegistryError>;

    /// Changes the presentation label of the locked connection.
    fn rename(&mut self, id: &ConnectionId, label: Option<String>) -> Result<(), RegistryError>;

    /// Assigns the immutable configuration key of the locked connection.
    fn assign_key(&mut self, id: &ConnectionId, key: ConnectionKey) -> Result<(), RegistryError>;

    /// Removes the locked connection from metadata.
    fn remove(&mut self, id: &ConnectionId) -> Result<(), RegistryError>;

    /// Inserts or replaces a ready connection for the locked id.
    fn upsert_ready(&mut self, connection: ProviderConnection) -> Result<(), RegistryError>;
}

/// Crash-safe JSON metadata at an injected registry path.
#[derive(Debug, Clone)]
pub struct JsonConnectionMetadataStore {
    path: PathBuf,
    directory_sync: Arc<dyn DirectorySync>,
}

impl JsonConnectionMetadataStore {
    /// Creates a registry at `path`; no filesystem access occurs until the first operation.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            directory_sync: Arc::new(PlatformDirectorySync),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_directory_sync_error(path: PathBuf, error_kind: io::ErrorKind) -> Self {
        Self {
            path,
            directory_sync: Arc::new(FailingDirectorySync { error_kind }),
        }
    }

    fn resolved_registry_path(
        &self,
        create_parent: bool,
    ) -> Result<Option<PathBuf>, RegistryError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.exists() {
            if !create_parent {
                return Ok(None);
            }
            fs::create_dir_all(parent).map_err(|_| RegistryError::Io)?;
        }
        let parent = fs::canonicalize(parent).map_err(|_| RegistryError::Io)?;
        let name = self.path.file_name().ok_or(RegistryError::UnsafePath)?;
        Ok(Some(parent.join(name)))
    }

    fn load_state(path: &Path) -> Result<RegistryState, RegistryError> {
        ensure_regular_or_missing(path)?;
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RegistryState::default());
            }
            Err(_) => return Err(RegistryError::Io),
        };
        let document = serde_json::from_slice::<RegistryDocument>(&contents)
            .map_err(|_| RegistryError::Malformed)?;
        if document.schema != REGISTRY_SCHEMA && document.schema != LEGACY_REGISTRY_SCHEMA {
            return Err(RegistryError::Malformed);
        }
        let mut state = RegistryState {
            connections: document.connections,
            retired_keys: if document.schema == LEGACY_REGISTRY_SCHEMA {
                Vec::new()
            } else {
                document.retired_keys
            },
        };
        validate_registry(&state)?;
        state
            .connections
            .sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        state.retired_keys.sort();
        Ok(state)
    }

    fn load_path(path: &Path) -> Result<Vec<ProviderConnection>, RegistryError> {
        Self::load_state(path).map(|state| state.connections)
    }

    fn mutation_lock_path(registry_path: &Path, suffix: &str) -> Result<PathBuf, RegistryError> {
        let name = registry_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(RegistryError::UnsafePath)?;
        Ok(registry_path.with_file_name(format!("{name}.{suffix}.lock")))
    }

    fn lock_file(path: &Path) -> Result<File, RegistryError> {
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                ensure_regular_or_missing(path)?;
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .map_err(|_| RegistryError::Io)?
            }
            Err(_) => return Err(RegistryError::Io),
        };
        file.lock_exclusive().map_err(|_| RegistryError::Io)?;
        Ok(file)
    }

    fn mutate(
        registry_path: &Path,
        directory_sync: &dyn DirectorySync,
        mutation: impl FnOnce(&mut RegistryState) -> Result<(), RegistryError>,
    ) -> Result<(), RegistryError> {
        ensure_regular_or_missing(registry_path)?;
        let global_lock_path = Self::mutation_lock_path(registry_path, "global")?;
        let _global_lock = Self::lock_file(&global_lock_path)?;
        let mut state = Self::load_state(registry_path)?;
        mutation(&mut state)?;
        validate_registry(&state)?;
        state
            .connections
            .sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        state.retired_keys.sort();
        write_registry(registry_path, &state, directory_sync)
    }
}

impl ConnectionMetadataStore for JsonConnectionMetadataStore {
    fn load(&self) -> Result<Vec<ProviderConnection>, RegistryError> {
        match self.resolved_registry_path(false)? {
            Some(path) => Self::load_path(&path),
            None => Ok(Vec::new()),
        }
    }

    fn lock_connection(
        &self,
        id: &ConnectionId,
    ) -> Result<Box<dyn LockedConnectionMetadata>, RegistryError> {
        id.validate_stored()
            .map_err(|_| RegistryError::InvalidConnection)?;
        let registry_path = self
            .resolved_registry_path(true)?
            .ok_or(RegistryError::Io)?;
        ensure_regular_or_missing(&registry_path)?;
        let lock_path = Self::mutation_lock_path(&registry_path, id.as_str())?;
        let lock = Self::lock_file(&lock_path)?;
        Ok(Box::new(JsonLockedConnectionMetadata {
            registry_path,
            locked_id: id.clone(),
            _connection_lock: lock,
            directory_sync: Arc::clone(&self.directory_sync),
        }))
    }
}

struct JsonLockedConnectionMetadata {
    registry_path: PathBuf,
    locked_id: ConnectionId,
    _connection_lock: File,
    directory_sync: Arc<dyn DirectorySync>,
}

impl JsonLockedConnectionMetadata {
    fn mutate(
        &self,
        mutation: impl FnOnce(&mut RegistryState) -> Result<(), RegistryError>,
    ) -> Result<(), RegistryError> {
        JsonConnectionMetadataStore::mutate(
            &self.registry_path,
            self.directory_sync.as_ref(),
            mutation,
        )
    }
}

impl LockedConnectionMetadata for JsonLockedConnectionMetadata {
    fn load(&mut self) -> Result<Vec<ProviderConnection>, RegistryError> {
        JsonConnectionMetadataStore::load_path(&self.registry_path)
    }

    fn create_pending(&mut self, connection: ProviderConnection) -> Result<(), RegistryError> {
        if connection.id != self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        if connection.state != ConnectionState::PendingCredential {
            return Err(RegistryError::InvalidConnection);
        }
        connection
            .validate_persisted()
            .map_err(|_| RegistryError::InvalidConnection)?;
        let id = connection.id.clone();
        self.mutate(move |state| {
            if state.connections.iter().any(|existing| existing.id == id) {
                return Err(RegistryError::InvalidConnection);
            }
            if state.connections.len() == MAX_CONNECTIONS {
                return Err(RegistryError::CapacityExceeded);
            }
            state.connections.push(connection);
            Ok(())
        })
    }

    fn mark_ready(&mut self, id: &ConnectionId) -> Result<(), RegistryError> {
        if id != &self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        self.mutate(|state| {
            let index = state
                .connections
                .iter()
                .position(|connection| connection.id == *id)
                .ok_or(RegistryError::InvalidConnection)?;
            if state.connections[index].state != ConnectionState::PendingCredential {
                return Err(RegistryError::InvalidConnection);
            }
            let provider = state.connections[index].provider;
            let key = state.connections[index].key.as_ref();
            if state
                .connections
                .iter()
                .enumerate()
                .any(|(other_index, connection)| {
                    other_index != index
                        && connection.provider == provider
                        && connection.state == ConnectionState::Ready
                        && (key.is_none() || connection.key.is_none())
                })
            {
                return Err(RegistryError::InvalidConnection);
            }
            state.connections[index].state = ConnectionState::Ready;
            Ok(())
        })
    }

    fn rename(&mut self, id: &ConnectionId, label: Option<String>) -> Result<(), RegistryError> {
        if id != &self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        let label = normalize_label(label).map_err(|_| RegistryError::InvalidConnection)?;
        self.mutate(|state| {
            let connection = state
                .connections
                .iter_mut()
                .find(|connection| connection.id == *id)
                .ok_or(RegistryError::InvalidConnection)?;
            connection.label = label;
            Ok(())
        })
    }

    fn assign_key(&mut self, id: &ConnectionId, key: ConnectionKey) -> Result<(), RegistryError> {
        if id != &self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        self.mutate(move |state| {
            let index = state
                .connections
                .iter()
                .position(|connection| connection.id == *id)
                .ok_or(RegistryError::InvalidConnection)?;
            if state.connections[index].key.is_some()
                || key_in_use(
                    state.connections[index].provider,
                    &key,
                    &state.connections,
                    &state.retired_keys,
                )
            {
                return Err(RegistryError::InvalidConnection);
            }
            state.connections[index].key = Some(key);
            Ok(())
        })
    }

    fn remove(&mut self, id: &ConnectionId) -> Result<(), RegistryError> {
        if id != &self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        self.mutate(|state| {
            let index = state
                .connections
                .iter()
                .position(|connection| connection.id == *id)
                .ok_or(RegistryError::InvalidConnection)?;
            let removed = state.connections.remove(index);
            if let Some(key) = removed.key {
                if state.retired_keys.len() == MAX_RETIRED_KEYS {
                    return Err(RegistryError::CapacityExceeded);
                }
                state.retired_keys.push(RetiredConnectionKey {
                    provider: removed.provider,
                    key,
                });
            }
            Ok(())
        })
    }

    fn upsert_ready(&mut self, connection: ProviderConnection) -> Result<(), RegistryError> {
        if connection.id != self.locked_id {
            return Err(RegistryError::InvalidConnection);
        }
        if connection.state != ConnectionState::Ready {
            return Err(RegistryError::InvalidConnection);
        }
        connection
            .validate_persisted()
            .map_err(|_| RegistryError::InvalidConnection)?;
        let id = connection.id.clone();
        self.mutate(move |state| {
            if connection.provider == crate::ProviderKind::ChatGptSubscription
                && state.connections.iter().any(|existing| {
                    existing.provider == crate::ProviderKind::ChatGptSubscription
                        && existing.id != id
                })
            {
                return Err(RegistryError::InvalidConnection);
            }
            if let Some(existing) = state
                .connections
                .iter_mut()
                .find(|existing| existing.id == id)
            {
                if existing.key != connection.key {
                    return Err(RegistryError::InvalidConnection);
                }
                *existing = connection;
                return Ok(());
            }
            if state.connections.len() == MAX_CONNECTIONS {
                return Err(RegistryError::CapacityExceeded);
            }
            state.connections.push(connection);
            Ok(())
        })
    }
}

#[derive(Debug, Default)]
struct RegistryState {
    connections: Vec<ProviderConnection>,
    retired_keys: Vec<RetiredConnectionKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
struct RetiredConnectionKey {
    provider: ProviderKind,
    key: ConnectionKey,
}

#[derive(Debug, Deserialize, Serialize)]
struct RegistryDocument {
    schema: String,
    connections: Vec<ProviderConnection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    retired_keys: Vec<RetiredConnectionKey>,
}

fn validate_registry(state: &RegistryState) -> Result<(), RegistryError> {
    if state.connections.len() > MAX_CONNECTIONS || state.retired_keys.len() > MAX_RETIRED_KEYS {
        return Err(RegistryError::CapacityExceeded);
    }
    for connection in &state.connections {
        connection
            .validate_persisted()
            .map_err(|_| RegistryError::InvalidConnection)?;
    }
    let mut ids = state
        .connections
        .iter()
        .map(|connection| connection.id.as_str())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    if ids.windows(2).any(|ids| ids[0] == ids[1]) {
        return Err(RegistryError::InvalidConnection);
    }
    let subscription_rows = state
        .connections
        .iter()
        .filter(|connection| connection.provider == crate::ProviderKind::ChatGptSubscription)
        .count();
    if subscription_rows > 1 {
        return Err(RegistryError::InvalidConnection);
    }
    let mut keys = state
        .connections
        .iter()
        .filter_map(|connection| {
            connection
                .key
                .as_ref()
                .map(|key| (connection.provider, key.as_str()))
        })
        .chain(
            state
                .retired_keys
                .iter()
                .map(|retired| (retired.provider, retired.key.as_str())),
        )
        .collect::<Vec<_>>();
    keys.sort_unstable();
    if keys.windows(2).any(|keys| keys[0] == keys[1]) {
        return Err(RegistryError::InvalidConnection);
    }
    Ok(())
}

fn key_in_use(
    provider: ProviderKind,
    key: &ConnectionKey,
    connections: &[ProviderConnection],
    retired_keys: &[RetiredConnectionKey],
) -> bool {
    connections
        .iter()
        .any(|connection| connection.provider == provider && connection.key.as_ref() == Some(key))
        || retired_keys
            .iter()
            .any(|retired| retired.provider == provider && retired.key == *key)
}

fn ensure_regular_or_missing(path: &Path) -> Result<(), RegistryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(RegistryError::UnsafePath),
        Ok(metadata) if !metadata.file_type().is_file() => Err(RegistryError::UnsafePath),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(RegistryError::Io),
    }
}

fn write_registry(
    registry_path: &Path,
    state: &RegistryState,
    directory_sync: &dyn DirectorySync,
) -> Result<(), RegistryError> {
    ensure_regular_or_missing(registry_path)?;
    let parent = registry_path.parent().ok_or(RegistryError::UnsafePath)?;
    let name = registry_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RegistryError::UnsafePath)?;
    let temporary_path = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec(&RegistryDocument {
        schema: REGISTRY_SCHEMA.to_owned(),
        connections: state.connections.clone(),
        retired_keys: state.retired_keys.clone(),
    })
    .map_err(|_| RegistryError::Io)?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut temporary = options
        .open(&temporary_path)
        .map_err(|_| RegistryError::Io)?;
    let write_result = temporary
        .write_all(&bytes)
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.sync_all());
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        return Err(RegistryError::Io);
    }
    drop(temporary);

    if fs::rename(&temporary_path, registry_path).is_err() {
        let _ = fs::remove_file(&temporary_path);
        return Err(RegistryError::Io);
    }
    match directory_sync.sync_parent(parent) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::Unsupported => Ok(()),
        Err(_) => Err(RegistryError::DurabilityUnknown),
    }
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent).and_then(|directory| directory.sync_all())
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

impl From<ValidationError> for RegistryError {
    fn from(_: ValidationError) -> Self {
        Self::InvalidConnection
    }
}
