# Extension Startup Profile - 2026-05-12

## Summary

This report records 100-sample startup evidence for an installed but inactive
extension manifest. The benchmark harness mode
`yach-tui-startup-profile-with-inactive-extension-report` creates a temporary
`YACH_EXTENSION_MANIFEST_DIR` containing one valid extension manifest, starts
`yach tui`, and reports the same startup trace mark summaries as the baseline
startup profile.

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
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 100
```

## Results

### Baseline Native TUI Startup Profile

```text
samples_requested=100
samples_collected=100
workload=yach/tui_startup_profile/observed_process_to_first_render_pty count=100 p50=21.750ms p95=26.963ms p99=31.981ms max=347.414ms
workload=yach/tui_startup_profile/cli_args_parsed_since_main count=100 p50=7.000us p95=9.000us p99=10.000us max=260.000us
workload=yach/tui_startup_profile/command_run_start_since_main count=100 p50=7.000us p95=9.000us p99=10.000us max=260.000us
workload=yach/tui_startup_profile/native_backend_session_started_since_main count=100 p50=182.000us p95=241.000us p99=266.000us max=1.294ms
workload=yach/tui_startup_profile/native_backend_setup_start_since_main count=100 p50=174.000us p95=233.000us p99=255.000us max=1.216ms
workload=yach/tui_startup_profile/native_backend_task_spawned_since_main count=100 p50=219.000us p95=279.000us p99=304.000us max=1.620ms
workload=yach/tui_startup_profile/native_client_initialize_sent_since_main count=100 p50=208.000us p95=269.000us p99=294.000us max=1.484ms
workload=yach/tui_startup_profile/process_main_start_since_main count=100 p50=0ns p95=0ns p99=0ns max=0ns
workload=yach/tui_startup_profile/run_tui_start_since_main count=100 p50=220.000us p95=279.000us p99=304.000us max=1.620ms
workload=yach/tui_startup_profile/tokio_runtime_created_since_main count=100 p50=168.000us p95=209.000us p99=231.000us max=1.062ms
workload=yach/tui_startup_profile/tui_alternate_screen_entered_since_main count=100 p50=251.000us p95=317.000us p99=336.000us max=2.073ms
workload=yach/tui_startup_profile/tui_app_created_since_main count=100 p50=227.000us p95=285.000us p99=311.000us max=1.867ms
workload=yach/tui_startup_profile/tui_cursor_hidden_since_main count=100 p50=252.000us p95=318.000us p99=337.000us max=2.076ms
workload=yach/tui_startup_profile/tui_event_stream_created_since_main count=100 p50=313.000us p95=387.000us p99=436.000us max=2.379ms
workload=yach/tui_startup_profile/tui_first_backend_event_received_since_main count=59 p50=315.000us p95=389.000us p99=2.412ms max=2.412ms
workload=yach/tui_startup_profile/tui_first_render_end_since_main count=100 p50=423.000us p95=503.000us p99=558.000us max=2.902ms
workload=yach/tui_startup_profile/tui_first_render_start_since_main count=100 p50=338.000us p95=415.000us p99=464.000us max=2.600ms
workload=yach/tui_startup_profile/tui_raw_mode_enabled_since_main count=100 p50=241.000us p95=303.000us p99=327.000us max=1.942ms
workload=yach/tui_startup_profile/tui_terminal_created_since_main count=100 p50=272.000us p95=338.000us p99=380.000us max=2.241ms
```

### Installed Inactive Extension Startup Profile

```text
samples_requested=100
samples_collected=100
workload=yach/tui_startup_profile_with_inactive_extension/observed_process_to_first_render_pty count=100 p50=22.936ms p95=27.483ms p99=30.661ms max=30.965ms
workload=yach/tui_startup_profile_with_inactive_extension/cli_args_parsed_since_main count=100 p50=7.000us p95=9.000us p99=11.000us max=11.000us
workload=yach/tui_startup_profile_with_inactive_extension/command_run_start_since_main count=100 p50=7.000us p95=9.000us p99=11.000us max=12.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_session_started_since_main count=100 p50=187.000us p95=238.000us p99=258.000us max=264.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_setup_start_since_main count=100 p50=180.000us p95=232.000us p99=249.000us max=257.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_backend_task_spawned_since_main count=100 p50=232.000us p95=289.000us p99=310.000us max=315.000us
workload=yach/tui_startup_profile_with_inactive_extension/native_client_initialize_sent_since_main count=100 p50=224.000us p95=279.000us p99=295.000us max=304.000us
workload=yach/tui_startup_profile_with_inactive_extension/process_main_start_since_main count=100 p50=0ns p95=0ns p99=0ns max=3.000us
workload=yach/tui_startup_profile_with_inactive_extension/run_tui_start_since_main count=100 p50=232.000us p95=289.000us p99=310.000us max=315.000us
workload=yach/tui_startup_profile_with_inactive_extension/tokio_runtime_created_since_main count=100 p50=172.000us p95=222.000us p99=231.000us max=251.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_alternate_screen_entered_since_main count=100 p50=264.000us p95=322.000us p99=348.000us max=352.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_app_created_since_main count=100 p50=241.000us p95=297.000us p99=322.000us max=324.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_cursor_hidden_since_main count=100 p50=265.000us p95=324.000us p99=349.000us max=353.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_event_stream_created_since_main count=100 p50=327.000us p95=402.000us p99=417.000us max=433.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_backend_event_received_since_main count=51 p50=321.000us p95=391.000us p99=418.000us max=418.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_render_end_since_main count=100 p50=442.000us p95=527.000us p99=539.000us max=585.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_render_start_since_main count=100 p50=351.000us p95=430.000us p99=442.000us max=469.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_raw_mode_enabled_since_main count=100 p50=254.000us p95=312.000us p99=334.000us max=338.000us
workload=yach/tui_startup_profile_with_inactive_extension/tui_terminal_created_since_main count=100 p50=289.000us p95=346.000us p99=376.000us max=377.000us
```

## Interpretation

The inactive-extension mode reports the same startup trace mark families as the
baseline profile and keeps `tui_first_render_end_since_main` within the same
sub-millisecond p95 envelope.

The baseline `tui_first_render_end_since_main` p95 was `503us`; the installed
inactive extension p95 was `527us`. The delta is `+24us` (`+0.024ms`), which is
well below the 5ms investigation threshold.

No trace marker named `extension_host_spawned_before_first_render` appeared.
The expected manifest scan labels, `extension_manifest_scan_scheduled`,
`extension_manifest_scan_started`, and `extension_manifest_scan_finished`, are
not emitted because manifest discovery is not wired into the CLI yet. When that
discovery path is added, those labels should be emitted only after the TUI first
render marker and should not activate or spawn extension hosts.

## Limitations

- The harness sets up a valid inactive extension manifest directory, but the
  current CLI does not scan it.
- The PTY/process measurement includes process launch, loader work, harness
  overhead, terminal setup, polling granularity, and any local machine noise.
