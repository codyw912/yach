# Transcript Scroll Cache Optimization — 2026-04-27

## Summary

This report records the before/after evidence for caching wrapped transcript lines across ordinary scroll renders. The optimization keeps a `TranscriptRenderCache` alive across render frames and invalidates it when transcript content or viewport width changes.

**Measurement class:** live terminal.

This is yach-only evidence. It does not compare against Pi and still excludes OS input delivery and crossterm event polling.

## Environment

- **Date:** 2026-04-27
- **Commit SHA:** implementation worktree on `feat/transcript-viewport-cache` after `3e6e15e`
- **Machine/environment:** Apple M2 Max, macOS 26.3.1 build 25D2128, Darwin 25.3.0 arm64
- **Build/profile mode:** release
- **Timing boundary:** one-line scroll state change through live Crossterm terminal draw/flush

## Commands

```sh
script -q /dev/null \
  just dev cargo run -p yach-bench --release -- \
    terminal-transcript-scroll-report --samples 200

script -q /dev/null \
  just dev cargo run -p yach-bench --release -- \
    terminal-transcript-scroll-stress-report --samples 50
```

## Results

| Workload | Baseline p95 | Optimized p95 | Baseline max | Optimized max |
|---|---:|---:|---:|---:|
| 10,000-entry live scroll | 4.435 ms | 152.041 µs | 5.556 ms | 2.483 ms |
| 50,000-entry live scroll | 22.962 ms | 151.667 µs | 23.895 ms | 226.333 µs |

Raw optimized output:

```text
samples=200
transcript_entries=10000 scroll_lines_per_sample=1
workload=terminal/large_transcript_scroll_to_draw_flush_live count=200 p50=62.000us p95=152.041us p99=400.000us max=2.483ms
```

```text
samples=50
transcript_entries=50000 scroll_lines_per_sample=1
workload=terminal/huge_transcript_scroll_to_draw_flush_live count=50 p50=69.792us p95=151.667us p99=226.333us max=226.333us
```

## Claim supported

On this machine and timing boundary, ordinary transcript scroll renders no longer scale with total transcript size after the wrapped-line cache is warm. The 50,000-entry live scroll p95 improved from 22.962 ms to 151.667 µs and is now below a 16 ms frame budget.

## Confidence and limitations

- **Confidence:** Strong for the measured yach scroll path. The same harness and fixture sizes from the baseline report were reused.
- **Limitations:** Yach-only. The first render or any render after transcript content/width changes still rebuilds the cache. The measurement excludes OS input delivery, crossterm event polling, and terminal emulator presentation beyond backend draw/flush.
- **Follow-up:** Measure active streaming with frequent cache invalidation if real dogfood sessions show stream-render pressure. Same-machine Pi large-transcript comparison remains separate evidence work.
