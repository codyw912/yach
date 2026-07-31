#!/bin/bash
# Digest of the sources that determine the yach binary inside the
# yach-runtime image.
#
# `just runtime-image` stamps this into the image; the eval runners
# recompute it and compare. Without that, a run after a code change
# silently measures the previous build — it completes, cells score, and
# the numbers look like a result.
#
# Defined once, used by both sides, so the two can never drift.
set -euo pipefail

cd "$(dirname "$0")/../.."

if command -v sha256sum >/dev/null 2>&1; then
  hash_cmd=sha256sum
else
  hash_cmd="shasum -a 256"
fi

# The Dockerfile builds `-p yach` from the whole tree, so the inputs that
# can change the binary are the crate sources and the workspace manifests.
find crates Cargo.toml Cargo.lock -type f \
  \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) \
  | LC_ALL=C sort \
  | xargs $hash_cmd \
  | $hash_cmd \
  | cut -d' ' -f1
