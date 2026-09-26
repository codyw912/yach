use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{SessionEvent, SessionLoadResult, SessionLog};

pub trait SessionEventSink: Sync {
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
#[expect(
    clippy::struct_field_names,
    reason = "call-count fields share a calls suffix"
)]
struct SessionPersistCounters {
    append_event_calls: AtomicUsize,
    append_events_calls: AtomicUsize,
    append_without_sync_calls: AtomicUsize,
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
    pub(crate) fn persist_append_without_sync_calls(&self) -> usize {
        self.counters
            .append_without_sync_calls
            .load(Ordering::SeqCst)
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn persist_sync_calls(&self) -> usize {
        self.counters.sync_calls.load(Ordering::SeqCst)
    }
}

impl JsonlSessionStore {
    fn write_events(&self, events: &[SessionEvent]) -> io::Result<fs::File> {
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
        Ok(file)
    }

    fn sync_written(&self, file: &fs::File) -> io::Result<()> {
        file.sync_data()?;
        #[cfg(test)]
        self.counters.sync_calls.fetch_add(1, Ordering::SeqCst);
        #[cfg(not(test))]
        let _ = self;
        Ok(())
    }

    /// Append events in order without a durability barrier.
    ///
    /// Call [`Self::flush_durable`] when the written events must survive a crash.
    pub fn append_events_without_sync(&self, events: &[SessionEvent]) -> io::Result<()> {
        #[cfg(test)]
        self.counters
            .append_without_sync_calls
            .fetch_add(1, Ordering::SeqCst);
        if events.is_empty() {
            return Ok(());
        }
        self.write_events(events).map(|_| ())
    }

    /// Make previously written session events durable on disk.
    pub fn flush_durable(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            create_session_dir(parent)?;
        }
        let file = open_append_file(&self.path)?;
        self.sync_written(&file)
    }
}

impl SessionEventSink for JsonlSessionStore {
    fn append_event(&self, event: &SessionEvent) -> io::Result<()> {
        #[cfg(test)]
        self.counters
            .append_event_calls
            .fetch_add(1, Ordering::SeqCst);
        let file = self.write_events(std::slice::from_ref(event))?;
        self.sync_written(&file)
    }

    fn append_events(&self, events: &[SessionEvent]) -> io::Result<()> {
        #[cfg(test)]
        self.counters
            .append_events_calls
            .fetch_add(1, Ordering::SeqCst);
        let file = self.write_events(events)?;
        self.sync_written(&file)
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
