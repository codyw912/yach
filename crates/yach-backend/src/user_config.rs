use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, value};
use yach_connections::ConnectionKey;
use yach_proto::ThinkingLevel;

const CONFIG_NAME: &str = "config.toml";
const LOCK_NAME: &str = "config.toml.lock";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserModelDefault {
    pub provider: String,
    pub model: String,
    pub connection: Option<ConnectionKey>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserConfigSnapshot {
    pub thinking_default: Option<ThinkingLevel>,
    pub model_default: Option<UserModelDefault>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserConfigError {
    HomeUnavailable,
    Invalid,
    UnsafePath,
    UnsafePermissions,
    Io,
    DurabilityUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyActiveModelTarget {
    pub connection_id: yach_connections::ConnectionId,
    pub model: String,
}

#[derive(Deserialize)]
struct LegacyActiveModelDocument {
    schema: String,
    connection_id: String,
    model_id: String,
}

impl fmt::Display for UserConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HomeUnavailable => "user home directory is unavailable",
            Self::Invalid => "user config is invalid",
            Self::UnsafePath => "user config path is unsafe",
            Self::UnsafePermissions => "user config permissions are unsafe",
            Self::Io => "user config storage is unavailable",
            Self::DurabilityUnknown => {
                "user config was updated but storage durability could not be confirmed"
            }
        })
    }
}

impl std::error::Error for UserConfigError {}

#[derive(Clone, Debug)]
pub struct UserConfigStore {
    path: PathBuf,
}

impl UserConfigStore {
    pub fn for_current_user() -> Result<Self, UserConfigError> {
        let home = home_dir().ok_or(UserConfigError::HomeUnavailable)?;
        Ok(Self::in_home(&home))
    }

    #[must_use]
    pub fn in_home(home: &Path) -> Self {
        Self {
            path: home.join(".yach").join(CONFIG_NAME),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<UserConfigSnapshot, UserConfigError> {
        let document = self.load_document()?;
        parse_snapshot(&document)
    }

    pub fn persist_thinking_default(
        &self,
        thinking_level: ThinkingLevel,
    ) -> Result<(), UserConfigError> {
        self.update(|document| {
            let thinking = table_mut(document.as_table_mut(), "thinking")?;
            thinking["default"] = value(thinking_level.as_str());
            Ok(())
        })
    }

    pub fn persist_model_default(&self, target: &UserModelDefault) -> Result<(), UserConfigError> {
        validate_model_default(target)?;
        self.update(|document| {
            let model = table_mut(document.as_table_mut(), "model")?;
            let default = table_mut(model, "default")?;
            default["provider"] = value(target.provider.as_str());
            default["model"] = value(target.model.as_str());
            match &target.connection {
                Some(connection) => default["connection"] = value(connection.as_str()),
                None => {
                    default.remove("connection");
                }
            }
            Ok(())
        })
    }

    #[must_use]
    pub fn legacy_active_model_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("active-model.json")
    }

    pub fn load_legacy_active_model(
        &self,
    ) -> Result<Option<LegacyActiveModelTarget>, UserConfigError> {
        let path = self.legacy_active_model_path();
        ensure_regular_private_or_missing(&path)?;
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(UserConfigError::Io),
        };
        let document = serde_json::from_str::<LegacyActiveModelDocument>(&raw)
            .map_err(|_| UserConfigError::Invalid)?;
        if document.schema != "yach.active-model.v1"
            || document.model_id.is_empty()
            || document.model_id.len() > 256
        {
            return Err(UserConfigError::Invalid);
        }
        let connection_id = if document.connection_id == "environment" {
            yach_connections::ConnectionId::environment()
        } else {
            yach_connections::ConnectionId::parse_stored(&document.connection_id)
                .map_err(|_| UserConfigError::Invalid)?
        };
        Ok(Some(LegacyActiveModelTarget {
            connection_id,
            model: document.model_id,
        }))
    }

