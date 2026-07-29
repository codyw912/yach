#!/bin/bash
# Oracle: fix the column, run it, record the total.
set -euo pipefail
cat > scripts/sum.sh <<'INNER'
#!/usr/bin/env bash
# Print the total of the quantities in data.txt.
set -euo pipefail
data="$(dirname "$0")/../data.txt"
awk '{ total += $2 } END { print total }' "$data"
INNER
chmod +x scripts/sum.sh
bash scripts/sum.sh > result.txt
