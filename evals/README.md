# Eval portfolio

Harness-regression tasks for yach, authored as Harbor-format task
directories so one asset set serves three runners: the local gate
(`just eval-gate`), the provider-rotation matrix (`just rotate`), and
a yacht custom-eval course for cross-harness comparison. Design:
`docs/project/specs/2026-07-28-eval-portfolio-design.md`.

Founding principle: **verifiers assert on artifacts, not utterances** —
file state, command output, and outcome-document fields. A verifier
never greps the assistant's response prose; if a behavior only shows
up in prose, surface it in the outcome document first.

## Task layout

```
evals/tasks/<task-id>/
├── task.toml               # metadata + timeouts (Harbor's schema)
├── instruction.md          # the prompt the agent receives
├── run.sh                  # optional: custom invocation sequence
├── fixture/                # workspace seed (source of truth)
├── environment/Dockerfile  # task container for the yacht/harbor path
├── solution/solve.sh       # oracle solution (mandatory)
└── tests/test.sh           # verifier: writes reward 1 or 0
```

Task directories are self-contained on purpose — yacht pins them by
content digest, so shared fixtures are copied per task, not
referenced.

## The image is the binary under test

Every runner executes `yach` **inside the `yach-runtime` container**, so
an eval measures whatever that image was built from — not the working
tree. Run `just runtime-image` after any code change.

This fails silently and convincingly: the run completes, cells score,
and the numbers look like a result. It produced one "the fix didn't
work" conclusion that was really a stale image, so it is checked rather
than left to discipline. `just runtime-image` stamps a content digest of
the crate sources and workspace manifests into the image
(`evals/scripts/source-digest.sh`); the gate and the sweep recompute it
and refuse to start on a mismatch, naming the rebuild command. An image
built before the stamp existed carries no digest and is treated as
stale, which is the correct reading of it.

## Runner contract

- The runner seeds a scratch workspace from `fixture/`, runs the
  agent, then invokes `tests/test.sh`.
- Default invocation is single-shot:
  `yach run --full-auto --model <pinned> --prompt "$(cat instruction.md)"`
  with the outcome document written to `.yach-eval/outcome.json` in
  the workspace. A task that needs anything else (multiple
  invocations, a different approval posture) supplies `run.sh`,
  executed with the workspace as cwd and the model id in
  `$YACH_EVAL_MODEL`; it must leave its final outcome document at
  `.yach-eval/outcome.json`.
- `solution/solve.sh` runs with the workspace as cwd and must produce
  every artifact the verifier asserts on — including a plausible
  `.yach-eval/outcome.json` when the verifier reads outcome fields.

## Verifier contract

