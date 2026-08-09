#!/bin/bash
# Resolver-neutral regression for the adaptive live release gate.
set -euo pipefail

evals_dir=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

fixture_evals="$scratch/evals"
mkdir -p "$fixture_evals/scripts" "$fixture_evals/checks" \
  "$fixture_evals/tasks/sample/fixture" "$fixture_evals/tasks/sample/tests" \
  "$scratch/bin"
cp "$evals_dir/scripts/gate.sh" "$fixture_evals/scripts/gate.sh"
cp "$evals_dir/scripts/run-task-cell.sh" "$fixture_evals/scripts/run-task-cell.sh"
cp "$evals_dir/scripts/evidence.sh" "$fixture_evals/scripts/evidence.sh"
printf '%s\n' '#!/bin/bash' 'exit 0' > "$fixture_evals/scripts/check-image-fresh.sh"
cat > "$fixture_evals/checks/noop.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
profile=${FAKE_GATE_PROFILE:-primary}
if [ "$profile" = "fallback" ]; then
  if [ -n "${YACH_RIG_PRIMARY_ONLY+x}" ]; then
    echo 'primary profile variable reached fallback check' >&2
    exit 1
  fi
  if [ "${YACH_EVAL_MODEL+x}" != "x" ] \
    || [ "$YACH_EVAL_MODEL" != "$FAKE_EXPECTED_FALLBACK_MODEL" ]; then
    echo 'fallback model did not reach driver check' >&2
    exit 1
  fi
  count_file=${FAKE_FALLBACK_CHECK_COUNT:-}
  results=${FAKE_FALLBACK_CHECK_RESULTS:-pass}
elif [ -n "${FAKE_EXPECTED_PRIMARY_CHECK_MODEL:-}" ]; then
  if [ "${YACH_EVAL_MODEL+x}" != "x" ] \
    || [ "$YACH_EVAL_MODEL" != "$FAKE_EXPECTED_PRIMARY_CHECK_MODEL" ]; then
    echo 'normalized primary model did not reach driver check' >&2
    exit 1
  fi
  count_file=${FAKE_PRIMARY_CHECK_COUNT:-}
  results=${FAKE_PRIMARY_CHECK_RESULTS:-pass}
else
  count_file=${FAKE_PRIMARY_CHECK_COUNT:-}
  results=${FAKE_PRIMARY_CHECK_RESULTS:-pass}
fi

count=0
if [ -n "$count_file" ]; then
  if [ -f "$count_file" ]; then
    count=$(cat "$count_file")
  fi
  count=$((count + 1))
  printf '%s\n' "$count" > "$count_file"
fi
mode=$(printf '%s' "$results" | cut -d, -f"$((count == 0 ? 1 : count))")
if [ "$mode" = "provider-failure" ]; then
  echo 'check: provider failed' >&2
  exit 42
fi
exit 0
EOF
printf '%s\n' 'Exercise the gate.' > "$fixture_evals/tasks/sample/instruction.md"
printf '%s\n' '#!/bin/bash' 'exit 0' > "$fixture_evals/tasks/sample/tests/test.sh"

cat > "$scratch/bin/docker" <<'EOF'
#!/bin/bash
set -eu
if [ "${1:-}" = "image" ] && [ "${2:-}" = "inspect" ]; then
  exit 0
fi
if [ "${1:-}" != "run" ]; then
  echo 'unexpected fake docker invocation' >&2
  exit 2
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
  mode=$(cat "$work/.fake-agent-mode")
  case "$mode" in
    behavior-fail|tool-loop-failure) printf '0\n' > "$logs/verifier/reward.txt" ;;
    missing-reward) ;;
    provider-verifier-error) exit 9 ;;
    *) printf '1\n' > "$logs/verifier/reward.txt" ;;
  esac
  exit 0
fi

