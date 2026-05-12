# Extension Startup Profile - 2026-05-12

## Summary

This report adds focused startup evidence for an installed but inactive extension
manifest. The benchmark harness now has a
`yach-tui-startup-profile-with-inactive-extension-report` mode that creates a
temporary `YACH_EXTENSION_MANIFEST_DIR` containing one valid extension manifest,
starts `yach tui`, and reports the same startup trace mark summaries as the
baseline startup profile.

The CLI does not currently read `YACH_EXTENSION_MANIFEST_DIR`, so no manifest
scan trace marks are expected in this run. No extension host is spawned from the
startup path.

## Environment

- Date: 2026-05-12
- Base prerequisite: `d89102f`
- Build/profile mode: release benchmark and release `yach-cli`
- Machine: local macOS development machine

## Commands

```bash
just dev cargo build -p yach-cli --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 5
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 5
just dev cargo fmt --check
```

## Results

### Baseline Native TUI Startup Profile

```text
samples_requested=5
samples_collected=5
workload=yach/tui_startup_profile/observed_process_to_first_render_pty count=5 p50=28.878ms p95=326.963ms p99=326.963ms max=326.963ms
workload=yach/tui_startup_profile/cli_args_parsed_since_main count=5 p50=9.000us p95=158.000us p99=158.000us max=158.000us
workload=yach/tui_startup_profile/command_run_start_since_main count=5 p50=9.000us p95=158.000us p99=158.000us max=158.000us
workload=yach/tui_startup_profile/native_backend_session_started_since_main count=5 p50=225.000us p95=1.183ms p99=1.183ms max=1.183ms
workload=yach/tui_startup_profile/native_backend_setup_start_since_main count=5 p50=216.000us p95=1.090ms p99=1.090ms max=1.090ms
workload=yach/tui_startup_profile/native_backend_task_spawned_since_main count=5 p50=275.000us p95=1.615ms p99=1.615ms max=1.615ms
workload=yach/tui_startup_profile/native_client_initialize_sent_since_main count=5 p50=264.000us p95=1.446ms p99=1.446ms max=1.446ms
workload=yach/tui_startup_profile/process_main_start_since_main count=5 p50=0ns p95=0ns p99=0ns max=0ns
workload=yach/tui_startup_profile/run_tui_start_since_main count=5 p50=275.000us p95=1.615ms p99=1.615ms max=1.615ms
workload=yach/tui_startup_profile/tokio_runtime_created_since_main count=5 p50=208.000us p95=819.000us p99=819.000us max=819.000us
workload=yach/tui_startup_profile/tui_alternate_screen_entered_since_main count=5 p50=324.000us p95=2.147ms p99=2.147ms max=2.147ms
workload=yach/tui_startup_profile/tui_app_created_since_main count=5 p50=289.000us p95=1.786ms p99=1.786ms max=1.786ms
workload=yach/tui_startup_profile/tui_cursor_hidden_since_main count=5 p50=326.000us p95=2.150ms p99=2.150ms max=2.150ms
workload=yach/tui_startup_profile/tui_event_stream_created_since_main count=5 p50=418.000us p95=2.545ms p99=2.545ms max=2.545ms
workload=yach/tui_startup_profile/tui_first_backend_event_received_since_main count=5 p50=421.000us p95=2.581ms p99=2.581ms max=2.581ms
workload=yach/tui_startup_profile/tui_first_render_end_since_main count=5 p50=574.000us p95=3.088ms p99=3.088ms max=3.088ms
workload=yach/tui_startup_profile/tui_first_render_start_since_main count=5 p50=455.000us p95=2.779ms p99=2.779ms max=2.779ms
workload=yach/tui_startup_profile/tui_raw_mode_enabled_since_main count=5 p50=305.000us p95=1.871ms p99=1.871ms max=1.871ms
workload=yach/tui_startup_profile/tui_terminal_created_since_main count=5 p50=357.000us p95=2.366ms p99=2.366ms max=2.366ms
```

### Installed Inactive Extension Startup Profile

```text
samples_requested=5
samples_collected=5
workload=yach/tui_startup_profile_with_inactive_extension/observed_process_to_first_render_pty count=5 p50=31.447ms p95=34.678ms p99=34.678ms max=34.678ms
workload=yach/tui_startup_profile_with_inactive_extension/cli_args_parsed_since_main count=5 p50=8.000us p95=12.000us p99=12.000us max=12.000us
workload=yach/tui_startup_profile_with_inactive_extension/command_run_start_since_main count=5 p50=8.000us p95=12.000us p99=12.000us max=12.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_session_started_since_main count=5 p50=240.000us p95=278.000us p99=278.000us max=278.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_setup_start_since_main count=5 p50=232.000us p95=269.000us p99=269.000us max=269.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_task_spawned_since_main count=5 p50=291.000us p95=334.000us p99=334.000us max=334.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_client_initialize_sent_since_main count=5 p50=279.000us p95=323.000us p99=323.000us max=323.000us
workload=yach/tui_startup_profile_with_inactive_extension/process_main_start_since_main count=5 p50=0ns p95=0ns p99=0ns max=0ns
workload=yach/tui_startup_profile_with_inactive_extension/run_tui_start_since_main count=5 p50=291.000us p95=334.000us p99=334.000us max=334.000us
workload=yach/tui_startup_profile_with_inactive_extension/tokio_runtime_created_since_main count=5 p50=222.000us p95=262.000us p99=262.000us max=262.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_alternate_screen_entered_since_main count=5 p50=329.000us p95=369.000us p99=369.000us max=369.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_app_created_since_main count=5 p50=301.000us p95=343.000us p99=343.000us max=343.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_cursor_hidden_since_main count=5 p50=331.000us p95=370.000us p99=370.000us max=370.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_event_stream_created_since_main count=5 p50=395.000us p95=448.000us p99=448.000us max=448.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_backend_event_received_since_main count=3 p50=397.000us p95=413.000us p99=413.000us max=413.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_render_end_since_main count=5 p50=533.000us p95=598.000us p99=598.000us max=598.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_render_start_since_main count=5 p50=425.000us p95=481.000us p99=481.000us max=481.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_raw_mode_enabled_since_main count=5 p50=318.000us p95=358.000us p99=358.000us max=358.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_terminal_created_since_main count=5 p50=354.000us p95=399.000us p99=399.000us max=399.000us
```

## Interpretation

The inactive-extension mode reports the same startup trace mark families as the
baseline profile and keeps `tui_first_render_end_since_main` within the same
low-millisecond envelope. The baseline's PTY/process p95 includes one large
outlier in this five-sample run; the internal trace marks are the more useful
comparison for Rust-side startup work.

No trace marker named `extension_host_spawned_before_first_render` appeared.
The expected manifest scan labels, `extension_manifest_scan_scheduled`,
`extension_manifest_scan_started`, and `extension_manifest_scan_finished`, are
not emitted because manifest discovery is not wired into the CLI yet. When that
discovery path is added, those labels should be emitted only after the TUI first
render marker and should not activate or spawn extension hosts.

## Limitations

- This is a focused five-sample run, not a full 100-sample release benchmark.
- The harness sets up a valid inactive extension manifest directory, but the
  current CLI does not scan it.
- The PTY/process measurement includes process launch, loader work, harness
  overhead, terminal setup, polling granularity, and any local machine noise.
