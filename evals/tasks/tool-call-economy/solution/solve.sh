#!/bin/bash
# Oracle: the minimal correct solution is one create call.
set -euo pipefail
printf 'ready\n' > report.txt
mkdir -p .yach-eval
cat > .yach-eval/outcome.json <<'JSON'
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed",
  "response": "oracle placeholder",
  "turns": [
    {
      "prompt": "create report.txt",
      "outcome": "completed",
      "failure_reason": null,
      "tool_calls": [ { "name": "create_text_file", "count": 1 } ],
      "compactions": 0,
      "duration_ms": 1
    }
  ],
  "tokens": { "context_estimate": 0, "provenance": "estimated" },
  "session_path": "",
  "duration_ms": 1
}
JSON
