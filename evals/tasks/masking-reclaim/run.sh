#!/bin/bash
# Resume a large synthetic session. The pre-turn threshold pass must mask old
# results, then the model must reread chapter 1 to recover its hidden codeword.
set -euo pipefail

export YACH_RIG_PROVIDER_CONTEXT_WINDOW=68000
mkdir -p .yach-eval
yach run --full-auto ${YACH_EVAL_MODEL:+--model "$YACH_EVAL_MODEL"} \
  --session eval-masking \
  --prompt "Read notes/chapter-1.md, then write only its CODEWORD value (without the CODEWORD: prefix) followed by a newline to answer.txt." \
  --outcome .yach-eval/outcome.json
