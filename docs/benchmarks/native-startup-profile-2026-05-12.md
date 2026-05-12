# Native Startup Profile - 2026-05-12

## Summary

This report adds a granular native-default TUI startup profile after `yach tui`
became native by default.

The key result: traced Rust-side startup from `main` entry to first TUI render
is already low-millisecond. The larger startup cost is outside the app loop:
process launch, loader work, PTY/script harness overhead, and terminal setup
before the release binary reaches Rust `main`.

## Environment

- Date: 2026-05-12
- Commit base: `9082482`
- Worktree branch: `startup-performance-profiling`
- Build/profile mode: release benchmark and release `yach-cli`
- Machine: local macOS development machine

Important methodology note: `yach-bench` spawns the release `yach-cli` binary.
Build the release CLI before collecting startup profile data, otherwise the
benchmark can accidentally run a stale binary:

```bash
just dev cargo build -p yach-cli --release
```

## Commands

```bash
just dev cargo build -p yach-cli --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-report --samples 100
just dev cargo run -p yach-bench --release -- yach-cli-startup-report --samples 100
```

## Results

### Native TUI Startup Profile

```text
workload=yach/tui_startup_profile/observed_process_to_first_render_pty count=100 p50=24.522ms p95=27.043ms p99=30.595ms max=373.077ms
workload=yach/tui_startup_profile/process_main_start_since_main count=100 p50=0ns p95=0ns p99=0ns max=3.000us
workload=yach/tui_startup_profile/cli_args_parsed_since_main count=100 p50=6.000us p95=9.000us p99=10.000us max=137.000us
workload=yach/tui_startup_profile/command_run_start_since_main count=100 p50=6.000us p95=9.000us p99=10.000us max=137.000us
workload=yach/tui_startup_profile/tokio_runtime_created_since_main count=100 p50=184.000us p95=237.000us p99=303.000us max=767.000us
workload=yach/tui_startup_profile/native_backend_setup_start_since_main count=100 p50=190.000us p95=245.000us p99=312.000us max=873.000us
workload=yach/tui_startup_profile/native_backend_session_started_since_main count=100 p50=197.000us p95=251.000us p99=325.000us max=951.000us
workload=yach/tui_startup_profile/native_client_initialize_sent_since_main count=100 p50=230.000us p95=287.000us p99=374.000us max=1.226ms
workload=yach/tui_startup_profile/native_backend_task_spawned_since_main count=100 p50=241.000us p95=298.000us p99=383.000us max=1.328ms
workload=yach/tui_startup_profile/run_tui_start_since_main count=100 p50=241.000us p95=299.000us p99=383.000us max=1.328ms
workload=yach/tui_startup_profile/tui_event_stream_created_since_main count=100 p50=343.000us p95=398.000us p99=503.000us max=2.355ms
workload=yach/tui_startup_profile/tui_first_render_start_since_main count=100 p50=369.000us p95=423.000us p99=534.000us max=2.555ms
workload=yach/tui_startup_profile/tui_first_render_end_since_main count=100 p50=462.000us p95=535.000us p99=665.000us max=2.798ms
```

### Legacy First-Output Proxy

```text
workload=yach/tui_startup_first_output_pty count=100 p50=12.939ms p95=18.339ms p99=23.650ms max=26.159ms
```

### CLI First-Output Proxy

```text
workload=yach/cli_startup_first_output count=100 p50=32.019ms p95=35.000ms p99=36.669ms max=39.985ms
```

## Interpretation

The current native TUI path records first render about `0.54ms` p95 after Rust
`main` starts with startup tracing enabled. The trace now buffers phase marks in
memory and flushes after the first-render marker, so phase timings avoid
per-marker file I/O but still include lightweight in-process tracing overhead.

The process/PTY harness observes the first-render trace flush about `27ms` p95,
with one large outlier in this run. That number includes process launch,
terminal harness behavior, trace flush visibility, and polling granularity; it
should not be treated as an exact timestamp for the rendered frame.

This supports two conclusions:

- Extension discovery and activation must remain entirely off the startup path.
- Further startup optimization should focus on process/binary startup,
  dependency loading, PTY/terminal measurement overhead, and making first-frame
  benchmarks distinguish first output from first rendered input-ready frame.

The current profile does not prove hardware keypress-to-focused-input latency.
It measures process launch to observed first-render trace output in a PTY
harness plus internal phase timings from Rust `main`.
