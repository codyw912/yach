# Extension Cost Attribution Design

**Outcome:** plane:YACH-8

Status: proposed
Date: 2026-09-13

## Problem

Yach intends to deliver most new functionality as extensions. That plan is
only defensible if the cost of running an extension can be explained, because
a number nobody can decompose cannot justify a design choice.

Today it cannot be. `baseline-2026-09-12.md` reports:

| workload | p50 |
|---|---|
| `turn/scripted/tools_4/builtin_child` | 27.48 ms |
| `turn/scripted/tools_4/hashline_ext` | 42.16 ms |

A 14.68 ms difference on a 4-tool-call turn, and the baseline explicitly
declines to attribute it: "unattributed between extension-host round-trip,
activation, and tool-path differences." Both rows are wall-clock child
process measurements with no interior.

That opacity blocks the actual decision. One host process is started per
extension per session (`extension.rs:1151`, one `shared_invoker` reused by
every registered tool), so the cost splits into a one-time part and a
per-call part:

- If 14.68 ms is mostly session startup, it amortises over a long session and
  an extension-heavy design is fine.
- If it is mostly per-call round trip, cost scales with tool use and a chatty
  extension is disqualifying on a hot path.

These imply opposite decisions. One wall-clock number cannot distinguish
them, so no extension tradeoff can currently be argued from evidence.

The gap is also a measurement blocker, not only a product one: the harness
has no phase decomposition for the extension child at all. `provider_phase!`
and `tools_phase!` (`core_loop.rs:28-68`) both target
`CachedChildKind::TextOnly` or `Tools4Builtin`; `hashline_ext_child`
(`:450`) records `run.wall` only.

## Goal

Decompose extension cost into independently measured one-time and per-call
components, so a reader can state where an extension's time goes and decide
whether a feature belongs in the core or an extension.

Non-goals, deliberately excluded:

- Optimising whatever the decomposition reveals. This slice measures.
- Per-extension comparison ("extension A versus extension B"). The schema
  gains the field that unblocks it; the comparison matrix is separate work.
- Live `terminal/*` TTY rows, still skipped for lack of a TTY.

## Seams

Four real boundaries in the current code carry extension cost:

| seam | location | lifetime |
|---|---|---|
| host process spawn | `ExtensionProcessHostTransport::spawn`, `extension.rs:1192` | one-time per extension per session |
| initialize + tool registration | `session.initialize_and_register`, `extension.rs:1203` | one-time per extension per session |
| tool invocation round trip | `invoker.invoke`, `tools.rs:1835` | per call |
| result handling | `ExtensionHostInvocation::ToolResult` match, `tools.rs:1845` | per call |

Both activation call sites (`activate_background_metadata_extensions` at
`extension.rs:1148` and the single-record path at `:860`) funnel through
`activate_extension_host_record`, so instrumenting that one function covers
both.

Result handling is bookkeeping between two marks that already bracket it; it
gets no separate mark until evidence says it matters.

## Decision

### Activation is startup-scoped, not turn-scoped

Extension activation completes before the first prompt. `prompt_received` is
marked at `runner.rs:2037`, while `extension_manifest_scan_scheduled` fires
during session setup (`extension_state.rs:60`). `phase_offset`
(`core_loop.rs:589-600`) computes offsets from `prompt_received` with
`saturating_sub`, so any pre-prompt mark measured that way clamps to zero.

Activation marks therefore use `TraceScope::Startup` and are read by
`trace_labels_since_main` (`startup.rs:160`), which measures from
`process_main_start` — the existing pattern for `extension_manifest_scan_*`.

Two new startup labels, emitted in `activate_extension_host_record`:

- `extension_host_spawned` — after `ExtensionProcessHostTransport::spawn`
  returns, isolating process creation.
- `extension_host_ready` — after `initialize_and_register` returns,
  isolating the handshake and tool registration.

The sink reaches that code as its own parameter on both activation entry
points, not as a config field; see Risks for why the config stays a `Copy`
policy value. Activation runs inside `tokio::task::spawn_blocking`
(`extension_state.rs:153`); `TraceSink` is `Clone` over `Arc<Mutex<Inner>>`
with a shared `start: Instant`, so a clone crosses that boundary while
keeping one time origin.

### Round trip needs new paired marks

The existing `tool_dispatched(n)` mark cannot bound an invocation. All marks
are emitted in one loop at `runner.rs:8338-8345` **before** the execution
loop at `:8346`, so `tool_dispatched(2)` precedes call 1. A
`tool_dispatched(n)` to `tool_result_appended(n)` interval spans every
earlier call in the batch and is not a round trip.

Two new turn-scoped labels bracket the host call in the `Host` route of
`ExtensionToolHandler::execute` (`tools.rs:1828-1844`):

- `extension_invoke_start`, `n` = tool index
- `extension_invoke_end`, `n` = tool index