profile=${FAKE_GATE_PROFILE:-primary}
if [ "$profile" = "fallback" ]; then
  count_file=$FAKE_FALLBACK_COUNT
  results=$FAKE_FALLBACK_RESULTS
else
  count_file=$FAKE_PRIMARY_COUNT
  results=$FAKE_PRIMARY_RESULTS
fi
count=0
if [ -f "$count_file" ]; then
  count=$(cat "$count_file")
fi
count=$((count + 1))
printf '%s\n' "$count" > "$count_file"
mode=$(printf '%s' "$results" | cut -d, -f"$count")
if [ -z "$mode" ]; then
  mode=pass
fi
mkdir -p "$work/.yach-eval"
printf '%s\n' "$mode" > "$work/.fake-agent-mode"
case "$mode" in
  pass|behavior-fail|missing-reward)
    printf '%s\n' '{"outcome":"completed","turns":[{"outcome":"completed","failure_reason":null}]}' \
      > "$work/.yach-eval/outcome.json"
    exit 0
    ;;
  provider-failure|provider-verifier-error)
    printf '%s\n' '{"outcome":"failed","turns":[{"outcome":"failed","failure_reason":"turn_end provider failed"}]}' \
      > "$work/.yach-eval/outcome.json"
    echo 'status: provider failed (rate_limited): quota exhausted' >&2
    echo 'status: turn_end provider failed' >&2
    exit 1
    ;;
  tool-loop-failure)
    printf '%s\n' '{"outcome":"failed","turns":[{"outcome":"failed","failure_reason":"turn_end tool loop failed"}]}' \
      > "$work/.yach-eval/outcome.json"
    echo 'status: turn_end tool loop failed' >&2
    exit 1
    ;;
  setup-failure)
    echo 'provider setup failed' >&2
    exit 2
    ;;
  *)
    echo "unknown fake mode: $mode" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$scratch/bin/docker"

cat > "$scratch/fallback-runner" <<'EOF'
#!/bin/bash
set -euo pipefail
export FAKE_GATE_PROFILE=fallback
unset YACH_RIG_PRIMARY_ONLY
export YACH_RIG_PROVIDER=rig-openai
export YACH_RIG_OPENAI_API_KEY=fallback-key
exec "$@"
EOF
chmod +x "$scratch/fallback-runner"

run_case() {
  name=$1
  expected_status=$2
  primary_results=$3
  fallback_model_value=${6-fallback-model}
  fallback_results=$4
  fallback_enabled=$5
  primary_check_results=${7-pass}
  fallback_check_results=${8-pass}
  case_root="$scratch/$name"
  mkdir -p "$case_root"
  primary_count="$case_root/primary-count"
  primary_check_count="$case_root/primary-check-count"
  fallback_check_count="$case_root/fallback-check-count"
  fallback_count="$case_root/fallback-count"
  stderr="$case_root/stderr"

  set +e
  if [ "$fallback_enabled" -eq 1 ]; then
    PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
      FAKE_GATE_PROFILE=primary \
      FAKE_PRIMARY_RESULTS="$primary_results" \
      FAKE_FALLBACK_RESULTS="$fallback_results" \
      FAKE_PRIMARY_CHECK_RESULTS="$primary_check_results" \
      FAKE_FALLBACK_CHECK_RESULTS="$fallback_check_results" \
      FAKE_PRIMARY_CHECK_COUNT="$primary_check_count" \
      FAKE_FALLBACK_CHECK_COUNT="$fallback_check_count" \
      FAKE_EXPECTED_FALLBACK_MODEL="$fallback_model_value" \
      FAKE_PRIMARY_COUNT="$primary_count" \
      FAKE_FALLBACK_COUNT="$fallback_count" \
      YACH_RIG_PRIMARY_ONLY=primary-only \
      YACH_RIG_PROVIDER=rig-openai \
      YACH_RIG_OPENAI_API_KEY=primary-key \
      YACH_EVAL_FALLBACK_RUNNER="$scratch/fallback-runner" \
      YACH_EVAL_FALLBACK_MODEL="$fallback_model_value" \
      bash "$fixture_evals/scripts/gate.sh" >/dev/null 2>"$stderr"
  else
    PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
      FAKE_GATE_PROFILE=primary \
      FAKE_PRIMARY_RESULTS="$primary_results" \
      FAKE_FALLBACK_RESULTS="$fallback_results" \
      FAKE_PRIMARY_CHECK_RESULTS="$primary_check_results" \
      FAKE_FALLBACK_CHECK_RESULTS="$fallback_check_results" \
      FAKE_PRIMARY_CHECK_COUNT="$primary_check_count" \
      FAKE_FALLBACK_CHECK_COUNT="$fallback_check_count" \
      FAKE_EXPECTED_FALLBACK_MODEL="$fallback_model_value" \
      FAKE_PRIMARY_COUNT="$primary_count" \
      FAKE_FALLBACK_COUNT="$fallback_count" \
      YACH_RIG_PRIMARY_ONLY=primary-only \
      YACH_RIG_PROVIDER=rig-openai \
      YACH_RIG_OPENAI_API_KEY=primary-key \
      bash "$fixture_evals/scripts/gate.sh" >/dev/null 2>"$stderr"
  fi
  status=$?
  set -e

  if [ "$status" -ne "$expected_status" ]; then
    cat "$stderr" >&2
    echo "FAIL $name: expected status $expected_status, got $status" >&2
    exit 1
  fi
}

