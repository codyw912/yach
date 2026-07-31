#!/bin/bash
# Refuse to measure against an image built from different sources than
# the working tree.
#
# Evals execute the binary inside yach-runtime, so a run after a code
# change describes the previous build unless the image is rebuilt. That
# failure is silent and convincing — the run completes and the cells
# score — so it is checked rather than documented.
set -uo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
want=$(bash "$evals_dir/scripts/source-digest.sh")
have=$(docker image inspect -f '{{index .Config.Labels "yach.source"}}' yach-runtime 2>/dev/null)

if [ "$have" != "$want" ]; then
  echo "yach-runtime was built from different sources than the working tree" >&2
  echo "run 'just runtime-image' first — evals run the container binary, so" >&2
  echo "results would describe the previous build" >&2
  exit 2
fi
