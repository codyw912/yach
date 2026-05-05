# Current Performance Baseline — 2026-05-05

## Summary

This report refreshes yach-only performance evidence before the next native backend implementation chunk. It covers current headless replay, live Crossterm draw/flush proxies, transcript scroll, and synthetic-ready TUI PTY first-output.

**Measurement classes:** headless component proxy, live terminal draw/flush proxy, PTY first-output proxy.

This is not a same-machine Pi comparison and does not measure real provider/network latency.

## Environment

- **Date:** 2026-05-05
- **Commit SHA:** `1b29f60`
- **Machine/environment:** Apple M2 Max class machine, macOS 26.3.1 build 25D2128, Darwin 25.3.0 arm64
- **Build/profile mode:** release for benchmark binary
- **Provider delay setting:** `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` was not set in the benchmark shell.

## Provider delay note

`YACH_NATIVE_PROVIDER_TEST_DELAY_MS` is an opt-in native-provider runtime test hook. It defaults to `0` and is only read by the native-provider TUI path. The `yach-bench` commands below use synthetic/headless/live-terminal benchmark harnesses, not real native-provider calls, so the delay hook should not affect these measurements. It should remain unset for benchmark runs unless a future benchmark explicitly measures native-provider cancellation behavior.

## Commands

```bash
just dev cargo run -p yach-bench --release -- headless-report --samples 1000
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-report --samples 500
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-keypress-report --samples 500
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-active-stream-report --samples 500
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-async-backlog-stress-report --samples 500
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-heavy-output-report --samples 500
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-transcript-scroll-report --samples 200
script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-transcript-scroll-stress-report --samples 50
script -q /dev/null just dev cargo run -p yach-bench --release -- yach-tui-ready-startup-report --samples 100
```

## Results

### Headless replay/component proxy

```text
samples=1000
workload=startup/backend_ready_to_first_interactive_headless count=1000 p50=234.500us p95=238.834us p99=319.209us max=567.375us
workload=keypress/idle_keypress_to_paint_headless count=1000 p50=232.875us p95=238.916us p99=538.042us max=1.288ms
workload=keypress/active_stream_replay_headless/100 count=1000 p50=11.881ms p95=13.888ms p99=15.277ms max=18.697ms
workload=replay/heavy_tool_output_tail_headless/102400 count=1000 p50=389.417us p95=520.708us p99=957.126us max=1.477ms
workload=paste/large_multiline_component/102400 count=1000 p50=3.242ms p95=3.967ms p99=5.301ms max=7.312ms
workload=viewport/huge_transcript_scroll_headless/10000 count=1000 p50=2.389ms p95=3.056ms p99=3.974ms max=6.143ms
```

### Live terminal draw/flush proxies

```text
samples=500
workload=terminal/startup_ready_keypress_draw_flush_live count=500 p50=140.917us p95=397.792us p99=1.195ms max=2.695ms

samples=500
workload=terminal/idle_keypress_to_draw_flush_live count=500 p50=78.375us p95=182.084us p99=710.375us max=2.034ms

samples=500
workload=terminal/active_stream_keypress_to_draw_flush_live count=500 p50=80.583us p95=137.250us p99=647.834us max=3.757ms

samples=500
workload=terminal/async_backlog_stress_keypress_to_draw_flush_live count=500 p50=119.541us p95=298.875us p99=522.084us max=1.575ms
async_backlog_profile=stress events_per_burst=50 producer_sleep_us=100 events_sent=25000 drained=25000 max_drained_per_sample=700

samples=500
workload=terminal/heavy_output_keypress_to_draw_flush_live count=500 p50=90.583us p95=156.250us p99=299.875us max=1.760ms
```

### Transcript scroll

```text
samples=200
transcript_entries=10000 scroll_lines_per_sample=1
workload=terminal/large_transcript_scroll_to_draw_flush_live count=200 p50=58.250us p95=149.500us p99=327.000us max=678.167us

samples=50
transcript_entries=50000 scroll_lines_per_sample=1
workload=terminal/huge_transcript_scroll_to_draw_flush_live count=50 p50=48.500us p95=636.166us p99=1.110ms max=1.110ms
```

### PTY startup proxy

```text
samples_requested=100
samples_collected=100
workload=yach/tui_ready_startup_first_output_pty count=100 p50=46.396ms p95=55.868ms p99=61.898ms max=63.115ms
```

## Claim supported

This report supports a narrow yach-only current baseline claim:

- Current headless and live-terminal synthetic TUI paths remain well below the PRD latency guardrails on this machine.
- Warm-cache transcript scroll remains below a 16 ms frame budget for the measured 10k/50k transcript workloads.
- Synthetic PTY first-output startup remains below 100 ms p99 on this machine.

## Caveats

- Live terminal commands emit alternate-screen control sequences; result lines above were collected from command tails.
- Measurements are local synthetic proxies and should not be interpreted as real provider streaming latency.
- The provider delay test hook was left unset rather than removed because it is not active in these benchmark harnesses.
