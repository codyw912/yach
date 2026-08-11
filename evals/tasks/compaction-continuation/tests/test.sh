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
turn_masked_results=$(jq -er '[.turns[].masked_results // 0] | add // 0' "$outcome") \
  || fail "outcome masked_results total is invalid"
turn_masked_bytes=$(jq -er '[.turns[].masked_bytes // 0] | add // 0' "$outcome") \
  || fail "outcome masked_bytes total is invalid"
[ "$compactions" -ge 1 ] 2>/dev/null || [ "$turn_masked_results" -ge 1 ] 2>/dev/null \
  || fail "compaction never fired (checkpoints=$compactions, masked_results=$turn_masked_results); the fixture threshold may need lowering"

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
mask_events=$(jq -sc '[.[] | select(.type == "tool_result_masked")]' "$session_log") \
  || fail "session log is not valid JSONL"

if [ "$checkpoint_count" -ge 1 ] 2>/dev/null; then
  jq -e 'all(.[]; ((.masked_results | type == "number" and . == floor and . >= 0)
                     and (.masked_bytes | type == "number" and . == floor and . >= 0)))' \
    <<<"$checkpoint_details" >/dev/null \
    || fail "compaction accounting lacks non-negative integer masked_results/masked_bytes"
  masked_results=$(jq -er '[.[].masked_results] | add // 0' <<<"$checkpoint_details") \
    || fail "compaction masked_results total is invalid"
  masked_bytes=$(jq -er '[.[].masked_bytes] | add // 0' <<<"$checkpoint_details") \
    || fail "compaction masked_bytes total is invalid"
  jq -e --argjson results "$masked_results" --argjson bytes "$masked_bytes" \
    'all(.[]; (.bytes_freed | type == "number" and . == floor and . >= 0))
     and length == $results
     and ((map(.bytes_freed) | add // 0) == $bytes)' <<<"$mask_events" >/dev/null \
    || fail "mask events do not exactly match checkpoint accounting"
else
  [ "$turn_masked_results" -ge 1 ] 2>/dev/null && [ "$turn_masked_bytes" -ge 1 ] 2>/dev/null \
    || fail "mask-only compaction lacks positive per-turn accounting"
  jq -e --argjson results "$turn_masked_results" --argjson bytes "$turn_masked_bytes" \
    'all(.[]; (.bytes_freed | type == "number" and . == floor and . >= 0))
     and length == $results
     and ((map(.bytes_freed) | add // 0) == $bytes)' <<<"$mask_events" >/dev/null \
    || fail "mask events do not exactly match per-turn mask-only accounting"
fi

[ -f "$ws/answer.txt" ] || fail "answer.txt missing — the post-compaction turn did not complete its work"
expected=$(tr -d ' \n' < "$ws/codeword.txt")
actual=$(tr -d ' \n' < "$ws/answer.txt")
[ "$actual" = "$expected" ] || fail "answer.txt content wrong: '$actual'"

echo 1 > "$logs/verifier/reward.txt"
