#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
visual_dir="$repo_root/tests/visual"
artifact_root="$repo_root/target/tui-visual"
workspace="$artifact_root/workspace"

cd "$repo_root"
cargo build -p yach

rm -rf "$artifact_root"
mkdir -p "$artifact_root/home" "$artifact_root/sessions" "$workspace"
cp "$visual_dir/session.jsonl" "$artifact_root/sessions/session-visual.jsonl"

export HOME="$artifact_root/home"
export YACH_SESSION_DIR="$artifact_root/sessions"
export YACH_VISUAL_ROOT="$artifact_root"
export YACH_VISUAL_BINARY="${CARGO_TARGET_DIR:-$repo_root/target}/debug/yach"

if (($# == 0)); then
  set -- session narrow
fi

for name in "$@"; do
  name="${name%.tape}"
  tape="$visual_dir/$name.tape"
  if [[ ! -f "$tape" ]]; then
    printf 'unknown visual tape: %s\n' "$name" >&2
    exit 2
  fi
  vhs "$tape"
done

printf 'visual artifacts: %s\n' "$artifact_root"
