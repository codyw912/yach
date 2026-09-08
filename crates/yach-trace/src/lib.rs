//! Append-only JSONL lifecycle trace. Zero cost when unset: `from_env`
//! returns `None` and callers hold an `Option<TraceSink>` whose `mark`
//! they gate on `if let Some`.

use std::fmt;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub t_us: u64,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceScope<'a> {
    Startup,
    Turn(&'a str),
}

#[derive(Debug)]
pub struct TraceOpenError {
    pub path: PathBuf,
    pub source: std::io::Error,
}

impl fmt::Display for TraceOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot open trace file {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for TraceOpenError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceParseError {
    TruncatedLine { line_no: usize },
    Malformed { line_no: usize, message: String },
}

#[derive(Serialize)]
struct RecordRef<'a> {
    t_us: u64,
    scope: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn_id: Option<&'a str>,
    label: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
}

struct Inner {
    path: PathBuf,
    writer: Option<Box<dyn std::io::Write + Send>>,
    diag: Box<dyn std::io::Write + Send>,
    warned: bool,
}

fn write_record(
    writer: &mut dyn std::io::Write,
    t_us: u64,
    scope: &'static str,
    turn_id: Option<&str>,
    label: &str,
    n: Option<u32>,
) -> std::io::Result<()> {
    let record = RecordRef {
        t_us,
        scope,
        turn_id,
        label,
        n,
    };
    serde_json::to_writer(&mut *writer, &record)
        .map_err(std::io::Error::other)
        .and_then(|()| writer.write_all(b"\n"))
}

impl Inner {
    fn disable(&mut self, error: &std::io::Error) {
        self.writer = None;
        if !self.warned {
            self.warned = true;
            let _ = writeln!(
                self.diag,
                "yach-trace: write to {} failed ({error}); tracing disabled",
                self.path.display()
            );
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.as_mut()
            && let Err(error) = writer.flush()
        {
            self.disable(&error);
        }
    }
}

#[derive(Clone)]
pub struct TraceSink {
    start: Instant,
    path: Arc<PathBuf>,
    inner: Arc<Mutex<Inner>>,
}

impl fmt::Debug for TraceSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceSink")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl TraceSink {
    pub fn from_env(name: &str) -> Result<Option<Self>, TraceOpenError> {
        match std::env::var_os(name) {
            None => Ok(None),
            Some(path) => Self::open(Path::new(&path)).map(Some),
        }
    }

    pub fn open(path: &Path) -> Result<Self, TraceOpenError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| TraceOpenError {
                path: path.to_path_buf(),
                source,
            })?;
        let path = path.to_path_buf();
        Ok(Self {
            start: Instant::now(),
            path: Arc::new(path.clone()),
            inner: Arc::new(Mutex::new(Inner {
                path,
                writer: Some(Box::new(BufWriter::new(file))),
                diag: Box::new(std::io::stderr()),
                warned: false,
            })),
        })
    }

    #[cfg(test)]
    fn with_writer_and_diag(
        writer: Box<dyn std::io::Write + Send>,
        diag: Box<dyn std::io::Write + Send>,
    ) -> Self {
        let path = PathBuf::from("<test>");
        Self {
            start: Instant::now(),
            path: Arc::new(path.clone()),
            inner: Arc::new(Mutex::new(Inner {
                path,
                writer: Some(writer),
                diag,
                warned: false,
            })),
        }
    }

    pub fn mark(&self, scope: TraceScope<'_>, label: &str) {
        self.write(scope, label, None);
    }

    pub fn mark_n(&self, scope: TraceScope<'_>, label: &str, n: u32) {
        self.write(scope, label, Some(n));
    }

    fn write(&self, scope: TraceScope<'_>, label: &str, n: Option<u32>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        if inner.writer.is_none() {
            return;
        }
        let t_us = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
        let result = {
            let Some(writer) = inner.writer.as_mut() else {
                return;
            };
            match scope {
                TraceScope::Startup => write_record(&mut **writer, t_us, "startup", None, label, n),
                TraceScope::Turn(id) => {
                    write_record(&mut **writer, t_us, "turn", Some(id), label, n)
                }
            }
        };
        if let Err(error) = result {
            inner.disable(&error);
        }
    }

    pub fn flush(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(writer) = inner.writer.as_mut() else {
            return;
        };
        if let Err(error) = writer.flush() {
            inner.disable(&error);
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.inner
            .lock()
            .map(|inner| inner.writer.is_none())
            .unwrap_or(true)
    }
}

