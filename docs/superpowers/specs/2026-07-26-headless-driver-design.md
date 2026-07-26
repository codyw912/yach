# Headless Driver Design (`yach run`)

Date: 2026-07-26

Status: proposed; awaiting owner review.

## Context

Two consumers need to drive yach without the TUI:

1. **Provider rotation** (board: active): scripted sessions across
   OpenAI/ChatGPT, opencode Zen, and Fireworks to elicit
   provider-specific failure shapes, locally and cheaply.
2. **yacht integration** (board: slated, phase 2): yach as a
   custom harness in yacht (`~/dev/yacht`), whose launcher contract is a
   subprocess — prompt, env, cwd, transcript path in; structured result
   out. The owner wants the custom-harness route exercised before an
   in-tree yacht adapter exists, so this driver's surface must be
   boringly conventional: plain flags, exit codes, files, one JSON
   result.

The backend already supports this shape: the whole test suite drives
`run_native_dogfood_loop_with_provider_requester` through scripted
`ClientEvent`/`BackendEvent` channels; the TUI is just one client. The
driver is a second, non-interactive client — not a new backend.

## Goals

- One-shot and scripted multi-turn sessions from the command line with
  no interactive input.
- Machine-readable outcome (single JSON document) with stable exit
  codes.
- An explicit full-auto approval posture usable in disposable task
  directories.
- Session log persisted exactly as interactive sessions persist it (the
  JSONL is the transcript artifact and the sesh/analysis input).

## Non-goals

- Not the approval-model redesign (board: UX sprint). Full-auto here is
  a blunt, explicit, opt-in flag; per-tool/per-risk policy comes later.
- Not sandboxing or isolation; the isolation landscape stays an open
  owner question. Safety comes from explicitness and disposable
  workdirs, not confinement.
- Not CI/eval infrastructure (that is yacht's job; board phase 2).
- Not provider matrix orchestration — a `just rotate` recipe or yacht
  drives the matrix by invoking this once per cell.

## CLI surface

```
yach run [--prompt <TEXT> | --script <PATH>]
         [--project-root <DIR>]        # default: cwd
         [--session-path <FILE>]       # default: .yach/native-sessions/<generated>.jsonl
         [--full-auto]                 # auto-approve tool/edit reviews (required for writes)
         [--turn-timeout-secs <N>]     # default 600; whole-turn wall clock
         [--outcome <FILE|->]          # default: `-` (stdout)
         [--quiet]                     # suppress streaming progress on stderr
```

- `--prompt` runs one turn. `--script` runs turns sequentially from a
  JSON Lines file, one object per turn: `{"prompt": "..."}`; later turns
  wait for the previous turn to finish. A turn failure stops the script
  (remaining turns are reported as `skipped`); rotation scenarios want
  the failure evidence, not blind continuation.
- Model/provider selection stays where it is today: `YACH_RIG_*` env
  vars and `.yach/config.json` in the project root. No new flags; yacht
  and `just rotate` already control env per invocation.
- Streaming deltas/status go to stderr line-buffered (silenceable with
  `--quiet`); stdout carries exactly the outcome JSON so a launcher can
  `stdout | jq` without filtering.

## Approval posture

Without `--full-auto`, the driver auto-approves nothing: any
`ToolReviewRequested` or local-edit preview immediately fails the turn
with outcome `approval_required`, naming the tool. This makes read-only
scenarios work by default (read/search/list tools are not review-gated)
while write scenarios fail loudly instead of hanging or silently
approving.

With `--full-auto`, every review request is approved. Hard denials are
unaffected: sensitive-file deny-by-default and shell env-stripping are
not review prompts and stay enforced. The flag is the operator's
declaration that the project root is disposable; documentation says so
plainly.

## Outcome document

One JSON object, `schema: "yach-run-outcome/1"`. Aligned with the
neutral machine-evidence direction proposed to yacht (feedback doc,
2026-07-26) — if yacht lands its own schema, a `--outcome-format`
adapter can follow; until then this is the native shape:

```json
{
  "schema": "yach-run-outcome/1",
  "outcome": "completed" | "failed" | "timeout" | "approval_required",
  "response": "<final turn's assistant text>",
  "turns": [
    {
      "prompt": "...",
      "outcome": "completed" | "failed" | "timeout" | "skipped",
      "failure_reason": null | "<turn_finished reason verbatim>",
      "tool_calls": [{ "name": "read_text_file", "count": 4 }],
      "compactions": 1,
      "duration_ms": 12345
    }
  ],
  "tokens": { "context_estimate": 23481, "provenance": "estimated" },
  "session_path": "/abs/path/session.jsonl",
  "duration_ms": 45678
}
```

- `tokens.provenance` is `"estimated"` until the hybrid provider-usage
  accounting lands (board: context system); the field exists now so
  consumers never mistake chars/4 for reported usage — the lesson from
  the context-tracker research.
- `tool_calls` carries names with counts (yacht feedback #5).
- Everything richer (per-tool payloads, checkpoint details, failure
  classification) is deliberately NOT duplicated here: the session JSONL
  is the deep artifact; the outcome document is the summary a launcher
  branches on.

## Exit codes

- `0` — all turns completed.
- `1` — a turn failed (provider error, tool-loop failure).
- `2` — setup/config error (bad flags, missing provider env, unreadable
  project root); no session was run.
- `3` — approval required without `--full-auto`.
- `4` — turn timeout.

The outcome JSON is still emitted on every nonzero exit except `2`.

## Implementation sketch

A headless client in `yach-cli` (sibling of the TUI client): spawn the
existing native loop with the real provider requester, then a small
event pump per turn — send `PromptSubmitted`, answer review events per
the approval posture, accumulate deltas/tool rows/compaction statuses,
stop at `PromptFinished`, then aggregate the outcome document from
collected events plus the session log. Expected to need zero backend
changes; if event coverage gaps appear (e.g. a review event without
enough context to answer), those become protocol fixes, which is
dogfood signal in itself.

Testing follows the existing pattern: drive `yach run`'s pump against
`FakeProviderRequester` fixtures for each outcome class, plus one
script-file test; timeout and approval-required paths asserted on exit
code + outcome JSON shape.

## Open questions for owner review

1. Flag name: `--full-auto` (Codex-family) vs something scarier
   (`--auto-approve-all`). Cohort norm is the former; the scarier name
   is more honest about v1's bluntness.
2. Should `--script` support per-turn `expect` fields (assertion
   hooks)? Leaning no for v1 — assertions belong to the consumer
   (rotation recipe / yacht / sesh), keeping the driver a driver.
3. Outcome schema ownership once yacht defines its neutral evidence
   format: emit both, or adopt yacht's and drop the native shape?
