#!/bin/bash
# Verifier: exploration must leave the workspace byte-identical (the
# session plumbing under .yach/ and .yach-eval/ excluded) and the
# outcome document must show a completed turn that actually read the
# project — asserted on outcome fields, never on response prose.
set -uo pipefail

ws="${EVAL_WORKSPACE:-/app}"
logs="${EVAL_LOGS_DIR:-/logs}"
mkdir -p "$logs/verifier"

fail() {
  echo "verifier: $*" >&2
  echo 0 > "$logs/verifier/reward.txt"
  exit 0
}

sha() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$@"; else shasum -a 256 "$@"; fi
}

command -v jq >/dev/null 2>&1 || fail "verifier needs jq"
outcome="$ws/.yach-eval/outcome.json"
[ -f "$outcome" ] || fail "outcome document missing"

jq -e '.outcome == "completed"' "$outcome" >/dev/null \
  || fail "outcome is not completed: $(jq -r '.outcome' "$outcome")"
jq -e '([.turns[].tool_calls[]
        | select(.name == "read_text_file" or .name == "list_project_paths"
                 or .name == "search_project_text")
        | .count] | add // 0) >= 1' "$outcome" >/dev/null \
  || fail "no read/list/search tool calls recorded in the outcome document"

count=$(cd "$ws" && find . -type f ! -path './.yach/*' ! -path './.yach-eval/*' | wc -l | tr -d ' ')
[ "$count" = "5" ] || fail "workspace file set changed (expected 5 files, found $count)"

expected='b09a26de9be19d914446d7422eb43a0d9eaf077ab7c9139837f294d80416e6c3  TODO.md
5995919f7d7338134caeeca8b4028c06abb2c7d82a1fe42ee22bdc3e7cf7bde6  notes/2026-07-21.md
a01fb2429ec40a6f1c12c0e31bbab5f96997699ac0a2e0bd31180b0945d34e4b  notes/2026-07-23.md
259b8f10f3060ee39e8bc9f5eb44e967c6b235615e17e95983bd5c34cf71b5a8  notes/2026-07-25.md
5f562c8533179ba99f669606cd1aec62bf3cf98a14d5e99fa22bd91955ad9e56  scripts/tally.sh'
actual=$(cd "$ws" && sha TODO.md notes/2026-07-21.md notes/2026-07-23.md notes/2026-07-25.md scripts/tally.sh 2>/dev/null)
[ "$actual" = "$expected" ] || fail "workspace files were modified"

echo 1 > "$logs/verifier/reward.txt"
