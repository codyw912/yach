set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
  just --list

run *args:
  cargo run -- {{args}}

build:
  cargo build

build-linux-arm64:
  nix build .#packages.aarch64-linux.default

build-linux-x86_64:
  nix build .#packages.x86_64-linux.default

build-linux-x86_64-remote:
  just build-linux-x86_64

build-linux-x86_64-orb:
  orb_machine="${ORB_X86_MACHINE:-}" && \
  orb_user="${ORB_X86_USER:-$(id -un)}" && \
  if [ -z "$orb_machine" ]; then \
    if ! command -v orb >/dev/null 2>&1; then \
      echo "OrbStack CLI not found; install OrbStack or set ORB_X86_MACHINE manually" >&2; \
      exit 1; \
    fi; \
    if orb list | awk '$1 == "x86-builder" && $5 == "amd64" { found=1 } END { exit(found ? 0 : 1) }'; then \
      orb_machine="x86-builder"; \
    else \
      candidates=$(orb list | awk '$5 == "amd64" { print $1 }'); \
      count=$(printf '%s\n' "$candidates" | awk 'NF { count += 1 } END { print count + 0 }'); \
      if [ "$count" -eq 1 ]; then \
        orb_machine="$candidates"; \
      elif [ "$count" -eq 0 ]; then \
        echo "No OrbStack amd64 machine found; create one or set ORB_X86_MACHINE" >&2; \
        exit 1; \
      else \
        echo "Multiple OrbStack amd64 machines found; set ORB_X86_MACHINE to choose one" >&2; \
        printf '%s\n' "$candidates" >&2; \
        exit 1; \
      fi; \
    fi; \
  fi && \
  remote_store=$(printf 'ssh-ng://%s%%40%s@127.0.0.1' "$orb_user" "$orb_machine") && \
  export NIX_SSHOPTS="-p 32222 -i $HOME/.orbstack/ssh/id_ed25519 -o IdentitiesOnly=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null" && \
  out=$(nix build --store "$remote_store" .#packages.x86_64-linux.default --print-out-paths --no-link) && \
  nix copy --no-check-sigs --from "$remote_store" "$out" && \
  ln -sfn "$out" result-x86_64-linux-orb && \
  printf '%s\n' "$out"

check:
  cargo check

test:
  cargo test

fmt:
  cargo fmt --all

lint:
  cargo clippy --all-targets --all-features -- -D warnings

smoke-build target="x86_64-unknown-linux-musl":
  cargo zigbuild --target {{target}}

smoke-build-release target="x86_64-unknown-linux-musl":
  cargo zigbuild --release --target {{target}}

smoke-x86_64-release:
  cargo zigbuild --release --target x86_64-unknown-linux-musl

smoke-aarch64-release:
  cargo zigbuild --release --target aarch64-unknown-linux-musl

cross-build target="x86_64-unknown-linux-musl":
  just smoke-build {{target}}

cross-build-release target="x86_64-unknown-linux-musl":
  just smoke-build-release {{target}}

cross-x86_64-release:
  just smoke-x86_64-release

cross-aarch64-release:
  just smoke-aarch64-release

cross-bin target="x86_64-unknown-linux-musl" profile="release":
  printf '%s/%s/%s\n' "${CARGO_TARGET_DIR:-target}" "{{target}}" "{{profile}}"

release-arm64-bin:
  printf 'result/bin\n'

release-x86_64-bin:
  printf 'result/bin\n'

remote-x86-bin:
  just release-x86_64-bin

orb-x86-bin:
  printf 'result-x86_64-linux-orb/bin\n'
