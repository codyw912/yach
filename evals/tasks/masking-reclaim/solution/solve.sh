#!/bin/bash
# Model-free oracle: preserve the seeded session and manufacture the sole live
# turn's masking evidence expected from a successful resumed run.
set -euo pipefail

codeword=$(sed -n 's/^CODEWORD: //p' notes/chapter-1.md)
printf '%s\n' "$codeword" > answer.txt

bytes_freed=$(wc -c < notes/chapter-1.md | tr -d ' ')
cat >> .yach/sessions/eval-masking.jsonl <<JSON
{"type":"tool_result_masked","session_id":"eval-masking","turn_id":"turn-9","masked_turn_id":"turn-1","tool_request_id":"tool-request-1","bytes_freed":$bytes_freed,"reason":"threshold_pre_pass"}
{"type":"turn_finished","session_id":"eval-masking","turn_id":"turn-9","outcome":"completed","reason":null}
JSON

mkdir -p .yach-eval
cat > .yach-eval/outcome.json <<JSON
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed",
  "response": "oracle placeholder",
  "turns": [
    {
      "prompt": "Recover the codeword from chapter 1.",
      "outcome": "completed",
      "failure_reason": null,
      "tool_calls": [
        { "name": "read_text_file", "count": 1 },
        { "name": "create_text_file", "count": 1 }
      ],
      "compactions": 0,
      "duration_ms": 1,
      "masked_results": 1,
      "masked_bytes": $bytes_freed
    }
  ],
  "tokens": { "context_estimate": 0, "provenance": "estimated" },
  "session_path": ".yach/sessions/eval-masking.jsonl",
  "duration_ms": 1
}
JSON
