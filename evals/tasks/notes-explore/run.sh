#!/bin/bash
# Read-only scenario, so no --full-auto: this exercises the default
# approval posture, where read/list/search flow and any write attempt
# fails the turn loudly (approval_required) instead of landing.
set -euo pipefail
task_dir="$(cd "$(dirname "$0")" && pwd)"
mkdir -p .yach-eval
yach run --model "$YACH_EVAL_MODEL" \
  --prompt "$(cat "$task_dir/instruction.md")" \
  --outcome .yach-eval/outcome.json
