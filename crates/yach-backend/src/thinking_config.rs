use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use yach_proto::ThinkingLevel;

use crate::{UserConfigError, UserConfigStore};

const LEGACY_PREFERENCES_SCHEMA: &str = "yach.project-preferences.v1";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct LegacyYachConfig {
    thinking: LegacyThinkingConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct LegacyThinkingConfig {
    default: Option<ThinkingLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LegacyProjectPreferences {
    schema: String,
    thinking_level: ThinkingLevel,
}

#[must_use]
pub fn load_default_thinking_level() -> Option<ThinkingLevel> {
    let home = home_dir()?;
    load_default_thinking_level_in(&home)
}

pub fn persist_default_thinking_level(thinking_level: ThinkingLevel) -> io::Result<()> {
    let store = UserConfigStore::for_current_user().map_err(config_io_error)?;
    store
        .persist_thinking_default(thinking_level)
        .map_err(config_io_error)
}

fn load_default_thinking_level_in(home: &Path) -> Option<ThinkingLevel> {
    let store = UserConfigStore::in_home(home);
    let toml_exists = store.path().exists();
    match store.load() {
        Ok(snapshot) => {
            if let Some(level) = snapshot.thinking_default {
                return Some(level);
            }
        }
        Err(_) if toml_exists => return None,
        Err(_) => return None,
    }

    let json_path = legacy_user_config_path_in(home);
    let legacy_json = load_legacy_json_default(&json_path);
    let legacy = legacy_json
        .as_ref()
        .map(|(level, _)| *level)
        .or_else(|| load_latest_legacy_preference(home))?;
    if store.persist_thinking_default(legacy).is_ok()
        && legacy_json.is_some_and(|(_, only_known_fields)| only_known_fields)
    {
        let _ = fs::remove_file(json_path);
    }
    Some(legacy)
}

#[cfg(test)]
fn persist_default_thinking_level_in(home: &Path, thinking_level: ThinkingLevel) -> io::Result<()> {
    UserConfigStore::in_home(home)
        .persist_thinking_default(thinking_level)
        .map_err(config_io_error)
}

fn load_legacy_json_default(path: &Path) -> Option<(ThinkingLevel, bool)> {
    let raw = fs::read_to_string(path).ok()?;
    let config = serde_json::from_str::<LegacyYachConfig>(&raw).ok()?;
    let level = config.thinking.default?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let only_known_fields = value.as_object().is_some_and(|root| {
        root.keys().all(|key| key == "thinking")
            && root
                .get("thinking")
                .and_then(Value::as_object)
                .is_some_and(|thinking| thinking.keys().all(|key| key == "default"))
    });
    Some((level, only_known_fields))
}

fn load_latest_legacy_preference(home: &Path) -> Option<ThinkingLevel> {
    fs::read_dir(home.join(".yach").join("preferences"))
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            let raw = fs::read_to_string(entry.path()).ok()?;
            let preferences = serde_json::from_str::<LegacyProjectPreferences>(&raw).ok()?;
            (preferences.schema == LEGACY_PREFERENCES_SCHEMA).then_some((
                modified,
                entry.file_name(),
                preferences.thinking_level,
            ))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
        .map(|(_, _, level)| level)
}

fn config_io_error(error: UserConfigError) -> io::Error {
    let kind = match error {
        UserConfigError::HomeUnavailable => io::ErrorKind::NotFound,
        UserConfigError::Invalid => io::ErrorKind::InvalidData,
        UserConfigError::UnsafePath | UserConfigError::UnsafePermissions => {
            io::ErrorKind::PermissionDenied
        }
        UserConfigError::Io | UserConfigError::DurabilityUnknown => io::ErrorKind::Other,
    };
    io::Error::new(kind, error)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn legacy_user_config_path_in(home: &Path) -> PathBuf {
    home.join(".yach").join("config.json")
}

#[cfg(test)]
mod tests {
    use super::{
        LEGACY_PREFERENCES_SCHEMA, LegacyProjectPreferences, legacy_user_config_path_in,
        load_default_thinking_level_in, persist_default_thinking_level_in,
    };
    use crate::UserConfigStore;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use yach_proto::ThinkingLevel;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yach-thinking-config-{label}-{}-{id}",
                std::process::id()
            ));
            assert!(fs::create_dir_all(&path).is_ok());
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn create_private_yach_dir(home: &Path) -> PathBuf {
        let directory = home.join(".yach");
        assert!(fs::create_dir_all(&directory).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert!(fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).is_ok());
        }
        directory
    }

    #[test]
    fn explicit_default_preserves_unrelated_toml() {
        let home = TempDir::new("preserve");
        let store = UserConfigStore::in_home(home.path());
        assert!(store.persist_thinking_default(ThinkingLevel::Low).is_ok());
        let mut raw = fs::read_to_string(store.path()).unwrap_or_default();
        raw.push_str("\n[shell]\nallow = [\"git status\"]\n");
        assert!(fs::write(store.path(), raw).is_ok());

        assert!(persist_default_thinking_level_in(home.path(), ThinkingLevel::High).is_ok());
        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::High)
        );
        let config = fs::read_to_string(store.path()).unwrap_or_default();
        assert!(config.contains("[shell]"));
        assert!(config.contains("git status"));
    }

    #[test]
    fn legacy_json_thinking_default_migrates_and_removes_known_only_file() {
        let home = TempDir::new("json-migration");
        let legacy = legacy_user_config_path_in(home.path());
        assert!(create_private_yach_dir(home.path()).exists());
        assert!(fs::write(&legacy, r#"{"thinking":{"default":"medium"}}"#).is_ok());

        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::Medium)
        );
        assert!(!legacy.exists());
        assert_eq!(
            UserConfigStore::in_home(home.path())
                .load()
                .ok()
                .and_then(|snapshot| snapshot.thinking_default),
            Some(ThinkingLevel::Medium)
        );
    }

    #[test]
    fn legacy_json_with_unrelated_fields_remains_after_migration() {
        let home = TempDir::new("json-preserve");
        let legacy = legacy_user_config_path_in(home.path());
        assert!(create_private_yach_dir(home.path()).exists());
        assert!(
            fs::write(
                &legacy,
                r#"{"thinking":{"default":"low"},"shell":{"allow":[]}}"#
            )
            .is_ok()
        );

        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::Low)
        );
        assert!(legacy.exists());
    }

    #[test]
    fn latest_legacy_project_preference_migrates_to_toml_default() {
        let home = TempDir::new("migration-home");
        let preferences_dir = create_private_yach_dir(home.path()).join("preferences");
        assert!(fs::create_dir_all(&preferences_dir).is_ok());
        let write_legacy = |name: &str, thinking_level: ThinkingLevel| {
            let legacy = LegacyProjectPreferences {
                schema: String::from(LEGACY_PREFERENCES_SCHEMA),
                thinking_level,
            };
            fs::write(
                preferences_dir.join(name),
                serde_json::to_vec(&legacy).unwrap_or_default(),
            )
        };
        assert!(write_legacy("older.json", ThinkingLevel::Low).is_ok());
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(write_legacy("newer.json", ThinkingLevel::Max).is_ok());

        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::Max)
        );
        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::Max)
        );
    }
}