run_case flaky-pass 0 behavior-fail,pass,pass '' 0
if [ "$(cat "$scratch/flaky-pass/primary-count")" -ne 3 ]; then
  echo 'FAIL flaky-pass: a first behavioral miss did not receive two targeted reruns' >&2
  exit 1
fi

run_case tool-loop-pass 0 tool-loop-failure,pass,pass '' 0
if [ "$(cat "$scratch/tool-loop-pass/primary-count")" -ne 3 ]; then
  echo 'FAIL tool-loop-pass: completed non-provider exit did not receive behavioral adjudication' >&2
  exit 1
fi

run_case provider-verifier-error 1 provider-verifier-error pass 1
if [ "$(cat "$scratch/provider-verifier-error/primary-count")" -ne 1 ] \
  || [ -e "$scratch/provider-verifier-error/fallback-count" ]; then
  echo 'FAIL provider-verifier-error: broken verifier was retried as a provider failure' >&2
  exit 1
fi

run_case missing-reward 1 missing-reward,pass,pass '' 0
if [ "$(cat "$scratch/missing-reward/primary-count")" -ne 1 ]; then
  echo 'FAIL missing-reward: absent verifier evidence entered the behavioral vote' >&2
  exit 1
fi

mv "$fixture_evals/tasks/sample/fixture" "$fixture_evals/tasks/sample/fixture.saved"
run_case staging-failure 1 pass '' 0
mv "$fixture_evals/tasks/sample/fixture.saved" "$fixture_evals/tasks/sample/fixture"
if [ -e "$scratch/staging-failure/primary-count" ]; then
  echo 'FAIL staging-failure: agent ran after fixture staging failed' >&2
  exit 1
fi

run_case reproduced-failure 1 behavior-fail,behavior-fail,pass '' 0
if [ "$(cat "$scratch/reproduced-failure/primary-count")" -ne 3 ]; then
  echo 'FAIL reproduced-failure: majority vote did not use three valid attempts' >&2
  exit 1
fi

run_case provider-retry 0 provider-failure,pass '' 0
if [ "$(cat "$scratch/provider-retry/primary-count")" -ne 2 ]; then
  echo 'FAIL provider-retry: provider-invalid evidence was not retried once' >&2
  exit 1
fi

