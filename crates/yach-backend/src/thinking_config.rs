use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use yach_proto::ThinkingLevel;

const LEGACY_PREFERENCES_SCHEMA: &str = "yach.project-preferences.v1";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct YachConfig {
    thinking: ThinkingConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ThinkingConfig {
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
    let home = home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset; cannot persist default thinking level",
        )
    })?;
    persist_default_thinking_level_in(&home, thinking_level)
}

fn load_default_thinking_level_in(home: &Path) -> Option<ThinkingLevel> {
    if let Some(level) = load_config_default(&user_config_path_in(home)) {
        return Some(level);
    }
    let legacy = load_latest_legacy_preference(home)?;
    // Migrate the dogfood-era project preference to the explicit global
    // configuration default. Failure is non-fatal: this launch still uses the
    // recovered value and the next explicit selection retries persistence.
    let _ = persist_default_thinking_level_in(home, legacy);
    Some(legacy)
}

fn load_config_default(path: &Path) -> Option<ThinkingLevel> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<YachConfig>(&raw).ok())
        .and_then(|config| config.thinking.default)
}

fn persist_default_thinking_level_in(home: &Path, thinking_level: ThinkingLevel) -> io::Result<()> {
    let path = user_config_path_in(home);
    let mut config = match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid user config: {error}"),
            )
        })?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(error) => return Err(error),
    };
    let root = config.as_object_mut().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "user config must be a JSON object",
        )
    })?;
    let thinking = root
        .entry(String::from("thinking"))
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "user config thinking section must be a JSON object",
            )
        })?;
    thinking.insert(
        String::from("default"),
        Value::String(String::from(thinking_level.as_str())),
    );
    write_private_json(&path, &config)
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
fn write_private_json(path: &Path, value: &Value) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("user config path has no parent"));
    };
    create_private_dir(parent)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn user_config_path_in(home: &Path) -> PathBuf {
    home.join(".yach").join("config.json")
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::{
        LEGACY_PREFERENCES_SCHEMA, LegacyProjectPreferences, load_default_thinking_level_in,
        persist_default_thinking_level_in, user_config_path_in,
    };
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

    #[test]
    fn explicit_default_preserves_unrelated_user_config() {
        let home = TempDir::new("preserve");
        let config_path = user_config_path_in(home.path());
        assert!(fs::create_dir_all(config_path.parent().unwrap_or(home.path())).is_ok());
        assert!(
            fs::write(
                &config_path,
                r#"{"shell":{"allow":["git status"]},"thinking":{"default":"low"}}"#,
            )
            .is_ok()
        );

        assert!(persist_default_thinking_level_in(home.path(), ThinkingLevel::High).is_ok());
        assert_eq!(
            load_default_thinking_level_in(home.path()),
            Some(ThinkingLevel::High)
        );
        let config = fs::read_to_string(config_path).unwrap_or_default();
        assert!(config.contains("\"shell\""));
        assert!(config.contains("\"git status\""));
    }

    #[test]
    fn latest_legacy_project_preference_migrates_to_global_config_default() {
        let home = TempDir::new("migration-home");
        let preferences_dir = home.path().join(".yach").join("preferences");
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
