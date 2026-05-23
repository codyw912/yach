# Extension Runtime Profile - 2026-05-23

## Summary

This report records release-mode startup and metadata-runtime evidence for the
first conservative extension runtime slice.

The key result: installed extension manifest discovery stays off the first TUI
render path. With one installed inactive extension and with 50 installed
manifests, `extension_manifest_scan_started` was recorded after
`tui_first_render_end` in all 100 samples. No extension host is spawned before
first render.

## Environment

- Date: 2026-05-23
- Worktree branch: `extension-runtime-startup-profile`
- Build/profile mode: release benchmark and release `yach-cli`
- Machine: local macOS development machine

The benchmark harness launches `yach-cli` under `script` to provide a PTY. The
PTY measurement includes process launch, loader work, terminal setup, harness
overhead, trace flush visibility, and polling granularity. The
`*_since_main` timings are Rust-side trace marks from `main`.

## Commands

```bash
just dev cargo build -p yach-cli -p yach-bench --release
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-with-inactive-extension-report --samples 100
just dev cargo run -p yach-bench --release -- yach-tui-startup-profile-many-extensions-report --samples 100
just dev cargo run -p yach-bench --release -- extension-runtime-profile-report --samples 100
```

## Results

### Baseline Native TUI Startup

```text
samples_requested=100
samples_collected=100
workload=yach/tui_startup_profile/observed_process_to_first_render_pty count=100 p50=25.193ms p95=28.790ms p99=31.243ms max=35.712ms
workload=yach/tui_startup_profile/tui_first_render_end_since_main count=100 p50=519.000us p95=644.000us p99=657.000us max=4.997ms
workload=yach/tui_startup_profile/native_backend_task_spawned_since_main count=100 p50=279.000us p95=334.000us p99=384.000us max=3.499ms
```

Baseline waits only for first render. Background scan marks can appear
opportunistically before the benchmark kills the process, but they are not the
baseline terminal condition.

### One Installed Inactive Extension

```text
samples_requested=100
samples_collected=100
workload=yach/tui_startup_profile_with_inactive_extension/observed_process_to_first_render_pty count=100 p50=25.190ms p95=28.224ms p99=29.559ms max=31.122ms
workload=yach/tui_startup_profile_with_inactive_extension/tui_first_render_end_since_main count=100 p50=501.000us p95=602.000us p99=629.000us max=659.000us
workload=yach/tui_startup_profile_with_inactive_extension/extension_manifest_scan_scheduled_since_main count=100 p50=1.452ms p95=1.623ms p99=1.863ms max=3.928ms
workload=yach/tui_startup_profile_with_inactive_extension/extension_manifest_scan_started_since_main count=100 p50=1.677ms p95=1.859ms p99=2.080ms max=4.255ms
workload=yach/tui_startup_profile_with_inactive_extension/extension_manifest_scan_finished_since_main count=100 p50=1.976ms p95=2.202ms p99=2.407ms max=4.552ms
manifest_count_per_sample=1
extension_host_spawned_before_first_render=false
extension_manifest_scan_started_before_first_render_count=0
workload=yach/tui_startup_profile_with_inactive_extension/extension_manifest_scan_duration count=100 p50=298.000us p95=344.000us p99=387.000us max=547.000us
workload=yach/tui_startup_profile_with_inactive_extension/extension_manifest_scan_start_after_first_render_delta count=100 p50=1.151ms p95=1.333ms p99=1.554ms max=3.797ms
```

### Fifty Installed Manifests

```text
samples_requested=100
samples_collected=100
workload=yach/tui_startup_profile_many_extensions/observed_process_to_first_render_pty count=100 p50=28.526ms p95=31.601ms p99=33.235ms max=33.312ms
workload=yach/tui_startup_profile_many_extensions/tui_first_render_end_since_main count=100 p50=548.000us p95=636.000us p99=696.000us max=882.000us
workload=yach/tui_startup_profile_many_extensions/extension_manifest_scan_scheduled_since_main count=100 p50=1.499ms p95=1.671ms p99=2.396ms max=3.057ms
workload=yach/tui_startup_profile_many_extensions/extension_manifest_scan_started_since_main count=100 p50=1.738ms p95=1.949ms p99=2.659ms max=3.277ms
workload=yach/tui_startup_profile_many_extensions/extension_manifest_scan_finished_since_main count=100 p50=4.179ms p95=4.615ms p99=5.692ms max=6.237ms
manifest_count_per_sample=50
extension_host_spawned_before_first_render=false
extension_manifest_scan_started_before_first_render_count=0
workload=yach/tui_startup_profile_many_extensions/extension_manifest_scan_duration count=100 p50=2.425ms p95=2.736ms p99=3.177ms max=3.578ms
workload=yach/tui_startup_profile_many_extensions/extension_manifest_scan_start_after_first_render_delta count=100 p50=1.182ms p95=1.377ms p99=1.777ms max=2.764ms
```

### Metadata Host Activation And Invocation

```text
samples_requested=100
samples_collected=100
workload=extension_runtime/metadata_host_activation count=100 p50=208ns p95=292ns p99=1.542us max=54.750us
workload=extension_runtime/metadata_tool_invocation_round_trip count=100 p50=416ns p95=1.459us p99=7.042us max=23.459us
```

This mode profiles the in-process fake transport used by
`ExtensionHostSession::initialize_and_register` and `invoke_tool`; it measures
the protocol/session overhead boundary, not OS process spawn or JavaScript/Rust
host startup.

## Interpretation

The extension runtime path preserves the first-frame invariant for installed
inactive extensions. On this run, Rust-side `tui_first_render_end_since_main`
p95 was `644us` for baseline, `602us` with one installed inactive extension,
and `636us` with 50 installed manifests. The many-manifest run completed the
background scan at `4.615ms` p95 after Rust `main`, with scan start at least
`1.377ms` p95 after first render.

The one-manifest scan duration was `344us` p95. The 50-manifest package scan
duration was `2.736ms` p95. Both runs reported zero scan starts before first
render and no host spawn before first render.

During implementation, the startup benchmark harness was corrected to keep the
child PTY stdin open while sampling. Closing stdin immediately could race EOF
against the post-first-render event path and make the benchmark miss extension
scan markers even though the app path was valid.

## Limitations

- The activation profile uses a fake in-memory transport; process spawn,
  TypeScript runtime startup, Rust extension binary startup, and stdio pipe I/O
  still need separate measurement once real host launch lands.
- The PTY/process measurement is useful for local trend tracking, but internal
  trace marks are the stronger evidence for first-render path placement.
- The benchmark package roots are synthetic fixtures, not installed extension
  packages from a real package manager or Git checkout.
