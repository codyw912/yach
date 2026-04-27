# Compatibility Tracker

This tracker makes Pi parity explicit. It is an index of status and evidence, not a replacement for detailed plans, checkpoints, or test reports.

Last updated: 2026-04-26

## Status rules

- Keep implementation status separate from evidence status when they differ.
- Link evidence before using `verified`.
- Use `unknown` instead of guessing.
- Use `blocked` only with a named blocker.

## Phase 1 compatibility matrix

| Area | PRD ref | Category / tier | Adapter path | Implementation status | Evidence status / link | Blocker / unknown | Next action |
|---|---|---|---|---|---|---|---|
| Prompt streaming | §6.4 | Tier A stock RPC | RPC | `verified` at adapter; `implemented-unverified` in TUI | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | TUI needs live/manual evidence and stream correlation hardening | Re-check after M2 hardening pass. |
| Dialogs: select/confirm/input/editor | §6.4 | Tier A stock RPC | RPC | `verified` at adapter; `implemented-unverified` in TUI | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Dialog Unicode/cursor safety and manual UX validation remain | Fix high-priority dialog hardening before M2 complete. |
| Notifications/status/widgets/title | §6.4 | Tier A stock RPC | RPC | `verified` for mapped dispatch actions; `implemented-unverified` in TUI | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Rich component-backed UI still Tier B | Keep tracker split between stock RPC and rich UI. |
| Editor text updates | §6.4 | Tier A stock RPC | RPC | `planned` | `../status/m0-m1-checkpoint.md` marks omitted | Needs protocol/UI surface | Plan with protocol updates when prioritized. |
| Session switch/fork/stats/export | §6.4 | Tier A stock RPC | RPC | `partial` | `../status/m0-m1-checkpoint.md`, `../status/m2-tui-checkpoint.md` | Basic select/fork paths exist; real session list/tree/stats/export remain incomplete | Track basic M2 hardening separately from M3 session compatibility. |
| Settings/resources/packages | §6.1 | Resource compatibility | RPC / SDK sidecar TBD | `planned` | No evidence yet | Needs ResourceLoader/setting parity work | Track under M3 plan. |
| Existing Pi session files/tree | §6.2 | Session compatibility | RPC / SDK sidecar TBD | `planned` | No evidence yet | Needs session-file compatibility design | Track under M3 plan. |
| Logic suite examples | §6.5 | Canonical logic suite | RPC/SDK depending on surface | `planned` | No evidence yet | Suite harness not created | Define suite during M3. |
| Rich UI suite examples | §6.5 | Tier B rich UI | SDK sidecar | `deferred` | No evidence yet | SDK sidecar/rich protocol not built | Track under M4. |

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
