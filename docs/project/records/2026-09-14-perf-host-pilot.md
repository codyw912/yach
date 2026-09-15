# Perf Measurement Host Pilot (2026-09-14)

**Outcome:** plane:YACH-8
**Follows:** `2026-09-14-perf-harness-noise-floor.md`,
`2026-09-14-execute-row-spread-diagnosis.md`

## Question

`extension/execute/*` had returned `inconclusive` on twelve consecutive A/A
comparisons against an identical tree, and once `regressed`, with p50 steady
and p95 swinging an order of magnitude. Sample count was excluded as a cause.
Host CPU contention was confirmed present on the development machine but not
shown sufficient.

The open question: does running the same A/A comparison on a dedicated-vCPU
cloud host produce spreads that fit inside the row budgets?

## Method

`scripts/perf-host-pilot.sh`, run unchanged on both hosts: N A/A comparisons
(working copy against its own base, so every delta is harness noise by
construction), with CPU pressure, memory pressure and steal time sampled at
1 Hz *during* each run. Each run writes an explicit output path, requires
`base_mode == worker`, and retains its raw A/B document, telemetry and log.

| host | label | cores | CPU | kernel | virt |
|---|---|---|---|---|---|
| development machine | `final` | 6 | AMD Ryzen 9 3900X | 6.18.47 | kvm |
| Hetzner CCX23 | `hetzner-ccx23` | 4 dedicated | AMD EPYC-Milan | 6.12.107-cloud | kvm |

The hosts differ in CPU model, core count and kernel, so this is **not** a
controlled comparison of contention. It is an operational question about
whether a dedicated instance is a usable measurement host.

Raw evidence for both hosts -- per-run A/B documents with build provenance
and raw `samples_ns`, plus per-second telemetry -- is retained locally under
`.perf/host-pilot-2026-09-14/` (gitignored; see its README). The CCX23
instance was destroyed and its access key revoked after collection, so those
copies are the only remaining data from that host. Every figure needed to
read this record is reproduced below, so the conclusions do not depend on
them surviving.

## Results

Both corrected runs, all verdicts `unchanged`:

| row | CCX23 spread min/med/max | CCX23 p50 | dev machine spread | dev p50 |
|---|---|---|---|---|
| `activation/spawn` | 8/10/19% | 159 us | not in that run | - |
| `activation/handshake` | 5/8/10% | 912 us | not in that run | - |
| `activation/total` | 4/9/12% | 1074 us | not in that run | - |
| `execute/one_call` | 5/14/26% | 186 us | 9/12/15% | 210 us |
| `execute/tools_4_total` | 2/11/19% | 448 us | 11/26/40% | 524 us |

| host | runs | observations | verdicts | steal/run | CPU PSI some |
|---|---|---|---|---|---|
| CCX23 | 6 | 30 | `unchanged` x30 | **0** | 0.31-0.68 |
| dev machine | 2 | 4 | `unchanged` x4 | 42-48 | 0.06-0.07 |

Toolchain equivalence was verified by store path, not version string: `rustc`
and `cargo` resolve to
`zryc7mk7irfwaiji24abvf9icvqiazvw-rust-stable-1.94.0-1.94.0` on both hosts,
and `pkg-config` to `1nv3i8mpypy3d516f4pd95m0w72r73jy-pkg-config-wrapper-0.29.2`.
Same input hashes mean the same derivation.

## What this supports

**CCX23 was consistently within budget across six runs.** Thirty
row-observations, every one `unchanged`, worst spread 26% against budgets of
32% and 48%. Zero steal ticks in every run. It is a usable measurement host.

**A quiet development machine is also within budget.** The corrected local
run returned `unchanged` on all four observations with 9-15% spread on
`one_call`. The earlier spreads of 113-1566% came from contended windows, not
from the hardware being unsuitable.

## What this does not support

**No attribution of the p50 difference.** CCX23's lower p50s
(`tools_4_total` 448 vs 524 us) cannot be credited to reduced contention: the
hosts differ in CPU model, core count, kernel version, and the CCX23 tree
carries the environment patch below. Any of those could account for it.

**No controlled improvement factor.** Comparing CCX23's best spreads against
the development machine's *contended* extremes would overstate the effect by
comparing different questions. Against the corrected local run the two hosts
are broadly comparable, with CCX23 showing zero steal and a wider margin on
`tools_4_total`.

