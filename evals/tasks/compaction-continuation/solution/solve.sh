#!/bin/bash
# Oracle: the post-compaction turn's artifact, plus an outcome document
# recording that compaction fired.
set -euo pipefail
cp codeword.txt answer.txt
mkdir -p .yach-eval
cat > .yach-eval/outcome.json <<'JSON'
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed",
  "response": "oracle placeholder",
  "turns": [
    { "prompt": "chapter 1", "outcome": "completed", "failure_reason": null,
      "tool_calls": [ { "name": "read_text_file", "count": 1 } ],
      "compactions": 0, "duration_ms": 1 },
    { "prompt": "chapter 3", "outcome": "completed", "failure_reason": null,
      "tool_calls": [ { "name": "read_text_file", "count": 1 } ],
      "compactions": 1, "duration_ms": 1 },
    { "prompt": "codeword", "outcome": "completed", "failure_reason": null,
      "tool_calls": [ { "name": "read_text_file", "count": 1 },
                      { "name": "create_text_file", "count": 1 } ],
      "compactions": 0, "duration_ms": 1 }
  ],
  "tokens": { "context_estimate": 0, "provenance": "estimated" },
  "session_path": "",
  "duration_ms": 1
}
JSON