These are the authoritative round-trip boundary. The interval is computed per
sample and per `n`; percentiles are taken over those intervals. Percentiles
are never subtracted from each other.

### A new interval primitive

`phase_offset` returns cumulative offsets and cannot express a duration. A
sibling helper in `core_loop.rs` pairs marks:

```
fn mark_interval(records, start_label, end_label, n) -> Result<Duration, String>
```

It locates both marks for the given `n` within one sample, errors when either
is missing or when `end` precedes `start`, and returns the difference. An
unpaired mark is an error, never a zero — a silently-zero row is the class of
defect that produced two PRs of false CI failures in the measurement
framework already.

### `extension_id` on trace records

`TraceRecord` gains `extension_id: Option<String>` with `#[serde(default,
skip_serializing_if = "Option::is_none")]`, plus `mark_ext` /
`mark_ext_n` constructors that populate it. Existing marks are unchanged and
existing serialised traces still parse.

This is added now because it is the field the baseline names as the blocker,
and because adding it later would invalidate traces recorded in between. It
is *not* used to build per-extension comparison in this slice: with one
extension active the value is constant. The field makes the comparison
possible later; it does not deliver it.

### New workload rows

| id | class | family | measures |
|---|---|---|---|
| `startup/phase/extension_host_spawned` | latency | startup | process spawn, one-time |
| `startup/phase/extension_host_ready` | latency | startup | handshake + registration, one-time |
| `extension/invoke/hashline_ext/round_trip` | latency | interval | one host call, 4 per turn |
| `turn/phase/tool_dispatched` and `tool_result_appended` over the hashline child | latency | turn | existing labels against the extension child |

The last row requires `CachedChildKind::Tools4Hashline`, so the existing
phase machinery can target the extension child. The startup rows require the
hashline extension to be active, so they follow the `HOME`-override pattern
`hashline_ext_child` already uses (`core_loop.rs:454-457`).

### Residual is reported, not hidden

The startup offsets and round-trip totals do not sum to the 14.68 ms
wall-clock difference. Child process wall time includes work outside every
marked seam: manifest scanning, the extra process itself, scheduling, and
teardown.

The baseline will therefore publish the measured components **and** the
unexplained residual as an explicit figure, rather than implying a complete
partition. A decomposition that claims to account for everything it does not
measure is the same defect as the retracted extension-overhead number this
work exists to replace.

## Gate treatment

These are child-process **latency** rows with `emit_alloc: false`. In
deterministic mode `worker.rs:122` sets `wants_row = false` for them
(`!deterministic || Size | Count`), and `wants_alloc` is false because
`emit_alloc` is false, so `:127` skips them entirely. **They never enter the
deterministic CI gate.**

They land in the paired A/B latency gate via `just perf`, where they need
evidence-derived `latency_pct` budgets. The first A/B reports `added`, which
is that one comparison's verdict and not a standing exemption; every
subsequent change judges them against a budget.

Budgets are derived from measured round spread using the existing convention
(`ceil(spread) + 3`, as recorded for `yach/*_first_output_pty` at 16.0 and
`yach/cli_startup_first_output` at 6.0) and the derivation is written into the
thresholds `comment` field. Budgets are set from observation, never guessed
ahead of measurement.

## Testing

- `mark_interval` unit tests: paired marks yield the difference; a missing
  start, a missing end, and a reversed pair each error rather than return
  zero; the correct `n` is selected when several pairs exist.
- Trace schema: a record without `extension_id` still deserialises; a record
  with one round-trips.
- Mark emission: the activation path emits both startup labels in order on
  success and neither `ready` label when spawn fails.
- Workload registration: the new ids appear in the registry, and the
  deterministic planner excludes them (asserting the gate treatment above,
  which is the claim most likely to silently regress).

Existing `perf::report` tests that assert on workload id lists will need the
new ids added.

## Risks

- **Trace volume.** Four extra marks per turn plus two per session. Marks are
  free when tracing is unset (`from_env` returns `None`, callers hold
  `Option<TraceSink>`), so the shipping path is unaffected.
- **New rows destabilising `just perf`.** The mitigation is deriving budgets
  from observed spread before proposing them, and the rows are outside the
  deterministic gate, so a noisy row cannot false-fail an unrelated PR the
  way `#alloc_count` did.
- **`ExtensionBackgroundActivationConfig` gains a non-`Copy` field.** It
  currently derives `Debug, Clone, Copy, PartialEq, Eq` (`extension.rs:954`).
  Adding `Option<TraceSink>` drops `Copy` and `Eq`, touching every
  construction site. The alternative — a separate trace parameter threaded
  through both activation entry points — avoids the derive change at the cost
  of a wider signature. Decision: pass the sink as its own parameter and
  leave the config a plain `Copy` value object, because the config describes
  policy and a trace sink is not policy.
