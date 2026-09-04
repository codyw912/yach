#!/bin/bash
# Adaptive release gate: run every eval task in the yach-runtime container
# against one pinned live model, adjudicate a first behavioral miss with two
# targeted reruns, then run the driver-contract checks. Provider-invalid
# attempts retry once and can fall back through YACH_EVAL_FALLBACK_RUNNER.
# Requires docker, jq, and resolved YACH_RIG_* provider variables. Design:
# docs/project/specs/2026-07-28-eval-portfolio-design.md
set -euo pipefail

model="${YACH_EVAL_MODEL:-claude-haiku-4-5}"
evals_dir=$(cd "$(dirname "$0")/.." && pwd)
gate_dir="$evals_dir/.gate"
fallback_runner="${YACH_EVAL_FALLBACK_RUNNER:-}"
fallback_model="${YACH_EVAL_FALLBACK_MODEL:-}"
live_start=$SECONDS

if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to classify provider failures" >&2
  exit 2
fi
if [ -n "$fallback_runner" ] && [ ! -x "$fallback_runner" ]; then
  echo "YACH_EVAL_FALLBACK_RUNNER is not executable: $fallback_runner" >&2
  exit 2
fi

if ! docker image inspect yach-runtime >/dev/null 2>&1; then
  echo "yach-runtime image missing - run 'just runtime-image' first" >&2
  exit 2
fi
bash "$evals_dir/scripts/check-image-fresh.sh" || exit 2
if ! env | grep -q '^YACH_RIG_'; then
  echo "no YACH_RIG_* provider variables in the environment" >&2
  exit 2
fi
# A variable holding an unresolved secret reference is present but useless:
# every task then fails on auth and scores 0, which reads as a catastrophic
# regression instead of a setup mistake. Refuse up front instead.
unresolved=$(env | grep -E '^YACH_RIG_[A-Z0-9_]*(API_KEY|TOKEN|SECRET)=' \
  | grep -c '=[a-z][a-z0-9+.-]*://' || true)
if [ "$unresolved" -gt 0 ]; then
  echo "YACH_RIG_* variables hold unresolved secret references" >&2
  echo "resolve them with your secret manager before running the gate" >&2
  exit 2
fi

cell_script="$evals_dir/scripts/run-task-cell.sh"
# shellcheck source=evals/scripts/evidence.sh
source "$evals_dir/scripts/evidence.sh"
failures=0
degraded=0
printf '%-22s %-9s %-7s %-11s %-6s %s\n' \
  TASK PROFILE ATTEMPT RESULT EXIT SECONDS >&2


