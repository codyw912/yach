#!/bin/bash
# Activate one profile from a resolved, collision-free matrix environment.
# Only the selected profile's ordinary YACH_RIG_* variables reach the command.
set -euo pipefail

if [ "$#" -lt 3 ]; then
  echo "usage: $0 <profile-index> <profile-file> <command>..." >&2
  exit 2
fi

profile_index=$1
profile=$2
shift 2

while IFS= read -r inherited_name; do
  unset "$inherited_name"
done < <(compgen -A variable YACH_RIG_)

while IFS= read -r line || [ -n "$line" ]; do
  if [[ "$line" =~ ^[[:space:]]*$ ]] || [[ "$line" =~ ^[[:space:]]*\# ]]; then
    continue
  fi
  if ! [[ "$line" =~ ^[[:space:]]*(YACH_RIG_[A-Z0-9_]+)= ]]; then
    echo "invalid profile assignment in $profile: $line" >&2
    exit 2
  fi
  key=${BASH_REMATCH[1]}
  alias_name="YACH_EVAL_PROFILE_${profile_index}_${key}"
  if ! declare -p "$alias_name" >/dev/null 2>&1; then
    echo "resolved profile bundle is missing $alias_name" >&2
    exit 2
  fi
  value=${!alias_name}
  export "$key=$value"
done < "$profile"

while IFS= read -r alias_name; do
  unset "$alias_name"
done < <(compgen -A variable YACH_EVAL_PROFILE_)

exec "$@"
