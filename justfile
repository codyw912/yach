set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
  just --list

@dev +args:
  if [[ -n "${DEVENV_PROFILE:-}" || -n "${IN_NIX_SHELL:-}" ]]; then \
    {{args}}; \
  elif command -v direnv >/dev/null 2>&1 && [[ -f .envrc ]]; then \
    direnv exec . {{args}}; \
  else \
    nix develop --no-pure-eval -c {{args}}; \
  fi

@dev-shell command:
  if [[ -n "${DEVENV_PROFILE:-}" || -n "${IN_NIX_SHELL:-}" ]]; then \
    bash -lc {{quote(command)}}; \
  elif command -v direnv >/dev/null 2>&1 && [[ -f .envrc ]]; then \
    direnv exec . bash -lc {{quote(command)}}; \
  else \
    nix develop --no-pure-eval -c bash -lc {{quote(command)}}; \
  fi

# One-shot sync: update the working copy to merged main and rebuild the dogfood binary.
sync:
  jj git fetch
  jj new main@origin
  just --justfile "{{justfile()}}" dev cargo build

run *args:
  just --justfile "{{justfile()}}" dev cargo run -p yach-cli -- {{args}}

build:
  just --justfile "{{justfile()}}" dev cargo build

check:
  just --justfile "{{justfile()}}" dev cargo check

test:
  just --justfile "{{justfile()}}" dev cargo test

fmt:
  just --justfile "{{justfile()}}" dev cargo fmt --all

fmt-check:
  just --justfile "{{justfile()}}" dev cargo fmt --all --check

lint:
  just --justfile "{{justfile()}}" dev cargo clippy --all-targets --all-features -- -D warnings

# Build the yach-runtime container image for isolated headless runs.
runtime-image:
  docker build -f containers/yach-runtime/Dockerfile -t yach-runtime .

# One isolated headless session (a single rotation cell): the fixture
# directory is mounted at /work and is the only host-visible path;
# YACH_RIG_* vars pass through from the environment as-is — resolve any
# secret references with your secret manager before invoking.
# Usage: just run-isolated <fixture-dir> --prompt "..." [more `yach run` flags]
run-isolated fixture *args:
  docker run --rm \
    $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
    -v "{{absolute_path(fixture)}}:/work" \
    yach-runtime run --full-auto {{args}}

# Provider-matrix rotation: one isolated cell per <name>.env profile in
# the profiles directory. Each profile defines one cell's YACH_RIG_* vars
# (plain values, used as-is; keep profile dirs untracked). Every cell
# gets a fresh copy of the fixture template; outcomes land in
# <outdir>/<name>.json with session logs inside <outdir>/<name>-fixture/.
# Usage: just rotate <profiles-dir> <template-dir> <outdir> [yach run flags]
rotate profiles template outdir *args:
  #!/usr/bin/env bash
  set -euo pipefail
  shopt -s nullglob
  profiles=({{absolute_path(profiles)}}/*.env)
  if [[ ${#profiles[@]} -eq 0 ]]; then
    echo "no *.env profiles in {{profiles}}" >&2
    exit 2
  fi
  mkdir -p "{{absolute_path(outdir)}}"
  for profile in "${profiles[@]}"; do
    name=$(basename "$profile" .env)
    fixture="{{absolute_path(outdir)}}/$name-fixture"
    rm -rf "$fixture"
    cp -R "{{absolute_path(template)}}" "$fixture"
    echo "=== rotation cell: $name ===" >&2
    if env $(grep -v '^\s*#' "$profile" | grep -v '^\s*$' | xargs) \
      just --justfile "{{justfile()}}" run-isolated "$fixture" {{args}} \
      > "{{absolute_path(outdir)}}/$name.json"; then
      echo "=== cell $name: exit 0 ===" >&2
    else
      echo "=== cell $name: exit $? ===" >&2
    fi
  done
