set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set positional-arguments

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

# One-shot sync: update the working copy to merged main and rebuild the local binary.
sync:
  jj git fetch
  jj new main@origin
  just --justfile "{{justfile()}}" dev cargo build

run *args:
  just --justfile "{{justfile()}}" dev cargo run -p yach -- {{args}}

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

# Regenerate the baked model catalog from models.dev (build-time tool;
# the runtime never fetches). Review the data diff like any change.
catalog-snapshot:
  curl -sf https://models.dev/api.json -o /tmp/models-dev-api.json
  cargo run -p yach-catalog --bin snapshot -- /tmp/models-dev-api.json crates/yach-catalog/data/catalog.json "$(date +%F)"

# Validate every eval task's verifier against its oracle solution — no
# model calls, no secrets, no containers. Design:
# docs/superpowers/specs/2026-07-28-eval-portfolio-design.md
eval-validate:
  bash evals/scripts/validate.sh

# Regression gate: every eval task against a pinned cheap model in the
# yach-runtime container, then the driver-contract checks. Needs docker
# and YACH_RIG_* provider variables; YACH_EVAL_MODEL overrides the
# model. Run artifacts persist under evals/.gate/ for inspection.
eval-gate:
  bash evals/scripts/gate.sh

# Provider-matrix sweep of one eval task. Profiles own provider and model via
# YACH_RIG_* variables. A configured YACH_ROTATE_PROFILE_RUNNER receives one
# generated, collision-free dotenv bundle and wraps the whole sweep once.
# Rows append to <outdir>/results.tsv; per-cell artifacts land below
# <outdir>/<task>/<name>-rN/.
# Usage: just eval-sweep <profiles-dir> <task-dir> <outdir> [repeat]
eval-sweep profiles task outdir repeat="1":
  bash evals/scripts/matrix.sh "{{absolute_path(profiles)}}" "{{absolute_path(outdir)}}" "{{repeat}}" "{{absolute_path(task)}}"

# Multi-task provider matrix under the same one-shot profile-runner boundary.
# Usage: just eval-matrix <profiles-dir> <outdir> <repeat> <task-dir>...
eval-matrix profiles outdir repeat *tasks:
  #!/usr/bin/env bash
  set -euo pipefail
  invocation_dir={{quote(invocation_directory())}}
  task_paths=()
  for task in "${@:4}"; do
    if [[ "$task" = /* ]]; then
      task_paths+=("$task")
    else
      task_paths+=("$invocation_dir/$task")
    fi
  done
  bash evals/scripts/matrix.sh "{{absolute_path(profiles)}}" "{{absolute_path(outdir)}}" "{{repeat}}" "${task_paths[@]}"

# Build the yach-runtime container image for isolated headless runs.
runtime-image:
  docker build --label yach.source="$(bash evals/scripts/source-digest.sh)" -f containers/yach-runtime/Dockerfile -t yach-runtime .

# One isolated headless session (a single rotation cell): the fixture
# directory is mounted at /work and is the only host-visible path;
# YACH_RIG_* vars pass through from the environment as-is — resolve any
# secret references with your secret manager before invoking.
# Usage: just run-isolated <fixture-dir> --prompt "..." [more `yach run` flags]
run-isolated fixture *args:
  docker run --rm \
    $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
    -v "{{absolute_path(fixture)}}:/work" \
    yach-runtime yach run --full-auto {{args}}

# Provider-matrix rotation: one isolated cell per <name>.env profile in
# the profiles directory. Each profile defines one cell's YACH_RIG_* vars
# (keep profile dirs untracked). By default values are used as-is; to
# keep secret references in profiles instead of plain values, set
# YACH_ROTATE_PROFILE_RUNNER to a command that resolves them — it is
# invoked as `<runner> <profile-file> <cell command...>` and must exec
# the cell command with the profile's variables resolved and exported.
# Every cell gets a fresh copy of the fixture template; outcomes land in
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
    if [[ -n "${YACH_ROTATE_PROFILE_RUNNER:-}" ]]; then
      cell=("$YACH_ROTATE_PROFILE_RUNNER" "$profile" \
        just --justfile "{{justfile()}}" run-isolated "$fixture" {{args}})
    else
      cell=(env $(grep -v '^\s*#' "$profile" | grep -v '^\s*$' | xargs) \
        just --justfile "{{justfile()}}" run-isolated "$fixture" {{args}})
    fi
    if "${cell[@]}" > "{{absolute_path(outdir)}}/$name.json"; then
      echo "=== cell $name: exit 0 ===" >&2
    else
      echo "=== cell $name: exit $? ===" >&2
    fi
  done

# Publish the workspace crates to crates.io in dependency order. Must run
# from a synced, clean working copy: cargo publish refuses uncommitted
# changes, and jj's checked-out change reads as dirty to git.
publish:
  #!/usr/bin/env bash
  set -euo pipefail
  if [[ -n "$(git status --porcelain)" ]]; then
    echo "working copy is not clean — run 'jj new main@origin' first" >&2
    exit 2
  fi
  for crate in yach-proto yach-ui yach-backend yach; do
    just --justfile "{{justfile()}}" dev cargo publish -p "$crate"
  done
