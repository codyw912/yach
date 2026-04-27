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
| Prompt streaming | §6.4 | Tier A stock RPC | RPC | `verified` | `../status/m0-m1-checkpoint.md` | None known from checkpoint | Re-check in M2 TUI checkpoint. |
| Dialogs: select/confirm/input/editor | §6.4 | Tier A stock RPC | RPC | `verified` | `../status/m0-m1-checkpoint.md` | Editor text updates still separately omitted | Re-check with TUI dialog UX. |
| Notifications/status/widgets/title | §6.4 | Tier A stock RPC | RPC | `verified` for mapped dispatch actions | `../status/m0-m1-checkpoint.md` | Rich component-backed UI still Tier B | Keep tracker split between stock RPC and rich UI. |
| Editor text updates | §6.4 | Tier A stock RPC | RPC | `planned` | `../status/m0-m1-checkpoint.md` marks omitted | Needs protocol/UI surface | Plan with protocol updates when prioritized. |
| Session switch/fork/stats/export | §6.4 | Tier A stock RPC | RPC | `implemented-unverified` / partial | `../status/m0-m1-checkpoint.md` | Stats/export not fully modeled in checkpoint | Audit during M2/M3 checkpoint. |
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