    pub fn remove_legacy_active_model(&self) -> Result<(), UserConfigError> {
        let path = self.legacy_active_model_path();
        ensure_regular_private_or_missing(&path)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(UserConfigError::Io),
        }
    }

    fn load_document(&self) -> Result<DocumentMut, UserConfigError> {
        ensure_regular_private_or_missing(&self.path)?;
        match fs::read_to_string(&self.path) {
            Ok(raw) => raw.parse().map_err(|_| UserConfigError::Invalid),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(DocumentMut::new()),
            Err(_) => Err(UserConfigError::Io),
        }
    }

    fn update(
        &self,
        update: impl FnOnce(&mut DocumentMut) -> Result<(), UserConfigError>,
    ) -> Result<(), UserConfigError> {
        let parent = self.path.parent().ok_or(UserConfigError::UnsafePath)?;
        create_private_dir(parent)?;
        ensure_private_directory(parent)?;
        ensure_regular_private_or_missing(&self.path)?;

        let lock_path = parent.join(LOCK_NAME);
        ensure_regular_private_or_missing(&lock_path)?;
        let lock = open_private_lock(&lock_path)?;
        lock.lock_exclusive().map_err(|_| UserConfigError::Io)?;

        let mut document = self.load_document()?;
        // Refuse to modify a document containing malformed known fields.
        let _ = parse_snapshot(&document)?;
        update(&mut document)?;
        let rendered = document.to_string();
        write_document(&self.path, rendered.as_bytes())
    }
}

fn parse_snapshot(document: &DocumentMut) -> Result<UserConfigSnapshot, UserConfigError> {
    let thinking_default = match document.get("thinking") {
        None => None,
        Some(item) => {
            let table = item.as_table().ok_or(UserConfigError::Invalid)?;
            match table.get("default") {
                None => None,
                Some(value) => {
                    let raw = value.as_str().ok_or(UserConfigError::Invalid)?;
                    Some(ThinkingLevel::parse(raw).ok_or(UserConfigError::Invalid)?)
                }
            }
        }
    };

    let model_default = match document.get("model") {
        None => None,
        Some(item) => {
            let table = item.as_table().ok_or(UserConfigError::Invalid)?;
            match table.get("default") {
                None => None,
                Some(item) => {
                    let table = item.as_table().ok_or(UserConfigError::Invalid)?;
                    let provider = required_bounded_string(table, "provider")?;
                    let model = required_bounded_string(table, "model")?;
                    let connection = table
                        .get("connection")
                        .map(|item| {
                            item.as_str()
                                .ok_or(UserConfigError::Invalid)
                                .and_then(|raw| {
                                    ConnectionKey::parse(raw).map_err(|_| UserConfigError::Invalid)
                                })
                        })
                        .transpose()?;
                    let target = UserModelDefault {
                        provider,
                        model,
                        connection,
                    };
                    validate_model_default(&target)?;
                    Some(target)
                }
            }
        }
    };

    Ok(UserConfigSnapshot {
        thinking_default,
        model_default,
    })
}

fn required_bounded_string(table: &Table, key: &str) -> Result<String, UserConfigError> {
    let value = table
        .get(key)
        .and_then(Item::as_str)
        .ok_or(UserConfigError::Invalid)?;
    if value.is_empty() || value.len() > 256 {
        return Err(UserConfigError::Invalid);
    }
    Ok(value.to_owned())
}

fn validate_model_default(target: &UserModelDefault) -> Result<(), UserConfigError> {
    if target.provider.is_empty()
        || target.provider.len() > 256
        || target.model.is_empty()
        || target.model.len() > 256
    {
        return Err(UserConfigError::Invalid);
    }
    Ok(())
}

fn table_mut<'a>(table: &'a mut Table, key: &str) -> Result<&'a mut Table, UserConfigError> {
    if !table.contains_key(key) {
        table.insert(key, Item::Table(Table::new()));
    }
    table
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .ok_or(UserConfigError::Invalid)
}

