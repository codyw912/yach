# Native MVP Dogfood Checkpoint

Date: 2026-06-03

## Purpose

This checkpoint turns the MVP convergence bar into a repeatable pass/fail run.
Use it to decide the next blocker before taking more extension-platform work.

## No-Secret Verification

Run these before live provider dogfood:

| Area | Command | Pass Signal |
| --- | --- | --- |
| Provider adapter seam | `just run smoke-rig-provider-request` | `rig_smoke_outcome=Completed`, `completed=true`, `matched_expected_text=true` |
| Native-provider tool loop | `just dev cargo test -p yach-backend native_provider_agent -- --nocapture` | read/search/list/edit/review loop tests pass |
| TUI review state | `just dev cargo test -p yach-ui tool_review -- --nocapture` | review accepts, submits, finishes, and returns to input mode |
| Paste batching | `just dev cargo test -p yach-ui prompt_paste_inserts_text_as_batch -- --nocapture` | paste inserts as one prompt update |
| Startup/profile smoke | `just dev cargo run -p yach-bench -- yach-tui-startup-profile-report --samples 10` | startup marks are collected; extension scan starts after first render |

Notes:

- The provider adapter seam requires provider credentials. If credentials are
  unavailable, record that as `blocked` rather than `failed`.
- The startup/profile smoke is a local signal, not a release benchmark.

## Latest No-Secret Run

Date: 2026-07-14

| Area | Result | Evidence |
| --- | --- | --- |
| Provider adapter seam | blocked | `smoke-rig-provider-request` returned `rig_smoke_outcome=MissingConfig`; `completed=false`; `matched_expected_text=false`; missing `YACH_RIG_ANTHROPIC_API_KEY`. |
| Native-provider tool loop | pass | `native_provider_agent`: 15 passed. |
| TUI review state | pass | `tool_review`: 5 passed. |
| Paste batching | pass | `prompt_paste_inserts_text_as_batch`: 1 passed. |
| Startup/profile smoke | pass | `count=10` per mark; `tui_first_render_end_since_main` p95 `6.166ms`. |

Resolved since the 2026-07-13 run: after PR #125 made native-provider the
default backend, credless `yach tui` exited with `native provider setup
failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY` before first
render, breaking both unconfigured launch and this no-secret startup-profile
check. The default TUI now launches without provider credentials, reports the
setup error in the initial backend status plus a `provider-unconfigured`
model id, fails submitted prompts with the setup error and relaunch guidance
instead of fixture text, and records `provider_unconfigured` turn evidence in
the session log. Verified end to end by spawning the release TUI credless: the
full startup trace through `tui_first_render_end` appears and a submitted
prompt persists a failed turn with the setup-error reason.

## Latest Live Native-Provider Run

Date: 2026-07-14

