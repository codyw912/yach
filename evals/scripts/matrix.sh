#!/bin/bash
# Run one or more eval tasks across every provider profile. When a profile
# runner is configured, all profile values are aliased into one dotenv bundle
# so the runner wraps the entire matrix exactly once.
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
sweep="$script_dir/sweep.sh"

run_tasks() {
  local aliases=$1
  local matrix_status=0
  local task_status
  shift

  for task_dir in "$@"; do
    if YACH_SWEEP_PREFLIGHT_DONE=1 YACH_SWEEP_PROFILE_ALIASES="$aliases" \
      bash "$sweep" "$profiles_dir" "$task_dir" "$outdir" "$repeat"; then
      continue
    else
      task_status=$?
    fi
    if [ "$task_status" -eq 130 ]; then
      return 130
    fi
    if [ "$matrix_status" -eq 0 ]; then
      matrix_status=$task_status
    fi
  done

  return "$matrix_status"
}

if [ "${1:-}" = "--resolved" ]; then
  shift
  profiles_dir=$1
  outdir=$2
  repeat=$3
  shift 3

  run_tasks 1 "$@"
  exit 0
fi

if [ "$#" -lt 4 ]; then
  echo "usage: $0 <profiles-dir> <outdir> <repeat> <task-dir>..." >&2
  exit 2
fi

profiles_dir=$1
outdir=$2
repeat=$3
shift 3

if ! [[ "$repeat" =~ ^[1-9][0-9]*$ ]]; then
  echo "repeat must be a positive integer" >&2
  exit 2
fi

shopt -s nullglob
profiles=("$profiles_dir"/*.env)
if [ ${#profiles[@]} -eq 0 ]; then
  echo "no *.env profiles in $profiles_dir" >&2
  exit 2
fi
task_names=("")
for task_dir in "$@"; do
  if [ ! -f "$task_dir/tests/test.sh" ] || [ ! -r "$task_dir/tests/test.sh" ] \
    || [ ! -d "$task_dir/fixture" ] || [ ! -r "$task_dir/fixture" ]; then
    echo "not an eval task directory (requires fixture/ and tests/test.sh): $task_dir" >&2
    exit 2
  fi
  task_name=$(basename "$task_dir")
  for existing_name in "${task_names[@]}"; do
    if [ -n "$existing_name" ] && [ "$task_name" = "$existing_name" ]; then
      echo "duplicate eval task name: $task_name" >&2
      exit 2
    fi
  done
  task_names+=("$task_name")
done

if ! docker image inspect yach-runtime >/dev/null 2>&1; then
  echo "yach-runtime image missing - run 'just runtime-image' first" >&2
  exit 2
fi
bash "$script_dir/check-image-fresh.sh" || exit 2

if [ -z "${YACH_ROTATE_PROFILE_RUNNER:-}" ]; then
  run_tasks 0 "$@"
  exit 0
fi

bundle=$(mktemp "${TMPDIR:-/tmp}/yach-eval-profiles.XXXXXX")
trap 'rm -f "$bundle"' EXIT
chmod 600 "$bundle"

profile_index=0
for profile in "${profiles[@]}"; do
  while IFS= read -r line || [ -n "$line" ]; do
    if [[ "$line" =~ ^[[:space:]]*$ ]] || [[ "$line" =~ ^[[:space:]]*\# ]]; then
      continue
    fi
    if ! [[ "$line" =~ ^[[:space:]]*(YACH_RIG_[A-Z0-9_]+)=(.*)$ ]]; then
      echo "invalid profile assignment in $profile: $line" >&2
      exit 2
    fi
    key=${BASH_REMATCH[1]}
    value=${BASH_REMATCH[2]}
    printf 'YACH_EVAL_PROFILE_%s_%s=%s\n' \
      "$profile_index" "$key" "$value" >> "$bundle"
  done < "$profile"
  profile_index=$((profile_index + 1))
done

"$YACH_ROTATE_PROFILE_RUNNER" "$bundle" \
  bash "$script_dir/matrix.sh" --resolved \
  "$profiles_dir" "$outdir" "$repeat" "$@"
