#!/bin/bash
# Run every repeat for ONE profile, inside a single invocation.
#
# The sweep used to call the profile runner once per cell, so a 25-cell
# sweep asked the secret manager to resolve 25 times — each one a chance
# to prompt, and a chance to time out if nobody is at the keyboard. The
# runner now wraps this script instead, so a profile resolves once and
# its repeats all run inside that one resolution.
#
# Prints one "<repeat> <agent_exit> <verifier_exit>" line per repeat.
set -uo pipefail

task_dir=$1
cell_root=$2
repeat=$3
cell_script=$4

for r in $(seq 1 "$repeat"); do
  work="$cell_root-r$r/work"
  logs="$cell_root-r$r/logs"
  out=$(bash "$cell_script" "$task_dir" "$work" "$logs" 2>>"$cell_root-r$r/cell.log")
  echo "$r ${out:-na na}"
done
