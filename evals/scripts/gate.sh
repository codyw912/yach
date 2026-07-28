#!/bin/bash
# Regression gate: run every eval task in the yach-runtime container
# against a pinned model, verify each with the task's own verifier,
# then run the driver-contract checks. Requires docker, jq, and
# YACH_RIG_* provider variables in the environment; override the model
# with YACH_EVAL_MODEL. Design:
# docs/superpowers/specs/2026-07-28-eval-portfolio-design.md
set -euo pipefail

model="${YACH_EVAL_MODEL:-claude-haiku-4-5}"
evals_dir=$(cd "$(dirname "$0")/.." && pwd)
gate_dir="$evals_dir/.gate"

if ! docker image inspect yach-runtime >/dev/null 2>&1; then
  echo "yach-runtime image missing - run 'just runtime-image' first" >&2
  exit 2
fi
if ! env | grep -q '^YACH_RIG_'; then
  echo "no YACH_RIG_* provider variables in the environment" >&2
  exit 2
fi

# shellcheck disable=SC2046 # word-splitting the -e flags is intended
env_flags=$(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ')

failures=0
printf '%-22s %-7s %-6s %s\n' TASK REWARD EXIT SECONDS >&2

for task_dir in "$evals_dir"/tasks/*/; do
  task=$(basename "$task_dir")
  # Kept after the run for inspection: work/ holds the workspace
  # (including .yach/ session logs), logs/ the verifier reward.
  work="$gate_dir/$task/work"
  logs="$gate_dir/$task/logs"
  rm -rf "$gate_dir/$task"
  mkdir -p "$work" "$logs"
  cp -R "$task_dir/fixture/." "$work/"

  start=$SECONDS
  set +e
  if [ -f "$task_dir/run.sh" ]; then
    # shellcheck disable=SC2086
    docker run --rm $env_flags -e YACH_EVAL_MODEL="$model" \
      -v "$work:/work" -v "$task_dir:/task:ro" \
      yach-runtime bash /task/run.sh >/dev/null
  else
    # shellcheck disable=SC2086
    docker run --rm $env_flags -e YACH_EVAL_MODEL="$model" \
      -v "$work:/work" -v "$task_dir:/task:ro" \
      yach-runtime bash -c \
      'mkdir -p .yach-eval && yach run --full-auto --model "$YACH_EVAL_MODEL" --prompt "$(cat /task/instruction.md)" --outcome .yach-eval/outcome.json' \
      >/dev/null
  fi
  agent_exit=$?
  docker run --rm -e EVAL_WORKSPACE=/work -e EVAL_LOGS_DIR=/logs \
    -v "$work:/work" -v "$logs:/logs" -v "$task_dir:/task:ro" \
    yach-runtime bash /task/tests/test.sh
  verifier_exit=$?
  set -e

  reward=$(cat "$logs/verifier/reward.txt" 2>/dev/null || echo "missing")
  printf '%-22s %-7s %-6s %s\n' "$task" "$reward" "$agent_exit" "$((SECONDS - start))" >&2
  if [ "$verifier_exit" -ne 0 ]; then
    echo "  verifier itself exited nonzero for $task - verifier bug" >&2
    failures=$((failures + 1))
  elif [ "$reward" != "1" ]; then
    failures=$((failures + 1))
  fi
done

for check in "$evals_dir"/checks/*.sh; do
  name=$(basename "$check" .sh)
  if bash "$check"; then
    echo "check ok   $name" >&2
  else
    echo "check FAIL $name" >&2
    failures=$((failures + 1))
  fi
done

if [ "$failures" -ne 0 ]; then
  echo "eval gate: $failures failure(s)" >&2
  exit 1
fi
echo "eval gate: all tasks and checks passed" >&2
