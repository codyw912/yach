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

# Isolated headless session: the fixture directory is mounted at /work and
# is the only host-visible path; YACH_RIG_* creds pass through from the
# environment (wrap with `op run --env-file .env.local --` locally).
# Usage: just rotate <fixture-dir> --prompt "..." [more `yach run` flags]
rotate fixture *args:
  docker run --rm \
    $(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ') \
    -v "{{absolute_path(fixture)}}:/work" \
    yach-runtime run --full-auto {{args}}
