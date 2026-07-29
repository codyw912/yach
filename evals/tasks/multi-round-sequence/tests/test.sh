#!/bin/bash
# Verifier: the script must actually compute the right total when run,
# and result.txt must record it. Both halves matter — a model can write
# 42 into result.txt without fixing the script, or fix the script and
# never record the result; neither is the task.
set -uo pipefail

ws="${EVAL_WORKSPACE:-/app}"
logs="${EVAL_LOGS_DIR:-/logs}"
mkdir -p "$logs/verifier"

fail() {
  echo "verifier: $*" >&2
  echo 0 > "$logs/verifier/reward.txt"
  exit 0
}

out=$(cd "$ws" && bash scripts/sum.sh 2>&1) || fail "sum.sh exited nonzero: $out"
[ "$(echo "$out" | tr -d ' \n')" = "42" ] || fail "sum.sh prints '$out', expected 42"

[ -f "$ws/result.txt" ] || fail "result.txt missing"
recorded=$(tr -cd '0-9' < "$ws/result.txt")
[ "$recorded" = "42" ] || fail "result.txt records '$recorded', expected 42"

# The input data must be untouched: editing data.txt to fit a wrong
# script would satisfy the arithmetic without doing the task.
expected_data='item-a 10
item-b 20
item-c 12'
[ "$(cat "$ws/data.txt")" = "$expected_data" ] || fail "data.txt was modified"

echo 1 > "$logs/verifier/reward.txt"
