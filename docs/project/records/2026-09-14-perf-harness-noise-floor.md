# Perf Harness Noise Floor: A/A Self-Comparison (2026-09-14)

**Outcome:** plane:YACH-8

## Why

Two open questions needed the same evidence. The extension rows landed with
`latency_pct` budgets of 32.0 and 48.0 derived from repeated p95 spread, wide
enough that the threshold comments call them trend evidence rather than
gates. Separately, `baseline-2026-09-10.md` recorded "post-merge worker-mode
self-comparison (`just perf` with `@ == main`)" as outstanding: the paired
gate had never been run against an identical tree, so its own noise was
unmeasured.

## Method

`just perf --filter 'extension/*' --samples 20`, four independent
invocations, on a working copy verified identical to `main@origin`
(`jj diff --from main@origin --to @ --stat` reported 0 files changed). Every
run reported `base-mode: worker`, so both sides ran the real worker rather
than the external shipping-binary fallback.

Because the trees are identical, every delta is harness noise by
construction.

Result documents were written to `.perf/results/20260914T100249-ab.json`,
`20260914T101018-ab.json`, `20260914T101040-ab.json` and
`20260914T101101-ab.json`. `.perf/` is gitignored, so those files are local
to the machine that produced them and are not reachable from this
repository; the per-run figures below are the durable record. Regenerate with
the command above on an identical tree.

## Observed

| row | worst abs delta | worst base_spread | verdicts |
|---|---|---|---|
| `extension/activation/hashline_ext/spawn` | 14.04% | 24.34% | unchanged x4 |
| `extension/activation/hashline_ext/handshake` | 5.07% | 24.87% | unchanged x4 |
| `extension/activation/hashline_ext/total` | 7.40% | 19.01% | unchanged x4 |
| `extension/execute/hashline_ext/one_call` | 2.09% | 45.31% | unchanged x4 |
| `extension/execute/hashline_ext/tools_4_total` | 11.56% | 39.35% | unchanged x4 |

Per run, as `median_delta_pct` / `sign_agreement` / `base_spread_pct`:

| row | run 1 | run 2 | run 3 | run 4 |
|---|---|---|---|---|
| `activation/spawn` | -14.04 / 0.80 / 22.73 | +2.80 / 0.60 / 24.34 | +6.55 / 0.60 / 22.70 | +2.53 / 0.60 / 15.68 |
| `activation/handshake` | -5.07 / 0.80 / 8.57 | +3.40 / 0.60 / 5.71 | +4.97 / 0.80 / 7.27 | +3.12 / 0.80 / 24.87 |
| `activation/total` | -7.21 / 0.80 / 12.27 | +0.20 / 0.60 / 18.76 | +3.02 / 1.00 / 7.84 | +7.40 / 0.60 / 19.01 |
| `execute/one_call` | +1.06 / 0.60 / 45.31 | -2.09 / 0.60 / 42.86 | -1.26 / 0.60 / 23.34 | -1.36 / 0.60 / 38.64 |
| `execute/tools_4_total` | +3.88 / 0.60 / 28.44 | +11.08 / 0.60 / 39.35 | +7.72 / 0.60 / 36.20 | +11.56 / 1.00 / 17.64 |

Sign agreement sits at 0.60 in most observations. Nothing follows from that
about detection sensitivity: with no true effect present, round-to-round
signs are expected to be mixed, and a real slowdown can flip every round the
same way and reach 1.00. Establishing sensitivity needs an injected known
slowdown or an effect-size analysis, neither of which this record attempts.

`spawn` swung -14.04% on the first run and +2.53% to +6.55% on the other
three, so that figure is one observation and not a characteristic value.

## What this supports

**The gate does not false-fail on an unchanged tree.** All twenty
row-observations returned `unchanged`. That is the property the
self-comparison existed to check, and it holds.

**High spread can turn a within-budget delta into `inconclusive`.**
`judge_latency` (`verdict.rs:96-101`) evaluates the classification arms in
order: a delta over threshold with at least 0.8 sign agreement is
`regressed` or `improved` first, *regardless* of spread. Spread only decides
the outcome for a delta within threshold, where `spread > threshold` yields
`inconclusive` instead of `unchanged`. Since these A/A deltas are all within
budget, spread is what governs their verdicts, and `execute/one_call`
reached 45.31% against its 48.0 budget.

**The noisiest rows are in the `execute` family, not `activation`.**
`execute/one_call` has the widest spread (45.31%) and
`execute/tools_4_total` the widest positive delta (+11.56%). The widest
delta in absolute terms is `activation/spawn`'s single -14.04% observation.

## What this does not support

**No detection floor is established.** Four runs bound nothing about future
noise; these are observed maxima on one machine on one day, not a
distribution. No claim is made that a tighter budget would false-fail, only
that `one_call` currently has roughly 3 percentage points of spread headroom.

**No budget change is justified by this record.** The budgets were derived
from repeated p95 spread and are consistent with what these runs show. They
are not re-derived here.

## Consequence for the noise follow-up

The target is `execute/*` spread, specifically `one_call`, not `spawn` as
previously assumed.

One hypothesis worth testing before touching the sampler: these rows take one
interval per child process, so sample count translates directly into child
spawns and their scheduling variance. Whether a higher `--samples` collapses
the spread is a cheap experiment and should precede any sampler change.

## Follow-up

- Test whether higher sample counts reduce `execute/*` spread.
- Re-run this A/A comparison on a quiet machine and on CI hardware; a single
  host does not characterise the gate.
- The remaining extension-attribution follow-ups are unchanged and recorded
  in `2026-09-13-extension-cost-attribution-measurement.md`.