run_case provider-fallback 0 provider-failure,provider-failure pass 1
if [ "$(cat "$scratch/provider-fallback/primary-count")" -ne 2 ] \
  || [ "$(cat "$scratch/provider-fallback/fallback-count")" -ne 1 ] \
  || [ "$(cat "$scratch/provider-fallback/fallback-check-count")" -ne 1 ]; then
  echo 'FAIL provider-fallback: fallback did not run once after two invalid attempts' >&2
  exit 1
fi
if ! grep -q 'degraded' "$scratch/provider-fallback/stderr"; then
  echo 'FAIL provider-fallback: fallback success did not report degraded coverage' >&2
  exit 1
fi

run_case provider-owned-fallback 0 provider-failure,provider-failure pass 1 ""
if [ "$(cat "$scratch/provider-owned-fallback/fallback-check-count")" -ne 1 ]; then
  echo 'FAIL provider-owned-fallback: empty fallback model did not reach driver checks' >&2
  exit 1
fi

cp -R "$fixture_evals/tasks/sample" "$fixture_evals/tasks/sample-two"
run_case sticky-fallback 0 provider-failure,provider-failure pass,pass 1
rm -rf "$fixture_evals/tasks/sample-two"
if [ "$(cat "$scratch/sticky-fallback/primary-count")" -ne 2 ] \
  || [ "$(cat "$scratch/sticky-fallback/fallback-count")" -ne 2 ]; then
  echo 'FAIL sticky-fallback: later tasks retried a primary provider already proven unavailable' >&2
  exit 1
fi

cp "$fixture_evals/checks/noop.sh" "$fixture_evals/checks/second.sh"
run_case check-outage-sticky 0 pass '' 1 fallback-model \
  provider-failure,provider-failure pass,pass
rm "$fixture_evals/checks/second.sh"
if [ "$(cat "$scratch/check-outage-sticky/primary-count")" -ne 1 ] \
  || [ -e "$scratch/check-outage-sticky/fallback-count" ] \
  || [ "$(cat "$scratch/check-outage-sticky/primary-check-count")" -ne 2 ] \
  || [ "$(cat "$scratch/check-outage-sticky/fallback-check-count")" -ne 2 ]; then
  echo 'FAIL check-outage-sticky: driver-check outage did not retry once and keep fallback active' >&2
  exit 1
fi

run_case missing-fallback 1 provider-failure,provider-failure '' 0
if [ "$(cat "$scratch/missing-fallback/primary-count")" -ne 2 ]; then
  echo 'FAIL missing-fallback: provider retry count was not bounded at two' >&2
  exit 1
fi
if ! grep -q 'fallback' "$scratch/missing-fallback/stderr"; then
  echo 'FAIL missing-fallback: unavailable fallback was not explained' >&2
  exit 1
fi

run_case setup-failure 1 setup-failure '' 1
if [ "$(cat "$scratch/setup-failure/primary-count")" -ne 1 ] \
  || [ -e "$scratch/setup-failure/fallback-count" ]; then
  echo 'FAIL setup-failure: hard setup failure was retried or sent to fallback' >&2
  exit 1
fi

primary_empty_root="$scratch/primary-empty-model"
mkdir -p "$primary_empty_root"
set +e
PATH="$scratch/bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  FAKE_GATE_PROFILE=primary \
  FAKE_PRIMARY_RESULTS=pass \
  FAKE_FALLBACK_RESULTS='' \
  FAKE_FALLBACK_CHECK_COUNT="$primary_empty_root/fallback-check-count" \
  FAKE_EXPECTED_FALLBACK_MODEL='' \
  FAKE_EXPECTED_PRIMARY_CHECK_MODEL=claude-haiku-4-5 \
  FAKE_PRIMARY_COUNT="$primary_empty_root/primary-count" \
  FAKE_FALLBACK_COUNT="$primary_empty_root/fallback-count" \
  YACH_RIG_PROVIDER=rig-openai \
  YACH_RIG_OPENAI_API_KEY=primary-key \
  YACH_EVAL_MODEL="" \
  bash "$fixture_evals/scripts/gate.sh" \
  >/dev/null 2>"$primary_empty_root/stderr"
