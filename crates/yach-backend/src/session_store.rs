use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::{NativeSessionEvent, NativeSessionLog};

pub trait NativeSessionEventSink {
    fn append_event(&self, event: &NativeSessionEvent) -> io::Result<()>;

    fn append_events(&self, events: &[NativeSessionEvent]) -> io::Result<()> {
        for event in events {
            self.append_event(event)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeJsonlSessionStore {
    path: PathBuf,
}

impl NativeJsonlSessionStore {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> io::Result<NativeSessionLog> {
        NativeSessionLog::load_from_file(&self.path)
    }
}

impl NativeSessionEventSink for NativeJsonlSessionStore {
    fn append_event(&self, event: &NativeSessionEvent) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(event).map_err(io::Error::other)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()
    }

    fn append_events(&self, events: &[NativeSessionEvent]) -> io::Result<()> {
        let mut buffer = Vec::new();
        for event in events {
            serde_json::to_writer(&mut buffer, event).map_err(io::Error::other)?;
            buffer.write_all(b"\n")?;
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&buffer)?;
        file.flush()
    }
}
