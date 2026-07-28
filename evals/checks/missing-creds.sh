#!/bin/bash
# Driver contract: with no provider credentials, yach run must exit 2
# (setup error) and emit no outcome document on stdout. Model-free and
# credential-free; needs only the yach-runtime image.
set -uo pipefail

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

out=$(docker run --rm -v "$scratch:/work" yach-runtime \
  yach run --quiet --prompt "hello" 2>/dev/null)
code=$?

if [ "$code" -ne 2 ]; then
  echo "check: expected exit 2 (setup error), got $code" >&2
  exit 1
fi
if [ -n "$out" ]; then
  echo "check: expected empty stdout on setup error, got: $out" >&2
  exit 1
fi
