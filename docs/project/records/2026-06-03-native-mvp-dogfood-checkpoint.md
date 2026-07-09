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

Date: 2026-06-17

| Area | Result | Evidence |
| --- | --- | --- |
| Provider adapter seam | blocked | `smoke-rig-provider-request` returned `rig_smoke_outcome=MissingConfig`; `completed=false`; `matched_expected_text=false`; missing `YACH_RIG_ANTHROPIC_API_KEY`. |
| Native-provider tool loop | pass | `native_provider_agent`: 15 passed. |
| TUI review state | pass | `tool_review`: 5 passed. |
| Paste batching | pass | `prompt_paste_inserts_text_as_batch`: 1 passed. |
| Startup/profile smoke | pass | `samples_collected=10`; process-to-first-render p95 `56.719ms`, `tui_first_render_end_since_main` p95 `15.860ms`. |

No local code blocker was found in the no-secret run. The provider seam and
live native-provider dogfood pass need provider credentials before they can
verify explicit resume and recoverable failure visibility.

## Latest Live Native-Provider Run

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

Resolved since this run: native sessions now use distinct logs and resume
targets the selected/latest log instead of cumulative `default` history.
Remaining blocker: `list_project_paths` needs visible listed-path output in the
TUI transcript if the next live run still reproduces the collapsed preview.

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
| 6 | Use `/resume`, select the current native session, then confirm the transcript hydrates without replacing active text. | Prior session state is available enough for practical resume/dogfood inspection. |
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
