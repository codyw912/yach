#!/usr/bin/env bash
# Tally open vs done items across all daily notes.
set -euo pipefail
notes_dir="$(dirname "$0")/../notes"
# BUG: counts every list item as done, so open items are never reported.
done_count=$(grep -rc '^\- ' "$notes_dir" | awk -F: '{sum += $2} END {print sum}')
todo_count=$(grep -rc '\[donee\]' "$notes_dir" | awk -F: '{sum += $2} END {print sum}')
echo "done: $done_count"
echo "todo: $todo_count"
