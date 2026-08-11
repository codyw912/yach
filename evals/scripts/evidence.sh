#!/bin/bash
# Shared outcome-evidence classifiers for eval gate and matrix runners.

# shellcheck disable=SC2034 # consumed by scripts that source this file
EVAL_PROVIDER_INVALID_EXIT=42

has_provider_failure_json() {
  outcome_json=$1
  printf '%s\n' "$outcome_json" \
    | jq -e 'any(.turns[]?; .failure_reason == "turn_end provider failed")' \
      >/dev/null 2>&1
}

has_provider_failure_outcome() {
  outcome_dir=$1
  for outcome in "$outcome_dir"/outcome*.json; do
    [ -f "$outcome" ] || continue
    if jq -e 'any(.turns[]?; .failure_reason == "turn_end provider failed")' \
      "$outcome" >/dev/null 2>&1; then
      return 0
    fi
  done
  return 1
}
