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

Ubicloud premium runners were off for every run (operator setting), so the
`ubicloud-*` labels ran on standard runners.

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
  which only a GitHub-hosted run on `main` refreshes. The follow-up change
  has to keep one of those runs, or forks build cold.
