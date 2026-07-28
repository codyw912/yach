# Eval Portfolio Design

Date: 2026-07-28

Status: approved (owner, 2026-07-28, interactive design session).

## Context

Founding principle (owner, 2026-07-27): **evals assert on artifacts,
not utterances.** The first cross-model rotation run made the case
concretely: a model imitated yach's round-echo format in prose,
fabricated a `create_text_file` success, and the turn "completed" with
the README never written. Response text is not evidence; file state
and machine-readable outcome fields are.

Three threads converge here:

1. **Provider rotation** (board: active) produced a working scenario —
   the daily-notes fixture with a buggy `scripts/tally.sh` — driven by
   `yach run` across provider cells (`just rotate`), but pass/fail is
   still eyeballed from outcome JSONs.
2. **The yacht integration** (green 2026-07-27) proved yach runs as a
   declared harness in yacht (`~/dev/yacht`), whose custom-eval course
   kind runs Harbor-format task directories with reward-file verifiers
   — exactly the artifact-assertion model. Constraint learned there:
   custom-eval courses run only on yacht's harbor backend and need a
   pinned static Linux binary (the queued musl packaging item).
3. **Behavioral-fixes research** (2026-07-26) adopted Pi's posture:
   every rotated provider's real failure shapes join a regression
   corpus. The portfolio is that corpus's executable half.

## Decisions (owner, 2026-07-28)

- **Primary job: harness regression gate.** Rotation scenarios become
  repeatable tasks with artifact assertions, run cheaply against a
  pinned model before/after harness changes. Matrix sweeps and
  cross-harness comparison build on the same assets later.
- **Home: in-repo, dual-runner.** Tasks live in `evals/tasks/` in this
  repo, authored Harbor-format from day one. A `just` recipe runs the
  gate locally with no yacht dependency; yacht runs the identical
  directory as a custom-eval course for comparison work.
- **Initial roster:** `notes-tally-fix`, `notes-explore`,
  `session-continuation`, plus shell-based driver-contract checks.

## Portfolio shape

One asset set, two runners, three tracks:

- **Assets** — `evals/tasks/<task-id>/`, each a Harbor-format task
  directory versioned with the harness it gates.
- **Track 1, regression gate (now)** — `just eval-gate`: every task
  run locally in the `yach-runtime` container against a pinned cheap
  model, verified by the task's own `tests/test.sh`.
- **Track 2, provider/model matrix (grows from rotation)** — `just
  rotate` gains verifier awareness and a repeat count; rewards replace
  eyeball inspection.
- **Track 3, cross-harness comparison (later)** — the same task
  directory as a yacht custom-eval course: yach as declared harness
  (pinned musl binary + sha256 + `evidence_map`), claude-code and Pi
  vessels, `--repetitions` for paired statistics.

## Task format and authoring contract

Harbor task layout, per yacht's custom-eval reference:

```
evals/tasks/<task-id>/
├── task.toml               # metadata + timeouts (Harbor schema)
├── instruction.md          # the prompt the agent receives
├── fixture/                # workspace seed (gate/matrix runners)
├── environment/Dockerfile  # task container (yacht/harbor path)
├── solution/solve.sh       # oracle solution (mandatory here)
└── tests/test.sh           # verifier: writes reward 1 or 0
```

Local conventions, recorded in `evals/README.md`, that keep one task
runnable by both runners:

- `fixture/` is the workspace seed. The gate copies it to a scratch
  dir and mounts it; the `environment/Dockerfile` bakes the same
  content for the harbor path. The fixture is the source of truth;
  the Dockerfile COPYs it.
- Verifiers assert only on paths under `$EVAL_WORKSPACE` (the runner
  sets it; under Harbor it defaults to the task workdir) and on
  `$EVAL_WORKSPACE/.yach-eval/outcome.json`, which the gate runner
  drops before invoking the verifier. The outcome document is an
  artifact like any other; this is how contract fields (turn outcomes,
  tool-call counts) are asserted without touching response prose.
- Reward goes to `${EVAL_LOGS_DIR:-/logs}/verifier/reward.txt` —
  Harbor's fixed path when `EVAL_LOGS_DIR` is unset, a scratch dir
  locally.
- `solution/solve.sh` is mandatory: `just eval-validate` proves every
  verifier accepts its oracle without spending model tokens.
- Verifiers never grep the assistant's response text. Reward comes
  from file state, command output, and outcome-document fields.

## Initial roster