- Bash, asserting only on paths under `$EVAL_WORKSPACE` (default
  `/app`, Harbor's workdir) and `$EVAL_WORKSPACE/.yach-eval/`.
- Reward goes to `${EVAL_LOGS_DIR:-/logs}/verifier/reward.txt` — the
  unset-default is Harbor's fixed path; local runners point
  `EVAL_LOGS_DIR` at a scratch dir.
- Exit 0 whether the reward is 1 or 0; a nonzero exit means the
  verifier itself broke. Needs only bash, coreutils, and `jq`.
- The agent's session artifacts (`.yach/`, `.yach-eval/`) are runner
  plumbing: workspace-integrity checks must exclude them.

`just eval-validate` runs every task's oracle against its verifier
(no model calls, no secrets, no containers) — a verifier that rejects
its own oracle is broken, and this catches it before a model run.

## Release evidence policy

Normal releases use the deterministic project checks, `just eval-validate`,
and one `just eval-gate` pass over every task with the pinned live profile.
The gate warns when its live portion exceeds the two-minute target. A green
first attempt stops; fixed repeated sampling is not part of the normal release
path. Attempt artifacts land under
`evals/.gate/<task>/<primary|fallback>-attempt-<N>/`.

The gate adjudicates a first behavioral miss automatically with two more valid
attempts in fresh workspaces and blocks only when at least two of three fail.
Provider-invalid attempts do not vote. The primary profile retries once; after
two provider-invalid attempts, the gate invokes the executable named by
`YACH_EVAL_FALLBACK_RUNNER`, passing the cell command and arguments directly.
That wrapper must scrub the inherited primary `YACH_RIG_*` variables, export
one resolved fallback profile, and execute its arguments. Once the primary is
unavailable, the remaining tasks and driver checks stay on fallback instead of
re-probing it. `YACH_EVAL_FALLBACK_MODEL`
optionally pins the fallback model; otherwise the fallback profile's provider
model applies. A fallback pass exits successfully but reports degraded
coverage. An unavailable or unconfigured fallback, setup failure, broken
verifier, or harness failure remains a hard gate failure.

The live `approval-required` and `outcome-schema` driver checks surface
structured provider failures through a reserved check status. The gate applies
the same one-retry-then-sticky-fallback rule when an outage begins during the
check phase; other nonzero check exits remain hard failures.

Live compatibility checks are risk-triggered. When a change affects request
construction, streaming, tools, sessions, or compaction for a wire path, run
one initial repetition of the relevant tasks against one stable representative
of each affected path: Anthropic, OpenAI Responses, or OpenAI-compatible.
Provider unavailability leaves that wire path explicitly unverified; it is
never counted as a behavioral failure.

Repeated provider/model matrices are experiments, not release gates. Use them
for intentional behavioral measurements, new provider or model
characterization, and investigations. Patch only missing cells when an
experiment requires complete coverage; never restart already-valid cells.


## Provider-matrix experiments

`just eval-sweep <profiles-dir> <task-dir> <outdir> [repeat]` runs one
task across provider profiles (track 2). `just eval-matrix <profiles-dir>
<outdir> <repeat> <task-dir>...` runs multiple tasks under the same profile
loading boundary. Each cell gets a fresh fixture workspace, the profile's
`YACH_RIG_*` variables own provider *and* model (`YACH_EVAL_MODEL` stays unset
— only the gate pins a model), and the task's verifier scores the cell. Rows
append to `<outdir>/results.tsv` (cell, task, repeat, reward, agent exit,
seconds); per-cell artifacts, including session logs, `agent.stderr`, and
`cell.log`, land in `<outdir>/<task>/<name>-rN/`.

Repeats exist because intermittent quirks (the echo-imitation class fired in
2 of 3 runs) need repeated cells to distinguish "fixed" from "not elicited".
No statistics here — which cells fail and how often; statistics are yacht's
job.

A cell that never ran records `reward=error` with `agent_exit=na`. Agent setup
exit 2 and a structured outcome carrying `turn_end provider failed` also
record `reward=error`, preserving the numeric exit. Tool-loop exit 1,
approval-required exit 3, timeout exit 4, and any other completed headless
outcome remain verifier-scored behavioral data. Invalid evidence prints its
cause immediately and makes the sweep exit nonzero after later profiles run;
behavioral reward 0 does not. This distinction prevents infrastructure and
provider failures from silently poisoning the baseline without misclassifying
intentional headless outcomes.

When `YACH_ROTATE_PROFILE_RUNNER` is configured, the matrix driver treats
profile values as opaque, rewrites every assignment to a collision-free
environment name, and invokes the runner exactly once with the generated
dotenv bundle and matrix command. The runner must execute that command with
the bundle variables exported; how it transforms values is outside the eval
driver's contract. Before each profile starts, the driver maps only that
profile back to its ordinary `YACH_RIG_*` names and removes every generated
alias from cell subprocesses. The temporary bundle contains the original
profile values, is mode `0600`, and is deleted when the matrix exits. The
driver never writes transformed values. A runner failure aborts before any
cell starts and preserves its exit status.
Interrupt the outer `eval-matrix` command rather than an individual sweep or
cell; normal signal-driven exit unwinds the runner and executes the bundle
cleanup trap.

## Driver-contract checks

`evals/checks/*.sh` are standalone shell checks of the `yach run`
contract itself (exit codes, outcome schema) — not Harbor tasks,
because they exercise postures a fixed task command cannot (no
`--full-auto`, missing credentials). Each runs the `yach-runtime`
container directly; the gate runs them after the tasks. They expect
`YACH_RIG_*` provider variables in the environment (except the
credential-free ones) and `jq` on the host.

## Known deferred detail

`environment/Dockerfile` is authored for the yacht/harbor path
(track 3) but untested until the musl packaging item lands; the build
context and workdir conventions get confirmed then. The local gate
ignores it and mounts a `fixture/` copy into the `yach-runtime`
image instead.
