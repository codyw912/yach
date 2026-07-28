#!/bin/bash
# Verifier: journal.txt must show both turns landed (alpha then beta),
# and both invocations must have produced outcome documents with the
# final one completed. Artifact assertions only.
set -uo pipefail

ws="${EVAL_WORKSPACE:-/app}"
logs="${EVAL_LOGS_DIR:-/logs}"
mkdir -p "$logs/verifier"

fail() {
  echo "verifier: $*" >&2
  echo 0 > "$logs/verifier/reward.txt"
  exit 0
}

command -v jq >/dev/null 2>&1 || fail "verifier needs jq"

[ -f "$ws/journal.txt" ] || fail "journal.txt missing"
content=$(cat "$ws/journal.txt")
[ "$content" = "$(printf 'alpha\nbeta')" ] || fail "journal.txt content wrong: $content"

[ -f "$ws/.yach-eval/outcome-turn-1.json" ] || fail "turn-1 outcome document missing"
[ -f "$ws/.yach-eval/outcome.json" ] || fail "final outcome document missing"
jq -e '.outcome == "completed"' "$ws/.yach-eval/outcome.json" >/dev/null \
  || fail "final outcome is not completed: $(jq -r '.outcome' "$ws/.yach-eval/outcome.json")"

echo 1 > "$logs/verifier/reward.txt"
