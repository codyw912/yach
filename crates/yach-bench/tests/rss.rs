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
    const CHILD_BYTES: usize = 32 * 1024 * 1024;
    const PARENT_BYTES: usize = 4 * CHILD_BYTES;

    let bin = env!("CARGO_BIN_EXE_yach-bench");
    let first = sample_alloc_wait(bin, CHILD_BYTES);
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

    // Keep a much larger, touched parent allocation alive across the second
    // spawn. Inherited ru_maxrss must exceed the child's entire RSS budget,
    // regardless of how much memory the viewport allocator retained.
    let mut parent = vec![0_u8; PARENT_BYTES];
    for chunk in parent.chunks_mut(4096) {
        chunk[0] = 1;
    }
    std::hint::black_box(&parent);

    let second = sample_alloc_wait(bin, CHILD_BYTES);
    assert!(second.is_ok(), "second rss: {second:?}");
    let Ok(second) = second else {
        return;
    };
    std::hint::black_box(parent);

    // Independent execs need not have equal startup/allocator RSS. Instead,
    // allow a full child allocation of overhead while excluding the much
    // larger parent image. Doubling the child's allocation must also fail.
    for (sample, bytes) in [("first", first), ("second", second)] {
        assert!(
            (CHILD_BYTES as u64..(2 * CHILD_BYTES) as u64).contains(&bytes),
            "{sample} child rss outside allocation envelope: bytes={bytes} first={first} second={second}"
        );
    }
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
