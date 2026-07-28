#!/bin/bash
# Oracle: both turns' artifacts — the two-line journal and the two
# outcome documents the runner would have written.
set -euo pipefail

printf 'alpha\nbeta\n' > journal.txt
mkdir -p .yach-eval

write_outcome() {
  cat > "$1" <<EOF
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed",
  "response": "oracle placeholder",
  "turns": [
    {
      "prompt": "$2",
      "outcome": "completed",
      "failure_reason": null,
      "tool_calls": [ { "name": "$3", "count": 1 } ],
      "compactions": 0,
      "duration_ms": 1
    }
  ],
  "tokens": { "context_estimate": 0, "provenance": "estimated" },
  "session_path": "",
  "duration_ms": 1
}
EOF
}

write_outcome .yach-eval/outcome-turn-1.json "create journal.txt" "create_text_file"
write_outcome .yach-eval/outcome.json "append to the file" "edit_text_file"
