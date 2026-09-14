# Perf Measurement Tooling: Takeaways (2026-09-14)

**Outcome:** plane:YACH-8

Closes the measurement-tooling work that began when the deterministic gate
false-failed PR #270. Written to support a decision about the longer-term
setup, not to make one.

Preceding records, in order:
`2026-09-13-extension-cost-attribution-measurement.md`,
`2026-09-14-perf-harness-noise-floor.md`,
`2026-09-14-execute-row-spread-diagnosis.md`,
`2026-09-14-perf-host-pilot.md`.

## What was wrong

The deterministic CI gate failed a pull request on a row that change could
not reach: `startup/backend_ready_to_first_interactive_headless#alloc_count`
varied 417/416/414 on unmodified `main` against a budget of 0. Fixed in #271
by warming process-global state outside the allocation window, for all six
headless rows.

Separately, the paired latency gate's `extension/execute/*` rows returned
`inconclusive` repeatedly against an identical tree, and once `regressed`.

## What the gates actually are

Worth stating plainly, because conflating them caused several wrong turns:

| gate | classes | comparison | where it runs |
|---|---|---|---|
| `Perf (deterministic)` | size, count | exact against budget | shared CI, every PR |
| paired A/B (`just perf`) | latency, memory | median delta vs budget, p95 primary | manually, on a developer's machine |

Latency rows are excluded from the CI gate by `worker.rs:122`, so their
behaviour on shared runners has never been observed. The budget-relevant
statistic for latency is **p95** (`ab.rs:892`), and a high `base_spread`
turns a within-budget delta into `inconclusive` (`verdict.rs:96-101`).

## What is measured

A/A comparisons — working copy against its own base, so every delta is
harness noise by construction.

**Pilot-script runs**, with pressure and steal sampled at 1 Hz *during* each
run and raw artifacts retained:

| compute | runs | rows | observations | verdicts | worst spread | steal/run |
|---|---|---|---|---|---|---|
| dev machine, quiet | 2 | 2 | 4 | `unchanged` x4 | 39.6% | 42-48 |
| Hetzner CCX23 (4 dedicated vCPU) | 6 | 5 | 30 | `unchanged` x30 | 25.8% | 0 |
| shared CI runner | 0 | - | 0 | not measured | unknown | unknown |

**Earlier ad-hoc `just perf` runs** on the development machine, before the
pilot script existed. These lack the pilot's pressure and steal sampling and
its retained raw artifacts, and their row sets differ, so they are not
directly comparable with the table above:

- Four A/A comparisons of `extension/*` returned `unchanged` on twenty
  row-observations, with no load recorded
  (`2026-09-14-perf-harness-noise-floor.md`).
- A later sample-count sweep and interleaved experiment saw
  `extension/execute/*` return `inconclusive` on every observation, once
  `regressed`, with spread to 1566%
  (`2026-09-14-execute-row-spread-diagnosis.md`). The six interleaved runs
  **do** carry contemporaneous 1 Hz CPU load, measured as cores busy: about
  3 of 6 continuously busy, steady at 2.96-3.37 mean across all six while
  spread swung 113.8% to 401.4%. That per-run pairing is what shows
  contention present but not sufficient.

Observed headroom against current budgets (32% activation, 48% execute):

| compute | tightest headroom |
|---|---|
| CCX23 | +13.2 points (`activation/spawn`) |
| dev machine, quiet | +8.4 points (`execute/tools_4_total`) |

These are observed maxima over a handful of runs on one day, not a
distribution. They establish no detection floor and do not by themselves
determine whether a tighter budget is viable on either host.

## Takeaways

**1. Both tested hosts produced within-budget runs; no hardware class is
ruled out.** A quiet development machine and a dedicated cloud instance each
returned every A/A observation `unchanged` inside budget. Nothing measured
supports "this hardware cannot measure this".

Contention is a plausible contributor to the earlier instability, not an
established sole cause. `2026-09-14-execute-row-spread-diagnosis.md` found
contention present but **not sufficient**: load was steady across six
interleaved runs while spread still swung 113.8% to 401.4%, and sample count
was excluded. The quiet-versus-loaded contrast on one machine is suggestive;
the mechanism behind the residual run-to-run variation remains unidentified.

