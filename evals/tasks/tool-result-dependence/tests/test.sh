#!/bin/bash
# Verifier: answer.txt must carry the exact token that exists only
# inside secret.txt. The token is arbitrary, so a model that never
# received the tool result cannot produce it except by luck — this is
# the direct measure of whether tool output reaches the model.
set -uo pipefail

ws="${EVAL_WORKSPACE:-/app}"
logs="${EVAL_LOGS_DIR:-/logs}"
mkdir -p "$logs/verifier"

fail() {
  echo "verifier: $*" >&2
  echo 0 > "$logs/verifier/reward.txt"
  exit 0
}

[ -f "$ws/answer.txt" ] || fail "answer.txt missing"
expected=$(tr -d ' \n' < "$ws/secret.txt")
actual=$(tr -d ' \n' < "$ws/answer.txt")
[ "$actual" = "$expected" ] || fail "answer.txt does not carry the secret token: '$actual'"

# The source file must survive unmodified; rewriting it would let a
# wrong answer trivially match.
[ "$expected" = "zephyr-8842-quill" ] || fail "secret.txt was modified"

echo 1 > "$logs/verifier/reward.txt"
