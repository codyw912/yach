#!/bin/bash
# Provider-matrix sweep of one eval task: one cell per <name>.env
# profile, repeated <repeat> times for intermittence hunting. Each cell
# gets a fresh workspace from the task's fixture, runs the agent with
# the profile's YACH_RIG_* variables (which own provider and model),
# then the task's verifier; a row lands in <outdir>/results.tsv. Set
# YACH_ROTATE_PROFILE_RUNNER to resolve secret references in profiles,
# exactly as `just rotate` does. Design:
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
if ! docker image inspect yach-runtime >/dev/null 2>&1; then
  echo "yach-runtime image missing - run 'just runtime-image' first" >&2
  exit 2
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
for profile in "${profiles[@]}"; do
  name=$(basename "$profile" .env)
  for r in $(seq 1 "$repeat"); do
    cell_dir="$outdir/$name-r$r"
    work="$cell_dir/work"
    logs="$cell_dir/logs"
    rm -rf "$cell_dir"
    mkdir -p "$work" "$logs"
    cp -R "$task_dir/fixture/." "$work/"
    echo "=== sweep cell: $name r$r ($task) ===" >&2

    start=$SECONDS
    set +e
    if [ -n "${YACH_ROTATE_PROFILE_RUNNER:-}" ]; then
      cell_out=$("$YACH_ROTATE_PROFILE_RUNNER" "$profile" \
        bash "$cell_script" "$task_dir" "$work" "$logs")
    else
      # shellcheck disable=SC2046
      cell_out=$(env $(grep -v '^\s*#' "$profile" | grep -v '^\s*$' | xargs) \
        bash "$cell_script" "$task_dir" "$work" "$logs")
    fi
    set -e
    agent_exit=$(echo "$cell_out" | tail -1 | cut -d' ' -f1)
    reward=$(cat "$logs/verifier/reward.txt" 2>/dev/null || echo "missing")
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$name" "$task" "$r" "$reward" "$agent_exit" "$((SECONDS - start))" >> "$results"
    if [ "$reward" != "1" ]; then
      failures=$((failures + 1))
    fi
  done
done

echo "sweep: $(( ${#profiles[@]} * repeat )) cells, $failures below reward 1; results: $results" >&2
[ "$failures" -eq 0 ]
