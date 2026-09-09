#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use yach_bench::perf::rss::{peak_rss_bytes, Spawn, StopBoundary};

#[cfg(target_os = "linux")]
#[test]
fn child_peak_rss_is_at_least_its_allocation() {
    let bin = env!("CARGO_BIN_EXE_yach-bench");
    assert!(
        Path::new(bin).is_file(),
        "yach-bench package binary missing at {bin}"
    );
    let mut cmd = Command::new(bin);
    cmd.args(["perf", "__alloc-and-wait", "33554432"]);
    let bytes = peak_rss_bytes(
        cmd,
        Spawn::Piped,
        StopBoundary::FirstOutputByte,
        Duration::from_secs(10),
    );
    assert!(bytes.is_ok(), "peak_rss_bytes failed: {bytes:?}");
    let Ok(bytes) = bytes else {
        return;
    };
    assert!(bytes >= 32 * 1024 * 1024, "bytes={bytes}");
    assert!(bytes < 256 * 1024 * 1024, "bytes={bytes}");
}
