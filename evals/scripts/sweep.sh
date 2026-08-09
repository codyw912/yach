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
    # A cell that never produced the "<agent_exit> <verifier_exit>" line
    # never ran the task — profile/launch failure, not a bad score.
    # Recording both as a low reward would poison a baseline rate, so
    # they are distinguished and the cause is surfaced immediately
    # rather than left two files deep.
    if [ -z "$agent_exit" ]; then
      reward="error"
      # `|| true` matters under `set -euo pipefail`: grep exits 1 when it
      # matches nothing, and with pipefail that failure propagates out of
      # the assignment and kills the run before any row is written.
      cause=$(grep -vE '^\s*$' "$cell_dir/cell.log" 2>/dev/null | tail -1 || true)
      if [ -z "$cause" ]; then
        cause=$(grep -vE '^\s*$' "$profile_log" 2>/dev/null | tail -1 || true)
      fi
      echo "  cell did not launch: ${cause:-no stderr captured}" >&2
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

echo "sweep: $(( ${#profiles[@]} * repeat )) cells, $failures below reward 1, $errors failed to launch; results: $results" >&2
# A sweep measures a rate: cells scoring 0 are the data, not a failure of
# the run, so they do not make it exit nonzero. Cells that never launched
# are a genuine problem — they invalidate the rate — and do. (The gate is
# the pass/fail tool; conflating the two made every normal measurement
# report itself as a broken recipe.)
if [ "$errors" -ne 0 ]; then
  echo "sweep: cells that failed to launch are recorded as reward=error and must not be read as a rate" >&2
  exit 1
fi