run_attempt() {
  task_dir=$1
  task=$2
  profile=$3
  attempt=$4
  cell_dir="$gate_dir/$task/$profile-attempt-$attempt"
  work="$cell_dir/work"
  logs="$cell_dir/logs"
  start=$SECONDS
  staging_cause=""
  if ! rm -rf "$cell_dir"; then
    staging_cause="could not clear prior attempt workspace"
  elif ! mkdir -p "$work" "$logs"; then
    staging_cause="could not create attempt workspace"
  elif ! cp -R "$task_dir/fixture/." "$work/"; then
    staging_cause="could not stage task fixture"
  fi
  if [ -n "$staging_cause" ]; then
    attempt_result="harness-error"
    agent_exit=""
    printf '%-22s %-9s %-7s %-11s %-6s %s\n' \
      "$task" "$profile" "$attempt" "$attempt_result" na \
      "$((SECONDS - start))" >&2
    echo "  hard failure: $staging_cause" >&2
    return 0
  fi

  if [ "$profile" = "fallback" ]; then
    if cell_out=$(YACH_EVAL_MODEL="$fallback_model" \
      "$fallback_runner" bash "$cell_script" "$task_dir" "$work" "$logs" \
      2>"$cell_dir/cell.log"); then
      runner_status=0
    else
      runner_status=$?
    fi
  elif cell_out=$(YACH_EVAL_MODEL="$model" \
    bash "$cell_script" "$task_dir" "$work" "$logs" \
    2>"$cell_dir/cell.log"); then
    runner_status=0
  else
    runner_status=$?
  fi

  agent_exit=$(printf '%s\n' "$cell_out" | tail -1 | cut -d' ' -f1)
  verifier_exit=$(printf '%s\n' "$cell_out" | tail -1 | cut -d' ' -f2)
  reward=$(cat "$logs/verifier/reward.txt" 2>/dev/null || echo "missing")
  attempt_result=""

  if [ "$runner_status" -ne 0 ] || [ -z "$agent_exit" ]; then
    attempt_result="harness-error"
  elif [ "$agent_exit" -eq 2 ]; then
    attempt_result="setup-error"
  elif [ -z "$verifier_exit" ] || [ "$verifier_exit" -ne 0 ]; then
    attempt_result="verifier-error"
  elif [ "$reward" != "0" ] && [ "$reward" != "1" ]; then
    attempt_result="verifier-error"
  elif [ "$agent_exit" -ne 0 ] \
    && has_provider_failure_outcome "$work/.yach-eval"; then
    attempt_result="provider-error"
  elif [ "$reward" = "1" ]; then
    attempt_result="pass"
  else
    attempt_result="behavior-fail"
  fi

  printf '%-22s %-9s %-7s %-11s %-6s %s\n' \
    "$task" "$profile" "$attempt" "$attempt_result" \
    "${agent_exit:-na}" "$((SECONDS - start))" >&2
  case "$attempt_result" in
    provider-error)
      cause=$(grep -E '^status: provider failed \(' "$logs/agent.stderr" \
        2>/dev/null | tail -1 || true)
      echo "  provider invalid: ${cause:-no detailed provider status captured}" >&2
      ;;
    behavior-fail)
      reason=$(grep '^verifier:' "$cell_dir/cell.log" 2>/dev/null | tail -1 || true)
      echo "  behavioral miss${reason:+: $reason}" >&2
      ;;
    harness-error|setup-error|verifier-error)
      cause=$(grep -vE '^\s*$' "$cell_dir/cell.log" 2>/dev/null | tail -1 || true)
      echo "  hard failure: ${cause:-no stderr captured}" >&2
      ;;
  esac
}

evaluate_profile() {
  task_dir=$1
  task=$2
  profile=$3
  attempt=0
  valid=0
  behavioral_failures=0
  provider_errors=0
  adjudicating=0

  while :; do
    attempt=$((attempt + 1))
    run_attempt "$task_dir" "$task" "$profile" "$attempt"
    case "$attempt_result" in
      pass)
        valid=$((valid + 1))
        if [ "$adjudicating" -eq 0 ]; then
          return 0
        fi
        ;;
      behavior-fail)
        valid=$((valid + 1))
        behavioral_failures=$((behavioral_failures + 1))
        adjudicating=1
        ;;
      provider-error)
        provider_errors=$((provider_errors + 1))
        if [ "$provider_errors" -ge 2 ]; then
          return 2
        fi
        continue
        ;;
      *)
        return 3
        ;;
    esac

    if [ "$adjudicating" -eq 1 ] && [ "$valid" -ge 3 ]; then
      if [ "$behavioral_failures" -ge 2 ]; then
        return 1
      fi
      return 0
    fi
  done
}

fallback_active=0

evaluate_fallback_task() {
  task_dir=$1
  task=$2
  if evaluate_profile "$task_dir" "$task" fallback; then
    fallback_status=0
  else
    fallback_status=$?
  fi

  case "$fallback_status" in
    0)
      degraded=$((degraded + 1))
      echo "  degraded coverage: fallback profile passed" >&2
      return 0
      ;;
    1)
      echo "  reproduced behavioral failure on fallback profile" >&2
      ;;
    2)
      echo "  fallback provider remained unavailable" >&2
      ;;
  esac
  return 1
}

