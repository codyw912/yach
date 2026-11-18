#!/usr/bin/env bash
# Prove that vendor/rig-core is exactly the published yach-rig-core release
# that yach-backend requires, so a local vendor edit cannot ship unpublished.
#
# Usage: scripts/vendor-rig-check.sh [published.crate]
# Without an argument the published crate is downloaded from crates.io.
# Run from the repository root inside the dev shell.
set -euo pipefail

vendor=vendor/rig-core
metadata="$(cargo metadata --locked --format-version 1)"
read -r name version manifest < <(jq -r \
  '.packages[] | select(.name == "yach-rig-core")
    | "\(.name) \(.version) \(.manifest_path)"' <<<"$metadata")
if [[ "$manifest" != "$(pwd)/$vendor/Cargo.toml" ]]; then
  echo "yach-rig-core resolves from $manifest, not $vendor" >&2
  exit 2
fi
requirement="$(jq -r '.packages[] | select(.name == "yach-backend")
  | .dependencies[] | select(.name == "yach-rig-core") | .req' <<<"$metadata")"
if [[ "$requirement" != "^$version" ]]; then
  echo "yach-backend requires yach-rig-core $requirement; vendor is $version" >&2
  exit 2
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/yach-vendor-rig-check.XXXXXX")"
trap 'rm -rf "$work"' EXIT
published="${1:-}"
if [[ -z "$published" ]]; then
  published="$work/published.crate"
  if ! curl -fsSL -A "yach release-check" -o "$published" \
    "https://static.crates.io/crates/$name/$name-$version.crate"; then
    echo "$name $version is not published on crates.io; publish the vendor first" >&2
    exit 2
  fi
fi

cargo package --quiet --locked --no-verify --allow-dirty \
  --manifest-path "$vendor/Cargo.toml" --target-dir "$work/target"
mkdir "$work/local" "$work/published"
tar -xzf "$work/target/package/$name-$version.crate" -C "$work/local"
tar -xzf "$published" -C "$work/published"

# Cargo regenerates Cargo.toml and .cargo_vcs_info.json when packaging; the
# source manifest is compared through Cargo.toml.orig.
if ! diff -r --exclude=Cargo.toml --exclude=.cargo_vcs_info.json \
  "$work/published/$name-$version" "$work/local/$name-$version"; then
  echo "$vendor differs from published $name $version; bump its version and publish it" >&2
  exit 2
fi
echo "$vendor matches published $name $version"
