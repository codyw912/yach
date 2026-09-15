# Extension Cost Attribution Measurement (2026-09-13)

**Outcome:** plane:YACH-8
**Spec:** `docs/project/specs/2026-09-13-extension-cost-attribution-design.md`

## Why

`baseline-2026-09-12.md` reported `turn/scripted/tools_4/hashline_ext` at
42.16 ms against `builtin_child` at 27.48 ms and declined to attribute the
difference. Yach intends to deliver most new functionality as extensions, so
a cost nobody can decompose cannot justify that plan.

## Environment

- **Date:** 2026-09-13
- **Machine:** AMD Ryzen 9 3900X, Linux 6.18.47, host fingerprint `a1eba7588fa53470`
- **Profile:** release, rustc 1.94.0
- **Command:** `cargo run --release -p yach-bench -- perf run --filter 'extension/*' --samples 30`
- **Host:** the extension host is the yach binary itself with
  `__extension-host hashline` (`main.rs:4116-4117`), not a Node runtime.
- **Host load: not recorded.** The result schema does not capture load, and
  no load observation was taken during these runs. Measurements the
  following day on the same host found roughly 3 of 6 visible cores
  continuously busy with unrelated work and `/proc/loadavg` at 8.0-8.4
  (`2026-09-14-execute-row-spread-diagnosis.md`). Whether these runs were
  similarly contended is unknown. Re-measuring on a host with load recorded
  would establish how much, and in which direction, these figures depend on
  host conditions.

## Measured

Paired per-sample intervals, n=30. Multi-call totals are summed within each
sample before summarising; no percentile is subtracted from another.

| id | p50 | p95 | p99 |
|---|---|---|---|
| `extension/activation/hashline_ext/spawn` | 253 µs | 305 µs | 373 µs |
| `extension/activation/hashline_ext/handshake` | 970 µs | 1.16 ms | 1.17 ms |
| `extension/activation/hashline_ext/total` | 1.24 ms | 1.40 ms | 1.44 ms |
| `extension/execute/hashline_ext/one_call` | 199 µs | 276 µs | 308 µs |
| `extension/execute/hashline_ext/tools_4_total` | 523 µs | 729 µs | 773 µs |
| `turn/phase/hashline_ext/tool_dispatched` | 302 µs | 404 µs | 427 µs |
| `turn/phase/hashline_ext/tool_result_appended` | 958 µs | 1.07 ms | 1.08 ms |

## What this supports

**Starting an extension costs ~1.24 ms, once per extension per session.**

The split between the two activation rows does **not** separate process
startup from protocol cost. `ExtensionProcessHostTransport::spawn`
(`extension.rs:408-429`) returns once `Command::spawn` has returned and the
stdout reader thread is running; it never waits for the child to be ready.
So:

- `spawn` (253 µs) is OS process creation plus fd and reader-thread setup.
- `handshake` (970 µs) is spawn-return to ready. It contains the child's
  remaining startup — a `yach __extension-host` process reaching its read
  loop — **and** the initialize/register exchange, with no boundary between
  them.

No conclusion about which of those dominates is supported by these rows.
Separating them needs a mark emitted by the child on entry to its read loop,
which is follow-up work.

**Executing an extension tool costs ~199 µs, and four cost ~523 µs.** The
`execute/*` rows bracket `execute_with_resources`, so they include registry
lookup, the permission check, the shared-invoker mutex wait, and argument
cloning — not only the host protocol call. That is deliberate: it is what a
turn actually pays.

For a 4-call turn, measured extension-attributable work is therefore roughly
1.8 ms: ~1.24 ms one-time plus ~0.52 ms of execution.

## What this does not support

**No residual figure is published.** The measured components do not partition
the wall-clock gap, and the gap is not stable enough to subtract from. Two
runs an hour apart on the same idle machine:

| run | `builtin_child` p50 | `hashline_ext` p50 | gap |
|---|---|---|---|
| A | 26.78 ms | 37.58 ms | 10.80 ms |
| B | 35.67 ms | 53.82 ms | 18.15 ms |

A 7 ms swing in an 11-18 ms difference means subtracting ~1.8 ms of
components yields a number with no meaning. A defensible residual needs a
matched per-sample control subtraction, which these rows do not provide.

Consequently **no extension-overhead figure is claimed**, and the earlier
retracted figure stays retracted. What is now available is the absolute cost
of the extension seams themselves.

## Gate treatment

These are child-process latency rows with `emit_alloc: false`, so
`worker.rs:122` excludes them from the deterministic CI gate entirely. They
belong to the paired A/B latency gate, which judges **p95**
(`ab.rs:892`).

Budgets derive from repeated p95, not p50. That distinction matters here:

| family | p50 spread | p95 spread | budget |
|---|---|---|---|
| `extension/activation/*` | 3.05-9.59% | 16.83-28.92% | 32.0 |
| `extension/execute/*` | 6.94-16.83% | 18.33-44.85% | 48.0 |

Deriving from p50 would have produced 13.0 and 20.0 and false-failed honest
changes on a statistic the gate does not judge.

**These budgets are too wide to gate meaningfully.** A 48% budget cannot
catch a regression smaller than roughly half the row's value. The rows are
trend evidence whose absolutes answer the design question. Tightening them
needs a less noisy sampler and is not claimed here.

A `round_trip_last` row measuring only the fourth call was built and dropped:
26.51% spread on p50, and a budget admitting that catches nothing real.

## Follow-up

- Reduce sampler noise so the extension rows can carry a budget that gates.
- A mark emitted by the extension host child when it reaches its read loop,
  so child startup can be separated from the initialize/register exchange
  inside the 970 us `handshake` row.
- A matched control subtraction, if a residual figure is wanted.
- Per-extension comparison: `TraceRecord.extension_id` now exists and is
  populated, but with one extension active the value is constant, so no
  comparison is delivered here.
