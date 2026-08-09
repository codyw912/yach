#!/bin/bash
# Run every repeat for one already-active profile. Keeping repeats in one
# subprocess preserves profile-level environment setup while every cell still
# gets its own fixture, logs, agent process, and verifier process.
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
