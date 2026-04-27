# Compatibility Tracker

This tracker makes Pi parity explicit. It is an index of status and evidence, not a replacement for detailed plans, checkpoints, or test reports.

Last updated: 2026-04-27

## Status rules

- Keep implementation status separate from evidence status when they differ.
- Link evidence before using `verified`.
- Use `unknown` instead of guessing.
- Use `blocked` only with a named blocker.

## Phase 1 compatibility matrix

| Area | PRD ref | Category / tier | Adapter path | Implementation status | Evidence status / link | Blocker / unknown | Next action |
|---|---|---|---|---|---|---|---|
| Prompt streaming | §6.4 | Tier A stock RPC | RPC | `verified` at adapter and M2 TUI basic loop | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | Stream/local-cancel hardening is implemented and manual smoke passed; broader dogfood/perf evidence remains | Track user-perceived latency under performance evidence. |
| Tool start/finish events | §6.4 | Tier A stock RPC | RPC | `verified` at adapter and compact M2 TUI display | `../status/compatibility-evidence-2026-04-27.md` | Expandable details/overflow UI remains future work | Keep richer tool-output UX in M3/M4 UI backlog. |
| Dialogs: select/confirm/input/editor | §6.4 | Tier A stock RPC | RPC | `verified` at adapter and M2 TUI basic loop | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | Organic extension-driven dialog coverage remains limited | Cover with canonical compatibility suite once fixtures exist. |
| Notifications/status/widgets/title | §6.4 | Tier A stock RPC | RPC | `verified` for mapped dispatch actions; `implemented-unverified` for richer TUI presentation | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | Rich component-backed UI still Tier B; current widget/status presentation is intentionally compact | Keep tracker split between stock RPC and rich UI. |
| Editor text updates | §6.4 | Tier A stock RPC | RPC | `planned` | `../status/compatibility-evidence-2026-04-27.md` | Pi RPC exposes extension UI `set_editor_text`; yach has no protocol/UI surface yet | Add protocol event and TUI behavior when editor parity is prioritized. |
| Model selection | §6.4 | Tier A stock RPC | RPC | `verified` for M2 TUI basic loop | `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | Backend rejection rollback/pending UI can improve later | Keep broad model/provider compatibility for later provider work. |
| Thinking control | §6.4 / §7.1 | Tier A stock RPC | RPC | `partial` | `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | UI/protocol/serializer exist, but live backend acceptance/reporting needs focused smoke evidence | Add thinking-level smoke and update evidence. |
| Session clone/current-branch duplication | §6.2 / §6.4 | Tier A stock RPC | RPC | `partial` | `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | Current Ctrl+F maps to stock RPC `clone`; this is not entry-id `fork` or tree navigation | Design M3 session tree/fork UX around structured session data. |
| Entry-id fork and fork messages | §6.2 / §6.4 | Tier A stock RPC | RPC | `partial` | `../status/compatibility-evidence-2026-04-27.md`; `just test` 2026-04-27 covers protocol serialization for `fork` with `entryId`/`position`, parsing `get_fork_messages`, and TUI fork-point selection event flow; `just run smoke-pi-rpc` 2026-04-27 confirms live `get_fork_messages` succeeds on an empty smoke session; `just run smoke-pi-rpc-fork-seeded` confirms seeded live entry-id fork succeeds | Pi RPC exposes `fork` and `get_fork_messages`; yach now models the entry-id fork request/response and has a basic TUI fork-point picker, but not a full session tree | Add real session tree/session-file navigation and stronger assertions around fork-result state/selected text if needed. |
| Session stats/messages/export | §6.2 / §6.4 | Tier A stock RPC | RPC | `partial` for structured stats/messages; `planned` for export | `../status/compatibility-evidence-2026-04-27.md`; `just test` 2026-04-27 covers typed `get_messages` and `get_session_stats` response parsing | `get_session_stats` and `get_messages` now have structured proto/adapter events, but no full TUI session browser and `export_html` remains unmodeled | Use typed stats/messages in M3 session UI; model export when prioritized. |
| Dynamic slash commands from resources/extensions | §6.3 / §6.4 | Tier A stock RPC | RPC / SDK sidecar TBD | `planned` | `../status/compatibility-evidence-2026-04-27.md` | Pi RPC exposes `get_commands`; yach currently uses static built-in slash commands | Decide command invocation/loading path with resource compatibility work. |
| Settings/resources/packages | §6.1 | Resource compatibility | RPC / SDK sidecar / native file I/O TBD | `unknown/planned` | `../status/compatibility-evidence-2026-04-27.md` | No yach settings parser, ResourceLoader equivalent, package manifest parser, extension discovery, or `/reload` semantics; implementation path undecided | Decide SDK sidecar vs native file/resource loading for M3. |
| Existing Pi session files/tree | §6.2 | Session compatibility | RPC / SDK sidecar / native file I/O TBD | `partial` | `../status/m2-tui-checkpoint.md`, `../status/compatibility-evidence-2026-04-27.md` | `sessionFile` is parsed and current branch can clone, but opening existing files, recents, tree browsing, branch summaries, and compaction details are missing | Design session file/tree model for M3. |
| Logic suite examples | §6.5 | Canonical logic suite | RPC/SDK depending on surface | `planned` | `../status/compatibility-evidence-2026-04-27.md` | Suite harness not created | Define fixtures for hello/question/todo/dynamic-tools/provider-payload after resource path decision. |
| Rich UI suite examples | §6.5 | Tier B rich UI | SDK sidecar | `deferred` | `../status/compatibility-evidence-2026-04-27.md` | SDK sidecar/rich protocol not built | Track under M4. |

## Entry template

Use `templates/compatibility-entry-template.md` when adding rows that need more detail than the matrix can hold.

Required fields:

- Area
- PRD reference
- Category/tier
- Adapter path
- Implementation status
- Evidence status/link
- Blocker/unknown
- Next action
