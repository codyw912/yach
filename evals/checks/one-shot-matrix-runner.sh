#!/bin/bash
# Resolver-neutral regression for one-shot eval matrix profile loading.
set -euo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
matrix="$evals_dir/scripts/matrix.sh"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

mkdir -p "$scratch/bin" "$scratch/profiles"
jq_path=$(command -v jq) || {
  echo 'FAIL setup: jq is required' >&2
  exit 1
}
ln -s "$jq_path" "$scratch/bin/jq"
for name in alpha beta; do
  mkdir -p "$scratch/task-$name/fixture" "$scratch/task-$name/tests"
  printf '%s\n' '#!/bin/bash' 'exit 0' > "$scratch/task-$name/tests/test.sh"
  chmod +x "$scratch/task-$name/tests/test.sh"
done
mkdir -p "$scratch/duplicate/task-alpha/fixture" "$scratch/duplicate/task-alpha/tests"
printf '%s\n' '#!/bin/bash' 'exit 0' > "$scratch/duplicate/task-alpha/tests/test.sh"
chmod +x "$scratch/duplicate/task-alpha/tests/test.sh"
mkdir -p "$scratch/missing-fixture/tests"
printf '%s\n' '#!/bin/bash' 'exit 0' > "$scratch/missing-fixture/tests/test.sh"
chmod +x "$scratch/missing-fixture/tests/test.sh"
cat > "$scratch/disable-compgen" <<'EOF'
# Match runner shells built without programmable-completion builtins.
enable -n compgen 2>/dev/null || true
EOF


cat > "$scratch/profiles/alpha.env" <<'EOF'
# The values are deliberately opaque to the matrix driver.
YACH_RIG_PROVIDER=rig-openai
YACH_RIG_OPENAI_API_KEY=opaque-alpha
YACH_RIG_OPENAI_MODEL=alpha-model
EOF
cat > "$scratch/profiles/beta.env" <<'EOF'
YACH_RIG_PROVIDER=rig-openai
YACH_RIG_OPENAI_API_KEY=opaque-beta
YACH_RIG_OPENAI_MODEL=beta-model
EOF

cat > "$scratch/bin/docker" <<'EOF'
#!/bin/bash
set -eu
if [ "${1:-}" = "image" ] && [ "${2:-}" = "inspect" ]; then
  if [ -n "${FAKE_PREFLIGHT_COUNT:-}" ]; then
    count=0
    if [ -f "$FAKE_PREFLIGHT_COUNT" ]; then
      count=$(cat "$FAKE_PREFLIGHT_COUNT")
    fi
    printf '%s\n' "$((count + 1))" > "$FAKE_PREFLIGHT_COUNT"
  fi
  if [ "${FAKE_DOCKER_MODE:-ready}" = "missing" ]; then
    exit 1
  fi
  if [ "${3:-}" = "-f" ]; then
    printf '%s\n' "$FAKE_SOURCE_DIGEST"
  fi
  exit 0
fi
if [ "${1:-}" != "run" ]; then
  echo 'unexpected fake docker invocation' >&2
  exit 2
fi

alias_leaked=0
for variable_name in "${!YACH_EVAL_PROFILE_@}"; do
  alias_leaked=1
done
if [ "$alias_leaked" -eq 1 ]; then
  printf 'profile alias reached cell subprocess\n' > "$FAKE_ALIAS_LEAK"
  exit 97
fi
if [ -n "${YACH_RIG_ANTHROPIC_API_KEY+x}" ]; then
  printf 'ambient profile variable reached cell subprocess\n' > "$FAKE_PROFILE_LEAK"
  exit 98
fi

logs=""
work=""
verifier=0
for argument in "$@"; do
  case "$argument" in
    *:/logs) logs=${argument%:/logs} ;;
    *:/work) work=${argument%:/work} ;;
    /task/tests/test.sh) verifier=1 ;;
  esac
