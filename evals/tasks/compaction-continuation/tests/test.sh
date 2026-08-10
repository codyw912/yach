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

session_path=$(jq -r '.session_path // empty' "$outcome")
[ -n "$session_path" ] || fail "outcome session_path missing"
case "$session_path" in
  /*) session_log="$session_path" ;;
  *) session_log="$ws/$session_path" ;;
esac
[ -f "$session_log" ] || fail "session log missing: $session_path"

checkpoint_details=$(jq -sc '[.[] | select(.type == "compaction_checkpoint") | .details]' "$session_log") \
  || fail "session log is not valid JSONL"
checkpoint_count=$(jq 'length' <<<"$checkpoint_details")
[ "$checkpoint_count" -ge 1 ] 2>/dev/null \
  || fail "compaction accounting missing from session log"
jq -e 'all(.[]; (.masked_results | type == "number") and (.masked_bytes | type == "number"))' \
  <<<"$checkpoint_details" >/dev/null \
  || fail "compaction accounting lacks numeric masked_results/masked_bytes"

masked_results=$(jq '[.[].masked_results] | add // 0' <<<"$checkpoint_details")
masked_bytes=$(jq '[.[].masked_bytes] | add // 0' <<<"$checkpoint_details")
if [ "$masked_bytes" -gt 0 ] 2>/dev/null; then
  mask_events=$(jq -sc '[.[] | select(.type == "tool_result_masked")]' "$session_log") \
    || fail "session log is not valid JSONL"
  jq -e --argjson results "$masked_results" --argjson bytes "$masked_bytes" \
    '([.[] | .bytes_freed] | add // 0) >= $bytes
     and length >= $results' <<<"$mask_events" >/dev/null \
    || fail "mask events do not cover compaction accounting"
fi

[ -f "$ws/answer.txt" ] || fail "answer.txt missing — the post-compaction turn did not complete its work"
expected=$(tr -d ' \n' < "$ws/codeword.txt")
actual=$(tr -d ' \n' < "$ws/answer.txt")
[ "$actual" = "$expected" ] || fail "answer.txt content wrong: '$actual'"

echo 1 > "$logs/verifier/reward.txt"
