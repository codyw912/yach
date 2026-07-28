#!/bin/bash
# Verifier: the tally script must produce correct counts when actually
# run, the README must exist, and the input notes must be untouched.
# Artifact assertions only — the response text is never consulted.
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

out=$(cd "$ws" && bash scripts/tally.sh 2>&1) || fail "tally.sh exited nonzero: $out"
echo "$out" | grep -qx 'done: 4' || fail "expected 'done: 4' in tally output, got: $out"
echo "$out" | grep -qx 'todo: 4' || fail "expected 'todo: 4' in tally output, got: $out"

[ -s "$ws/README.md" ] || fail "README.md missing or empty"

expected='5995919f7d7338134caeeca8b4028c06abb2c7d82a1fe42ee22bdc3e7cf7bde6  notes/2026-07-21.md
a01fb2429ec40a6f1c12c0e31bbab5f96997699ac0a2e0bd31180b0945d34e4b  notes/2026-07-23.md
259b8f10f3060ee39e8bc9f5eb44e967c6b235615e17e95983bd5c34cf71b5a8  notes/2026-07-25.md'
actual=$(cd "$ws" && sha notes/2026-07-21.md notes/2026-07-23.md notes/2026-07-25.md 2>/dev/null)
[ "$actual" = "$expected" ] || fail "input notes were modified"

echo 1 > "$logs/verifier/reward.txt"
