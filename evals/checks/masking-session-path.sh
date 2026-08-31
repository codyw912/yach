#!/usr/bin/env bash
# Deterministic driver check: the masking task must resume its seeded fixture
# explicitly instead of resolving a same-named session from user state.
set -euo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
workspace="$scratch/workspace"
mkdir -p "$workspace" "$scratch/bin"
cp -R "$evals_dir/tasks/masking-reclaim/fixture/." "$workspace/"

cat >"$scratch/bin/yach" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
session_path=""
while (($# > 0)); do
  case "$1" in
    --session-path)
      session_path=${2:-}
      shift 2
      ;;
    --session)
      echo "masking task used project-keyed --session instead of its seeded file" >&2
      exit 1
      ;;
    *) shift ;;
  esac
done
if [[ "$session_path" != "$EXPECTED_SESSION_PATH" ]]; then
  echo "masking task session path mismatch: $session_path" >&2
  exit 1
fi
touch "$MARKER"
EOF
chmod +x "$scratch/bin/yach"

marker="$scratch/driver-passed"
(
  cd "$workspace"
  PATH="$scratch/bin:$PATH" \
    EXPECTED_SESSION_PATH="$workspace/.yach/sessions/eval-masking.jsonl" \
    MARKER="$marker" \
    bash "$evals_dir/tasks/masking-reclaim/run.sh"
)
[[ -f "$marker" ]]
