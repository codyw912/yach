# One-Shot Eval Matrix Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve every provider profile through one generic runner invocation before a multi-task evaluation matrix starts.

**Architecture:** A new matrix wrapper rewrites every profile assignment to a collision-free environment alias, passes the generated dotenv bundle to `YACH_ROTATE_PROFILE_RUNNER` once, and runs the existing task sweep inside that resolved process. The sweep activates one profile at a time by mapping its aliases back to `YACH_RIG_*`, removes every alias before starting cells, and preserves the existing results/artifact schema.

**Tech Stack:** Bash 3.2, Just, Docker-compatible driver fakes, ShellCheck

## Global Constraints

- Public code and documentation are resolver-neutral: profile values are opaque and no credential manager or reference URI scheme is named.
- A configured runner is invoked exactly once per matrix, regardless of profile, task, or repeat counts.
- Transformed values remain in process environments; generated files contain only the original opaque profile values.
- Alias variables must not reach cell or verifier subprocesses.
- Existing `results.tsv` columns, artifact paths, reward semantics, and stale-image checks remain unchanged.
- `just eval-sweep` remains the one-task operator command; `just eval-matrix` accepts multiple task directories.

---

### Task 1: One-shot matrix regression

**Files:**
- Create: `evals/checks/one-shot-matrix-runner.sh`
- Remove: `evals/checks/sweep-credential-reresolution.sh`

**Interfaces:**
- Consumes: `evals/scripts/matrix.sh <profiles-dir> <outdir> <repeat> <task-dir>...`
- Produces: a secret-free driver regression covering one runner call, colliding profile keys, multiple tasks/repeats, environment isolation, preflight rejection, continued error accounting, and runner failure.

- [ ] **Step 1: Write the failing regression**

Create two profiles that assign different opaque sentinels to the same `YACH_RIG_OPENAI_API_KEY`, two minimal tasks, a fake runner that transforms each exact sentinel, and a fake Docker boundary. Run a 2-profile × 2-task × 2-repeat matrix and assert:

```text
runner invocations = 1
results rows = 8 plus header
alpha-model receives transformed-alpha
beta-model receives transformed-beta
no YACH_EVAL_PROFILE_* or unrelated ambient YACH_RIG_* variable reaches fake Docker
runtime preflight executes once before the runner boundary
```

Run failure cases and assert: runner exit `9` launches zero cells; missing fixtures and duplicate task names fail before the runner; and a recorded profile launch error does not prevent later task rows.

- [ ] **Step 2: Verify RED**

Run: `bash evals/checks/one-shot-matrix-runner.sh`

Expected: FAIL because `evals/scripts/matrix.sh` does not exist.

---

### Task 2: Matrix boundary and profile activation

**Files:**
- Create: `evals/scripts/matrix.sh`
- Create: `evals/scripts/activate-profile-aliases.sh`
- Modify: `evals/scripts/sweep.sh`
- Modify: `evals/scripts/run-profile-repeats.sh`

**Interfaces:**
- `matrix.sh <profiles-dir> <outdir> <repeat> <task-dir>...` validates inputs and invokes `YACH_ROTATE_PROFILE_RUNNER <generated-env-file> <resolved-matrix-command...>` once when configured.
- Generated aliases use `YACH_EVAL_PROFILE_<zero-based-index>_<original-key>`.
- `activate-profile-aliases.sh <index> <profile-file> <command...>` exports the selected aliases under their original names, unsets all `YACH_EVAL_PROFILE_*` variables, then `exec`s the command.
- `YACH_SWEEP_PROFILE_ALIASES=1` tells `sweep.sh` to activate aliases instead of loading values directly from each profile.

- [ ] **Step 1: Implement bundle construction and one-shot execution**

Accept only profile assignments whose keys match `YACH_RIG_[A-Z0-9_]+`. Preserve every value byte-for-byte after the first `=`. Store the generated bundle of opaque profile values in a `mktemp` file with mode `0600`, delete it through `trap`, and propagate the runner's exact exit status.

- [ ] **Step 2: Implement per-profile activation**

Read only assignment names from the original profile, recover each resolved alias through Bash indirect expansion, export the original `YACH_RIG_*` name, then unset every alias before `exec`.

- [ ] **Step 3: Remove retry and marker behavior**

Delete the obsolete wait/delay variables, retry loops, messages, and child-start markers. Keep one repeat subprocess per profile so repeat batching remains intact.

- [ ] **Step 4: Verify GREEN**

Run: `bash evals/checks/one-shot-matrix-runner.sh`

Expected: `ok one-shot matrix runner`.

---

### Task 3: Public commands and operator documentation

**Files:**
- Modify: `Justfile`
- Modify: `evals/README.md`
- Modify: `docs/project/board.md`
- Modify: `docs/project/next.md`
- Remove: `docs/superpowers/plans/2026-08-08-sweep-credential-reresolution.md`

**Interfaces:**
- `just eval-sweep <profiles-dir> <task-dir> <outdir> [repeat]` delegates to `matrix.sh` with one task.
- `just eval-matrix <profiles-dir> <outdir> <repeat> <task-dir>...` delegates to the same matrix boundary.

- [ ] **Step 1: Replace retry documentation**

Document one runner invocation per matrix, opaque profile values, no resolved-value files, per-profile alias activation, and interruption by terminating the matrix process. Do not name or prescribe a resolver implementation.

- [ ] **Step 2: Correct project status**

Replace the claimed retry fix with the one-shot matrix boundary and restore the 125-cell run as the next pre-release action.

---

### Task 4: Verification and publication

**Files:**
- Verify all files above.

- [ ] **Step 1: Run focused driver checks**

```bash
bash evals/checks/one-shot-matrix-runner.sh
bash -n evals/scripts/matrix.sh evals/scripts/activate-profile-aliases.sh evals/scripts/sweep.sh evals/scripts/run-profile-repeats.sh evals/checks/one-shot-matrix-runner.sh
shellcheck evals/scripts/matrix.sh evals/scripts/activate-profile-aliases.sh evals/scripts/sweep.sh evals/scripts/run-profile-repeats.sh evals/checks/one-shot-matrix-runner.sh
```

- [ ] **Step 2: Run project verification**

```bash
just eval-validate
just fmt-check
just lint
just test
```

- [ ] **Step 3: Checkpoint and update PR #239**

Use Jujutsu to describe the final revision, move `sweep-credential-reresolution` to it, push the bookmark, replace the PR title/body with the root-cause behavior and exact verification, and wait for required CI.