done
if [ "$verifier" -eq 1 ]; then
  mkdir -p "$logs/verifier"
  if [ "${FAKE_AGENT_MODE:-pass}" != "pass" ] \
    && [ "$YACH_RIG_OPENAI_MODEL" = "alpha-model" ]; then
    printf '0\n' > "$logs/verifier/reward.txt"
    echo 'verifier: report.txt missing' >&2
  else
    printf '1\n' > "$logs/verifier/reward.txt"
  fi
else
  printf '%s\t%s\n' \
    "$YACH_RIG_OPENAI_MODEL" "$YACH_RIG_OPENAI_API_KEY" \
    >> "$FAKE_OBSERVED_PROFILES"
  count=0
  if [ -f "$FAKE_CELL_COUNT" ]; then
    count=$(cat "$FAKE_CELL_COUNT")
  fi
  printf '%s\n' "$((count + 1))" > "$FAKE_CELL_COUNT"
  if [ "$YACH_RIG_OPENAI_MODEL" = "alpha-model" ]; then
    mkdir -p "$work/.yach-eval"
    case "${FAKE_AGENT_MODE:-pass}" in
      provider-failure)
        printf '%s\n' \
          '{"outcome":"failed","turns":[{"outcome":"failed","failure_reason":"turn_end provider failed"}]}' \
          > "$work/.yach-eval/outcome.json"
        echo 'status: provider failed (rate_limited): quota exhausted' >&2
        echo 'status: turn_end provider failed' >&2
        exit 1
        ;;
      tool-loop-failure)
        printf '%s\n' \
          '{"outcome":"failed","turns":[{"outcome":"failed","failure_reason":"turn_end tool loop failed"}]}' \
          > "$work/.yach-eval/outcome.json"
        echo 'status: provider failed (invalid_request): Native provider tool loop stopped before completion' >&2
        exit 1
        ;;
      approval-required)
        printf '%s\n' \
          '{"outcome":"approval_required","turns":[{"outcome":"failed","failure_reason":"approval required for tool"}]}' \
          > "$work/.yach-eval/outcome.json"
        echo 'status: approval required for tool' >&2
        exit 3
        ;;
    esac
  fi
fi
EOF
chmod +x "$scratch/bin/docker"

cat > "$scratch/resolve-bundle" <<'EOF'
#!/bin/bash
set -euo pipefail
bundle=$1
shift
count=0
if [ -f "$FAKE_RUNNER_COUNT" ]; then
  count=$(cat "$FAKE_RUNNER_COUNT")
fi
printf '%s\n' "$((count + 1))" > "$FAKE_RUNNER_COUNT"
printf '%s\n' "$bundle" > "$FAKE_BUNDLE_PATH"
cp "$bundle" "$FAKE_BUNDLE_CAPTURE"
if [ "${FAKE_RUNNER_MODE:-resolve}" = "fail" ]; then
  exit 9
fi
while IFS= read -r assignment || [ -n "$assignment" ]; do
  key=${assignment%%=*}
  value=${assignment#*=}
  case "$value" in
    opaque-*) value="transformed-${value#opaque-}" ;;
  esac
  export "$key=$value"
done < "$bundle"
if [ "${FAKE_RUNNER_MODE:-resolve}" = "drop-profile" ]; then
  unset YACH_EVAL_PROFILE_1_YACH_RIG_OPENAI_API_KEY
fi
# The runner contract does not promise to preserve internal parent variables.
unset YACH_SWEEP_PREFLIGHT_DONE
exec "$@"
EOF
chmod +x "$scratch/resolve-bundle"

source_digest=$(bash "$evals_dir/scripts/source-digest.sh")
out="$scratch/out"
runner_count="$scratch/runner-count"
cell_count="$scratch/cell-count"
observed="$scratch/observed-profiles"
bundle_path_record="$scratch/bundle-path"
bundle_capture="$scratch/bundle-capture"
alias_leak="$scratch/alias-leak"
profile_leak="$scratch/profile-leak"
preflight_count="$scratch/preflight-count"

PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  BASH_ENV="$scratch/disable-compgen" \
  FAKE_SOURCE_DIGEST="$source_digest" \
  FAKE_RUNNER_COUNT="$runner_count" \
  FAKE_CELL_COUNT="$cell_count" \
  FAKE_OBSERVED_PROFILES="$observed" \
  FAKE_BUNDLE_PATH="$bundle_path_record" \
  FAKE_BUNDLE_CAPTURE="$bundle_capture" \
  FAKE_ALIAS_LEAK="$alias_leak" \
  FAKE_PROFILE_LEAK="$profile_leak" \
  FAKE_PREFLIGHT_COUNT="$preflight_count" \
  YACH_RIG_ANTHROPIC_API_KEY=ambient-must-not-reach \
  YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
  bash "$matrix" "$scratch/profiles" "$out" 2 \
    "$scratch/task-alpha" "$scratch/task-beta"

if [ "$(cat "$runner_count")" -ne 1 ]; then
  echo "FAIL runner count: expected 1, got $(cat "$runner_count")" >&2
  exit 1
fi
if [ "$(cat "$cell_count")" -ne 8 ]; then
  echo "FAIL cell count: expected 8, got $(cat "$cell_count")" >&2
  exit 1
fi
if [ -e "$alias_leak" ]; then
  cat "$alias_leak" >&2
  echo 'FAIL alias scrub: generated aliases reached a cell subprocess' >&2
  exit 1
fi
if [ -e "$profile_leak" ]; then
  cat "$profile_leak" >&2
  echo 'FAIL profile isolation: an ambient YACH_RIG_* variable reached a cell' >&2
  exit 1
fi
if grep -R -q 'compgen: command not found' "$out"; then
  echo 'FAIL runner shell: profile activation depends on compgen' >&2
  exit 1
fi
if [ "$(cat "$preflight_count")" -ne 2 ]; then
  echo "FAIL preflight count: expected 2 Docker inspections, got $(cat "$preflight_count")" >&2
  exit 1
fi
if [ "$(wc -l < "$out/results.tsv")" -ne 9 ]; then
  echo 'FAIL results: expected one header and eight result rows' >&2
  exit 1
fi
if ! awk -F '\t' '
  NR > 1 {
    key = $1 ":" $2 ":" $3
    if (!seen[key]++) unique++
    if ($4 != "1" || $5 != "0") bad = 1
  }
  END { exit bad || unique != 8 }
' "$out/results.tsv"; then
  echo 'FAIL results: matrix rows are missing, duplicated, or unsuccessful' >&2
  exit 1
fi
if [ "$(awk -F '\t' '$1 == "alpha-model" && $2 == "transformed-alpha" { count++ } END { print count + 0 }' "$observed")" -ne 4 ] \
  || [ "$(awk -F '\t' '$1 == "beta-model" && $2 == "transformed-beta" { count++ } END { print count + 0 }' "$observed")" -ne 4 ]; then
  echo 'FAIL profile activation: a colliding key resolved to the wrong profile value' >&2
  exit 1
fi
alpha_alias=$(awk -F= '$2 == "opaque-alpha" { print $1 }' "$bundle_capture")
beta_alias=$(awk -F= '$2 == "opaque-beta" { print $1 }' "$bundle_capture")
if [ -z "$alpha_alias" ] || [ -z "$beta_alias" ] || [ "$alpha_alias" = "$beta_alias" ]; then
  echo 'FAIL bundle: colliding profile keys did not receive distinct aliases' >&2
  exit 1
fi
bundle_path=$(cat "$bundle_path_record")
if [ -e "$bundle_path" ]; then
  echo 'FAIL cleanup: generated profile bundle still exists' >&2
  exit 1
