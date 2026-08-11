#!/bin/bash
# Set caller positional parameters for live eval-check model selection. An
# unset override uses the stable gate default; an explicitly empty override
# leaves model selection to the active provider profile. The check scripts do
# not accept their own positional arguments.

if [ -z "${YACH_EVAL_MODEL+x}" ]; then
  set -- --model claude-haiku-4-5
elif [ -n "$YACH_EVAL_MODEL" ]; then
  set -- --model "$YACH_EVAL_MODEL"
else
  set --
fi
