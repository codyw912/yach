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

cell_script="$evals_dir/scripts/run-task-cell.sh"
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
  cell_out=$(YACH_EVAL_MODEL="$model" bash "$cell_script" "$task_dir" "$work" "$logs")
  set -e
  agent_exit=$(echo "$cell_out" | tail -1 | cut -d' ' -f1)
  verifier_exit=$(echo "$cell_out" | tail -1 | cut -d' ' -f2)

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
