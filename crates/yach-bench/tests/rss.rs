#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use yach_bench::perf::registry::RunCtx;
use yach_bench::perf::rss::{Spawn, StopBoundary, peak_rss_bytes};
use yach_bench::perf::workloads::tui::HEADLESS;

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

#[test]
fn child_peak_rss_pty_is_at_least_its_allocation() {
    let bin = env!("CARGO_BIN_EXE_yach-bench");
    for _ in 0..5 {
        let mut cmd = Command::new(bin);
        cmd.args(["perf", "__alloc-and-wait", "33554432"]);
        let bytes = peak_rss_bytes(
            cmd,
            Spawn::Pty,
            StopBoundary::FirstOutputByte,
            Duration::from_secs(10),
        );
        assert!(bytes.is_ok(), "peak_rss_bytes pty failed: {bytes:?}");
        let Ok(bytes) = bytes else {
            return;
        };
        assert!(bytes >= 32 * 1024 * 1024, "bytes={bytes}");
        assert!(bytes < 256 * 1024 * 1024, "bytes={bytes}");
    }
}

#[test]
fn peak_rss_is_stable_after_earlier_in_process_workload() {
    let bin = env!("CARGO_BIN_EXE_yach-bench");
    let first = sample_alloc_wait(bin, 1_048_576);
    assert!(first.is_ok(), "first rss: {first:?}");
    let Ok(first) = first else {
        return;
    };

    let ctx = RunCtx {
        samples: 1,
        yach_bin: None,
        yach_bench_yach_bin: None,
        yach_bench_bin: None,
        filter: None,
    };
    let viewport = HEADLESS
        .iter()
        .find(|workload| workload.id == "viewport/huge_transcript_scroll_headless/10000");
    assert!(viewport.is_some(), "missing viewport workload");
    let Some(viewport) = viewport else {
        return;
    };
    let ran = (viewport.run)(&ctx);
    let ran_error = ran.as_ref().err().cloned().unwrap_or_default();
    assert!(ran.is_ok(), "viewport workload failed: {ran_error}");

    let second = sample_alloc_wait(bin, 1_048_576);
    assert!(second.is_ok(), "second rss: {second:?}");
    let Ok(second) = second else {
        return;
    };

    let lo = first.min(second);
    let hi = first.max(second);
    assert!(
        hi <= lo + lo / 10,
        "rss changed more than 10% after earlier workload: first={first} second={second}"
    );
}

fn sample_alloc_wait(bin: &str, bytes: usize) -> Result<u64, String> {
    let mut cmd = Command::new(bin);
    cmd.args(["perf", "__alloc-and-wait", &bytes.to_string()]);
    peak_rss_bytes(
        cmd,
        Spawn::Piped,
        StopBoundary::FirstOutputByte,
        Duration::from_secs(10),
    )
}
