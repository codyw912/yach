# Changelog

Notable changes to yach are documented here, grouped in the style of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Breaking

#### CLI

- Replace `--backend native-fixture` with `--backend fixture`. The `native` and `native-provider` backend selectors are removed; omit `--backend` to use a provider. [#200](https://github.com/codyw912/yach/pull/200)

#### Protocol

- Tool-review decisions now use `"decision":"approve"` instead of `"decision":"apply"`. Clients must negotiate `structured_review_rows` to receive actionable reviews and handle `tool_review_resolved` events. [#248](https://github.com/codyw912/yach/pull/248)
- Update JSONL clients to protocol `0.3.0`. Model selection now uses `model_activation_requested` and `model_activation_finished`, replacing `model_selected`, `model_selected_detailed`, `model_changed`, and `model_change_failed`; backend state contains structured session and default model state instead of flat model fields. `Ready` reports only negotiated capabilities, and clients that negotiate `prompt_attempt_reset` must retract the specified UTF-8 suffix before a restarted attempt. [#263](https://github.com/codyw912/yach/pull/263) [#266](https://github.com/codyw912/yach/pull/266)

#### Persistence

- Default session logs move from `<project>/.yach/native-sessions/`, briefly renamed `.yach/sessions/`, to `~/.yach/sessions/<project-slug>--<canonical-path-sha256>/`. Existing project-local logs are not moved or discovered automatically. Copy them into the new project directory, set an absolute `YACH_SESSION_DIR`, or use `--session-path` for headless and RPC sessions. [#200](https://github.com/codyw912/yach/pull/200) [#258](https://github.com/codyw912/yach/pull/258)
- Built-in tool results, including persisted `result_content`, are plain text instead of JSON objects. File reads preserve their contents, shell results contain raw output, and errors or truncation use bracketed notices; scripts must not JSON-decode those result strings. [#216](https://github.com/codyw912/yach/pull/216)
- Connection credentials move from the system keychain to `~/.yach/credentials.json`, with private directory and file permissions. Keychain credentials are not imported; repair existing connections once through `/connect` by re-entering their keys. [#230](https://github.com/codyw912/yach/pull/230) [#233](https://github.com/codyw912/yach/pull/233)
- Extension grant files become versioned authority records with retained grant and revoke history. Existing grants load automatically and convert on the next grant or revoke; scripts reading these files must handle the new document shape. [#277](https://github.com/codyw912/yach/pull/277)

#### Config

- Model and thinking defaults now live in `~/.yach/config.toml`, separate from each session's active model. Ordinary model switches affect only the session once a default exists; use the explicit save-default action to change new sessions. Valid legacy `active-model.json` and thinking preferences migrate automatically when no explicit TOML default exists. Connection metadata advances to `yach.connections.v2` with stable connection keys; older binaries cannot read the upgraded registry. [#231](https://github.com/codyw912/yach/pull/231) [#263](https://github.com/codyw912/yach/pull/263)
- Project `.yach/config.json` can no longer grant `shell.allow` or `shell.env_allow` authority. Move intended command and environment allowances into user-owned `~/.yach/config.json`; repository settings can still restrict runtime bounds. [#259](https://github.com/codyw912/yach/pull/259)
- Project `.yach/theme.json` no longer selects the TUI theme. Put personal settings in `~/.yach/theme.json` or select a file explicitly with `YACH_THEME`; existing color-token overrides remain supported. [#308](https://github.com/codyw912/yach/pull/308)

### Added

- Continue a named headless session across invocations with `yach run --session <id>`. [#192](https://github.com/codyw912/yach/pull/192)
- Use OpenAI's Responses API with `YACH_RIG_PROVIDER=openai`, while retaining Chat Completions for compatible endpoints and configurable output-budget parameter spelling. [#205](https://github.com/codyw912/yach/pull/205) [#218](https://github.com/codyw912/yach/pull/218)
- Resolve model context windows, output budgets, and prices from a built-in catalog, background-fetched metadata, and user or project `models.toml` overrides. Headless outcomes include configuration sources and estimated cost when usage and prices are known. [#220](https://github.com/codyw912/yach/pull/220) [#221](https://github.com/codyw912/yach/pull/221)
- Create and manage named API-key connections through `/connect`, then select their models through `/model`. Discovery uses provider-returned IDs, and the searchable picker opens from cached results while refreshing in the background. [#223](https://github.com/codyw912/yach/pull/223) [#224](https://github.com/codyw912/yach/pull/224) [#225](https://github.com/codyw912/yach/pull/225) [#236](https://github.com/codyw912/yach/pull/236)
- Log in to a ChatGPT subscription through `/connect` with device-code login, adopt an existing login, reauthenticate, or log out. The picker includes a built-in Codex model catalog. [#244](https://github.com/codyw912/yach/pull/244)
- Connect to Anthropic Messages gateways with an auth token and optional base URL, through `/connect` or `YACH_RIG_ANTHROPIC_AUTH_TOKEN`. API-key authentication remains supported; configuring both credential variables is an error. [#331](https://github.com/codyw912/yach/pull/331)
- Use provider-native OpenAI Responses compaction on supported models, with portable summaries and persisted native replay for continuation and model switching. [#238](https://github.com/codyw912/yach/pull/238)
- Run `yach rpc` as a stdio JSONL server without a terminal. Initialize-gated negotiation, recoverable malformed input, and EOF cancellation share the same session behavior as the TUI. [#246](https://github.com/codyw912/yach/pull/246)
- Use the bundled hashline read/edit extension for snapshot-tagged reads and reviewed edits, with an all-or-none replacement of the built-in read/edit pair. [#252](https://github.com/codyw912/yach/pull/252)
- Switch between `review` and `accept-edits` with `/approval`, or explicitly confirm session-only `full-access` in the approval picker. Full access resets on restart or session switch; headless `--full-auto` selects the backend's full-access mode rather than approving individual review requests. These modes do not provide an OS sandbox. [#259](https://github.com/codyw912/yach/pull/259) [#261](https://github.com/codyw912/yach/pull/261)
- Grant or revoke declared extension network and process capabilities through the CLI or TUI. Capability-bearing extensions require user-owned consent before activation; grants are consent records, not OS enforcement. [#276](https://github.com/codyw912/yach/pull/276)
- Use optional intent-aware review with a bundled reviewer, user restrictions, and explicit risk or clarification holds. Automatic execution remains disabled: actions the reviewer would allow still require human approval, and human-only actions are handed back rather than executed. [#278](https://github.com/codyw912/yach/pull/278)
- Choose `full` or `minimal` distribution presets, enable or disable individual components, and use session-only `--preset` overrides for `run` and `rpc`. Removed bundles stay removed until explicitly restored; optional build features can omit the bundled extensions. Presets do not grant execution authority. [#279](https://github.com/codyw912/yach/pull/279)
- Customize the compaction summary prompt with a file named by user-scope `compaction.summary_prompt`. Project settings cannot replace this prompt. [#279](https://github.com/codyw912/yach/pull/279)
- Inspect session timelines with `yach sessions list` and `yach sessions show <id|latest>`, including JSON output, timestamps, tool durations, provider attempts, retries, and request IDs. Opt-in `YACH_CAPTURE_REQUESTS=<absolute dir>` records request bodies separately from session logs; older logs still load. [#283](https://github.com/codyw912/yach/pull/283)
- Refresh model listings explicitly with `/model refresh`, `Ctrl+R` in the picker, or `yach models refresh`. ChatGPT discovery tracks the latest stable Codex release rather than waiting for a yach release to expose newer models. [#289](https://github.com/codyw912/yach/pull/289)
- Render assistant replies as Markdown, including headings, lists, code blocks, links, quotes, and simple tables, with per-entry caching during streaming. [#290](https://github.com/codyw912/yach/pull/290)
- Choose dark, light, system, or monochrome appearance, 18 named palettes, or user-installed extension themes. `YACH_THEME` accepts a name or path, automatic appearance reads `COLORFGBG`, and `NO_COLOR` removes colors. [#253](https://github.com/codyw912/yach/pull/253) [#308](https://github.com/codyw912/yach/pull/308)
- Select `box`, `rule`, or `band` composer layouts through theme settings. `NO_COLOR` preserves the selected layout and spacing, and theme validation still runs when colors are disabled. [#319](https://github.com/codyw912/yach/pull/319)

### Changed

- Send native tool-call and result messages to providers instead of a flattened transcript with nested result envelopes, including when rebuilding context after resume or compaction. [#204](https://github.com/codyw912/yach/pull/204) [#210](https://github.com/codyw912/yach/pull/210)
- Clear older tool-result bodies before summarization when enough context can be reclaimed, while protecting recent results and retaining the original session log. Some turns can continue without a summary call. [#242](https://github.com/codyw912/yach/pull/242)
- Stream provider text as it arrives rather than collecting a whole response before displaying it. Transient failures use a bounded three-attempt policy, respect retry hints, and preserve or explicitly reset visible text instead of duplicating it. [#246](https://github.com/codyw912/yach/pull/246) [#265](https://github.com/codyw912/yach/pull/265) [#266](https://github.com/codyw912/yach/pull/266)
- Keep each tool call in one transcript row through live output, inline review, and completion. Finished rows expand on demand, successful rows are more compact, and rejection titles distinguish blocked, denied, cancelled, and failed outcomes. [#248](https://github.com/codyw912/yach/pull/248) [#312](https://github.com/codyw912/yach/pull/312)
- Use a full-width composer that grows with wrapped input up to eight rows, indicates overflow, and preserves transcript position while drafting. Unfocused terminals hide the cursor and dim the composer. [#245](https://github.com/codyw912/yach/pull/245) [#249](https://github.com/codyw912/yach/pull/249) [#251](https://github.com/codyw912/yach/pull/251)
- Restore terminal-native text selection and scrollback instead of capturing the mouse and using the alternate screen. [#262](https://github.com/codyw912/yach/pull/262)
- Show changed-line evidence and the next snapshot tag after applied edits, with specific guidance for unknown, ambiguous, or mismatched snapshots. [#262](https://github.com/codyw912/yach/pull/262)
- Apply selected thinking effort to supported providers and retain it in the session. [#262](https://github.com/codyw912/yach/pull/262)
- Recall submitted prompts with Up/Down without disrupting multiline cursor movement. [#274](https://github.com/codyw912/yach/pull/274)
- Approve an exact command and working directory for the current session, with grants cleared on restart. [#274](https://github.com/codyw912/yach/pull/274)
- Retain a larger live-output tail for running tools. [#274](https://github.com/codyw912/yach/pull/274)
- Flush session data durably at turn completion instead of syncing every event. Provider-attempt records are written immediately and flushed with the turn or manual compaction; an OS crash or power loss before that flush can lose the unsynced records. [#270](https://github.com/codyw912/yach/pull/270) [#284](https://github.com/codyw912/yach/pull/284)
- Simplify backend status and error wording by removing obsolete implementation names. [#199](https://github.com/codyw912/yach/pull/199) [#200](https://github.com/codyw912/yach/pull/200)
- Mark assumed context windows in `/status` and the context meter, and warn when an unlisted model uses the default window. An explicit `models.toml` context-window override removes the assumed marker. [#305](https://github.com/codyw912/yach/pull/305)

### Fixed

- Accept bracketed paste in connection credential and text dialogs without exposing secret input. [#226](https://github.com/codyw912/yach/pull/226)
- Stop showing a missing-environment provider setup error after quitting a session that uses stored connections. [#228](https://github.com/codyw912/yach/pull/228)
- Keep model activation correlated with the selected connection and open the thinking picker only after successful activation; preserve the acknowledged thinking level in status feedback. [#234](https://github.com/codyw912/yach/pull/234) [#235](https://github.com/codyw912/yach/pull/235)
- Let users remove the last active connection and clear its saved activation instead of restoring a dead target on restart. [#245](https://github.com/codyw912/yach/pull/245)
- Cancel streaming with Esc while keeping drafted input. [#245](https://github.com/codyw912/yach/pull/245)
- Distinguish incomplete searches from complete zero-match results when the file budget is exhausted. [#245](https://github.com/codyw912/yach/pull/245)
- Keep review denials and failed or cancelled turns distinct from assistant messages, including after resume. [#245](https://github.com/codyw912/yach/pull/245)
- Restore accurate tool/message counts and compaction history after resume, show compaction summaries, and report cumulative token totals. [#253](https://github.com/codyw912/yach/pull/253) [#274](https://github.com/codyw912/yach/pull/274)
- Keep action feedback visible instead of overwriting it with periodic state updates or dropping it first on narrow terminals. [#274](https://github.com/codyw912/yach/pull/274)
- Suggest a matching slash command for a typo instead of sending the mistyped command to the provider as a prompt. [#274](https://github.com/codyw912/yach/pull/274)
- Explain when a project symlink points outside the allowed root without weakening the project boundary. [#255](https://github.com/codyw912/yach/pull/255)
- Report the exact malformed hashline hunk with syntax-specific correction guidance. [#257](https://github.com/codyw912/yach/pull/257)
- Keep missing provider usage distinct from reported zero usage and tolerate unknown provider streaming events. [#214](https://github.com/codyw912/yach/pull/214)
- Distinguish provider failures from internal tool-loop failures in headless turn outcomes. [#241](https://github.com/codyw912/yach/pull/241)
- Avoid false extension catalog conflicts when home and project install stores alias the same files; failed extension commands now exit with status 1. [#277](https://github.com/codyw912/yach/pull/277)
- Accept ChatGPT subscription event streams served as `text/plain` without loosening other providers' content-type checks. [#286](https://github.com/codyw912/yach/pull/286)
- Discover gateway models whose listings omit optional OpenAI metadata. [#286](https://github.com/codyw912/yach/pull/286)
- Report invalid provider environment settings even when stored connections exist, rather than silently hiding the environment connection or using a stored default. [#331](https://github.com/codyw912/yach/pull/331)
- Recognize the newer Codex model family and its context windows, and invalidate model caches when the advertised client version changes so upgrades do not keep stale listings. [#287](https://github.com/codyw912/yach/pull/287) [#288](https://github.com/codyw912/yach/pull/288)
- Allow typing and pasting a draft while an already-reviewed tool runs. Keep Ctrl+C cancellation working when the slash popup is open. [#291](https://github.com/codyw912/yach/pull/291)
- Keep the live turn next to archived history and the newest transcript and approval controls visible after terminal resizing; large review diffs have bounded previews that expand with Ctrl+O. [#292](https://github.com/codyw912/yach/pull/292)
- Prevent slash-popup rendering from panicking in one-row or two-row terminal viewports. [#303](https://github.com/codyw912/yach/pull/303)
- Improve palette contrast and semantic error/warning colors across dark and light themes. [#311](https://github.com/codyw912/yach/pull/311)
- Keep approval mode visible on narrow terminals and color status messages by severity. [#311](https://github.com/codyw912/yach/pull/311)
- Restore tool targets in resumed transcript titles when recorded arguments are available. [#313](https://github.com/codyw912/yach/pull/313)

Plus 85 internal changes (CI, tests, docs, development tooling, refactors, and catalog maintenance).

## [0.1.0] - 2026-07-27

First published release. See the [GitHub release notes](https://github.com/codyw912/yach/releases/tag/v0.1.0).