**The earlier absolutes are not established as inflated.** Load was never
recorded during the 2026-09-13 extension measurement, so its relationship to
host conditions remains unknown in magnitude and direction.

## Environment deviation

The CCX23 tree carries one pilot-local commit: the flake pulled
`git+ssh://git@github.com/codyw912/nix-config.git` solely so `devenv.nix`
could import `devcontainer-sandbox.nix`, which a host without that private
access cannot fetch, making the dev shell unbuildable from a clean clone.

That module sets `devcontainer.settings`, convenience packages
(`bat bun direnv eza fd fzf gh git jq nodejs_22 ripgrep tmux zoxide lazygit
vim`), two helper scripts, and an `enterShell` that exports `BUN_INSTALL` and
prepends `/home/vscode/.bun/bin` to `PATH`. It contributes no build inputs:
the Rust toolchain comes from `devenv.nix` `languages.rust` via
`rust-overlay`.

Removing the import and the input let the dev shell build from the public
cache. Both A/A sides build the same patched source, and the store-path check
above confirms the toolchain is unchanged. The owner intends to remove the
devcontainer module from the upstream template that generates these files.

## Provisioning notes

Recorded because two failures cost real time and will recur:

- cloud-init `runcmd` has no `$HOME`, so the Nix installer aborts with
  `$HOME is not set`.
- Single-user Nix as root fails on a missing `nixbld` group; `/etc/nix/nix.conf`
  needs `build-users-group =` (empty) before installing.
- A `git clone` of a pinned commit gives jj no `main`. The pilot's clean-tree
  guard needs a colocated repo (`jj git init --colocate`) and a bookmark at
  the base revision.
- Pinned `main` has no `scripts/` directory; it must be created before the
  pilot script is copied in.
- A key added to the cloud account after a server boots is not applied
  retroactively.

Cost: about EUR 0.35 for the whole exercise at EUR 0.1626/hour.

## Open question: where perf measurement should run

This pilot answers "can a dedicated cloud host measure this" (yes). It does
not settle where measurement should live.

**Orchestration and compute are separate choices.** GitHub Actions is a
scheduler: it can run a job on its own shared runners, or dispatch the same
job to an ephemeral dedicated or self-hosted runner. "Use CI" and "use
shared hardware" are not the same decision, and conflating them would rule
out the combination that looks most promising.

| option | orchestration | compute | status |
|---|---|---|---|
| A | GitHub Actions | shared runners | latency rows are excluded there today (`worker.rs:122`); their behaviour on that hardware is **unmeasured** |
| B | GitHub Actions | ephemeral dedicated/self-hosted runner | untried; combines existing wiring with the compute this pilot exercised |
| C | manual/scripted | dedicated instance per run | demonstrated here |
| D | manual | persistent dedicated machine | untried |
| E | manual | development machine, load captured | demonstrated here (corrected run within budget) |

What is measured: C and E both produced all-`unchanged` A/A runs within
budget. Everything else below is a hypothesis to test, not a finding:

- Whether shared CI runners would show the tail instability this
  investigation chased is **untested**. It is plausible given what contention
  did on the development machine, but the deterministic job excludes latency
  rows, so no evidence exists either way. Running the pilot script on a
  shared runner would settle it cheaply.
- Whether a persistent machine has lower variance than a per-run instance is
  **untested**. It avoids per-run bootstrap; whether that changes measured
  spread is unknown.
- Option B's provisioning latency, cost model and result retention are all
  unexamined.

A decision needs an owner preference on cost and cadence, so it is not made
here.

**No schema change is a prerequisite.** The side-car collector in
`scripts/perf-host-pilot.sh` supplied contemporaneous pressure and steal data
for this pilot without touching the result format. Capturing load in the
schema would make every ordinary `perf run` self-describing, which is
worthwhile, but any of the options above can proceed before it.

## Follow-up

- Run the pilot script on a shared GitHub Actions runner. This is the
  cheapest outstanding experiment and it settles option A, which is
  currently argued from plausibility rather than evidence.
- Decide the measurement host strategy above, once A is measured.
- Capture load, pressure and steal in the result schema so an ordinary
  `perf run` is self-describing. Not a prerequisite for any option: the
  side-car collector already covers a deliberate pilot.
- Land the flake decoupling upstream so a clean clone can build the dev
  shell.
- The extension-attribution follow-ups remain as recorded in
  `2026-09-13-extension-cost-attribution-measurement.md`.
