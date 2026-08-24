set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set positional-arguments

publish_crates := "yach-proto yach-catalog yach-connections yach-hashline-extension yach-ui yach-backend yach"

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

# One-shot sync: move onto merged main and rebuild that binary.
# This fetches and runs `jj new main@origin`, so it leaves any local
# stack. Use `just local` to rebuild the current checkout instead.
sync:
  jj git fetch
  jj new main@origin
  just --justfile "{{justfile()}}" dev cargo build

# Rebuild the current working copy without fetching or moving `@`.
local:
  just --justfile "{{justfile()}}" dev cargo build

run *args:
  just --justfile "{{justfile()}}" dev cargo run -p yach -- {{args}}

# Render deterministic TUI recordings; pass basenames to select tapes.
tui-visual *tapes:
  just --justfile "{{justfile()}}" dev bash tests/visual/render.sh {{tapes}}

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

# Refresh the baked Codex subscription catalog from the pinned Codex commit.
# Default: fetch models.json from openai/codex at crates/yach-catalog/data/codex-models.pin.
# Local override: set both CODEX_MODELS_JSON (path) and CODEX_MODELS_PIN (commit).
catalog-codex-snapshot:
  #!/usr/bin/env bash
  set -euo pipefail
  pin_file=crates/yach-catalog/data/codex-models.pin
  dest=crates/yach-catalog/data/codex-models.json
  if [[ -n "${CODEX_MODELS_JSON:-}" ]]; then
    if [[ -z "${CODEX_MODELS_PIN:-}" ]]; then
      echo "catalog-codex-snapshot: CODEX_MODELS_JSON requires CODEX_MODELS_PIN" >&2
      exit 1
    fi
    if [[ ! -f "$CODEX_MODELS_JSON" ]]; then
      echo "catalog-codex-snapshot: CODEX_MODELS_JSON is not a file: $CODEX_MODELS_JSON" >&2
      exit 1
    fi
    cp "$CODEX_MODELS_JSON" "$dest"
    printf '%s\n' "$CODEX_MODELS_PIN" > "$pin_file"
    echo "wrote $dest from $CODEX_MODELS_JSON (pin $CODEX_MODELS_PIN)"
    exit 0
  fi
  pin="${CODEX_MODELS_PIN:-$(cat "$pin_file")}"
  src="$(mktemp)"
  trap 'rm -f "$src"' EXIT
  curl -sfL "https://raw.githubusercontent.com/openai/codex/${pin}/codex-rs/models-manager/models.json" -o "$src"
  cp "$src" "$dest"
  if [[ -n "${CODEX_MODELS_PIN:-}" ]]; then
    printf '%s\n' "$CODEX_MODELS_PIN" > "$pin_file"
  fi
  echo "wrote $dest from openai/codex@$pin"


# Validate every eval task's verifier against its oracle solution — no
# model calls, no secrets, no containers. Design:
# docs/superpowers/specs/2026-07-28-eval-portfolio-design.md
eval-validate:
  bash evals/scripts/validate.sh

# Adaptive regression gate: every eval task against a pinned cheap model in
# the yach-runtime container, then the driver-contract checks. Needs docker,
# jq, and resolved YACH_RIG_* provider variables. YACH_EVAL_MODEL overrides
# the model; YACH_EVAL_FALLBACK_RUNNER and YACH_EVAL_FALLBACK_MODEL configure
# bounded provider fallback. Attempt artifacts persist under evals/.gate/.
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

