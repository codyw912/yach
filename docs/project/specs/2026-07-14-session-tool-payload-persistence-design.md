# Session Tool Payload Persistence Design

Date: 2026-07-14

Status: implemented (PR #129 persistence and resume display, PR #130
provider transcript tool activity and benchmark refresh). Remaining
verification: a live dogfood pass of checkpoint steps 4 and 6/7 confirming
resumed transcripts show tool output as live runs did.

## Context

The accepted provider read/search content design
(`docs/project/specs/2026-05-18-provider-read-search-content-design.md`)
declared "Provider Results Are Bounded Context, Not Session Evidence":
session logs record redacted summaries (byte counts, match counts,
truncation flags) and never persist file bodies, search lines, directory
dumps, or raw queries.

Two 2026-07-14 findings changed the picture:

- The live dogfood run showed resumed transcripts render tool activity as
  debug-style redacted summaries, and resumed provider context drops tool
  events entirely (`transcript_messages()`,
  `crates/yach-backend/src/session.rs:401`;
  `native_provider_messages_from_log`,
  `crates/yach-backend/src/native_runner.rs:1196`). A resumed model does not
  know which tools ran or what they verified, which compounds the
  stale-evidence behavior recorded in the dogfood checkpoint.
- Comparison research
  (`docs/project/records/2026-07-14-resume-transcript-research.md`) shows
  Codex CLI, Claude Code, opencode, and Pi all persist tool arguments and
  results verbatim (after execution-time caps) and reuse the live rendering
  path on resume. Persisted content equals model-visible content; resume is
  lossless.

Project owner decision (2026-07-14): the divergence was not intentional and
resume should not be always-compacted history. The evidence policy changes so
that the session log is the full model-visible transcript.

## Goal

- Persist, per provider-visible tool call: the bounded tool arguments and the
  exact bounded provider-visible result content, alongside the existing
  structural evidence.
- Resumed TUI transcripts render tool rows through the same shaping used
  live, so a resumed session looks like the live run did.
- Resumed provider context includes prior tool activity so the model knows
  what ran, what it returned, and what was verified.
- Old session logs (redacted-only) continue to load and render with the
  current summary fallback.

## Non-Goals

- Raising provider result bounds or removing execution-time caps. Persisted
  content is exactly what the provider saw, never more.
- Persisting unbounded raw command output, streaming deltas, or in-progress
  tool state.
- Changing tool review, permission, or edit-transaction flow.
- File-change notifications or compaction design.
- Changing session log location, fsync policy, or JSONL format version
  mechanics beyond additive fields.
- Secrets scanning of persisted content (session logs are local, gitignored
  project state; the content persisted is already shown on screen live).

## Design Principles

### Persisted Equals Model-Visible

Every peer harness applies caps before the model sees output and persists the
capped payload. Yach already bounds provider-visible results
(`read_text_file`, `search_project`, `list_project_paths`, edit tool
results). The session log stores those bounded payloads verbatim. No second
truncation layer on the persistence path.

### The Session Log Stays Local And Honest

Session logs live in `.yach/native-sessions/` (gitignored). Persisting
content the user already saw live does not broaden the exposure surface, but
logs now contain project file content; that is the accepted tradeoff of this
policy change and should be stated in user-facing docs when session docs
exist. Structural evidence (hashes, counts, outcomes, permission decisions)
remains authoritative for local effects; content fields are additive.

### Additive Schema, Old Logs Load

New optional fields, never repurposed ones. Absent fields mean an old
redacted-era log; projection falls back to the current summary rendering.
JSONL forward/backward compatibility tests cover both directions.

## Schema Changes

`NativeSessionEvent::ToolRequestRecorded` gains:

- `argument_content: Option<String>` — the bounded, validated tool argument
  JSON as sent to execution. The existing `argument_summary` stays.

`NativeSessionEvent::ToolExecutionFinished` gains:

- `result_content: Option<String>` — the exact bounded provider-visible
  result payload. The existing `result_summary` stays and continues to carry
  byte counts and truncation flags.

Edit tools already return bounded structural results to the provider
(operation summaries, hunk counts); those provider-visible results persist
through the same `result_content` field. Local edit preview diffs shown in
review UI are not session events today and remain out of scope.

## Projection Changes

### TUI Hydration (Display)

`send_native_session_messages_from_log` includes, per tool call, the tool
name, a bounded argument hint derived from `argument_content` (path, pattern,
or query — same allowlist shape the live `ToolCallStarted` preview uses), and
the `result_content`. The UI hydration path renders these through the same
visible-progress shaping used live
(`native_provider_visible_list_progress` and siblings live in the backend;
the shaping seam should move or be shared so hydration and live rendering
produce identical rows). Old logs without content fields render the existing
summary row with an explicit "output not retained" note.

### Provider Resume Context

`native_provider_messages_from_log` includes tool activity for completed
turns. `ProviderMessage` is role+text, and the rig adapter seam does not
replay provider-native tool_use/tool_result blocks across sessions, so
historical tool calls resume as tool-role transcript messages: one message
per tool call pairing the tool name and bounded argument hint with the
persisted result content. This keeps the adapter seam unchanged while giving
the model the same information it had live. Modeling provider-native tool
blocks in resume context is a possible later refinement and is out of scope.

Local-edit and agent-edit evidence remain excluded from provider context as
today (`native_provider_messages_ignore_*` tests stay green).

## Approach Options Considered

### Option A: Synthesized Summary Notes Only

Keep redacted evidence; inject per-turn tool-activity notes into resume
context. Rejected: owner decision is that resume must be replay-fidelity,
not compacted history. Summaries also leave the display gap.

### Option B: Persist Bounded Content In The Session Log (Recommended)

Additive content fields as above. Matches peer architecture (Claude Code,
Codex, opencode, Pi all store model-visible payloads inline), keeps one
canonical log, and reuses existing bounds.

### Option C: Side-Car Content Store

Keep the log slim; spill content to side files keyed by tool request ID
(opencode/Pi do this only for overflow beyond caps). More moving parts,
partial-file failure modes, and no current size pressure: bounded results
are small. Revisit only if session logs grow enough to hurt append/load
benchmarks.

## Bounds And Size

Persisted content is already bounded by provider result caps. Session append
remains fsync-per-event; the append/load/projection benchmarks
(`yach-bench`) must be re-run after implementation and compared against the
current baseline. If load-time regression appears on large sessions, Option C
is the escape hatch.

## Verification

- JSONL compatibility: old redacted logs load, render, and resume without
  content; new logs round-trip content fields.
- Hydration parity: a session exercising read/search/list/edit tools renders
  the same tool rows after `/resume` as during the live run (UI test at the
  transcript-entry level).
- Resume context: provider request messages for a resumed session include
  tool-role messages with persisted result content; edit evidence stays
  excluded.
- Benchmarks: session append/load/projection deltas recorded in
  `docs/benchmarks/`.
- Live dogfood: rerun checkpoint steps 4 and 6/7 and confirm resumed
  transcript shows listed paths and search matches as live did.
