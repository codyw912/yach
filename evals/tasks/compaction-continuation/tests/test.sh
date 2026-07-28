#!/bin/bash
# Verifier: compaction must actually have fired, and the turn after it
# must still complete a fresh tool call and write its result.
#
# Deliberately NOT asserting recall through the summary: whether a
# codeword survives summarization measures summarizer quality and
# flakes. What this guards is the context-rebuild path — that the loop
# keeps working after a checkpoint, which is the risk the native
# tool-call refactor carries.
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

outcome="$ws/.yach-eval/outcome.json"
[ -f "$outcome" ] || fail "outcome document missing"
jq -e '.outcome == "completed"' "$outcome" >/dev/null \
  || fail "outcome is not completed: $(jq -r '.outcome' "$outcome")"

compactions=$(jq '[.turns[].compactions] | add // 0' "$outcome")
[ "$compactions" -ge 1 ] 2>/dev/null \
  || fail "compaction never fired (total compactions=$compactions); the fixture threshold may need lowering"

[ -f "$ws/answer.txt" ] || fail "answer.txt missing — the post-compaction turn did not complete its work"
expected=$(tr -d ' \n' < "$ws/codeword.txt")
actual=$(tr -d ' \n' < "$ws/answer.txt")
[ "$actual" = "$expected" ] || fail "answer.txt content wrong: '$actual'"

echo 1 > "$logs/verifier/reward.txt"