status=$?
set -e
if [ "$status" -ne 0 ]; then
  cat "$primary_empty_root/stderr" >&2
  echo 'FAIL primary-empty-model: tasks and checks used different normalized models' >&2
  exit 1
fi

mkdir -p "$scratch/model-bin"
cat > "$scratch/model-bin/docker" <<'EOF'
#!/bin/bash
set -eu
printf '%s\n' "$@" > "$FAKE_MODEL_ARGS"
if [ "${FAKE_MODEL_MODE:-}" = "provider-failure" ]; then
  printf '%s\n' \
    '{"schema":"yach-run-outcome/1","outcome":"failed","response":"","turns":[{"prompt":"p","outcome":"failed","failure_reason":"turn_end provider failed","tool_calls":[],"compactions":0,"duration_ms":1}],"tokens":{"context_estimate":1,"provenance":"estimated"},"session_path":"session","duration_ms":1}'
  exit 1
fi
case "$*" in
  *'Create a file named hello.txt'*)
    printf '%s\n' '{"outcome":"approval_required"}'
    exit 3
    ;;
  *)
    printf '%s\n' \
      '{"schema":"yach-run-outcome/1","outcome":"completed","response":"ok","turns":[{"prompt":"p","outcome":"completed","tool_calls":[],"compactions":0,"duration_ms":1}],"tokens":{"context_estimate":1,"provenance":"estimated"},"session_path":"session","duration_ms":1}'
    exit 0
    ;;
esac
EOF
chmod +x "$scratch/model-bin/docker"

for check in outcome-schema approval-required; do
  model_args="$scratch/$check-model-args"
  PATH="$scratch/model-bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    FAKE_MODEL_ARGS="$model_args" \
    YACH_RIG_PROVIDER=rig-openai \
    YACH_RIG_OPENAI_API_KEY=fallback-key \
    YACH_EVAL_MODEL="" \
    bash "$evals_dir/checks/$check.sh"
  if grep -Fx -q -- '--model' "$model_args"; then
    echo "FAIL $check: provider-owned fallback unexpectedly pinned a default model" >&2
    exit 1
  fi
done

explicit_args="$scratch/explicit-model-args"
PATH="$scratch/model-bin:/usr/bin:/bin:/usr/sbin:/sbin" \
  FAKE_MODEL_ARGS="$explicit_args" \
  YACH_RIG_PROVIDER=rig-openai \
  YACH_RIG_OPENAI_API_KEY=fallback-key \
  YACH_EVAL_MODEL=fallback-model \
  bash "$evals_dir/checks/outcome-schema.sh"
if ! awk 'previous == "--model" && $0 == "fallback-model" { found = 1 } { previous = $0 } END { exit !found }' \
  "$explicit_args"; then
  echo 'FAIL outcome-schema: explicit fallback model was not passed to yach run' >&2
  exit 1
fi

for check in outcome-schema approval-required; do
  provider_args="$scratch/$check-provider-args"
  set +e
  PATH="$scratch/model-bin:/usr/bin:/bin:/usr/sbin:/sbin" \
    FAKE_MODEL_ARGS="$provider_args" \
    FAKE_MODEL_MODE=provider-failure \
    YACH_RIG_PROVIDER=rig-openai \
    YACH_RIG_OPENAI_API_KEY=fallback-key \
    YACH_EVAL_MODEL=fallback-model \
    bash "$evals_dir/checks/$check.sh" >/dev/null 2>"$scratch/$check-provider.stderr"
  status=$?
  set -e
  if [ "$status" -ne 42 ]; then
    cat "$scratch/$check-provider.stderr" >&2
    echo "FAIL $check: provider failure did not return reserved status 42" >&2
    exit 1
  fi
done

echo 'ok adaptive gate'
