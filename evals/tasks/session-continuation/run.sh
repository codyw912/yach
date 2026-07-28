#!/bin/bash
# Two invocations continue one session (#192). The second prompt never
# names the file, so completing it requires the first turn's context.
set -euo pipefail
mkdir -p .yach-eval
# YACH_EVAL_MODEL is set by the gate (pinned model); sweep cells leave
# it unset and the profile's YACH_RIG_* variables pick the model.
yach run --full-auto ${YACH_EVAL_MODEL:+--model "$YACH_EVAL_MODEL"} --session eval-continuation \
  --prompt "Create a file named journal.txt whose content is exactly the single line: alpha" \
  --outcome .yach-eval/outcome-turn-1.json
yach run --full-auto ${YACH_EVAL_MODEL:+--model "$YACH_EVAL_MODEL"} --session eval-continuation \
  --prompt "Add a second line to the file you created in the previous turn: beta" \
  --outcome .yach-eval/outcome.json
