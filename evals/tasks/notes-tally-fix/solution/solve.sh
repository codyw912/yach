#!/bin/bash
# Oracle: the minimal correct solution, run with the workspace as cwd.
set -euo pipefail

cat > scripts/tally.sh <<'EOF'
#!/usr/bin/env bash
# Tally open vs done items across all daily notes.
set -euo pipefail
notes_dir="$(dirname "$0")/../notes"
done_count=$(grep -rc '\[done\]' "$notes_dir" | awk -F: '{sum += $2} END {print sum}')
todo_count=$(grep -rc '\[todo\]' "$notes_dir" | awk -F: '{sum += $2} END {print sum}')
echo "done: $done_count"
echo "todo: $todo_count"
EOF

cat > README.md <<'EOF'
# Daily notes

Personal daily task notes under `notes/`, one file per day, with items
marked `[done]` or `[todo]`. Run `bash scripts/tally.sh` to count done
and open items across all notes.
EOF
