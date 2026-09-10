use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

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

#[cfg(test)]
#[derive(Debug, Default)]
struct SessionPersistCounters {
    append_event_calls: AtomicUsize,
    append_events_calls: AtomicUsize,
    sync_calls: AtomicUsize,
}

#[derive(Debug, Clone)]
pub struct JsonlSessionStore {
    path: PathBuf,
    #[cfg(test)]
    counters: Arc<SessionPersistCounters>,
}

impl PartialEq for JsonlSessionStore {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for JsonlSessionStore {}

impl JsonlSessionStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            counters: Arc::new(SessionPersistCounters::default()),
        }
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

    #[cfg(test)]
    #[must_use]
    pub(crate) fn persist_append_event_calls(&self) -> usize {
        self.counters.append_event_calls.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn persist_append_events_calls(&self) -> usize {
        self.counters.append_events_calls.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn persist_sync_calls(&self) -> usize {
        self.counters.sync_calls.load(Ordering::SeqCst)
    }
}

impl SessionEventSink for JsonlSessionStore {
    fn append_event(&self, event: &SessionEvent) -> io::Result<()> {
        #[cfg(test)]
        self.counters
            .append_event_calls
            .fetch_add(1, Ordering::SeqCst);
        if let Some(parent) = self.path.parent() {
            create_session_dir(parent)?;
        }

        let mut file = open_append_file(&self.path)?;
        let line = serde_json::to_string(event).map_err(io::Error::other)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_data()?;
        #[cfg(test)]
        self.counters.sync_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn append_events(&self, events: &[SessionEvent]) -> io::Result<()> {
        #[cfg(test)]
        self.counters
            .append_events_calls
            .fetch_add(1, Ordering::SeqCst);
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
        file.sync_data()?;
        #[cfg(test)]
        self.counters.sync_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
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