**2. CCX23 showed more margin than the development machine, on limited
data.** Every CCX23 row sat 13-29 points inside budget at zero steal ticks;
the quiet development machine's worst row sat 8.4 points inside. Both pass.
Whether that difference persists is unestablished: two runs versus six, one
day, and the row sets differ.

**3. Whether budgets could tighten is an open question, not a finding.**
The current 32% and 48% were derived from repeated p95 spread; load was not
recorded during that derivation, so its relationship to host conditions is
unknown in magnitude and direction. The noise-floor record is explicit that
observed headroom establishes no detection floor. Deciding a tighter budget
needs a deliberate re-derivation with load captured, on whichever host is
chosen — and separately, an injected-slowdown test, since A/A data says
nothing about detection sensitivity.

**4. Knowing a run's conditions decided more than choosing its host.** Two
corrections came directly from measuring load properly: a "contention fully
explains it" claim was withdrawn once `ps %CPU` was recognized as a lifetime
average, and the corrected per-run sampling then showed steady load against
swinging spread, which is what bounded the claim to "present but not
sufficient". A third gap persists for want of it: the 2026-09-13 absolutes
have no recorded load, so their relationship to host conditions stays
unknown. Not every error was a telemetry error — the sample-count hypothesis
fell to interleaving, an ordering control, not to load data. A side-car
collector was sufficient for the pilot; a schema field would make every
ordinary `perf run` self-describing without one.

**5. Orchestration and compute are independent.** GitHub Actions can
dispatch to shared runners or to ephemeral dedicated ones. "Use CI" does not
imply "use shared hardware", and the combination of CI scheduling with
dedicated compute is untried.

**6. Provisioning a measurement host is about fifteen minutes of known
steps, once.** Recorded in the pilot record: cloud-init `runcmd` has no
`$HOME`; root single-user Nix needs an empty `build-users-group`; a pinned
clone needs jj colocation for the clean-tree guard; pinned `main` has no
`scripts/`; keys added after boot are not retroactive. Cost was about
EUR 0.40 for the whole exercise.

**7. One repository blocker is worth fixing regardless.** `flake.nix`
pulled a private `nix-config` input solely so `devenv.nix` could import a
devcontainer module that sets no build inputs. Any clean host — CI, a
collaborator, a benchmark instance — cannot build the dev shell. The fix is
two deletions, verified on the pilot to leave the toolchain identical by
store path. The owner is removing it from the upstream template.

## The decision, and what each option costs

| option | orchestration | compute | status | main cost |
|---|---|---|---|---|
| A | Actions | shared runners | **unmeasured** | free; may not be able to measure latency at all |
| B | Actions | ephemeral dedicated runner | untried | per-run minutes plus runner setup |
| C | scripted | dedicated instance per run | measured: 30/30 `unchanged`, 13-29 pts margin | ~EUR 0.16/hour plus ~15 min bootstrap |
| D | manual | persistent dedicated machine | untried | standing cost, becomes infrastructure |
| E | manual | quiet developer machine | measured: 4/4 `unchanged`, 8.4 pts margin | free; needs a quiet machine and discipline |

**E is what the project already does, and it is defensible for now**: it
measured inside budget, and the tooling to detect a contended run now
exists. The case for C or B is not "E cannot work" — nothing measured says
that. It is that E depends on a human keeping the machine quiet and noticing
when it was not, and that the margin observed there was the narrower of the
two. Both matter more once a latency regression must be caught
automatically rather than investigated deliberately.

**A is the one cheap experiment left.** Unattended per-pull-request runs are
available from any Actions-orchestrated option, A or B; A is simply the one
that needs no new compute. A temporary probe workflow is written
and committed. If shared runners measure these rows within budget, A
dominates; if they do not, the choice is between E's discipline and C/B's
cost.

## Left in place

- `scripts/perf-host-pilot.sh` — runs unchanged on any host; A/A comparisons
  with per-second telemetry, retained raw artifacts, and guards against
  measuring a dirty tree or an external-mode comparison.
- `.github/workflows/perf-latency-probe.yml` — temporary; delete once A is
  recorded.
- `.perf/host-pilot-2026-09-14/` — raw evidence for both measured hosts
  (gitignored).

## Follow-up

- Run the shared-runner probe and record option A.
- Decide the measurement setup with that cell filled.
- Capture load, pressure and steal in the result schema.
- Land the flake decoupling.
- Extension-attribution follow-ups remain as recorded in
  `2026-09-13-extension-cost-attribution-measurement.md`.