1. **`notes-tally-fix`** — the runs-2 rotation scenario promoted.
   Instruction: fix `scripts/tally.sh` so done/todo counts are
   correct, verify by running it, and write a `README.md`. Verifier:
   runs the script and asserts exact output (`done: 4`, `todo: 4`),
   asserts `README.md` exists and is non-empty, asserts the notes
   files are unmodified. Direct net for the fabricated-success class.
2. **`notes-explore`** — read-only turn on the same fixture.
   Instruction: explore and summarize. Verifier: asserts the fixture
   tree is byte-identical (no writes) and, from the outcome document,
   `outcome == "completed"` with at least one list/read tool call.
   Runs without `--full-auto`, exercising the default posture.
3. **`session-continuation`** — two `yach run --session <id>`
   invocations (#192): turn one creates a state file; turn two's
   instruction is only satisfiable from session context ("append to
   the file you created earlier"). Verifier asserts the final file
   shows both turns. Needs a task-local `run.sh` convention (the task
   supplies its invocation sequence when one shot is not enough);
   gate-runner-only until the harbor path needs multi-shot tasks.
4. **Driver-contract checks** — `evals/checks/*.sh`, plain shell,
   mostly model-free: a write task without `--full-auto` exits 3 with
   outcome `approval_required`; missing provider env exits 2 with no
   outcome document; emitted outcome JSON validates against the
   documented shape (field presence and types, `jq`).

## Gate runner: `just eval-gate`

Per task: copy `fixture/` to a scratch dir; run the `yach-runtime`
container with the scratch dir mounted, `yach run --full-auto --model
claude-haiku-4-5 --prompt "$(cat instruction.md)"` (the
`session-continuation` task substitutes its `run.sh`); write the
outcome JSON to `.yach-eval/outcome.json` in the workspace; run
`tests/test.sh` in the same image with the same mount; read the
reward. Then run every `evals/checks/*.sh`. Summary table on stderr —
task, reward, duration — and nonzero exit if any reward is below 1 or
any check fails.

Provider credentials come from the usual `YACH_RIG_*` environment
variables; the recipe adds no credential handling of its own. One gate
pass is a handful of haiku tasks — pennies. It is a
pre-merge-when-touching-the-loop tool, not a per-push CI job.

## Verifier validation: `just eval-validate`

Per task: fresh fixture copy, run `solution/solve.sh`, then
`tests/test.sh`, assert reward 1. Zero model calls, zero secrets —
suitable for CI. A verifier that rejects its own oracle is a broken
verifier, caught before it ever burns a model run.

## Matrix track: rotate verifier-awareness

`just rotate` grows: after each cell, run the task verifier and record
reward beside the outcome JSON; a `--repeat N` wrapper reruns cells
for intermittence hunting (the echo-imitation class showed 2/3 —
distinguishing "fixed" from "not elicited" needs repeated runs).
Sweep results land in `results.tsv`: cell, task, repeat, reward, exit
code. No statistics locally — the local matrix answers "which cells
fail, how often"; statistics are yacht's job.

## Comparison track: yacht custom-eval course

`evals/tasks/` runs verbatim as a yacht custom-eval course. Config
lives in the eval workspace (`~/dev/yach-evals`): yach declared with a
pinned static Linux binary (url-or-path + sha256) and the existing
`evidence_map`; claude-code and Pi vessels via yacht's built-in Harbor
agents; `--repetitions` (yacht's run-level repetition; per-task trials
are fixed at 1) for the paired sign test.

Prerequisite: the queued Harbor-course packaging item (musl static
artifacts for x86_64/aarch64 + sha256). Newly relevant: yacht's
recorded-baseline comparisons (ADR 0018) landed 2026-07-28 (yacht
#258) — a recorded logbook vessel can stand in for a live one, with
comparability enforced by content digest. That makes version-vs-version
("yach 0.1.0 vs 0.2.0 on the same course") a first-class comparison
that only runs the new vessel — the gate's big sibling.

## Build order

1. `evals/` layout: authoring contract README, the three task
   directories with fixtures/oracles/verifiers, driver-contract
   checks, `just eval-validate` green.
2. `just eval-gate`.
3. `just rotate` verifier awareness + `--repeat` + `results.tsv`.
4. yacht course config in the eval workspace when packaging lands.

## Non-goals

- No statistics machinery in this repo (Wilson intervals, sign tests
  are yacht's).
- No per-push CI gating; `eval-validate` is the only CI-friendly
  piece.
- No assertions on response prose, ever — if a behavior only shows up
  in prose, the fix is to surface it in the outcome document first.
- Not the approval-model redesign, not the isolation decision; the
  gate uses the container the way rotation already does.
