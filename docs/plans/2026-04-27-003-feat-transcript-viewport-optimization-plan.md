---
title: feat: Optimize transcript viewport rendering
type: feat
status: planned
date: 2026-04-27
origin: docs/project-os/performance-evidence.md
---

# feat: Optimize transcript viewport rendering

## Overview

Optimize yach's transcript rendering path so viewport changes avoid rebuilding wrapped lines for the full transcript on every render. The immediate trigger is measured performance evidence: a 50,000-entry live transcript scroll exceeds a 16 ms frame budget, while a 10,000-entry fixture remains healthy.

This plan keeps the work focused on the M2 TUI transcript surface. It does not change protocol behavior, Pi adapter behavior, or transcript storage semantics.

---

## Problem Frame

`crates/yach-ui/src/transcript.rs` currently renders by calling `render_lines(entries, width)` for every transcript render. That function iterates every `TranscriptEntry`, wraps each entry's full content, allocates a full `Vec<Line<'static>>`, then `skip`s and `take`s the visible viewport.

That is simple and correct for small transcripts, but it makes scroll/resize cost scale with total transcript size rather than visible work. Current evidence shows the scaling risk clearly:

| Workload | Evidence |
|---|---:|
| 10,000-entry live scroll | p95 4.435 ms |
| 50,000-entry live scroll | p95 22.962 ms |

The 50,000-entry path exceeds a 16 ms frame budget before richer transcript behavior, full output expansion, or real dogfood sessions increase pressure.

---

## Requirements Trace

- R1. Preserve existing transcript appearance for user, assistant, tool-call, and tool-result entries.
- R2. Preserve current scroll semantics: `scroll_offset` remains a rendered-line offset from the top of the wrapped transcript.
- R3. Avoid full-buffer line allocation on ordinary scroll renders after transcript content and width are unchanged.
- R4. Invalidate wrapping when transcript content changes, width changes, or transcript is replaced/cleared.
- R5. Keep `yach-ui` Pi-RPC-agnostic and preserve `yach-proto` boundaries.
- R6. Add regression tests for cache invalidation, line-count/max-scroll behavior, and visible output equivalence for representative entries.
- R7. Improve or at least not regress live benchmark evidence: 50,000-entry transcript scroll should return below a 16 ms p95 target on the measured machine, or the report should explain the remaining bottleneck.
- R8. Keep the benchmark evidence loop: update `docs/benchmarks/transcript-scroll-YYYY-MM-DD.md` and `docs/project-os/performance-evidence.md` after implementation measurements.

---

## Scope Boundaries

### In scope

- Transcript render cache or viewport renderer inside `crates/yach-ui`.
- Minimal app/layout plumbing needed to reuse cached wrapping across renders.
- Tests for rendering equivalence and invalidation.
- Benchmark/report updates after implementation.

### Out of scope

- Pi transcript/session fixture generation.
- Same-machine Pi comparison.
- Full-output expansion UI.
- Native backend work.
- Changing transcript data model across protocol boundaries.
- Changing user-facing scroll keybindings or adding new transcript navigation UX.

---

## Current Code Notes

Relevant current behavior:

- `layout::render` passes `entries`, `scroll_offset`, `is_streaming` into `transcript::render`.
- `transcript::render` calls `render_lines(entries, area.width)` every frame.
- `render_lines` wraps every entry, adds role/tool prefixes, inserts blank separator lines, and collects all rendered lines before viewport slicing.
- `rendered_line_count` and `max_scroll_start` also call `render_lines`, so line-count queries are full-buffer work too.
- `BenchmarkApp::render_to_terminal` clones all entries into a temporary `Vec<TranscriptEntry>` before rendering. That clone is also O(entries) and should be considered in optimization.

Important implication: optimizing only `transcript::render` may not fully fix live scroll if app/layout still clones the whole transcript for every frame.

---

## Proposed Design

Introduce a transcript viewport cache that stores wrapped entry output by width and transcript revision, then renders only the visible window without rebuilding the full line vector on each scroll.

A right-sized first implementation:

1. Add a revision counter to `Transcript`.
   - Increment on append, delta append, tool finish, clear, set/replace.
   - Expose `revision()` and `len()` for cache validation.
2. Add a `TranscriptRenderCache` in `crates/yach-ui::transcript`.
   - Stores the last width, transcript revision, per-entry rendered line metadata, and total rendered line count.
   - Rebuilds only when width or revision changes.
   - Provides `render_viewport(area, buf, entries, scroll_offset, is_streaming)`.
3. Change app/layout plumbing so the cache survives across renders.
   - Prefer storing `TranscriptRenderCache` on `App` and passing `&mut` through `RenderParams` or a narrow render context.
   - Avoid cloning every `TranscriptEntry` in `BenchmarkApp::render_to_terminal`; pass slices/references through the render path where possible.
4. Preserve pure helper functions for tests.
   - Keep simple `render_lines` or an equivalent test-only helper available for equivalence checks.
   - Keep `rendered_line_count`/`max_scroll_start` semantics stable, but route production calls through cached totals when possible.

### Cache shape

The initial cache can be line-oriented rather than fully virtualized:

```rust
struct TranscriptRenderCache {
    width: u16,
    revision: u64,
    entries_len: usize,
    total_lines: usize,
    entry_lines: Vec<CachedEntryLines>,
}

struct CachedEntryLines {
    start_line: usize,
    lines: Vec<Line<'static>>,
}
```

This still stores all wrapped lines after a rebuild, but ordinary scroll renders avoid rewrapping and reallocating. That should address the measured scroll bottleneck with lower implementation risk than a fully lazy visible-window renderer.

If memory becomes a problem, a follow-up can replace `Vec<Line<'static>>` with cheaper wrapped text spans or a true visible-window iterator.

