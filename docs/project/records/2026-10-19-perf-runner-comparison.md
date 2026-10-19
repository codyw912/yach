# Perf Runner Comparison (2026-10-19)

Kata: yach#k7g4

Question: which runner should `Perf (deterministic)` use for pushes to
`main` and same-repository pull requests? This note records the timing
evidence. The default-runner change is a separate follow-up that cites it.
Fork pull requests stay on `ubuntu-latest` whatever is chosen.

## Method

Every run is a `workflow_dispatch` of `ci.yml` on `main` (#320's inputs), so
only `Perf (deterministic)` measures. All runs used one pair:

- head `2f35f5ac1734390fac4ddacb4ee16c84a40bb5c3` (#324);
- base `3e86b1379d40a971f8c6b3272b2e500da8512663` (#322, its parent).

The head must contain #324, because the job runs
`.github/scripts/instruction_counts.sh` from the checked-out head and that
step is `continue-on-error`. An older head would pass while skipping the
step, and would time a shorter job.

The runs were interleaved in rounds (ubuntu-latest, ubicloud-standard-4,
ubicloud-standard-8, dispatched together), three rounds in all. Each round
started only after the previous one finished. A run counted only if all of
the following held:

- every step of `Perf (deterministic)` ran and succeeded, with the
  `Instruction counts` step's own conclusion checked as well as the job's;
- rust-cache logged `Restored from cache key "…" full match: true`, the same
  fact the step summary prints as "rust-cache exact hit"
  (`steps.cache.outputs.cache-hit`);
- the uploaded artifact contained a non-empty `ab.json`.

All nine counted runs met these conditions, so no replacement runs were
needed.

- Queue time: job `created_at` → `started_at`.
- Job time: job `started_at` → `completed_at`.
- CPU and core count: taken from each `ab.json` (`current[0].host`).

Ubicloud premium runners were off for the runs above (operator setting), so
the `ubicloud-*` labels ran on standard runners. A second set with premium on
follows under "Premium round".

## Warm-up (not counted)

| run | runner | CPU | queue | job | cache |
|---|---|---|---:|---:|---|
| [39261060616](https://github.com/codyw912/yach/actions/runs/39261060616) | ubuntu-latest | EPYC 7763 | 3 s | 420 s | exact hit |
| [39261088300](https://github.com/codyw912/yach/actions/runs/39261088300) | ubicloud-standard-4 | EPYC 9454P | 28 s | 600 s | none found, saved |

The two providers use separate caches. Ubicloud's transparent cache serves
`actions/cache` and `Swatinem/rust-cache` from its own storage, not from
GitHub's ([Ubicloud Cache](https://www.ubicloud.com/docs/github-actions-integration/ubicloud-cache)),
and the rust-cache key differs by provider as well. The environment hash is
`ea977744` on GitHub's runners and `b3412203` on Ubicloud's
(`v0-rust-perf-perf-deterministic-Linux-x64-<hash>-598b834d`). The
`ubuntu-latest` warm-up was already an exact hit, on the cache `main`'s push
run for `2f35f5ac` (run 39195148857) had saved. The Ubicloud cache started
empty, so the standard-4 warm-up built cold (600 s) and saved the Ubicloud
entry. Both Ubicloud sizes then hit that one entry: the key does not include
the vCPU count.

## Counted runs

| round | run | runner | CPU (cores) | queue | job | artifact |
|---|---|---|---|---:|---:|---|
| 1 | [39263027419](https://github.com/codyw912/yach/actions/runs/39263027419) | ubuntu-latest | EPYC 9V45 (4) | 3 s | 290 s | `perf-ab-ubuntu-latest-39263027419-1` |
| 1 | [39263031642](https://github.com/codyw912/yach/actions/runs/39263031642) | ubicloud-standard-4 | EPYC 9224 (4) | 23 s | 351 s | `perf-ab-ubicloud-standard-4-39263031642-1` |
| 1 | [39263035152](https://github.com/codyw912/yach/actions/runs/39263035152) | ubicloud-standard-8 | EPYC 9454P (8) | 23 s | 255 s | `perf-ab-ubicloud-standard-8-39263035152-1` |
| 2 | [39265487099](https://github.com/codyw912/yach/actions/runs/39265487099) | ubuntu-latest | EPYC 7763 (4) | 4 s | 424 s | `perf-ab-ubuntu-latest-39265487099-1` |
| 2 | [39265491248](https://github.com/codyw912/yach/actions/runs/39265491248) | ubicloud-standard-4 | EPYC 9454P (4) | 85 s | 368 s | `perf-ab-ubicloud-standard-4-39265491248-1` |
| 2 | [39265496837](https://github.com/codyw912/yach/actions/runs/39265496837) | ubicloud-standard-8 | EPYC 9454P (8) | 59 s | 291 s | `perf-ab-ubicloud-standard-8-39265496837-1` |
| 3 | [39266915715](https://github.com/codyw912/yach/actions/runs/39266915715) | ubuntu-latest | EPYC 9V74 (4) | 3 s | 372 s | `perf-ab-ubuntu-latest-39266915715-1` |
| 3 | [39266919107](https://github.com/codyw912/yach/actions/runs/39266919107) | ubicloud-standard-4 | EPYC 9454P (4) | 24 s | 289 s | `perf-ab-ubicloud-standard-4-39266919107-1` |
| 3 | [39266922240](https://github.com/codyw912/yach/actions/runs/39266922240) | ubicloud-standard-8 | EPYC 9454P (8) | 22 s | 230 s | `perf-ab-ubicloud-standard-8-39266922240-1` |

Every counted run had an exact cache hit, an `ab.json` for base `3e86b137`
and head `2f35f5ac`, and instruction counts of +0.00% on all four rows.

## Verdicts

All eleven comparison documents (the nine counted runs and both warm-ups)
have the same 29 verdicts: 28 `count` rows `unchanged` and
`binary/size_bytes` `unchanged`. The deterministic gate gives one answer on
every runner.

## Medians

| runner | job, median (range) | queue, median (range) | price/min | cost per run at median |
|---|---|---|---:|---:|
| ubuntu-latest | 372 s, 6.2 min (290–424 s) | 3 s (3–4 s) | $0 | $0 |
| ubicloud-standard-4 | 351 s, 5.85 min (289–368 s) | 24 s (23–85 s) | $0.0025 | $0.015 (6 billed min) |
| ubicloud-standard-8 | 255 s, 4.25 min (230–291 s) | 23 s (22–59 s) | $0.005 | $0.025 (5 billed min) |

Queue time is reported separately from job time. A pull request waits for
both, but only job time is billed.

Prices:

- `ubuntu-latest` is a standard GitHub-hosted runner, which is free on a
  public repository
  ([GitHub Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)).
- Ubicloud standard x64 rates are $0.0025/min for 4 vCPU and $0.005/min for
  8 vCPU ([Ubicloud pricing](https://www.ubicloud.com/docs/about/pricing)).
- "Billed min" rounds the median up to whole minutes. Ubicloud documents
  per-minute granularity; whether it rounds a partial minute up is
  [INFERENCE]. The exact-time cost is $0.0146 for standard-4 and $0.0213 for
  standard-8.
- Ubicloud's monthly $2.50 account credit is not netted out here.

## Observations

- `ubuntu-latest` landed on three different CPU models in three runs, and
  its job time followed the CPU: 290 s on EPYC 9V45, 372 s on EPYC 9V74,
  424 s on EPYC 7763. The warm-up, also on an EPYC 7763, took 420 s. Its
  spread (134 s) is the widest of the three runners. Queue time was
  consistently 3–4 s.
- `ubicloud-standard-4` ran on EPYC 9454P twice and on EPYC 9224 once. Its
  median job time is 21 s below `ubuntu-latest`'s; its median queue time is
  21 s above.
- `ubicloud-standard-8` was fastest in every round: 117 s below
  `ubuntu-latest`'s median, and 96 s below standard-4's. All three of its
  runs were on EPYC 9454P. Twice the vCPUs did not halve the job: a cached
  run is part compile and part fixed cost (checkout, apt, Valgrind, cache
  restore, the measurement).
- Both Ubicloud sizes queued longer in round 2 (85 s and 59 s) than in
  rounds 1 and 3 (22–24 s). Three rounds are not enough to say how often
  that happens.
- Switching the default changes which cache pull requests restore. Pushes to
  `main` save the `perf` cache that pull requests read. If they move to a
  Ubicloud label, that cache is the Ubicloud one (`b3412203`). Fork pull
  requests stay on `ubuntu-latest` and read GitHub's cache (`ea977744`),
  which only a GitHub-hosted run on `main` refreshes.

## Premium round

Same pair, same counting rules, with Ubicloud premium runners switched on
(operator setting, 2026-10-19). Premium is an account setting, not a label:
the same `ubicloud-standard-4/8` labels go to the premium fleet, AMD Ryzen 9
7950X3D, and fall back to standard runners when it is busy
([Use Premium Runners](https://www.ubicloud.com/docs/github-actions-integration/use-premium-runners)).
A Ubicloud run counted as premium only if its `ab.json` named the Ryzen CPU;
every standard run above was on an EPYC. All Ubicloud runs here were on the
Ryzen, so none fell back and no replacement runs were needed.

The rounds were not serialized this time: rounds 1 and 2 were dispatched 30 s
apart, and round 3 started while round 2's `ubuntu-latest` run was still
going. Each job had its own VM. The two slowest Ubicloud runs (437 s and
296 s) came from the overlapping rounds 1 and 2.

Warm-up (not counted):
[39270982140](https://github.com/codyw912/yach/actions/runs/39270982140),
ubicloud-standard-4, Ryzen 9 7950X3D, 21 s queue, 297 s job, exact cache hit.
Premium runners restored the same Ubicloud entry (`b3412203`) the standard
runs had saved.

| round | run | runner | CPU (cores) | queue | job | artifact |
|---|---|---|---|---:|---:|---|
| 1 | [39272426229](https://github.com/codyw912/yach/actions/runs/39272426229) | ubuntu-latest | EPYC 7763 (4) | 3 s | 416 s | `perf-ab-ubuntu-latest-39272426229-1` |
| 1 | [39272431356](https://github.com/codyw912/yach/actions/runs/39272431356) | ubicloud-standard-4 | Ryzen 9 7950X3D (4) | 24 s | 437 s | `perf-ab-ubicloud-standard-4-39272431356-1` |
| 1 | [39272437878](https://github.com/codyw912/yach/actions/runs/39272437878) | ubicloud-standard-8 | Ryzen 9 7950X3D (8) | 20 s | 183 s | `perf-ab-ubicloud-standard-8-39272437878-1` |
| 2 | [39272500036](https://github.com/codyw912/yach/actions/runs/39272500036) | ubuntu-latest | EPYC 7763 (4) | 3 s | 427 s | `perf-ab-ubuntu-latest-39272500036-1` |
| 2 | [39272507496](https://github.com/codyw912/yach/actions/runs/39272507496) | ubicloud-standard-4 | Ryzen 9 7950X3D (4) | 19 s | 241 s | `perf-ab-ubicloud-standard-4-39272507496-1` |
| 2 | [39272516014](https://github.com/codyw912/yach/actions/runs/39272516014) | ubicloud-standard-8 | Ryzen 9 7950X3D (8) | 25 s | 296 s | `perf-ab-ubicloud-standard-8-39272516014-1` |
| 3 | [39273262627](https://github.com/codyw912/yach/actions/runs/39273262627) | ubuntu-latest | EPYC 7763 (4) | 3 s | 439 s | `perf-ab-ubuntu-latest-39273262627-1` |
| 3 | [39273267033](https://github.com/codyw912/yach/actions/runs/39273267033) | ubicloud-standard-4 | Ryzen 9 7950X3D (4) | 23 s | 349 s | `perf-ab-ubicloud-standard-4-39273267033-1` |
| 3 | [39273270981](https://github.com/codyw912/yach/actions/runs/39273270981) | ubicloud-standard-8 | Ryzen 9 7950X3D (8) | 17 s | 182 s | `perf-ab-ubicloud-standard-8-39273270981-1` |

Every counted run had an exact cache hit, an `ab.json` for base `3e86b137`
and head `2f35f5ac`, a successful `Instruction counts` step, and instruction
counts of +0.00% on all four rows. All ten documents (nine counted, one
warm-up) have the same 29 `unchanged` verdicts as the standard runs.

| runner | job, median (range) | queue, median (range) | price/min | cost per run at median |
|---|---|---|---:|---:|
| ubuntu-latest | 427 s (416–439 s) | 3 s (3 s) | $0 | $0 |
| ubicloud-standard-4, premium | 349 s (241–437 s) | 23 s (19–24 s) | $0.004 | $0.024 (6 billed min) |
| ubicloud-standard-8, premium | 183 s (182–296 s) | 20 s (17–25 s) | $0.008 | $0.032 (4 billed min) |

Premium x64 rates are from
[Ubicloud pricing](https://www.ubicloud.com/docs/about/pricing); billed
minutes are rounded up as above ([INFERENCE]). Exact-time costs are $0.0233
and $0.0244.

- `ubicloud-standard-8` premium was fastest in every round, at a median of
  183 s: 72 s below standard-8 (255 s) and 244 s below this set's
  `ubuntu-latest` median.
- `ubicloud-standard-4` premium had the widest spread (196 s) and cost about
  as much per run as standard-8 premium.
- `ubuntu-latest` landed on EPYC 7763 all three times here, and its median
  (427 s) matches the 7763 runs in the first set (420 s and 424 s).

## Decision (owner, 2026-10-19)

`Perf (deterministic)` runs on `ubicloud-standard-8` for pushes to `main` and
same-repository pull requests, with premium on. Fork pull requests stay on
`ubuntu-latest`.

Volume: from 2026-09-19 to 2026-10-19, `ci.yml` ran 126 times on
`pull_request` (81) and `push` (45) events, besides 21 dispatches. At
$0.032 per run, perf on every one of those runs is about $4 a month before
Ubicloud's $2.50 credit. This is an upper bound: docs-only pull requests
skip the job.

Forks (owner, 2026-10-19): no GitHub-hosted run on `main` saves GitHub's
`perf` cache any more, so once its last entry goes 7 days unused and
expires, a fork pull request's perf job builds cold (about 20 minutes).
Keeping it warm would take a second, GitHub-hosted perf run of about 7
minutes on every push to `main`. Few fork pull requests are expected, so the
cold build is accepted.

Fork pull requests and the paid runner: the `ubuntu-latest` routing for forks
is not a security boundary. A `pull_request` run uses the workflow file from
the pull request's merge commit, so a fork can edit `ci.yml` and name
`ubicloud-standard-8` itself
([GitHub: securely using `pull_request_target`](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target)).
What keeps outside code off the paid runner is the repository's fork-run
approval policy, "Require approval for all external contributors"
(`all_external_contributors`): a maintainer reviews a fork's workflow changes
before any run. The owner changed it from `first_time_contributors`, which
let a contributor's later pull requests run unapproved, on 2026-10-19.
