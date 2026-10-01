# Session Diagnostics Gateway Smoke

Date: 2026-09-30
Plan: [2026-09-30-session-diagnostics.md](../plans/2026-09-30-session-diagnostics.md)
Spec: [2026-09-30-session-diagnostics-design.md](../specs/2026-09-30-session-diagnostics-design.md)
Code under test: the session was produced by a binary built from Task 5
commit `580bfbe8474120d11c1b5e6235da80977fbbe95c` (Tasks 1–5); the Task 6
change adds only README.md, `secretspec.toml`, and this record. The
inspector output below was regenerated from that same session file with the
final branch code, which adds `warnings=N` to `sessions list` and a
`retry N` label to retried attempt rows (none here).

## Setup

- Binary: `just dev cargo install --locked --path crates/yach-cli --root "$PWD/.devenv/state/smoke"`.
- Scratch project: `mktemp -d`, `git init`, a three-line `README.md`, and a
  two-line `widget.py`.
- Gate before the run: `just lint` clean; `just test` exit 0 (backend 943
  passed). `cargo test -p yach-backend -p yach -- --test-threads=4` also
  passed, since CI runs tests in parallel.

## Command

`secretspec` resolves `secretspec.toml` from the repository, so it runs from
the repository root and changes into the scratch project for `yach`:

```bash
SCRATCH=<scratch dir>
SMOKE_BIN="$PWD/.devenv/state/smoke/bin/yach"
SCRATCH="$SCRATCH" SMOKE_BIN="$SMOKE_BIN" \
YACH_RIG_PROVIDER=openai-compatible \
YACH_RIG_OPENAI_COMPAT_BASE_URL=http://omp-subscriptions.home.lan:4000/v1 \
YACH_RIG_OPENAI_COMPAT_MODEL=openai-codex/gpt-6.1-sol \
YACH_CAPTURE_REQUESTS="$SCRATCH/.capture" \
YACH_SESSION_DIR="$SCRATCH/.sessions" \
secretspec run --provider omp-subscriptions --scope gateway \
  --reason "yach session diagnostics gateway smoke" -- \
  sh -c 'cd "$SCRATCH" && exec "$SMOKE_BIN" run \
    --model openai-codex/gpt-6.1-sol \
    --prompt "List the files here, read README.md, and summarize it in one sentence."'
```

## Model selection

The run used `yach run --model openai-codex/gpt-6.1-sol`. The outcome
document reports `"model": "openai-codex/gpt-6.1-sol"`, and the session's
provider attempts record the same model. `~/.yach/config.toml` was not
changed; its `[model.default]` remains `provider = "openai-codex"`,
`model = "gpt-5.6-terra"`.

## Result

Exit 0, outcome `completed`, one turn, 9.3 s. The model called
`list_project_paths` and `read_text_file`, then answered correctly.

`yach sessions list`:

```text
session session-2878620-1790800062521649509  started=1790800062523  turns=1  last_outcome=completed  model=openai-codex/gpt-6.1-sol  warnings=0
```

`yach sessions show latest`:

```text
session session-2878620-1790800062521649509  turns=1  warnings=0
turn turn-0  "List the files here, read README.md, and summarize it in one sentence."
  +1ms attempt 1 turn succeeded request=a841fe82-38eb-485f-b5b4-32919c34c91f first_event=4259ms 4293ms capture=turn-0-turn-1.json
  +4337ms tool list_project_paths completed 0ms  tool payload redacted
  +4337ms tool read_text_file completed 0ms  tool payload redacted; resolved_tool=extension_replacement extension_id=yach.hashline extension_version=0.1.0 provider_name=read_text_file implementation=hashline_read replaced_builtin=read_text_file replacement_source=user
  +4338ms attempt 2 turn succeeded request=0aa25598-1c0f-4c49-ab43-964679b1ce48 first_event=3110ms 4948ms capture=turn-0-turn-2.json
  outcome completed  usage input=1473 output=102
```

`yach sessions show latest --json` produced one compact JSON line with the
same items, including `started_at_ms`, `first_event_ms`, and
`provider_request_id` for both attempts.

## Checks against the plan's expectations

| Expectation | Observed |
| --- | --- |
| At least one `provider_attempt` with `provider_request_id`, `first_event_ms`, `capture` | Both attempts carry all three |
| Tool items with `duration_ms` | Both tools: `duration_ms: 0` (sub-millisecond local reads) |
| Every session line carries `at_ms` | 11 of 11 lines |
| One capture file per attempt | 2 attempts, 2 files: `turn-0-turn-1.json` (4188 B), `turn-0-turn-2.json` (5317 B) |
| No credential in capture files | `grep -r -i -E 'authorization\|bearer\|api[-_]key' .capture` found nothing; same grep on the session file: 0 |
| Private modes | Session dir `0700`, session file `0600`; capture root and session dir `0700`, capture files `0600` |

Capture files hold the request body only (top-level keys `max_tokens`,
`messages`, `model`, `reasoning_effort`, `stream`, `stream_options`,
`tools`). Their contents are not reproduced here.

## Observations

- Time to first event is most of each attempt: 4259 of 4293 ms, and 3110 of
  4948 ms. Most of the latency is before the first streamed event, which fits
  reasoning or gateway buffering happening before output starts. This session
  cannot tell which. It is the kind of evidence the change was meant to
  produce.
- Tool durations round to `0ms` for fast local reads; the inspector has
  millisecond resolution.
- The turn's `usage` line (`input=1473 output=102`) is the provider usage on
  the turn's last assistant entry (`crates/yach-cli/src/sessions.rs:361-365`),
  so it covers the final request, not the whole turn. A per-turn total would
  need each attempt's usage; attempt events do not carry it today.

## Did not work / follow-ups

- `secretspec run` must start in the repository (it looks for
  `secretspec.toml` in the current directory and its parents), so the plan's
  command, run from the scratch directory, failed with "No secretspec.toml
  found". The command above shows the working form.
- `yach sessions list` prints `started=` as raw epoch milliseconds; a
  human-readable time would make the text output easier to scan.