fi
if grep -R -q 'transformed-alpha\|transformed-beta' "$out"; then
  echo 'FAIL persistence: resolved profile values reached matrix artifacts' >&2
  exit 1
fi

failed_runner_count="$scratch/failed-runner-count"
failed_cell_count="$scratch/failed-cell-count"
set +e
PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  FAKE_SOURCE_DIGEST="$source_digest" \
  FAKE_RUNNER_COUNT="$failed_runner_count" \
  FAKE_CELL_COUNT="$failed_cell_count" \
  FAKE_OBSERVED_PROFILES="$scratch/failed-observed" \
  FAKE_BUNDLE_PATH="$scratch/failed-bundle-path" \
  FAKE_BUNDLE_CAPTURE="$scratch/failed-bundle-capture" \
  FAKE_ALIAS_LEAK="$scratch/failed-alias-leak" \
  FAKE_RUNNER_MODE=fail \
  YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
  bash "$matrix" "$scratch/profiles" "$scratch/failed-out" 1 \
    "$scratch/task-alpha" >/dev/null 2>"$scratch/failed.stderr"
status=$?
set -e
if [ "$status" -ne 9 ] || [ "$(cat "$failed_runner_count")" -ne 1 ]; then
  echo 'FAIL runner failure: expected one invocation and exact status 9' >&2
  exit 1
fi
if [ -e "$failed_cell_count" ]; then
  echo 'FAIL runner failure: a cell launched after bundle resolution failed' >&2
  exit 1
fi

preflight_runner_count="$scratch/preflight-runner-count"
set +e
PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  FAKE_DOCKER_MODE=missing \
  FAKE_SOURCE_DIGEST="$source_digest" \
  FAKE_RUNNER_COUNT="$preflight_runner_count" \
  FAKE_BUNDLE_PATH="$scratch/preflight-bundle-path" \
  FAKE_BUNDLE_CAPTURE="$scratch/preflight-bundle-capture" \
  YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
  bash "$matrix" "$scratch/profiles" "$scratch/preflight-out" 1 \
    "$scratch/task-alpha" >/dev/null 2>"$scratch/preflight.stderr"
status=$?
set -e
if [ "$status" -ne 2 ] || [ -e "$preflight_runner_count" ]; then
  echo 'FAIL preflight: unavailable runtime should fail before invoking the runner' >&2
  exit 1
fi

for invalid_case in duplicate-task missing-fixture; do
  invalid_runner_count="$scratch/$invalid_case-runner-count"
  if [ "$invalid_case" = "duplicate-task" ]; then
    invalid_tasks=("$scratch/task-alpha" "$scratch/duplicate/task-alpha")
  else
    invalid_tasks=("$scratch/missing-fixture")
  fi
  set +e
  PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    FAKE_SOURCE_DIGEST="$source_digest" \
    FAKE_RUNNER_COUNT="$invalid_runner_count" \
    FAKE_BUNDLE_PATH="$scratch/$invalid_case-bundle-path" \
    FAKE_BUNDLE_CAPTURE="$scratch/$invalid_case-bundle-capture" \
    YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
    bash "$matrix" "$scratch/profiles" "$scratch/$invalid_case-out" 1 \
      "${invalid_tasks[@]}" >/dev/null 2>"$scratch/$invalid_case.stderr"
  status=$?
  set -e
  if [ "$status" -ne 2 ] || [ -e "$invalid_runner_count" ]; then
    echo "FAIL $invalid_case: invalid task inputs should fail before the runner" >&2
    exit 1
  fi
done

