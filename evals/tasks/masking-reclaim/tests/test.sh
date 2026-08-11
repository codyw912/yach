#!/bin/bash
# Verifier: only workspace artifacts count. A failed assertion awards zero but
# remains a successful verifier execution so the harness can record the score.
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

answer="$ws/answer.txt"
[ -f "$answer" ] || fail "answer.txt missing"
expected=$(sed -n 's/^CODEWORD: //p' "$ws/notes/chapter-1.md")
[ -n "$expected" ] || fail "seed codeword missing"
actual=$(tr -d '\r\n' < "$answer")
[ "$actual" = "$expected" ] || fail "answer.txt does not contain the chapter-1 codeword"

outcome="$ws/.yach-eval/outcome.json"
[ -f "$outcome" ] || fail "outcome document missing"
jq -e '.outcome == "completed"' "$outcome" >/dev/null \
  || fail "outcome is not completed"

masked_results=$(jq -er '
  [.turns[].masked_results] |
  if length > 0 and all(.[]; type == "number" and . == floor and . >= 0)
  then add else error("invalid masked_results") end
' "$outcome") || fail "outcome masked_results must be non-negative integers"
masked_bytes=$(jq -er '
  [.turns[].masked_bytes] |
  if length > 0 and all(.[]; type == "number" and . == floor and . >= 0)
  then add else error("invalid masked_bytes") end
' "$outcome") || fail "outcome masked_bytes must be non-negative integers"
[ "$masked_results" -ge 1 ] 2>/dev/null \
  || fail "outcome records no masked results"
[ "$masked_bytes" -gt 0 ] 2>/dev/null \
  || fail "outcome records no masked bytes"

session="$ws/.yach/sessions/eval-masking.jsonl"
[ -f "$session" ] || fail "resumed session log missing"
mask_events=$(jq -sc '[.[] | select(.type == "tool_result_masked")]' "$session") \
  || fail "session log is not valid JSONL"
jq -e --argjson results "$masked_results" --argjson bytes "$masked_bytes" '
  length >= 1
  and all(.[]; (.bytes_freed | type == "number" and . == floor and . >= 0))
  and length == $results
  and ((map(.bytes_freed) | add // 0) == $bytes)
' <<<"$mask_events" >/dev/null \
  || fail "mask events do not match outcome accounting"

echo 1 > "$logs/verifier/reward.txt"
