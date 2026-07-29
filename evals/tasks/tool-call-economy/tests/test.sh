#!/bin/bash
# Verifier: the artifact must be correct AND produced by exactly one
# edit-family tool call. Reads are not counted — inspecting before
# writing is legitimate; re-issuing the write is the behavior under
# measurement (the 2026-07-28 repetition finding).
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

[ -f "$ws/report.txt" ] || fail "report.txt missing"
content=$(tr -d '\n' < "$ws/report.txt")
[ "$content" = "ready" ] || fail "report.txt content wrong: '$content'"

outcome="$ws/.yach-eval/outcome.json"
[ -f "$outcome" ] || fail "outcome document missing"
jq -e '.outcome == "completed"' "$outcome" >/dev/null \
  || fail "outcome is not completed: $(jq -r '.outcome' "$outcome")"

edits=$(jq '[.turns[].tool_calls[]
             | select(.name == "create_text_file" or .name == "edit_text_file")
             | .count] | add // 0' "$outcome")
[ "$edits" = "1" ] || fail "expected exactly 1 edit-family tool call, got $edits"

echo 1 > "$logs/verifier/reward.txt"