continued_out="$scratch/continued-out"
continued_runner_count="$scratch/continued-runner-count"
continued_cell_count="$scratch/continued-cell-count"
set +e
PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  FAKE_SOURCE_DIGEST="$source_digest" \
  FAKE_RUNNER_COUNT="$continued_runner_count" \
  FAKE_CELL_COUNT="$continued_cell_count" \
  FAKE_OBSERVED_PROFILES="$scratch/continued-observed" \
  FAKE_BUNDLE_PATH="$scratch/continued-bundle-path" \
  FAKE_BUNDLE_CAPTURE="$scratch/continued-bundle-capture" \
  FAKE_ALIAS_LEAK="$scratch/continued-alias-leak" \
  FAKE_RUNNER_MODE=drop-profile \
  YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
  bash "$matrix" "$scratch/profiles" "$continued_out" 1 \
    "$scratch/task-alpha" "$scratch/task-beta" \
    >/dev/null 2>"$scratch/continued.stderr"
status=$?
set -e
if [ "$status" -ne 1 ] || [ "$(cat "$continued_runner_count")" -ne 1 ]; then
  echo 'FAIL continuation: recorded launch errors should return 1 after one runner call' >&2
  exit 1
fi
if [ "$(wc -l < "$continued_out/results.tsv")" -ne 5 ] \
  || ! awk -F '\t' '$2 == "task-alpha" { alpha++ } $2 == "task-beta" { beta++ } END { exit alpha != 2 || beta != 2 }' \
    "$continued_out/results.tsv"; then
  echo 'FAIL continuation: a launch error prevented later task rows' >&2
  exit 1
fi

run_nonzero_agent_case() {
  mode=$1
  expected_status=$2
  expected_reward=$3
  expected_agent_exit=$4
  case_out="$scratch/$mode-out"

  set +e
  PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    FAKE_SOURCE_DIGEST="$source_digest" \
    FAKE_RUNNER_COUNT="$scratch/$mode-runner-count" \
    FAKE_CELL_COUNT="$scratch/$mode-cell-count" \
    FAKE_OBSERVED_PROFILES="$scratch/$mode-observed" \
    FAKE_BUNDLE_PATH="$scratch/$mode-bundle-path" \
    FAKE_BUNDLE_CAPTURE="$scratch/$mode-bundle-capture" \
    FAKE_ALIAS_LEAK="$scratch/$mode-alias-leak" \
    FAKE_AGENT_MODE="$mode" \
    YACH_ROTATE_PROFILE_RUNNER="$scratch/resolve-bundle" \
    bash "$matrix" "$scratch/profiles" "$case_out" 1 \
      "$scratch/task-alpha" >/dev/null 2>"$scratch/$mode.stderr"
  status=$?
  set -e
  if [ "$status" -ne "$expected_status" ]; then
    echo "FAIL $mode: expected matrix status $expected_status, got $status" >&2
    exit 1
  fi
  if ! awk -F '\t' -v reward="$expected_reward" -v agent_exit="$expected_agent_exit" '
    $1 == "alpha" {
      found++
      if ($4 != reward || $5 != agent_exit) bad = 1
    }
    END { exit found != 1 || bad }
  ' "$case_out/results.tsv"; then
    echo "FAIL $mode: alpha row has the wrong evidence classification" >&2
    exit 1
  fi
  if ! awk -F '\t' '
    $1 == "beta" {
      found++
      if ($4 != "1" || $5 != "0") bad = 1
    }
    END { exit found != 1 || bad }
  ' "$case_out/results.tsv"; then
    echo "FAIL $mode: later profile did not complete normally" >&2
    exit 1
  fi
}

run_nonzero_agent_case provider-failure 1 error 1
if ! grep -q 'provider failed (agent exit 1): status: provider failed (rate_limited)' \
  "$scratch/provider-failure.stderr"; then
  echo 'FAIL provider failure: provider cause was not surfaced' >&2
  exit 1
fi

# Tool-loop and approval exits are behavioral outcomes scored by their
# verifiers, not provider/infrastructure failures.
run_nonzero_agent_case approval-required 0 0 3
run_nonzero_agent_case tool-loop-failure 0 0 1

echo 'ok one-shot matrix runner'
