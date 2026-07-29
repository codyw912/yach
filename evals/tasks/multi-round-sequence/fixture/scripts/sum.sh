#!/usr/bin/env bash
# Print the total of the quantities in data.txt.
set -euo pipefail
data="$(dirname "$0")/../data.txt"
# BUG: sums the label column instead of the quantity column.
awk '{ total += $1 } END { print total }' "$data"
