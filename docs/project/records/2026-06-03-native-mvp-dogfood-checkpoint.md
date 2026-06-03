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

## Live Native-Provider Dogfood

Run with provider env configured:

```sh
just run tui --backend native-provider
```

Then exercise this prompt sequence in one session:

| Step | Prompt | Expected Result |
| --- | --- | --- |
| 1 | `Use read_text_file to read README.md, then reply with a one sentence summary.` | Tool progress is visible before the answer; final answer summarizes README. |
| 2 | `Create a new file named dogfood-provider-edit.txt with the content "native provider edit dogfood ok".` | Review prompt appears, approval applies the create, and input returns after finish. |
| 3 | `Use read_text_file to inspect dogfood-provider-edit.txt, then replace "ok" with "passed".` | Read and edit tool progress are visible; review approval applies the change; final answer follows tool evidence. |
| 4 | `Use search_project to find "native provider edit dogfood passed", then list the current directory with list_project_paths.` | Search/list tool progress appears and results are summarized without freezing. |
| 5 | Quit and relaunch `just run tui --backend native-provider`. | Prior session state is available enough for practical resume/dogfood inspection. |

## Current Status

| MVP Bar | Status | Evidence / Notes |
| --- | --- | --- |
| launch quickly and type immediately | needs live pass | Startup traces are strong, but this checkpoint still needs a fresh live run. |
| provider prompts stream responses | needs live pass | `smoke-rig-provider-request` covers provider seam; TUI needs fresh run. |
| read/search/list tools | needs live pass | Backend tests pass; live checkpoint should confirm visible progress. |
| create/edit tools | needs live pass | Backend tests pass; live checkpoint should confirm review and local effects. |
| review without TUI freeze | needs live pass | PR #104 added progress visibility; UI review regressions pass. |
| multi-round without default cap | pass in tests | `native_provider_agent_default_loop_has_no_round_limit`. |
| persist/resume enough session state | needs live pass | Resume tests exist; live checkpoint should verify practical UX. |
| recoverable failures | not yet checkpointed | Add a deliberate failure prompt once the happy path passes. |
| Pi explicit reference only | pass | Native is default; Pi remains explicit `--backend pi`. |

## Next Blocker Rule

After the live run, record the first item that prevents using yach for real work
as the next blocker. Prefer fixing that blocker before adding extension package
UX, more lifecycle commands, or broader tool classes.