---

## Alternatives Considered

| Option | Pros | Cons | Decision |
|---|---|---|---|
| Cache full wrapped lines by width/revision | Simple, preserves output, likely fixes scroll renders | Memory grows with rendered lines; content changes still rebuild all | Recommended first pass |
| Fully lazy visible-window rendering | Best asymptotic render cost and memory | More complex prefix-sum/indexing and harder edge-case correctness | Defer until cache evidence says it is needed |
| Cap transcript size | Simple performance bound | Loses history and changes product behavior | Reject for now |
| Optimize only `wrap_text` | Smaller change | Does not fix O(total transcript) render work | Insufficient |
| Defer until Pi comparison | More comparison evidence | Yach already has a measured self-contained scaling problem | Do not block optimization planning |

---

## Implementation Units

### U1. Add transcript revision and cache invalidation hooks

Files:

- `crates/yach-ui/src/transcript.rs`
- `crates/yach-ui/src/app.rs`

Tasks:

- Add a monotonically increasing revision to `Transcript`.
- Increment it for all mutating methods.
- Ensure `BenchmarkApp::set_transcript` and production transcript replacement paths preserve/invalidate revision correctly.
- Add unit tests for revision changes on append, delta append, finish tool, and clear.

### U2. Add cached wrapping data structure

Files:

- `crates/yach-ui/src/transcript.rs`

Tasks:

- Introduce `TranscriptRenderCache`.
- Move current entry-to-lines logic into reusable helpers.
- Rebuild cached wrapped lines when width or revision changes.
- Store total rendered line count for fast max-scroll calculations.
- Add tests that cached visible lines match uncached `render_lines` for representative user, assistant, tool-call, tool-result, multiline, and Unicode content.

### U3. Wire cache into layout/app rendering

Files:

- `crates/yach-ui/src/app.rs`
- `crates/yach-ui/src/layout.rs`
- `crates/yach-ui/src/transcript.rs`

Tasks:

- Store the cache where it survives normal render frames, likely on `App`.
- Pass a mutable cache through the render path without exposing it as a stable public API.
- Remove or reduce per-render `entries().to_vec()` cloning in `BenchmarkApp::render_to_terminal` if possible.
- Ensure scroll clamping still uses correct total rendered line count after cache rebuilds.

### U4. Add regression benchmark/report path

Files:

- `crates/yach-bench/src/main.rs`
- `docs/benchmarks/transcript-scroll-YYYY-MM-DD.md`
- `docs/project-os/performance-evidence.md`

Tasks:

- Re-run 10,000-entry and 50,000-entry live scroll reports after implementation.
- Keep old numbers visible as baseline evidence.
- Update project OS with before/after results and limitations.

---

## Acceptance Criteria

- Transcript visual output remains equivalent for representative entries at the same width and scroll offset.
- Ordinary scroll renders do not rebuild wrapped lines when transcript revision and width are unchanged.
- Width changes rebuild cached wrapping correctly.
- Transcript mutations invalidate cached wrapping correctly.
- Existing yach UI tests pass.
- `just dev cargo test -p yach-ui` and `just dev cargo test -p yach-bench` pass.
- Live 50,000-entry transcript scroll p95 improves below 16 ms on the same measurement class, or the implementation report explains the remaining bottleneck with evidence.

---

## Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Cache invalidation bugs cause stale transcript output | Use transcript revision tests and visible-output equivalence tests. |
| Mutable cache plumbing widens public UI API | Keep cache types internal to `yach-ui` unless tests/benchmarks need narrow unstable access. |
| Cached full lines increase memory for huge transcripts | Treat as first pass; measure memory if dogfood sessions show pressure. Defer lazy virtualization until needed. |
| Scroll semantics shift accidentally | Preserve rendered-line offset semantics and test max-scroll/viewport equivalence. |
| App-level cloning remains the bottleneck | Include removal/reduction of `entries().to_vec()` in scope and measure after each step. |
| Streaming deltas invalidate too often | Accept full rebuild on content change in first pass; optimize incremental append/delta later only if active-stream evidence requires it. |

---

## Verification Plan

Automated:

```sh
just dev cargo test -p yach-ui
just dev cargo test -p yach-bench
```

Live measurement, from a PTY-capable shell:

```sh
script -q /dev/null \
  just dev cargo run -p yach-bench --release -- \
    terminal-transcript-scroll-report --samples 200

script -q /dev/null \
  just dev cargo run -p yach-bench --release -- \
    terminal-transcript-scroll-stress-report --samples 50
```

Compare against baseline:

- 10,000-entry p95: 4.435 ms
- 50,000-entry p95: 22.962 ms

---

## Project OS Updates After Implementation

- Update `docs/benchmarks/transcript-scroll-YYYY-MM-DD.md` or add a new dated before/after report.
- Update `docs/project-os/performance-evidence.md` with measured before/after results.
- Update `docs/project-os/next-work.md` when the optimization pass is implemented or if it uncovers a blocker.
- If the implementation changes a durable rendering invariant or public API seam, update `docs/project-os/architecture-invariants.md` and `docs/project-os/decisions.md`.

---

## Open Questions

- Should the first implementation cache full `Line<'static>` objects or a cheaper intermediate representation? Recommendation: start with full lines for correctness and speed of delivery.
- Should streaming deltas support incremental cache updates? Recommendation: defer until active-stream evidence shows rebuild cost during streaming is material.
- Should 50,000-entry support be a hard target for M2 dogfooding? Current evidence says it is valuable as a stress tier; product priority should decide whether it becomes a release gate.