pub fn parse_records(contents: &str) -> Result<Vec<TraceRecord>, TraceParseError> {
    let mut records = Vec::new();
    let mut lines = contents.split_inclusive('\n').enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        let line_no = index + 1;
        let is_last = lines.peek().is_none();
        if !line.ends_with('\n') && is_last {
            return Err(TraceParseError::TruncatedLine { line_no });
        }
        let trimmed = line.trim_end_matches('\n');
        if trimmed.is_empty() {
            continue;
        }
        let record: TraceRecord =
            serde_json::from_str(trimmed).map_err(|error| TraceParseError::Malformed {
                line_no,
                message: error.to_string(),
            })?;
        records.push(record);
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::{TraceParseError, TraceScope, TraceSink, parse_records};
    use std::io::{self, Write};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("yach-trace-test-{}-{n}.jsonl", std::process::id()))
    }

    #[test]
    fn unset_env_is_none_and_opens_nothing() {
        let sink = TraceSink::from_env("YACH_TRACE_TEST_UNSET_1");
        assert!(matches!(sink, Ok(None)));
    }

    #[test]
    fn marks_append_jsonl_records_in_order() {
        let path = temp_path();
        let sink = TraceSink::open(&path).map_err(|e| e.to_string());
        assert!(sink.is_ok(), "open failed: {sink:?}");
        let Ok(sink) = sink else { return };
        sink.mark(TraceScope::Startup, "process_main_start");
        sink.mark_n(TraceScope::Turn("turn-1"), "tool_dispatched", 2);
        sink.flush();
        let contents = std::fs::read_to_string(&path);
        assert!(contents.is_ok(), "read failed: {contents:?}");
        let Ok(contents) = contents else { return };
        let records = parse_records(&contents);
        assert!(records.is_ok(), "parse failed: {records:?}");
        let _ = std::fs::remove_file(&path);
        let Ok(records) = records else { return };
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].scope, "startup");
        assert_eq!(records[0].label, "process_main_start");
        assert_eq!(records[0].turn_id, None);
        assert_eq!(records[1].scope, "turn");
        assert_eq!(records[1].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(records[1].n, Some(2));
        assert!(records[1].t_us >= records[0].t_us);
    }

    #[test]
    fn drop_flushes() {
        let path = temp_path();
        {
            let sink = TraceSink::open(&path);
            assert!(sink.is_ok(), "open failed: {sink:?}");
            let Ok(sink) = sink else { return };
            sink.mark(TraceScope::Startup, "a");
        }
        let contents = std::fs::read_to_string(&path);
        assert!(contents.is_ok(), "read failed: {contents:?}");
        let Ok(contents) = contents else { return };
        let _ = std::fs::remove_file(&path);
        assert_eq!(parse_records(&contents).map(|r| r.len()), Ok(1));
    }

    #[test]
    fn unknown_keys_are_ignored_and_truncated_line_is_error() {
        let ok =
            parse_records("{\"t_us\":1,\"scope\":\"startup\",\"label\":\"x\",\"future\":true}\n");
        assert_eq!(ok.map(|r| r.len()), Ok(1));
        let bad =
            parse_records("{\"t_us\":1,\"scope\":\"startup\",\"label\":\"x\"}\n{\"t_us\":2,\"sco");
        assert_eq!(bad, Err(TraceParseError::TruncatedLine { line_no: 2 }));
    }

    struct FlushFailWriter {
        writes: Arc<AtomicUsize>,
    }

    impl Write for FlushFailWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected"))
        }
    }

    struct DiagWriter {
        output: Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl Write for DiagWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.output
                .lock()
                .map_err(|_| io::Error::other("poisoned"))?
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn flush_failure_disables_sink_once() {
        let writes = Arc::new(AtomicUsize::new(0));
        let diagnostics = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = TraceSink::with_writer_and_diag(
            Box::new(FlushFailWriter {
                writes: Arc::clone(&writes),
            }),
            Box::new(DiagWriter {
                output: Arc::clone(&diagnostics),
            }),
        );
        sink.mark(TraceScope::Startup, "a");
        sink.flush();
        assert!(sink.is_disabled());
        let writes_after_failure = writes.load(Ordering::Relaxed);
        sink.mark(TraceScope::Startup, "b");
        sink.flush();
        assert_eq!(writes.load(Ordering::Relaxed), writes_after_failure);
        drop(sink);
        let diagnostics = diagnostics.lock();
        assert!(
            diagnostics.is_ok(),
            "diagnostic lock failed: {diagnostics:?}"
        );
        let Ok(diagnostics) = diagnostics else { return };
        let diagnostics = String::from_utf8_lossy(&diagnostics);
        assert_eq!(diagnostics.lines().count(), 1);
    }
}
