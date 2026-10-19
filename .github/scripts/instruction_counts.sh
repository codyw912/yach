#!/usr/bin/env bash
# Report callgrind instruction counts (Ir) for the headless TUI rows, base
# against current. Report only: prints a Markdown table and never fails on a
# delta. Evidence that these counts are exact across runs and runner CPUs:
# docs/project/records/2026-10-19-callgrind-instruction-counts.md.
#
# Usage: instruction_counts.sh BASE_YACH_BENCH CURRENT_YACH_BENCH
#
# Collection is bounded to the workload function (`HEADLESS` closures in
# crates/yach-bench/src/perf/workloads/tui.rs), so worker start-up,
# provenance hashing and JSON output are not counted. Warm-up and per-sample
# setup inside the workload are. One sample per row.
set -uo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 BASE_YACH_BENCH CURRENT_YACH_BENCH" >&2
  exit 64
fi
base_bin=$1
current_bin=$2

rows=(
  startup/backend_ready_to_first_interactive_headless
  keypress/active_stream_on_history_headless/100
  replay/heavy_tool_output_tail_headless/102400
  viewport/huge_transcript_scroll_headless/10000
)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Prints the bounded Ir for one row, or nothing when the binary does not
# have the row or the run fails. Each binary is copied to the same path
# first: argv and path length change the count (the same binary run from
# two paths differs by up to 0.05%), so both sides must be invoked alike.
measure() {
  local bin=$1 row=$2 out="$work/callgrind.out" exe="$work/yach-bench"
  rm -f "$out" "$work/w.json"
  cp "$bin" "$exe" 2>/dev/null || return
  if ! valgrind --tool=callgrind --collect-atstart=no \
    '--toggle-collect=*yach_bench::perf::workloads::tui::HEADLESS::{closure#*' \
    --callgrind-out-file="$out" \
    "$exe" perf worker --ids "$row" --samples 1 --out "$work/w.json" >/dev/null 2>&1; then
    return
  fi
  # A row the binary does not have still exits 0 with nothing collected.
  if jq -e --arg id "$row" 'any(.workloads[]; .id == $id and .status == "ok")' "$work/w.json" >/dev/null 2>&1; then
    sed -n 's/^summary: //p' "$out"
  fi
}

echo "### Instruction counts (callgrind Ir, report only)"
echo ""
echo "| row | base | current | delta |"
echo "|---|---:|---:|---:|"
for row in "${rows[@]}"; do
  # The first callgrind run of the startup row on a fresh runner counts 234
  # fewer instructions than every later run of the same binary (see the
  # record). Base always goes first, so without a discarded run that
  # difference would show as a current-side delta.
  measure "$current_bin" "$row" >/dev/null
  b=$(measure "$base_bin" "$row")
  c=$(measure "$current_bin" "$row")
  if [[ -n "$b" && -n "$c" && "$b" -gt 0 ]]; then
    delta=$(awk -v b="$b" -v c="$c" 'BEGIN { printf "%+.2f%%", (c - b) * 100 / b }')
  else
    delta="n/a"
  fi
  echo "| \`$row\` | ${b:-n/a} | ${c:-n/a} | $delta |"
done
