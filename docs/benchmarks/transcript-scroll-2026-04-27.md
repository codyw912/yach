# Live Transcript Scroll Baseline — 2026-04-27

## Summary

This report records a live Crossterm terminal draw/flush measurement for scrolling a large yach transcript fixture.

**Measurement class:** live terminal.

This is yach-only evidence, not a same-machine Pi comparison. It strengthens the huge-transcript evidence surface before a stronger-equivalence Pi comparison is attempted.

## Environment

- **Date:** 2026-04-27
- **Commit SHA:** `501ce87` plus uncommitted transcript-scroll harness changes from this worktree
- **Machine/environment:** Apple M2 Max, macOS 26.3.1 build 25D2128, Darwin 25.3.0 arm64
- **Command or harness:** `script -q /dev/null just dev cargo run -p yach-bench --release -- terminal-transcript-scroll-report --samples 200`
- **Build/profile mode:** release
- **Sample count:** 200

## Timing boundaries

Before timed samples, the harness:

1. enters raw mode and alternate screen;
2. creates a live `CrosstermBackend` terminal;
3. creates `BenchmarkApp`;
4. applies synthetic connected and backend-ready state;
5. installs a deterministic 10,000-entry transcript fixture;
6. performs one initial live terminal render.

Each timed sample starts immediately before incrementing the transcript scroll offset by one line and stops after rendering/flushing the updated app state to the live Crossterm terminal.

Included:

- scroll-state mutation;
- transcript viewport rendering through the production layout path;
- ratatui rendering through `CrosstermBackend`;
- terminal draw/flush work performed by the backend.

Excluded:

- OS keyboard event delivery;
- crossterm event polling latency;
- real session file loading;
- Pi comparison behavior;
- terminal emulator presentation timing beyond the backend draw/flush boundary.

## Result

```text
samples=200
transcript_entries=10000 scroll_lines_per_sample=1
workload=terminal/large_transcript_scroll_to_draw_flush_live count=200 p50=3.688ms p95=4.435ms p99=4.966ms max=5.556ms
```

| Workload | Samples | Fixture / scale | p50 | p95 | p99 | max |
|---|---:|---|---:|---:|---:|---:|
| `terminal/large_transcript_scroll_to_draw_flush_live` | 200 | 10,000-entry transcript, one-line scroll per sample | 3.688 ms | 4.435 ms | 4.966 ms | 5.556 ms |

## Claim supported

This report supports a narrow live-terminal yach claim:

- On this machine and terminal environment, a 10,000-entry transcript fixture scrolls and redraws well under a 16 ms frame budget for this 200-sample run.

## Confidence and limitations

- **Confidence:** Useful first live-terminal evidence for yach large-transcript scrolling.
- **Limitations:** Yach-only. It does not prove bounded visible-work rendering and does not compare against Pi. The fixture is synthetic and the timing boundary excludes OS input delivery/event polling.
- **Interpretation:** This live 10,000-entry scroll path looks healthier than the earlier headless viewport proxy warning, but the current transcript rendering path should still be treated as scaling-risk until larger stress tiers or code inspection prove visible-work bounds.

## 50,000-entry stress follow-up

A second live-terminal sampler measured a 50,000-entry transcript fixture with the same one-line scroll timing boundary.

```sh
script -q /dev/null \
  just dev cargo run -p yach-bench --release -- \
    terminal-transcript-scroll-stress-report --samples 50
```

```text
samples=50
transcript_entries=50000 scroll_lines_per_sample=1
workload=terminal/huge_transcript_scroll_to_draw_flush_live count=50 p50=19.963ms p95=22.962ms p99=23.895ms max=23.895ms
```

| Workload | Samples | Fixture / scale | p50 | p95 | p99 | max |
|---|---:|---|---:|---:|---:|---:|
| `terminal/huge_transcript_scroll_to_draw_flush_live` | 50 | 50,000-entry transcript, one-line scroll per sample | 19.963 ms | 22.962 ms | 23.895 ms | 23.895 ms |

This stress result confirms the scaling-risk signal: the 50,000-entry live scroll path exceeds a 16 ms frame budget at p50/p95 on this machine. It should be treated as evidence for planning transcript viewport/render optimization, not as a product-comparison claim.

## Follow-up

1. Build a same-machine Pi large-transcript scroll comparison with equivalent fixture/session dimensions.
2. Plan transcript viewport/render optimization separately if 50,000-entry sessions are in near-term dogfood scope.
3. If optimizing, retain both 10,000-entry and 50,000-entry live scroll reports as regression fixtures.
