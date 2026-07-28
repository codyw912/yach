#!/bin/bash
# Driver contract: a write task without --full-auto must fail loudly —
# exit 3 with outcome approval_required naming no silent approval and
# no hang. Needs YACH_RIG_* provider variables and the yach-runtime
# image; one small model call.
set -uo pipefail

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

out=$(docker run --rm \
  $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
  -v "$scratch:/work" yach-runtime \
  yach run --model "${YACH_EVAL_MODEL:-claude-haiku-4-5}" \
  --quiet \
  --prompt "Create a file named hello.txt containing the single word: hi")
code=$?

if [ "$code" -ne 3 ]; then
  echo "check: expected exit 3 (approval required), got $code" >&2
  exit 1
fi
echo "$out" | jq -e '.outcome == "approval_required"' >/dev/null || {
  echo "check: outcome document does not report approval_required" >&2
  exit 1
}