fn write_document(path: &Path, bytes: &[u8]) -> Result<(), UserConfigError> {
    let parent = path.parent().ok_or(UserConfigError::UnsafePath)?;
    let temporary = parent.join(format!(".{CONFIG_NAME}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|_| UserConfigError::Io)?;
    if file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(&temporary);
        return Err(UserConfigError::Io);
    }
    drop(file);
    if fs::rename(&temporary, path).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(UserConfigError::Io);
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| UserConfigError::DurabilityUnknown)
}

fn open_private_lock(path: &Path) -> Result<File, UserConfigError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|_| UserConfigError::Io)?;
    ensure_regular_private(path, &file.metadata().map_err(|_| UserConfigError::Io)?)?;
    Ok(file)
}

fn ensure_regular_private_or_missing(path: &Path) -> Result<(), UserConfigError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure_regular_private(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(UserConfigError::Io),
    }
}

fn ensure_regular_private(_path: &Path, metadata: &fs::Metadata) -> Result<(), UserConfigError> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UserConfigError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(UserConfigError::UnsafePermissions);
        }
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), UserConfigError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| UserConfigError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UserConfigError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(UserConfigError::UnsafePermissions);
        }
    }
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<(), UserConfigError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path).map_err(|_| UserConfigError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(UserConfigError::UnsafePath);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(UserConfigError::UnsafePermissions);
            }
            if metadata.mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| UserConfigError::Io)?;
            }
        }
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| UserConfigError::Io)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (PathBuf, UserConfigStore) {
        let directory =
            std::env::temp_dir().join(format!("yach-user-config-{name}-{}", uuid::Uuid::new_v4()));
        assert!(fs::create_dir_all(&directory).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).is_ok());
        }
        let path = directory.join(CONFIG_NAME);
        (directory, UserConfigStore::at_path(path))
    }

    #[test]
    fn targeted_updates_preserve_unrelated_content() {
        let (directory, store) = temp_store("preserve");
        assert!(
            fs::write(
                store.path(),
                "# keep me\n[custom]\nvalue = 7\n\n[thinking]\ndefault = \"low\"\n",
            )
            .is_ok()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600)).is_ok());
        }

        assert!(
            store
                .persist_model_default(&UserModelDefault {
                    provider: "openai-codex".to_owned(),
                    model: "gpt-5.6-sol".to_owned(),
                    connection: None,
                })
                .is_ok()
        );

        let raw = fs::read_to_string(store.path());
        assert!(raw.is_ok());
        let Ok(raw) = raw else {
            return;
        };
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("[custom]"));
        assert!(raw.contains("value = 7"));
        assert_eq!(
            store.load().ok().and_then(|config| config.thinking_default),
            Some(ThinkingLevel::Low)
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn malformed_known_field_blocks_updates() {
        let (directory, store) = temp_store("malformed");

        assert!(fs::write(store.path(), "[thinking]\ndefault = 5\n").is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600)).is_ok());
        }
        assert_eq!(store.load(), Err(UserConfigError::Invalid));
        assert_eq!(
            store.persist_thinking_default(ThinkingLevel::High),
            Err(UserConfigError::Invalid)
        );
        assert_eq!(
            fs::read_to_string(store.path()).ok().as_deref(),
            Some("[thinking]\ndefault = 5\n")
        );
        let _ = fs::remove_dir_all(directory);
    }
    #[cfg(unix)]
    #[test]
    fn update_tightens_owned_existing_parent_directory() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory =
            std::env::temp_dir().join(format!("yach-user-config-mode-{}", uuid::Uuid::new_v4()));
        assert!(fs::create_dir_all(&directory).is_ok());
        assert!(fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).is_ok());
        let store = UserConfigStore::at_path(directory.join(CONFIG_NAME));

        assert!(store.persist_thinking_default(ThinkingLevel::High).is_ok());
        assert_eq!(
            fs::metadata(&directory)
                .ok()
                .map(|metadata| metadata.permissions().mode() & 0o777),
            Some(0o700)
        );
        let _ = fs::remove_dir_all(directory);
    }
}
