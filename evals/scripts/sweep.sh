#!/bin/bash
# Provider-matrix sweep of one eval task: one cell per <name>.env profile,
# repeated <repeat> times for intermittence hunting. Each cell gets a fresh
# workspace from the task's fixture, runs the agent with the profile's
# YACH_RIG_* variables, then the task's verifier; a row lands in
# <outdir>/results.tsv. The matrix wrapper can activate collision-free profile
# aliases resolved by one runner invocation. Design:
# docs/superpowers/specs/2026-07-28-eval-portfolio-design.md
set -euo pipefail

profiles_dir=$1
task_dir=$2
outdir=$3
repeat=${4:-1}
task=$(basename "$task_dir")

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to classify provider failures" >&2
  exit 2
fi

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

if [ ! -f "$task_dir/tests/test.sh" ]; then
  echo "not an eval task directory (no tests/test.sh): $task_dir" >&2
  exit 2
fi
if [ "${YACH_SWEEP_PREFLIGHT_DONE:-0}" != "1" ]; then
  if ! docker image inspect yach-runtime >/dev/null 2>&1; then
    echo "yach-runtime image missing - run 'just runtime-image' first" >&2
    exit 2
  fi
  bash "$(cd "$(dirname "$0")" && pwd)/check-image-fresh.sh" || exit 2
fi

cell_script=$(cd "$(dirname "$0")" && pwd)/run-task-cell.sh
shopt -s nullglob
profiles=("$profiles_dir"/*.env)
if [ ${#profiles[@]} -eq 0 ]; then
  echo "no *.env profiles in $profiles_dir" >&2
  exit 2
fi

mkdir -p "$outdir"
results="$outdir/results.tsv"
if [ ! -f "$results" ]; then
  printf 'cell\ttask\trepeat\treward\tagent_exit\tseconds\n' > "$results"
fi

failures=0
errors=0
repeats_script=$(cd "$(dirname "$0")" && pwd)/run-profile-repeats.sh
activate_script=$(cd "$(dirname "$0")" && pwd)/activate-profile-aliases.sh
profile_index=0
for profile in "${profiles[@]}"; do
  name=$(basename "$profile" .env)
  cell_root="$outdir/$task/$name"
  for r in $(seq 1 "$repeat"); do
    rm -rf "$cell_root-r$r"
    mkdir -p "$cell_root-r$r/work" "$cell_root-r$r/logs"
    cp -R "$task_dir/fixture/." "$cell_root-r$r/work/"
  done
  echo "=== sweep profile: $name x$repeat ($task) ===" >&2

  profile_log="$outdir/$task/$name.profile.log"
  : > "$profile_log"
  start=$SECONDS
  set +e
  if [ "${YACH_SWEEP_PROFILE_ALIASES:-0}" = "1" ]; then
    profile_out=$(bash "$activate_script" "$profile_index" "$profile" \
      bash "$repeats_script" "$task_dir" "$cell_root" "$repeat" "$cell_script" \
      2>>"$profile_log")
  else
    # shellcheck disable=SC2046
    profile_out=$(env $(grep -v '^\s*#' "$profile" | grep -v '^\s*$' | xargs) \
      bash "$repeats_script" "$task_dir" "$cell_root" "$repeat" "$cell_script" \
      2>>"$profile_log")
  fi
  set -e
  elapsed=$(( (SECONDS - start) / repeat ))

  for r in $(seq 1 "$repeat"); do
    cell_dir="$cell_root-r$r"
    logs="$cell_dir/logs"
    agent_exit=$(echo "$profile_out" | awk -v r="$r" '$1==r {print $2}' | tail -1)
    if [ "$agent_exit" = "na" ]; then
      agent_exit=""
    fi
    reward=$(cat "$logs/verifier/reward.txt" 2>/dev/null || echo "missing")
    invalid_reason=""
    if [ -z "$agent_exit" ]; then
      invalid_reason="cell did not launch"
    elif [ "$agent_exit" -eq 2 ]; then
      invalid_reason="agent setup failed"
    elif [ "$agent_exit" -ne 0 ] \
      && has_provider_failure_outcome "$cell_dir/work/.yach-eval"; then
      invalid_reason="provider failed"
    fi

    if [ -n "$invalid_reason" ]; then
      reward="error"
      # `|| true` matters under `set -euo pipefail`: grep exits 1 when it
      # matches nothing, and with pipefail that failure propagates out of
      # the assignment and kills the run before any row is written.
      if [ -z "$agent_exit" ]; then
        cause=$(grep -vE '^\s*$' "$cell_dir/cell.log" 2>/dev/null | tail -1 || true)
        if [ -z "$cause" ]; then
          cause=$(grep -vE '^\s*$' "$profile_log" 2>/dev/null | tail -1 || true)
        fi
      else
        cause=""
        if [ "$invalid_reason" = "provider failed" ]; then
          cause=$(grep -E '^status: provider failed \(' "$logs/agent.stderr" 2>/dev/null | tail -1 || true)
        fi
        if [ -z "$cause" ]; then
          cause=$(grep -vE '^\s*$' "$logs/agent.stderr" 2>/dev/null | tail -1 || true)
        fi
      fi
      echo "  $invalid_reason${agent_exit:+ (agent exit $agent_exit)}: ${cause:-no stderr captured}" >&2
      echo "  (stderr: $cell_dir/cell.log, $profile_log)" >&2
      errors=$((errors + 1))
    elif [ "$reward" != "1" ]; then
      # Surface why it scored low. Rates alone do not tell you whether a
      # cell failed the behavior under measurement or something else.
      reason=$(grep '^verifier:' "$cell_dir/cell.log" 2>/dev/null | tail -1 || true)
      echo "  reward=$reward ${reason:+- $reason}" >&2
      failures=$((failures + 1))
    fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$name" "$task" "$r" "$reward" "${agent_exit:-na}" "$elapsed" >> "$results"
  done
  profile_index=$((profile_index + 1))
done

echo "sweep: $(( ${#profiles[@]} * repeat )) cells, $failures below reward 1, $errors invalid; results: $results" >&2
# A sweep measures a rate: verifier reward 0 remains behavioral data even
# when the headless contract uses a nonzero exit for tool-loop or approval
# outcomes. Cells that never launch, fail setup, or carry the structured
# provider-failure reason are invalid evidence and make the sweep fail.
if [ "$errors" -ne 0 ]; then
  echo "sweep: invalid cells are recorded as reward=error and must not be read as a rate" >&2
  exit 1
fi
