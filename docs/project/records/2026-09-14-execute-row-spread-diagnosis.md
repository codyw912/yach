# `extension/execute/*` Spread: Investigation (2026-09-14)

**Outcome:** plane:YACH-8
**Follows:** `2026-09-14-perf-harness-noise-floor.md`

**Status: cause unresolved.** This record documents what was ruled out and
what was observed. It does not identify the mechanism.

## Question

The A/A noise-floor record identified `extension/execute/one_call` as the
widest-spread row (45.31% against a 48.0 budget) and proposed one cheap
experiment before any sampler change: each sample is one child process, so
does a higher `--samples` collapse the spread?

## Sample count is not implicated

A first sequential sweep (20, then 60, then 120) showed spread growing from
27% to 342% and appeared to implicate sample count. That sweep confounds
count with elapsed time, so it was repeated interleaved -- 20, 60, 20, 120,
60, 120 -- with CPU load sampled at 1 Hz during each run:

| samples | `one_call` base_spread, two runs each |
|---|---|
| 20 | 203.5% , 401.4% |
| 60 | 147.4% , 113.8% |
| 120 | 195.2% , 162.1% |

No monotonic effect. The two runs at n=20 are 198 percentage points apart at
the same sample count, so within-count variation exceeds any between-count
trend. The apparent trend in the sequential sweep was an ordering artifact.

## Contention is present but not shown sufficient

Load during the six interleaved runs, measured contemporaneously as cores
busy (summed per-process CPU-tick deltas, 1 Hz):

| run | samples | mean cores busy | max | `one_call` spread |
|---|---|---|---|---|
| 1 | 20 | 3.15 | 5.81 | 203.5% |
| 2 | 60 | 2.98 | 6.11 | 147.4% |
| 3 | 20 | 3.37 | 5.96 | 401.4% |
| 4 | 120 | 3.10 | 6.07 | 195.2% |
| 5 | 60 | 2.96 | 5.93 | 113.8% |
| 6 | 120 | 2.99 | 6.04 | 162.1% |

Roughly 3 of 6 visible cores were continuously busy with unrelated work, and
`/proc/loadavg` read 8.0-8.4 throughout. So the host is genuinely contended.

But load was steady across all six runs while spread swung 113.8% to 401.4%,
so steady contention does not by itself account for the run-to-run
variation. An earlier draft of this record claimed contention "fully
explains" the spread; that claim rested on a post-run `ps` snapshot, where
`%CPU` is a process-lifetime average and `ELAPSED` is process age. Neither
observes load during a given measurement slot. The claim is withdrawn.

## Ruled out: harness state across rounds

`CHILD_CACHE` (`core_loop.rs:290`) is a process-global memo keyed on
`(samples, kind)`, which looked able to make later rounds cheaper. It cannot:
`measure_slot` (`ab.rs:523-550`) calls `worker::spawn` for every slot, so
each round runs in a fresh process with an empty cache.

## The row is less stable than first recorded

All twelve row-observations in the interleaved experiment returned
`inconclusive`, including both n=20 runs. The four A/A runs in the
noise-floor record returned `unchanged` at the same sample count, so those
runs were not representative. `extension/execute/*` cannot currently be
relied on for a verdict on this host.

## Not done, and why

No budget changed, no sampler modified, no row removed. Each of those needs a
mechanism, and the mechanism is not established: contention is confirmed
present but not sufficient, and sample count is excluded.

The unrelated CPU-consuming processes were left running; they are not this
project's to manage.

## Next steps

1. **Re-measure with load recorded.** The 2026-09-13 extension figures,
   including the budget derivations, were taken without any load
   observation, so their dependence on host conditions is unknown in both
   magnitude and direction. A repeat run with contemporaneous load capture
   would settle it.
2. **Capture load in the result schema.** The schema records host
   fingerprint, cores, OS and kernel, but not load at run time. Without it a
   reader cannot distinguish a contended run from a quiet one -- the
   confusion this record had to resolve by hand.
3. **Look for a mechanism inside the row itself**, now that sample count is
   excluded: what varies between two n=20 runs under equal load. Per-sample
   raw intervals rather than per-round p95 would be the place to start.
