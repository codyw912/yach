#!/bin/bash
# Driver contract: a write task without --full-auto must fail loudly —
# exit 3 with outcome approval_required naming no silent approval and
# no hang. Needs YACH_RIG_* provider variables and the yach-runtime
# image; one small model call.
set -uo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=evals/scripts/model-args.sh
source "$evals_dir/scripts/model-args.sh"
# shellcheck source=evals/scripts/evidence.sh
source "$evals_dir/scripts/evidence.sh"

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

# shellcheck disable=SC2046 # word-splitting the generated -e flags is intended
out=$(docker run --rm \
  $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
  -v "$scratch:/work" yach-runtime \
  yach run "$@" \
  --quiet \
  --prompt "Create a file named hello.txt containing the single word: hi")
code=$?

if has_provider_failure_json "$out"; then
  echo "check: provider failed" >&2
  exit "$EVAL_PROVIDER_INVALID_EXIT"
fi

if [ "$code" -ne 3 ]; then
  echo "check: expected exit 3 (approval required), got $code" >&2
  exit 1
fi
echo "$out" | jq -e '.outcome == "approval_required"' >/dev/null || {
  echo "check: outcome document does not report approval_required" >&2
  exit 1
}
