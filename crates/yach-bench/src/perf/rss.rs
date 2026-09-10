use std::io::{self, Write as _};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::perf::Outcome;

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::process::{Child, Stdio};

#[cfg(target_os = "linux")]
type ChildReader = Box<dyn Read + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spawn {
    Piped,
    Pty,
}

#[derive(Debug, Clone)]
pub enum StopBoundary {
    FirstOutputByte,
    TraceLabel { path: PathBuf, label: &'static str },
}

#[must_use]
pub fn kib_to_bytes(kib: i64) -> u64 {
    u64::try_from(kib).unwrap_or(0).saturating_mul(1024)
}

pub(crate) fn alloc_and_wait(bytes: usize) -> Result<Outcome, String> {
    let mut buf = vec![0_u8; bytes];
    for chunk in buf.chunks_mut(4096) {
        chunk[0] = 1;
    }
    if let Some(last) = buf.last_mut() {
        *last = 1;
    }
    std::hint::black_box(&buf);
    {
        let mut stdout = io::stdout();
        stdout.write_all(b"x").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
    }
    thread::sleep(Duration::from_secs(30));
    std::hint::black_box(buf);
    Ok(Outcome {
        lines: Vec::new(),
        exit_code: 0,
    })
}

#[cfg(target_os = "linux")]
pub fn peak_rss_bytes(
    command: Command,
    spawn: Spawn,
    boundary: StopBoundary,
    timeout: Duration,
) -> Result<u64, String> {
    let (child, reader) = spawn_measured(command, spawn)?;
    let child_pid = child.id();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let sampler = thread::spawn(move || sample_child_vmhwm(child_pid, &stop_thread));
    let reached = reached_boundary(reader, boundary, timeout);
    stop.store(true, Ordering::Relaxed);
    let mut vmhwm = sampler.join().unwrap_or(0);
    if vmhwm == 0 {
        vmhwm = retry_child_vmhwm(child_pid);
    }

    let reaped = reap_maxrss_bytes(&child);
    match reached {
        Ok(true) => {
            let _ = reaped;
            if vmhwm == 0 {
                return Err(String::from("could not sample child VmHWM"));
            }
            Ok(vmhwm)
        }
        Ok(false) => Err(String::from("child exited before boundary")),
        Err(error) => {
            let _ = reaped;
            Err(error)
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn peak_rss_bytes(
    _command: Command,
    _spawn: Spawn,
    _boundary: StopBoundary,
    _timeout: Duration,
) -> Result<u64, String> {
    Err(String::from("unsupported_os"))
}

#[cfg(target_os = "linux")]
pub fn spawn_on_pty(mut command: Command) -> Result<(Child, File), String> {
    use std::os::fd::FromRawFd as _;
    use std::os::unix::process::CommandExt as _;

    let mut master: libc::c_int = 0;
    let mut slave: libc::c_int = 0;
    let winsize = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `openpty` writes the two fd out-params. A null name is documented;
    // termios may be null; winsize points at a live local.
    let rc = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &raw const winsize,
        )
    };
    if rc != 0 {
        return Err(String::from("openpty failed"));
    }

    // SAFETY: `pre_exec` runs in the child after fork and before exec.
    unsafe {
        command.pre_exec(move || child_attach_tty(master, slave));
    }

    match command.spawn() {
        Ok(child) => {
            // SAFETY: this is the parent's slave fd; the child already dup2'd it
            // onto stdio. Closing it here does not affect the child.
            unsafe {
                libc::close(slave);
            }
            // SAFETY: `master` is an open fd we uniquely own after spawn.
            let master_file = unsafe { File::from_raw_fd(master) };
            Ok((child, master_file))
        }
        Err(error) => {
            // SAFETY: spawn failed, so both fds are still ours to close.
            unsafe {
                libc::close(master);
                libc::close(slave);
            }
            Err(error.to_string())
        }
    }
}

#[cfg(target_os = "linux")]
fn child_attach_tty(master: libc::c_int, slave: libc::c_int) -> io::Result<()> {
    // SAFETY: `master` and `slave` are the fds `openpty` created. This runs in
    // the child before exec: become session leader, take the slave as the
    // controlling tty, dup2 it onto stdio, then close the originals so they
    // are not leaked across exec.
    unsafe {
        if libc::setsid() == -1 {
            return Err(io::Error::last_os_error());
        }
        if libc::ioctl(slave, libc::TIOCSCTTY, 0) == -1 {
            return Err(io::Error::last_os_error());
        }
        if libc::dup2(slave, 0) == -1 || libc::dup2(slave, 1) == -1 || libc::dup2(slave, 2) == -1 {
            return Err(io::Error::last_os_error());
        }
        libc::close(slave);
        libc::close(master);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn proc_status_bytes(pid: u32, field: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        if key != field {
            continue;
        }
        let value: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(value.saturating_mul(1024));
    }
    None
}

#[cfg(target_os = "linux")]
fn sample_child_vmhwm(pid: u32, stop: &AtomicBool) -> u64 {
    let mut peak = 0_u64;
    while !stop.load(Ordering::Relaxed) {
        if let Some(hwm) = proc_status_bytes(pid, "VmHWM") {
            peak = peak.max(hwm);
        }
        thread::yield_now();
    }
    if let Some(hwm) = proc_status_bytes(pid, "VmHWM") {
        peak = peak.max(hwm);
    }
    peak
}

#[cfg(target_os = "linux")]
fn retry_child_vmhwm(pid: u32) -> u64 {
    let mut peak = 0_u64;
    for _ in 0..50 {
        if let Some(hwm) = proc_status_bytes(pid, "VmHWM") {
            peak = peak.max(hwm);
            if peak > 0 {
                return peak;
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    peak
}

#[cfg(target_os = "linux")]
fn spawn_measured(
    mut command: Command,
    spawn: Spawn,
) -> Result<(Child, Option<ChildReader>), String> {
    match spawn {
        Spawn::Piped => {
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .stdin(Stdio::null());
            let mut child = command.spawn().map_err(|error| error.to_string())?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| String::from("missing stdout"))?;
            Ok((child, Some(Box::new(stdout))))
        }
        Spawn::Pty => {
            let (child, master) = spawn_on_pty(command)?;
            Ok((child, Some(Box::new(master))))
        }
    }
}

#[cfg(target_os = "linux")]
fn reached_boundary(
    reader: Option<ChildReader>,
    boundary: StopBoundary,
    timeout: Duration,
) -> Result<bool, String> {
    match boundary {
        StopBoundary::FirstOutputByte => {
            let reader = reader.ok_or_else(|| String::from("missing stdout"))?;
            wait_first_output_byte(reader, timeout)
        }
        StopBoundary::TraceLabel { path, label } => {
            if let Some(mut reader) = reader {
                thread::spawn(move || drain_reader(&mut reader));
            }
            wait_for_label(&path, label, timeout)
        }
    }
}

#[cfg(target_os = "linux")]
fn drain_reader(reader: &mut dyn Read) {
    let mut buf = [0_u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_first_output_byte(mut reader: ChildReader, timeout: Duration) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut first = [0_u8; 1];
        let _ = tx.send(reader.read(&mut first).map(|n| n > 0));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(reached)) => Ok(reached),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(String::from("timeout waiting for first output byte")),
    }
}

#[cfg(target_os = "linux")]
fn wait_for_label(path: &Path, label: &str, timeout: Duration) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && trace_has_label(&contents, label)?
        {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Err(String::from("timeout waiting for trace label"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "linux")]
fn trace_has_label(contents: &str, label: &str) -> Result<bool, String> {
    Ok(parse_trace_for_wait(contents)?
        .iter()
        .any(|record| record.label == label))
}

#[cfg(target_os = "linux")]
fn parse_trace_for_wait(contents: &str) -> Result<Vec<yach_trace::TraceRecord>, String> {
    match yach_trace::parse_records(contents) {
        Ok(records) => Ok(records),
        Err(yach_trace::TraceParseError::TruncatedLine { .. }) => match contents.rfind('\n') {
            Some(index) => parse_trace_for_wait(&contents[..=index]),
            None => Ok(Vec::new()),
        },
        Err(yach_trace::TraceParseError::Malformed { line_no, message }) => {
            Err(format!("trace line {line_no}: {message}"))
        }
    }
}

#[cfg(target_os = "linux")]
fn reap_maxrss_bytes(child: &Child) -> Result<u64, String> {
    let pid = i32::try_from(child.id()).map_err(|error| error.to_string())?;
    // SAFETY: `pid` is the child we spawned. `rusage` is a POD integer struct,
    // so zeroed is a valid empty value. SIGKILL stops the child; `wait4` reaps
    // it and fills rusage. Do not call `Child::wait` afterwards — it would fail
    // on the already-reaped pid. `Child::drop` does not wait, so dropping
    // `child` after this is correct.
    let (rc, usage) = unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status: libc::c_int = 0;
        let mut usage: libc::rusage = std::mem::zeroed();
        let rc = libc::wait4(pid, &raw mut status, 0, &raw mut usage);
        (rc, usage)
    };
    if rc < 0 {
        return Err(String::from("wait4 failed"));
    }
    Ok(kib_to_bytes(usage.ru_maxrss))
}

#[cfg(test)]
mod tests {
    use super::kib_to_bytes;

    #[test]
    fn linux_maxrss_is_kib() {
        assert_eq!(kib_to_bytes(1), 1024);
        assert_eq!(kib_to_bytes(0), 0);
        assert_eq!(kib_to_bytes(-1), 0);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_is_unsupported() {
        use super::{Spawn, StopBoundary, peak_rss_bytes};
        assert_eq!(
            peak_rss_bytes(
                std::process::Command::new("true"),
                Spawn::Piped,
                StopBoundary::FirstOutputByte,
                std::time::Duration::from_secs(1),
            ),
            Err(String::from("unsupported_os"))
        );
    }
}
