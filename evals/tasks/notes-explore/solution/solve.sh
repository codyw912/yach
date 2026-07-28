#!/bin/bash
# Oracle: a correct exploration changes nothing and leaves an outcome
# document showing a completed turn with read/list activity. The
# oracle fabricates the outcome document the runner would have written.
set -euo pipefail

mkdir -p .yach-eval
cat > .yach-eval/outcome.json <<'EOF'
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed",
  "response": "oracle summary placeholder",
  "turns": [
    {
      "prompt": "Explore this project.",
      "outcome": "completed",
      "failure_reason": null,
      "tool_calls": [
        { "name": "list_project_paths", "count": 1 },
        { "name": "read_text_file", "count": 5 }
      ],
      "compactions": 0,
      "duration_ms": 1
    }
  ],
  "tokens": { "context_estimate": 0, "provenance": "estimated" },
  "session_path": "",
  "duration_ms": 1
}
EOF
