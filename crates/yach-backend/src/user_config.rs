use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::Deserialize;
use toml_edit::{DocumentMut, Item, Table, value};
use yach_connections::ConnectionKey;
use yach_proto::ThinkingLevel;

use crate::components::{Component, ComponentSet, Preset};

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
    pub preset_applied: Option<Preset>,
    pub component_overrides: BTreeMap<Component, bool>,
    pub bundled_removed: BTreeSet<String>,
    pub unknown_components: Vec<String>,
}

impl UserConfigSnapshot {
    #[must_use]
    pub fn kernel_components(&self) -> ComponentSet {
        self.component_overrides
            .iter()
            .fold(ComponentSet::full(), |set, (component, enabled)| {
                set.with(*component, *enabled)
            })
    }
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

    pub fn persist_component(
        &self,
        component: Component,
        enabled: bool,
    ) -> Result<(), UserConfigError> {
        if !component.is_kernel() {
            return Err(UserConfigError::Invalid);
        }
        self.update(|document| {
            let components = table_mut(document.as_table_mut(), "components")?;
            components[component.name()] = value(enabled);
            Ok(())
        })
    }

    /// Writes the preset marker and the kernel component set. When
    /// `preserve_existing` is true (the implicit first-run apply only), a
    /// `[components]` key that already exists keeps its value — spec rule 3:
    /// only components with no prior record take the preset's default.
    /// `--reset` is always explicit and never preserves.
    pub fn persist_preset(
        &self,
        preset: Preset,
        reset: bool,
        preserve_existing: bool,
    ) -> Result<(), UserConfigError> {
        let selected = ComponentSet::from_preset(preset);
        self.update(|document| {
            table_mut(document.as_table_mut(), "preset")?["applied"] = value(preset.name());
            let components = table_mut(document.as_table_mut(), "components")?;
            for component in Component::ALL.into_iter().filter(|c| c.is_kernel()) {
                if preserve_existing && components.get(component.name()).is_some() {
                    continue;
                }
                components[component.name()] = value(selected.contains(component));
            }
            if reset {
                table_mut(document.as_table_mut(), "bundled")?.remove("removed");
            }
            Ok(())
        })
    }

    pub fn persist_bundled_removed(&self, id: &str, removed: bool) -> Result<(), UserConfigError> {
        self.update(|document| {
            let bundled = table_mut(document.as_table_mut(), "bundled")?;
            let mut ids: BTreeSet<String> = bundled
                .get("removed")
                .and_then(Item::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if removed {
                ids.insert(id.to_owned());
            } else {
                ids.remove(id);
            }
            let mut array = toml_edit::Array::new();
            for id in ids {
                array.push(id);
            }
            bundled["removed"] = value(array);
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

    let preset_applied = match document.get("preset") {
        None => None,
        Some(item) => {
            let table = item.as_table().ok_or(UserConfigError::Invalid)?;
            match table.get("applied") {
                None => None,
                Some(value) => {
                    let raw = value.as_str().ok_or(UserConfigError::Invalid)?;
                    Some(Preset::parse(raw).ok_or(UserConfigError::Invalid)?)
                }
            }
        }
    };

    let mut component_overrides = BTreeMap::new();
    let mut unknown_components = Vec::new();
    if let Some(item) = document.get("components") {
        let table = item.as_table().ok_or(UserConfigError::Invalid)?;
        for (key, value) in table {
            match Component::parse(key).filter(|component| component.is_kernel()) {
                Some(component) => {
                    let enabled = value.as_bool().ok_or(UserConfigError::Invalid)?;
                    component_overrides.insert(component, enabled);
                }
                None => unknown_components.push(key.to_owned()),
            }
        }
    }

    let mut bundled_removed = BTreeSet::new();
    if let Some(item) = document.get("bundled") {
        let table = item.as_table().ok_or(UserConfigError::Invalid)?;
        if let Some(removed) = table.get("removed") {
            let array = removed.as_array().ok_or(UserConfigError::Invalid)?;
            for value in array {
                let id = value.as_str().ok_or(UserConfigError::Invalid)?;
                bundled_removed.insert(id.to_owned());
            }
        }
    }

    Ok(UserConfigSnapshot {
        thinking_default,
        model_default,
        preset_applied,
        component_overrides,
        bundled_removed,
        unknown_components,
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
    fn set_private(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(fs::set_permissions(path, fs::Permissions::from_mode(0o600)).is_ok());
        }
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

    #[test]
    fn preset_tables_parse_and_unknown_components_are_diagnostics() {
        let (_directory, store) = temp_store("preset-parse");
        assert!(
            fs::write(
                store.path(),
                "[preset]\napplied = \"minimal\"\n\n[components]\nproject-tools = false\nlaser = true\n\n[bundled]\nremoved = [\"yach.hashline\"]\n",
            )
            .is_ok()
        );
        set_private(store.path());
        let snapshot = store.load();
        assert!(snapshot.is_ok(), "{snapshot:?}");
        let Ok(snapshot) = snapshot else { return };
        assert_eq!(snapshot.preset_applied, Some(Preset::Minimal));
        assert!(!snapshot.kernel_components().project_tools());
        assert!(snapshot.kernel_components().baseline_guidance());
        assert!(snapshot.bundled_removed.contains("yach.hashline"));
        assert_eq!(snapshot.unknown_components, vec![String::from("laser")]);
    }

    #[test]
    fn persist_preset_writes_components_preserving_unrelated_content_and_removed() {
        let (_directory, store) = temp_store("preset-persist");
        assert!(
            fs::write(
                store.path(),
                "# keep me\n[thinking]\ndefault = \"low\"\n\n[bundled]\nremoved = [\"yach.jev-reviewer\"]\n",
            )
            .is_ok()
        );
        set_private(store.path());
        assert!(store.persist_preset(Preset::Minimal, false, false).is_ok());
        let raw = fs::read_to_string(store.path()).unwrap_or_default();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("default = \"low\""));
        let Ok(snapshot) = store.load() else {
            unreachable!("valid config")
        };
        assert_eq!(snapshot.preset_applied, Some(Preset::Minimal));
        assert!(!snapshot.kernel_components().project_tools());
        assert!(snapshot.bundled_removed.contains("yach.jev-reviewer"));

        assert!(store.persist_preset(Preset::Full, true, false).is_ok());
        let Ok(snapshot) = store.load() else {
            unreachable!("valid config")
        };
        assert!(snapshot.kernel_components().project_tools());
        assert!(
            snapshot.bundled_removed.is_empty(),
            "--reset clears removals"
        );
    }

    #[test]
    fn persist_preset_first_run_preserves_existing_component_toggles() {
        let (_directory, store) = temp_store("preset-preserve");
        assert!(fs::write(store.path(), "[components]\nproject-tools = false\n").is_ok());
        set_private(store.path());
        assert!(store.persist_preset(Preset::Full, false, true).is_ok());
        let Ok(snapshot) = store.load() else {
            unreachable!("valid config")
        };
        assert_eq!(snapshot.preset_applied, Some(Preset::Full));
        assert!(
            !snapshot.kernel_components().project_tools(),
            "existing project-tools = false record must survive the first-run apply"
        );
        assert!(
            snapshot.kernel_components().baseline_guidance(),
            "components with no prior record take the preset default"
        );
    }

    #[test]
    fn malformed_component_value_is_invalid() {
        let (_directory, store) = temp_store("preset-malformed");
        assert!(fs::write(store.path(), "[components]\nproject-tools = \"yes\"\n").is_ok());
        set_private(store.path());
        assert_eq!(store.load(), Err(UserConfigError::Invalid));
    }
}