for task_dir in "$evals_dir"/tasks/*/; do
  task=$(basename "$task_dir")
  rm -rf "${gate_dir:?}/$task"

  if [ "$fallback_active" -eq 1 ]; then
    if ! evaluate_fallback_task "$task_dir" "$task"; then
      failures=$((failures + 1))
    fi
    continue
  fi

  if evaluate_profile "$task_dir" "$task" primary; then
    task_status=0
  else
    task_status=$?
  fi

  case "$task_status" in
    0) ;;
    1)
      echo "  reproduced behavioral failure: majority of three valid attempts" >&2
      failures=$((failures + 1))
      ;;
    2)
      if [ -z "$fallback_runner" ]; then
        echo "  provider remained unavailable; configure a fallback with YACH_EVAL_FALLBACK_RUNNER" >&2
        failures=$((failures + 1))
        continue
      fi
      fallback_active=1
      if ! evaluate_fallback_task "$task_dir" "$task"; then
        failures=$((failures + 1))
      fi
      ;;
    *)
      failures=$((failures + 1))
      ;;
  esac
done

run_check_attempt() {
  check=$1
  name=$2
  profile=$3
  attempt=$4
  start=$SECONDS

  if [ "$profile" = "fallback" ]; then
    if YACH_EVAL_MODEL="$fallback_model" \
      "$fallback_runner" bash "$check"; then
      check_status=0
    else
      check_status=$?
    fi
  elif YACH_EVAL_MODEL="$model" bash "$check"; then
    check_status=0
  else
    check_status=$?
  fi

  if [ "$check_status" -eq 0 ]; then
    check_result="pass"
  elif [ "$check_status" -eq "$EVAL_PROVIDER_INVALID_EXIT" ]; then
    check_result="provider-error"
  else
    check_result="hard-error"
  fi
  printf 'check %-16s %-9s attempt %-2s %-14s %ss\n' \
    "$name" "$profile" "$attempt" "$check_result" \
    "$((SECONDS - start))" >&2
}

evaluate_check_profile() {
  check=$1
  name=$2
  profile=$3
  attempt=0

  while :; do
    attempt=$((attempt + 1))
    run_check_attempt "$check" "$name" "$profile" "$attempt"
    case "$check_result" in
      pass) return 0 ;;
      provider-error)
        if [ "$attempt" -ge 2 ]; then
          return 2
        fi
        ;;
      *) return 1 ;;
    esac
  done
}

evaluate_fallback_check() {
  check=$1
  name=$2
  if evaluate_check_profile "$check" "$name" fallback; then
    fallback_check_status=0
  else
    fallback_check_status=$?
  fi
  if [ "$fallback_check_status" -eq 0 ]; then
    degraded=$((degraded + 1))
    echo "  degraded coverage: fallback check passed" >&2
    return 0
  fi
  if [ "$fallback_check_status" -eq 2 ]; then
    echo "  fallback provider remained unavailable during check: $name" >&2
  else
    echo "  hard fallback check failure: $name" >&2
  fi
  return 1
}

for check in "$evals_dir"/checks/*.sh; do
  name=$(basename "$check" .sh)
  if [ "$fallback_active" -eq 1 ]; then
    if ! evaluate_fallback_check "$check" "$name"; then
      failures=$((failures + 1))
    fi
    continue
  fi

  if evaluate_check_profile "$check" "$name" primary; then
    primary_check_status=0
  else
    primary_check_status=$?
  fi
  case "$primary_check_status" in
    0) ;;
    2)
      if [ -z "$fallback_runner" ]; then
        echo "  provider remained unavailable during check $name; configure a fallback with YACH_EVAL_FALLBACK_RUNNER" >&2
        failures=$((failures + 1))
        continue
      fi
      fallback_active=1
      if ! evaluate_fallback_check "$check" "$name"; then
        failures=$((failures + 1))
      fi
      ;;
    *)
      echo "  hard check failure: $name" >&2
      failures=$((failures + 1))
      ;;
  esac
done

elapsed=$((SECONDS - live_start))
if [ "$elapsed" -gt 120 ]; then
  echo "eval gate: live portion exceeded the two-minute target (${elapsed}s)" >&2
fi
if [ "$failures" -ne 0 ]; then
  echo "eval gate: $failures failure(s)" >&2
  exit 1
fi
if [ "$degraded" -ne 0 ]; then
  echo "eval gate: passed with degraded fallback coverage ($degraded item(s))" >&2
  exit 0
fi
echo "eval gate: all tasks and checks passed" >&2
