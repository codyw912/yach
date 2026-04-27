# Pi Comparison Prototype — 2026-04-27

## Summary

This report records the first methodology prototype for a clean same-machine Pi comparison. It is intentionally narrow and cautious: it measures clean Pi process invocation under a PTY until the first byte is emitted.

**Measurement class:** PTY harness / methodology prototype.

This is **not** an apples-to-apples yach-vs-Pi TUI responsiveness claim. It is a first check that we can invoke Pi with user extensions disabled and collect repeatable timing data from a pseudo-terminal.

## Environment

- **Date:** 2026-04-27
- **Yach commit SHA:** `e96ba5488d107a363ba53dfe2591fb8d5d4d7c56` plus uncommitted benchmark harness changes from this worktree
- **Pi version:** `0.70.2`
- **Pi binary:** `/Users/cody/.local/share/mise/installs/npm-mariozechner-pi-coding-agent/0.70.2/bin/pi`
- **Machine/environment:** Apple M2 Max, macOS 26.3.1 build 25D2128, Darwin 25.3.0 arm64
- **Command or harness:** `just dev cargo run -p yach-bench --release -- pi-clean-startup-report --samples 5`
- **Build/profile mode:** release
- **Sample count:** 5 initial Pi samples; 20-sample follow-up for Pi, yach CLI, yach full TUI, and yach synthetic-ready TUI first-output prototypes
- **Equivalence assessment:** `approximate` for Pi PTY first output vs yach TUI PTY first output; still a methodology prototype because first byte is not readiness

## Clean Pi invocation

The harness invokes Pi through macOS `script` to allocate a PTY and disable user-configured Pi surfaces:

```sh
script -q /dev/null \
  pi \
    --no-extensions \
    --no-skills \
    --no-prompt-templates \
    --no-themes \
    --no-context-files \
    --offline \
    --no-session
```

This follows `pi-comparison-methodology.md` by disabling extension discovery, skills, prompt templates, themes, context files, startup network operations, and session persistence.

## Timing boundaries

`t0` starts immediately before spawning the `script` process.

`t1` stops when the harness reads the first byte from `script` stdout.

Included:

- `script` process startup;
- PTY allocation;
- Pi process startup;
- enough Pi initialization to emit the first terminal byte.

Excluded / not measured:

- first complete frame;
- interactive prompt readiness;
- keyboard input latency;
- terminal emulator presentation timing;
- equivalent yach process startup.

## Result

Initial Pi prototype:

```text
samples_requested=5
samples_collected=5
workload=pi/clean_startup_first_output_pty count=5 p50=30.810ms p95=34.974ms p99=34.974ms max=34.974ms
```

20-sample follow-up:

```text
samples_requested=20
samples_collected=20
workload=pi/clean_startup_first_output_pty count=20 p50=44.181ms p95=98.281ms p99=104.374ms max=104.374ms
```

Asymmetric yach CLI first-output prototype:

```text
samples_requested=20
samples_collected=20
workload=yach/cli_startup_first_output count=20 p50=36.420ms p95=45.210ms p99=53.118ms max=53.118ms
```

Approximate yach full TUI PTY first-output prototype:

```text
samples_requested=20
samples_collected=20
workload=yach/tui_startup_first_output_pty count=20 p50=88.422ms p95=193.653ms p99=228.000ms max=228.000ms
```

Yach synthetic-ready TUI PTY first-output prototype, which bypasses real Pi RPC backend spawn/initialize and injects a ready backend state before running the TUI:

```text
samples_requested=20
samples_collected=20
workload=yach/tui_ready_startup_first_output_pty count=20 p50=34.952ms p95=42.749ms p99=54.897ms max=54.897ms
```

| Workload | Samples | p50 | p95 | p99 | max | Equivalence |
|---|---:|---:|---:|---:|---:|---|
| `pi/clean_startup_first_output_pty` | 5 | 30.810 ms | 34.974 ms | 34.974 ms | 34.974 ms | Pi-only PTY first byte |
| `pi/clean_startup_first_output_pty` | 20 | 44.181 ms | 98.281 ms | 104.374 ms | 104.374 ms | clean Pi PTY first byte |
| `yach/cli_startup_first_output` | 20 | 36.420 ms | 45.210 ms | 53.118 ms | 53.118 ms | yach CLI stdout first byte, not PTY/TUI |
| `yach/tui_startup_first_output_pty` | 20 | 88.422 ms | 193.653 ms | 228.000 ms | 228.000 ms | yach full TUI PTY first byte, includes backend spawn/initialize behavior |
| `yach/tui_ready_startup_first_output_pty` | 20 | 34.952 ms | 42.749 ms | 54.897 ms | 54.897 ms | yach synthetic-ready TUI PTY first byte, excludes backend spawn/initialize |

## Claim supported

This report supports only a methodology claim:

- A clean Pi PTY startup sampler exists and can collect first-output timing with extensions/skills/templates/themes/context files disabled.
- A yach CLI first-output sampler exists for process-first-output methodology experiments.
- A yach TUI PTY first-output sampler exists, giving an approximate PTY first-byte boundary for both Pi and yach TUI.
- A yach synthetic-ready TUI PTY first-output sampler exists, splitting post-ready TUI first-output cost from backend spawn/initialize behavior.

It does **not** support a product claim that yach is faster than Pi. First byte is not first stable frame or prompt readiness. The full yach TUI path is slower than clean Pi's first-byte result under this approximate boundary, while the synthetic-ready yach TUI path is faster than full yach TUI and near/under clean Pi on this first-byte boundary. That suggests a meaningful part of yach full TUI first-output cost is backend spawn/initialize, but this remains methodology signal only.

## Confidence and limitations

- **Confidence:** Good enough to show the harness is viable for clean Pi startup sampling.
- **Limitations:** First byte is not first complete frame or prompt readiness. All PTY first-byte paths include macOS `script` process overhead, but the internal startup work differs. Full yach TUI includes adapter/backend initialization behavior; synthetic-ready yach TUI bypasses it; clean Pi starts the interactive app directly with disabled extension surfaces. The yach CLI first-output sampler remains asymmetric and is listed only as a process-startup experiment.
- **Fairness risk:** Still high until yach and Pi use a shared readiness boundary such as first stable frame or ready prompt.

## Decision note

The full yach TUI PTY first-output result is intentionally not an optimization target for the Pi sidecar. The Pi sidecar is a transitional fast path for building and dogfooding the TUI; it is expected to add startup overhead and will be replaced by a Rust-native backend as soon as practical. Use these numbers to keep the transitional cost visible and to separate backend-ready TUI performance from sidecar startup cost, not to justify polishing the temporary sidecar path.

## Follow-up

1. Improve Pi and yach readiness detection from first byte to first stable prompt/frame.
2. Add explicit yach backend spawn/initialize timing only if needed to explain dogfood pain; do not optimize the Pi-sidecar startup path by default.
3. Prefer native-backend and stronger-equivalence workloads, such as idle PTY input-to-redraw or large transcript scroll/render, before any yach-vs-Pi claim.
