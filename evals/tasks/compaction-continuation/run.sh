#!/bin/bash
# One session, four turns via --script: the first turns grow context
# past the compaction threshold, and the final turn must still make a
# fresh tool call and write its result afterward.
#
# The threshold is reached by shrinking the window rather than by
# reading a huge fixture: compaction fires at used >= usable/10, where
# usable = context_window - max_output - reserve. With a 68k window,
# the default 32k max_output and the fixture's 1k reserve, usable is
# 35k and the trigger lands near 3.5k tokens — a few chapter reads in.
# auto_threshold_percent is clamped to a floor of 10, so lowering the
# percent alone does nothing.
set -euo pipefail
export YACH_RIG_PROVIDER_CONTEXT_WINDOW=68000
mkdir -p .yach-eval
yach run --full-auto ${YACH_EVAL_MODEL:+--model "$YACH_EVAL_MODEL"} \
  --script /task/script.jsonl \
  --outcome .yach-eval/outcome.json
