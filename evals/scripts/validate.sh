#!/bin/bash
# Oracle validation: run each task's solution against a fresh fixture
# copy, then its verifier, and require reward 1. No model calls, no
# secrets, no containers — a verifier that rejects its own oracle is
# broken, and this catches it before a model run.
set -euo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
failures=0

for task_dir in "$evals_dir"/tasks/*/; do
  task=$(basename "$task_dir")
  scratch=$(mktemp -d)
  logs=$(mktemp -d)
  cp -R "$task_dir/fixture/." "$scratch/"
  if ! (cd "$scratch" && bash "$task_dir/solution/solve.sh"); then
    echo "FAIL $task (oracle solve.sh exited nonzero)"
    failures=$((failures + 1))
    rm -rf "$scratch" "$logs"
    continue
  fi
  if ! EVAL_WORKSPACE="$scratch" EVAL_LOGS_DIR="$logs" bash "$task_dir/tests/test.sh"; then
    echo "FAIL $task (verifier exited nonzero — verifier bug, not a reward)"
    failures=$((failures + 1))
    rm -rf "$scratch" "$logs"
    continue
  fi
  reward=$(cat "$logs/verifier/reward.txt" 2>/dev/null || echo "missing")
  if [ "$reward" = "1" ]; then
    echo "ok   $task"
  else
    echo "FAIL $task (reward: $reward)"
    failures=$((failures + 1))
  fi
  rm -rf "$scratch" "$logs"
done

if [ "$failures" -ne 0 ]; then
  echo "$failures verifier(s) rejected their oracle" >&2
  exit 1
fi
