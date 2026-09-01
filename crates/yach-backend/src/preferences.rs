use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use yach_proto::ThinkingLevel;

const PREFERENCES_SCHEMA: &str = "yach.project-preferences.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredProjectPreferences {
    schema: String,
    thinking_level: ThinkingLevel,
}

#[must_use]
pub fn load_project_thinking_level(project_root: &Path) -> Option<ThinkingLevel> {
    preferences_path(project_root)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<StoredProjectPreferences>(&raw).ok())
        .filter(|preferences| preferences.schema == PREFERENCES_SCHEMA)
        .map(|preferences| preferences.thinking_level)
}

pub fn persist_project_thinking_level(
    project_root: &Path,
    thinking_level: ThinkingLevel,
) -> io::Result<()> {
    let path = preferences_path(project_root).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME and USERPROFILE are unset; cannot persist thinking level",
        )
    })?;
    let Some(parent) = path.parent() else {
        return Err(io::Error::other("preferences path has no parent"));
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
    serde_json::to_writer(
        &mut file,
        &StoredProjectPreferences {
            schema: String::from(PREFERENCES_SCHEMA),
            thinking_level,
        },
    )
    .map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

fn preferences_path(project_root: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let canonical = project_root.canonicalize().ok()?;
    Some(
        PathBuf::from(home)
            .join(".yach")
            .join("preferences")
            .join(format!(
                "{}.json",
                crate::runner::project_state_key(&canonical)
            )),
    )
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
