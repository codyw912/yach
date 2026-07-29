#!/bin/bash
# One eval-task cell: run the task's agent in the yach-runtime container
# against the workspace, then its verifier. Shared by the gate
# (pinned-model, YACH_EVAL_MODEL set) and the provider-matrix sweep
# (model owned by the profile's YACH_RIG_* variables, YACH_EVAL_MODEL
# unset). Prints "<agent_exit> <verifier_exit>" as the last stdout
# line; the reward lands in <logs-dir>/verifier/reward.txt.
set -uo pipefail

task_dir=$1
work=$2
logs=$3

# An unresolved secret reference is present but useless — the agent then
# fails on auth and the cell scores 0, which is indistinguishable from the
# task genuinely failing. Fail loudly instead of producing a bad datapoint.
if env | grep -E '^YACH_RIG_[A-Z0-9_]*(API_KEY|TOKEN|SECRET)=' \
  | grep -q '=[a-z][a-z0-9+.-]*://'; then
  echo "YACH_RIG_* variables hold unresolved secret references; resolve them with your secret manager" >&2
  exit 2
fi

# shellcheck disable=SC2046,SC2086 # word-splitting the -e flags is intended
env_flags=$(env | sed -n 's/^\(YACH_RIG_[A-Z0-9_]*\)=.*/-e \1/p' | tr '\n' ' ')
model_flags=""
if [ -n "${YACH_EVAL_MODEL:-}" ]; then
  model_flags="-e YACH_EVAL_MODEL=$YACH_EVAL_MODEL"
fi

if [ -f "$task_dir/run.sh" ]; then
  # shellcheck disable=SC2086
  docker run --rm $env_flags $model_flags \
    -v "$work:/work" -v "$task_dir:/task:ro" \
    yach-runtime bash /task/run.sh >/dev/null
else
  # shellcheck disable=SC2086
  docker run --rm $env_flags $model_flags \
    -v "$work:/work" -v "$task_dir:/task:ro" \
    yach-runtime bash -c \
    'mkdir -p .yach-eval && yach run --full-auto ${YACH_EVAL_MODEL:+--model "$YACH_EVAL_MODEL"} --prompt "$(cat /task/instruction.md)" --outcome .yach-eval/outcome.json' \
    >/dev/null
fi
agent_exit=$?

docker run --rm -e EVAL_WORKSPACE=/work -e EVAL_LOGS_DIR=/logs \
  -v "$work:/work" -v "$logs:/logs" -v "$task_dir:/task:ro" \
  yach-runtime bash /task/tests/test.sh
verifier_exit=$?

echo "$agent_exit $verifier_exit"
