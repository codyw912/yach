# How Other Harnesses Display Tool Calls In Resumed Sessions

Date: 2026-07-14

Context: the 2026-07-14 live dogfood run noted that resumed yach sessions do
not show tool call outputs the way live runs do. This record summarizes how
the standard comparison set (Codex CLI, Claude Code, opencode, Pi) persists
and replays tool interactions, from a source survey of session storage and
resume rendering code.

## Key finding

All four harnesses converge on the same architecture, and yach is a
deliberate outlier:

1. The session log is the full model-visible transcript. Tool call arguments
   and outputs are persisted verbatim, after execution-time caps (Codex ~10KB
   model truncation; opencode and Pi 2000 lines / 50KB with overflow spilled
   to side files). Persisted content equals what the model saw live, so
   resume is lossless relative to the live run.
2. Resume rendering reuses the live rendering path. Codex replays persisted
   items through the same event handlers as live events; opencode and Pi
   render live and resumed sessions from the same store and components;
   Claude Code re-renders history in the live collapsed style. The universal
   display convention is collapsed tool rows with an expand affordance,
   identical live and resumed. No harness has a distinct summary mode for
   resume.
3. Model context and display are separate pipelines over the same log, both
   full fidelity. Only compaction (all four) or explicit opt-in pruning
   (opencode) reduces model context.

## Per-harness notes

| Harness | Persistence | Resume display | Resume model context |
| --- | --- | --- | --- |
| Codex CLI | JSONL rollout files with verbatim `FunctionCall`/`FunctionCallOutput` (`codex-rs/rollout`) | Persisted turns routed through live event handlers (`tui/src/chatwidget/replay.rs`) | History rebuilt from newest compaction checkpoint with full tool outputs |
| Claude Code | Project JSONL is the literal messages array with full `tool_use`/`tool_result` blocks | Live collapsed style with expand (ctrl+o); resume-hang bugs on huge outputs prove it renders them | Full; users confirmed context survived even during display regressions |
| opencode | SQLite parts with complete `{input, output, metadata}`; 2000-line/50KB cap at execution | Same store and components for live and resumed rendering | Every turn rebuilds model messages from the DB, tool parts included |
| Pi | JSONL tree sessions with full tool args/results; bash capped with full output path recorded | `renderInitialMessages()` reuses the live tool component and `updateResult()` | Stored messages passed through verbatim |

## Implications for yach

Yach persists only redacted tool evidence (`NativeToolPayloadSummary`:
summary, byte count, redacted, truncated; no file bodies, search lines, or
directory dumps). Identical replay is impossible by design. Two separable
gaps follow:

- Display: the current replay row ("completed; bytes=N; content=redacted;
  truncated=false") reads like debug output. The norm to emulate is the
  shape of live rendering, not its content: a collapsed tool row with tool
  name, a bounded allowlisted argument hint (currently
  `ToolRequestRecorded.argument_summary` is just "tool payload redacted"),
  outcome, and the structural facts evidence does keep (match/entry counts,
  byte counts, hunk counts), labeled honestly, e.g. "output not retained
  (evidence policy)" — no dead expand affordance.
- Model context (sharper gap): `transcript_messages()`
  (`crates/yach-backend/src/session.rs:401`) drops tool events entirely, so
  a resumed model does not know which tools ran or what they verified. Peers
  re-feed full results. Without changing evidence policy, resume context
  should include a synthesized per-turn note pairing each tool call (name
  plus bounded argument hint) with outcome/counts — the same degraded mode
  peers accept after compaction. This frames yach resume as
  "always-compacted history": defensible if the summaries carry enough
  structure to reorient the model and the UI is honest that resume is
  summary fidelity, not replay fidelity.

This interacts with the stale-evidence finding
(`docs/project/records/2026-07-14-stale-evidence-harness-research.md`): a
resumed model that cannot see which tools ran is more likely to assert
filesystem state without fresh evidence.
