# Compatibility evidence pass — 2026-04-27

## Scope

This pass expands `docs/project-os/compatibility.md` with concrete evidence and explicit unknowns after the M2 TUI alpha became dogfoodable. It focuses on PRD §6.1-§6.5:

- settings, packages, and resources
- session compatibility
- extension parity categories
- Tier A stock RPC parity
- canonical compatibility suite placeholders

This is an evidence/status pass, not a compatibility implementation pass.

## Environment

- Date: 2026-04-27
- Branch/commit at audit start: `main` after `cab1753 docs(benchmarks): add performance buildout placeholder`
- Pi package inspected: `/Users/cody/.local/share/mise/installs/npm-mariozechner-pi-coding-agent/0.70.2/lib/node_modules/@mariozechner/pi-coding-agent`
- Primary Pi type source: `dist/modes/rpc/rpc-types.d.ts`

## Commands run

```text
just run print-capabilities
```

Observed:

```text
capability=PromptStreaming
capability=Dialogs
capability=Notifications
capability=StatusEntries
capability=Widgets
capability=SessionForking
```

```text
just run smoke-pi-rpc
```

Observed:

```text
smoke_outcome=Initialized
operation=initialize success=true
operation=get_state success=true
operation=select_model success=true
operation=fork_session success=true
operation=get_session_stats success=true
operation=get_messages success=true
operation=resolve_dialog success=true
```

```text
just run smoke-pi-rpc-prompt
```

Observed:

```text
prompt_smoke_outcome=Completed
saw_delta=true
saw_tool_start=false
saw_tool_finish=false
completed=true
response_chars=13
```

```text
just run smoke-pi-rpc-tool
```

Observed:

```text
prompt_smoke_outcome=Completed
saw_delta=true
saw_tool_start=true
saw_tool_finish=true
completed=true
response_chars=13
```

## Tier A stock RPC evidence

| Surface | Current yach status | Evidence | Limitations / next action |
|---|---|---|---|
| Prompt streaming | `verified` for adapter and M2 TUI basic loop | `smoke-pi-rpc-prompt` saw deltas and completion; parser/UI paths are covered in unit tests and manual M2 smoke. | Does not prove high-rate stream latency; track in performance evidence. |
| Tool start/finish events | `verified` for adapter parsing and compact TUI rendering | `smoke-pi-rpc-tool` saw tool start and finish; unit tests cover transcript/tool state updates. | Tool-output overflow/expandable detail remains future UI work. |
| Dialogs: select/confirm/input/editor | `verified` for adapter and M2 TUI smoke harness | `smoke-pi-rpc` resolves a dialog; `tui-dialog-smoke` was manually validated for confirm/input/select/editor during M2 polish. | Organic extension-driven dialog coverage remains part of the future compatibility suite. |
| Notifications/status/widgets/title | `verified` for adapter mapping; `implemented-unverified` for richer presentation | `print-capabilities` exposes notifications/status/widgets; parser maps `notify`, `setStatus`, `setWidget`, and `setTitle`; M2 status surfaces are present. | Component-backed/rich widgets are Tier B and require SDK sidecar/richer protocol. |
| Editor text updates | `planned` / gap | Pi RPC exposes extension UI method `set_editor_text` in `rpc-types.d.ts`; yach has no explicit protocol/UI event for remote editor replacement/text updates. | Add protocol event and TUI behavior when prioritizing editor parity. |
| Model selection | `verified` for M2 basic loop | `smoke-pi-rpc` `select_model` succeeds; TUI manual smoke verified backend model list, long-list scroll, selection request, and status update. | Backend rejection rollback/pending UI can be improved later. |
| Thinking level | `partial` | yach serializes `set_thinking_level` and UI can select thinking level. | Needs live smoke/evidence that current Pi accepts and reports thinking changes across models. |
| Session clone/fork | `partial` | `smoke-pi-rpc` `fork_session` succeeds through current yach `clone` serialization; M2 TUI Ctrl+F duplicates current active branch after a user message. | This is current-branch clone, not entry-id `fork`; tree browsing and cloned-session discoverability remain M3 work. |
| Session stats/messages | `implemented-unverified` / adapter-smoked only | `smoke-pi-rpc` succeeds for `get_session_stats` and `get_messages` raw commands. | Responses are not structured in `yach-proto`/TUI yet; stats/export/tree UX is missing. |
| Export HTML | `planned` | Pi RPC exposes `export_html`; yach has no protocol/UI command. | Model in proto/adapter if export is part of M3 compatibility scope. |
| Slash commands from resources/extensions | `planned` | Pi RPC exposes `get_commands`; yach currently has a static slash registry for built-in TUI commands. | Add dynamic command loading/invocation design after resource compatibility decisions. |

## Pi RPC command gap snapshot

Documented Pi RPC commands inspected from `rpc-types.d.ts`:

- Prompt/control: `prompt`, `steer`, `follow_up`, `abort`
- Session lifecycle/state: `new_session`, `get_state`, `switch_session`, `fork`, `clone`, `get_fork_messages`, `get_last_assistant_text`, `set_session_name`, `get_messages`
- Model/thinking/modes: `set_model`, `cycle_model`, `get_available_models`, `set_thinking_level`, `cycle_thinking_level`, `set_steering_mode`, `set_follow_up_mode`
- Compaction/retry: `compact`, `set_auto_compaction`, `set_auto_retry`, `abort_retry`
- Tool/execution/export: `bash`, `abort_bash`, `get_session_stats`, `export_html`
- Commands/resources: `get_commands`

Yach currently models or uses only a subset directly. Important M3 gaps:

1. `abort` for true backend cancellation.
2. `new_session`, `switch_session` by `sessionPath`, and `get_messages` as structured session/recent-session foundations.
3. Entry-id `fork` and `get_fork_messages` for tree/fork-from-entry UX.
4. `get_session_stats`, `export_html`, `set_session_name`, and branch/message metadata as structured proto events.
5. `get_commands` for dynamic slash commands from prompts/skills/extensions.
6. `set_editor_text` extension UI request for editor text update parity.
7. `compact` and auto-compaction/retry commands for explicit compaction/retry UI.

## Settings, packages, and resources (§6.1)

Current status: `unknown/planned`.

Evidence found:

- No yach crate currently implements Pi settings parsing for `~/.pi/agent/settings.json` or `.pi/settings.json`.
- No yach crate currently implements Pi package/resource discovery, package manifest parsing, skills/prompts/themes/context-file loading, extension path scanning, or `/reload` semantics.
- In the current architecture, these surfaces are implicitly handled by the live Pi backend, not by yach.
- `Capability::ThemeLoading` exists in `yach-proto`, but stock RPC capabilities do not expose theme loading.

Implication:

Yach is currently a thin Pi RPC shell for M2, not yet a Pi-compatible resource consumer. A user running yach through the stock Pi backend benefits from whatever Pi loads for that process, but yach cannot independently inspect, present, reload, or prove file-first resource compatibility.

Key design choice for M3:

- Build an SDK sidecar/resource bridge around Pi's existing ResourceLoader/SessionManager, or
- Implement direct Rust file/resource loading for the stable file-first surfaces.

Either path must preserve invariant I8: file-first configuration and resources stay first-class.

## Session compatibility (§6.2)

Current status: `partial`.

What exists:

- `BackendState.session_file` and `session_id` are parsed from `get_state` responses.
- The TUI shows an observed session id and can request current-branch clone through stock RPC `clone`.
- Raw `get_messages` and `get_session_stats` smoke commands succeed.

What is missing:

- Opening existing Pi session files as a first-class yach flow.
- Continuing recent sessions.
- Real session picker backed by session files/recents instead of observed IDs.
- Tree model and tree navigation.
- Entry-id fork via stock RPC `fork`.
- Branch summaries and compaction state beyond coarse status fields.
- Structured protocol types for messages/stats/export/fork-message responses.

Recommended M3 order:

1. Model Pi RPC session state/messages/stats/fork-message responses in `yach-proto`.
2. Add adapter parse/serialize coverage for session commands with unit tests.
3. Add a non-fullscreen CLI/session smoke path for `get_messages`, `get_session_stats`, `new_session`, `switch_session`, `clone`, and entry-id `fork` where safe.
4. Build session picker/tree UI from structured backend data.
5. Decide whether existing session-file discovery comes from SDK sidecar or direct Rust file scanning.

## Canonical compatibility suite (§6.5)

Current status: `planned`.

Logic suite placeholders from PRD:

- `hello.ts`
- `question.ts`
- `todo.ts`
- `dynamic-tools.ts`
- `provider-payload.ts`

Rich UI suite placeholders from PRD:

- `questionnaire.ts`
- `custom-footer.ts`
- `custom-header.ts`
- `modal-editor.ts`
- `overlay-test.ts`

Recommendation:

Create explicit fixtures under a future `docs/compatibility/` or `crates/yach-compat-tests/` surface once the M3 resource/session path is chosen. Keep rich UI suite deferred until SDK sidecar/rich protocol work begins.

## Compatibility tracker update summary

This pass should update `docs/project-os/compatibility.md` to:

- Link this evidence doc.
- Split or clarify Tier A rows so model selection, thinking, session tree/fork, stats/messages/export, editor text updates, and dynamic slash commands are not hidden inside one broad row.
- Mark settings/resources as `unknown/planned`, not merely planned, because implementation path is unresolved.
- Mark existing Pi session files/tree as `partial` overall with explicit unknowns, not simply planned.
- Keep Tier B/rich UI as `deferred` and blocked on SDK sidecar/richer protocol.
