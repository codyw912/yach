use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{SessionEvent, SessionLoadResult, SessionLog};

pub trait SessionEventSink {
    fn append_event(&self, event: &SessionEvent) -> io::Result<()>;

    fn append_events(&self, events: &[SessionEvent]) -> io::Result<()> {
        for event in events {
            self.append_event(event)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlSessionStore {
    path: PathBuf,
}

impl JsonlSessionStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<SessionLog> {
        SessionLog::load_from_file(&self.path)
    }

    pub fn load_with_warnings(&self) -> io::Result<SessionLoadResult> {
        SessionLog::load_from_file_with_warnings(&self.path)
    }
}

impl SessionEventSink for JsonlSessionStore {
    fn append_event(&self, event: &SessionEvent) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            create_session_dir(parent)?;
        }

        let mut file = open_append_file(&self.path)?;
        let line = serde_json::to_string(event).map_err(io::Error::other)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_data()
    }

    fn append_events(&self, events: &[SessionEvent]) -> io::Result<()> {
        let mut buffer = Vec::new();
        for event in events {
            serde_json::to_writer(&mut buffer, event).map_err(io::Error::other)?;
            buffer.write_all(b"\n")?;
        }

        if let Some(parent) = self.path.parent() {
            create_session_dir(parent)?;
        }

        let mut file = open_append_file(&self.path)?;
        file.write_all(&buffer)?;
        file.flush()?;
        file.sync_data()
    }
}

/// Create the session log directory owner-only: session logs persist
/// provider-visible tool payloads, so the directory should not be readable
/// by other users.
fn create_session_dir(parent: &Path) -> io::Result<()> {
    if parent.as_os_str().is_empty() || parent.exists() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)
}

fn open_append_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    configure_session_file_create_options(&mut options);
    options.open(path)
}

fn configure_session_file_create_options(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
}
