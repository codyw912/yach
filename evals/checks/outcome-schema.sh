#!/bin/bash
# Driver contract: a completed run's outcome document carries the
# documented yach-run-outcome/1 shape (field presence and types).
# Needs YACH_RIG_* provider variables and the yach-runtime image; one
# small model call.
set -uo pipefail

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

out=$(docker run --rm \
  $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
  -v "$scratch:/work" yach-runtime \
  yach run --model "${YACH_EVAL_MODEL:-claude-haiku-4-5}" \
  --quiet \
  --prompt "Reply with the single word: ok")
code=$?

if [ "$code" -ne 0 ]; then
  echo "check: expected exit 0, got $code" >&2
  exit 1
fi
echo "$out" | jq -e '
  .schema == "yach-run-outcome/1"
  and (.outcome | type == "string")
  and (.response | type == "string")
  and (.turns | type == "array" and length >= 1)
  and ([.turns[] | (.prompt | type == "string")
        and (.outcome | type == "string")
        and (.tool_calls | type == "array")
        and (.compactions | type == "number")
        and (.duration_ms | type == "number")] | all)
  and (.tokens.context_estimate | type == "number")
  and (.tokens.provenance | type == "string")
  and (.session_path | type == "string")
  and (.duration_ms | type == "number")' >/dev/null || {
  echo "check: outcome document failed shape validation: $out" >&2
  exit 1
}