| Area | Result | Evidence |
| --- | --- | --- |
| Read tool | pass | README read and summary completed; no issues reported. |
| Create tool | pass | Clean create with review approval; no stale-claim on first create. |
| Edit tool | pass | `ok` replaced with `passed` through review approval. |
| Search/list tools | pass with note | Listed paths now show in the transcript (PR #123 confirmed). The preview caps at 12 entries plus `... N more entries`; this is explicit backend display shaping in `native_provider_visible_list_progress`, acceptable for now but worth an expand/collapse affordance later. |
| Duplicate-create failure | not verified (finding) | The model (haiku) denied the duplicate create from in-session memory without issuing a fresh tool call, so `create_text_file` failure output was again not exercised. This is the stale-evidence risk in the opposite direction: if the user deletes the file externally, the model's belief is wrong and it never re-verifies. |
| Explicit `/resume` cross-session | pass | Run as `/resume` from a fresh session selecting the prior session; hydration worked as expected. The original step wording (select the current session while active) was a no-op test and has been rewritten below. |
| Resume after relaunch | pass with note | Session separation works (PR #124 confirmed). Hydrated transcripts show tool results only as redacted summaries (`completed; bytes=N; content=redacted`) because session evidence intentionally persists no file bodies, search lines, or directory dumps; a look-identical resume would need a spec'd decision to persist bounded display previews. |

Findings to carry forward, in priority order:

1. Stale-evidence behavior: the model asserts filesystem state from session
   memory instead of fresh tool evidence (both the 2026-07-07 stale claim and
   the 2026-07-14 tool-less denial). Candidate fixes are provider-loop
   steering (system-prompt guardrails instructing re-verification before
   filesystem claims) and comparison against how other harnesses steer this;
   see the provider tool guardrails item in `docs/project/next.md`.
2. Resumed transcripts do not visually match live runs (redacted tool result
   summaries only), and resumed provider context drops tool events entirely.
   Comparison-set research
   (`docs/project/records/2026-07-14-resume-transcript-research.md`) shows
   all peer harnesses persist model-visible tool payloads and reuse the live
   rendering path on resume. Owner decision 2026-07-14: change the evidence
   policy so session logs are the full model-visible transcript; draft
   design in
   `docs/superpowers/specs/2026-07-14-session-tool-payload-persistence-design.md`.
3. List/search preview caps (12 entries / 8 matches) could use a TUI
   expand/collapse affordance instead of a hard cap.

## Previous Live Native-Provider Run

Date: 2026-07-07

| Area | Result | Evidence |
| --- | --- | --- |
| Basic provider prompt | pass | `Say hello in one sentence.` returned a normal assistant response. |
| Read tool | pass | `read_text_file` completed before the README summary response. |
| Create tool | partial | The model first claimed `dogfood-provider-edit.txt` existed without current tool evidence after the file had been manually deleted, then created it successfully after correction. |
| Edit tool | pass | `dogfood-provider-edit.txt` changed from `ok` to `passed`. |
| Search/list tools | partial | `search_project` and `list_project_paths` completed, but the TUI collapsed the `list_project_paths` multi-line preview instead of showing listed paths. |
| Duplicate-create failure | not verified | The model read the existing file and answered instead of attempting `create_text_file`, so the failed-tool-result path was not exercised. |
| Explicit `/resume` while active | pass | Selecting the current session returns to the TUI without disrupting active state. |
| Resume after relaunch | failed | `/resume` and `--resume` hydrate cumulative previous work from the default native session log rather than a distinct most-recent session. |
| Plain relaunch | pass | Plain TUI relaunch remains fresh/non-resuming. |

Both blockers from this run were fixed and verified in the 2026-07-14 live
run: native sessions use distinct logs with resume targeting the
selected/latest log, and `list_project_paths` output shows in the transcript.

## Live Native-Provider Dogfood

Run with provider env configured:

```sh
just run tui
```

Then exercise this prompt sequence in one session:

| Step | Prompt | Expected Result |
| --- | --- | --- |
| 1 | `Use read_text_file to read README.md, then reply with a one sentence summary.` | Tool progress is visible before the answer; final answer summarizes README. |
| 2 | `Create a new file named dogfood-provider-edit.txt with the content "native provider edit dogfood ok".` | Review prompt appears, approval applies the create, and input returns after finish. |
| 3 | `Use read_text_file to inspect dogfood-provider-edit.txt, then replace "ok" with "passed".` | Read and edit tool progress are visible; review approval applies the change; final answer follows tool evidence. |
| 4 | `Use search_project to find "native provider edit dogfood passed", then list the current directory with list_project_paths.` | Search/list tool progress appears and results are summarized without freezing. |
| 5 | Create `dogfood-provider-edit.txt` again if it already exists. | The failed tool result shows a failure marker and a bounded error excerpt instead of only line/byte counts. |
| 6 | Quit, relaunch plain `just run tui` (fresh session), use `/resume`, and select the previous session. | The prior session's transcript hydrates into the fresh session; selecting the current session from within an active session is a no-op and must not mutate the transcript. |
| 7 | Quit and relaunch with `just run tui --resume`. | The latest native session hydrates on explicit CLI resume; plain `tui` startup remains fresh/non-resuming. |

## Current Status

| MVP Bar | Status | Evidence / Notes |
| --- | --- | --- |
| launch quickly and type immediately | needs live pass | Startup traces are strong, but this checkpoint still needs a fresh live run. |
| provider prompts stream responses | needs live pass | `smoke-rig-provider-request` covers provider seam; default TUI now uses the native-provider path and needs a fresh run. |
| read/search/list tools | partial | Backend emits bounded list previews, but the 2026-07-07 live run showed the TUI collapsed `list_project_paths` output. |
| create/edit tools | partial | Create/edit apply works, but the 2026-07-07 live run showed stale file-existence claims without current tool evidence. |
| review without TUI freeze | needs live pass | PR #104 added progress visibility; UI review regressions pass. |
| multi-round without default cap | pass in tests | `native_provider_agent_default_loop_has_no_round_limit`. |
| persist/resume enough session state | needs live pass | Session separation is implemented in tests; rerun `/resume` and `tui --resume` against live dogfood. |
| recoverable failures | needs live pass | Failed tool result excerpts are merged, but the 2026-07-07 duplicate-create prompt did not actually attempt `create_text_file`. |
| Pi explicit reference only | pass | Native is default; Pi remains explicit `--backend pi`. |

## Next Blocker Rule

After the live run, record the first item that prevents using yach for real work
as the next blocker. Prefer fixing that blocker before adding extension package
UX, more lifecycle commands, or broader tool classes.