# Validate every publishable package before any registry mutation. Package
# listing checks Cargo's distributable source selection without resolving
# newly-versioned sibling crates that are not in the registry yet.
# Verify synchronized versions, tests, and package contents without uploading.
release-check:
  #!/usr/bin/env bash
  set -euo pipefail
  crates=({{publish_crates}})
  metadata="$(just --justfile "{{justfile()}}" dev cargo metadata --locked --no-deps --format-version 1)"
  declare -A expected=()
  for crate in "${crates[@]}"; do
    expected["$crate"]=1
  done
  mapfile -t publishable < <(
    jq -r '.packages[]
      | select(((.publish // ["crates-io"]) | length) > 0)
      | .name' <<<"$metadata"
  )
  if [[ "${#publishable[@]}" -ne "${#crates[@]}" ]]; then
    echo "publishable package set changed; update publish_crates before releasing" >&2
    exit 2
  fi
  for crate in "${publishable[@]}"; do
    if [[ -z "${expected[$crate]+present}" ]]; then
      echo "publishable package '$crate' is missing from publish_crates" >&2
      exit 2
    fi
  done
  release_version=""
  for crate in "${crates[@]}"; do
    version="$(jq -r --arg crate "$crate" \
      '.packages[] | select(.name == $crate) | .version' <<<"$metadata")"
    if [[ -z "$version" || "$version" == "null" ]]; then
      echo "publishable package '$crate' is missing from cargo metadata" >&2
      exit 2
    fi
    if [[ -z "$release_version" ]]; then
      release_version="$version"
    elif [[ "$version" != "$release_version" ]]; then
      echo "publishable package '$crate' is $version; expected $release_version" >&2
      exit 2
    fi
  done
  rig_requirement="$(jq -r \
    '.packages[]
      | select(.name == "yach-backend")
      | .dependencies[]
      | select(.name == "rig-core")
      | .req' <<<"$metadata")"
  while IFS=$'\t' read -r package dependency requirement; do
    if [[ -z "${expected[$dependency]+present}" ]]; then
      echo "publishable package '$package' depends on unsequenced workspace crate '$dependency'" >&2
      exit 2
    fi
    if [[ "$requirement" != "^$release_version" ]]; then
      echo "$package requires $dependency $requirement; expected ^$release_version" >&2
      exit 2
    fi
  done < <(
    jq -r '.packages[]
      | select(((.publish // ["crates-io"]) | length) > 0)
      | .name as $package
      | .dependencies[]
      | select(.path != null)
      | [$package, .name, .req]
      | @tsv' <<<"$metadata"
  )
  just --justfile "{{justfile()}}" fmt-check
  just --justfile "{{justfile()}}" lint
  just --justfile "{{justfile()}}" test
  just --justfile "{{justfile()}}" eval-validate
  for crate in "${crates[@]}"; do
    just --justfile "{{justfile()}}" dev cargo package \
      --locked --allow-dirty --list -p "$crate" >/dev/null
  done
  probe_root="$(mktemp -d "${TMPDIR:-/tmp}/yach-release-rig-probe.XXXXXX")"
  trap 'rm -rf "$probe_root"' EXIT
  mkdir -p "$probe_root/crates"
  cp -R crates/yach-backend crates/yach-connections crates/yach-proto \
    "$probe_root/crates/"
  cat >"$probe_root/Cargo.toml" <<'EOF'
  [workspace]
  members = [
    "crates/yach-backend",
    "crates/yach-connections",
    "crates/yach-proto",
  ]
  resolver = "2"

  [workspace.lints.clippy]
  EOF
  if ! just --justfile "{{justfile()}}" dev cargo check \
    --manifest-path "$probe_root/Cargo.toml" -p yach-backend; then
    echo "release blocked: packaged yach-backend does not build against registry rig-core $rig_requirement" >&2
    echo "upstream/release the vendored Rig changes or use a published owned crate" >&2
    exit 2
  fi
  resolved_metadata="$(
    just --justfile "{{justfile()}}" dev cargo metadata --locked --format-version 1
  )"
  rig_manifest="$(jq -r \
    '.packages[] | select(.name == "rig-core") | .manifest_path' \
    <<<"$resolved_metadata")"
  if [[ "$rig_manifest" == "$(pwd)/vendor/rig-core/Cargo.toml" ]]; then
    echo "release blocked: [patch.crates-io] still resolves rig-core from vendor/rig-core" >&2
    echo "remove the patch only after its load-bearing changes are registry-resolvable" >&2
    exit 2
  fi
  rm -rf "$probe_root"
  trap - EXIT
  echo "release preflight passed for ${#crates[@]} crates at $release_version"

# Publish synchronized workspace crates to crates.io in dependency order.
# This is resume-safe before the final `yach` upload: versions already visible
# in the registry are skipped after Cargo's index polling completes.
# Publish the synchronized release from clean, up-to-date main.
publish:
  #!/usr/bin/env bash
  set -euo pipefail
  crates=({{publish_crates}})
  if [[ -n "$(jj diff --summary)" ]]; then
    echo "working copy has changes; checkpoint or abandon them before publishing" >&2
    exit 2
  fi
  jj git fetch --remote origin
  if [[ -n "$(jj diff --summary)" ]]; then
    echo "fetch changed the working copy; reconcile it before publishing" >&2
    exit 2
  fi
  main_commit="$(jj log -r main --no-graph -T 'commit_id ++ "\n"')"
  origin_main_commit="$(jj log -r 'main@origin' --no-graph -T 'commit_id ++ "\n"')"
  parent_commit="$(jj log -r '@-' --no-graph -T 'commit_id ++ "\n"')"
  if [[ "$main_commit" != "$origin_main_commit" ]]; then
    echo "local main is not synchronized with main@origin" >&2
    exit 2
  fi
  if [[ "$parent_commit" != "$main_commit" ]]; then
    echo "working copy parent is not main; run 'jj rebase -r @ -d main' after the release change merges" >&2
    exit 2
  fi
  if [[ -n "$(jj log -r '@ | main' --no-graph -T 'if(conflict, "conflict\n", "")')" ]]; then
    echo "working copy or main contains unresolved conflicts" >&2
    exit 2
  fi
  just --justfile "{{justfile()}}" release-check
  metadata="$(just --justfile "{{justfile()}}" dev cargo metadata --locked --no-deps --format-version 1)"
  release_version="$(jq -r --arg crate yach \
    '.packages[] | select(.name == $crate) | .version' <<<"$metadata")"
  if [[ "${YACH_RELEASE_EVAL_GATE_VERSION:-}" != "$release_version" ]]; then
    echo "live release evidence is missing for yach $release_version" >&2
    echo "run 'just eval-gate' with the pinned live profile, inspect the green evidence," >&2
    echo "then set YACH_RELEASE_EVAL_GATE_VERSION=$release_version for this publish invocation" >&2
    exit 2
  fi
  registry_has_version() {
    local output
    if output="$(just --justfile "{{justfile()}}" dev cargo info \
      --registry crates-io "$1@$release_version" 2>&1)"; then
      return 0
    fi
    if [[ "$output" == *"could not find"* ]]; then
      return 1
    fi
    printf '%s\n' "$output" >&2
    exit 2
  }
  if registry_has_version yach; then
    echo "yach $release_version is already published; bump every publishable crate first" >&2
    exit 2
  fi
  for crate in "${crates[@]}"; do
    if registry_has_version "$crate"; then
      echo "skipping $crate $release_version (already published)"
      continue
    fi
    just --justfile "{{justfile()}}" dev cargo publish \
      --locked --registry crates-io -p "$crate"
  done
