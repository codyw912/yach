# Headless run and session inspection

**Entry point:** `yach run --prompt "..."`, `yach sessions list`, `yach sessions show latest`; `scripts/drive-headless`.

**Preconditions:** scripted provider with one text round. No TTY needed.

**Steps:**

1. `yach run --prompt "headless question"`. → Exit 0. Stdout is one JSON document with `schema: "yach-run-outcome/1"`, `outcome: "completed"`, `response: "headless reply"`, `turns[0].prompt: "headless question"`. Streaming progress goes to stderr.
2. `yach sessions list`. → One line with `turns=1  last_outcome=completed  model=scripted-model`.
3. `yach sessions show latest`. → A per-turn timeline with `"headless question"` and `outcome completed`.

**Gotchas:**

- Exit codes are 0/1/2/3/4 for completed/failed/setup/approval-required/timeout. A missing `YACH_BENCH_SCRIPT` is a setup error (2).
- Stderr interleaves status lines with the reply text; assert on the stdout JSON.
