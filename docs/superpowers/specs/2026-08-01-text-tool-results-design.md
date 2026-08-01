# Text Tool Results

**Date:** 2026-08-01
**Status:** In review
**Prior work:** native tool-call messages
(`2026-07-28-native-tool-call-messages-design.md`, measured 82/100 ->
95/100), tool-result payload slim (#210, measured 125/125). Cohort
evidence and owner decisions: board entry "Return tool results as
text" (weighted reading 2026-07-31).

## Problem

Every built-in tool returns its result as an undeclared JSON object
serialized into the tool-result block. The weighted cohort — Codex,
nanocodex, pi, opencode, plus omp where it deviates from pi — is
unanimous: nobody sends the model a JSON envelope for a read, and on
shell the strongest precedent is a deletion (Codex PR 22706 removed
almost exactly yach's `bash` shape because "responses are already
plain text for model consumption"). The cost is measured, not
theoretical: before the payload slim, OpenAI-shape models wrote the
whole envelope into answer files 3 of 5 times on
`compaction-continuation`; the slim removed the outer envelope, but
`read_text_file` still nests file contents inside
`{"outcome":"read","path":...,"text":...}`, and every other tool
ships its own ad-hoc object. Undeclared structure is the defect;
none of these tools declares an output schema, and the only consumer
of these strings is a model.

## Owner decisions (from the board and this design's review)

1. **Scope: all seven built-ins.** One format family, no per-tool
   exceptions — read, bash, search, list, path info, edit statuses,
   and the synthesized denied/cancelled results.
2. **Reads are byte-exact.** pi is the model: no gutter, no header,
   no structural read summarization. yach's `edit_text_file`
   addresses by exact `find`/`replace` text, so anything added to
   read output would corrupt the very thing the editor matches on.
   omp's gutter+hashline shapes are a coupled pair yach is not
   adopting.
3. **Metadata flattens to bracketed notice lines, exception-only.**
   The pi/omp family, not Codex's always-labeled envelope. Clean
   results are pure content; `[…]` lines appear only when something
   needs saying (truncation, nonzero exit, empty output, no
   matches). This is the only shape compatible with byte-exact reads,
   so it is also the uniform rule.
4. **Rendering lives per tool, with a shared notice helper.** Each
   tool renders text where it builds JSON today; one small helper
   owns the notice vocabulary so the family stays consistent and
   greppable. No typed-result layer, no central renderer — seven
   tools whose outputs barely overlap do not justify one.

## Result shapes

### read_text_file

The file contents, byte-exact, nothing else. One exception:
`[empty file]` for a zero-byte file, because an empty tool-result
string is ambiguous (blank file or missing result?) and some
provider shapes handle empty content poorly. `path` and `byte_count`
drop — the model supplied the path; the count said nothing
actionable. Oversized reads already fail rather than truncate, so no
read truncation notice exists.

### bash

Captured stdout+stderr as-is, then notices only as needed:

- `[exit code N]` — only when nonzero.
- `[no output; exit code N]` — when the capture is empty (any exit).
- `[truncated: kept first A and last B of C bytes]` — when the
  bounded head+tail capture clipped.

`tool_request_id`, `approved_by`, `duration_ms`, and
`output_bytes_total` drop: the native block id binds the result to
its call, and approval/timing are session-log facts.

### search_project

grep format, one match per line:

```
src/runner.rs:3851: "output": outcome.output,
```

Lines the per-line bound clipped end with `…`. Trailing notices only
when exceptional: `[no matches; N files searched]`,
`[truncated: match limit reached]`,
`[some paths excluded by policy]`.

### list_project_paths

One entry per line; directories get a trailing `/`, files their
size:

```
src/
Cargo.toml  1534 bytes
```

Same truncation/exclusion notices as search.

### project_path_info

One prose line: `src/main.rs: file, 1534 bytes`. The hardcoded
`provider_visibility: "never"` field carries no information and
drops.

### edit_text_file

- Applied: `[applied]`, plus `[diff summary truncated]` when set.
- Review outcomes: `[rejected by review]`, `[denied: <reason>]`.
- Failures: the error rule below. `preview_id` and `transaction_id`
  are session machinery the model cannot use — dropped.

### Errors, denied, cancelled (all tools)

A bracketed verdict line, then the existing guidance prose
unchanged — those strings were written for cheap models and stay:

```
[error: timeout]
The command exceeded its timeout and was killed. Retry with a larger
timeout argument, or run a narrower command.
```

Denied/cancelled calls carry `[denied: <reason>]` /
`[cancelled: <reason>]` alone. This line is the error contract on
every wire: rig 0.41 hardcodes `is_error: None` on the Anthropic
tool-result (the portable `ToolResult` has no error flag), and
OpenAI's shape has no error slot at all, so structural error
signaling is unavailable through rig on every path. Recorded as an
upstream rig gap (same family as `max_completion_tokens`); the text
carries the signal everywhere regardless, which is what the cohort
finding demands.

## Consumers of the payload string

The design keeps the single-representation property: the string the
model sees is the string the session stores and the UI shows.

- **Session log** — stores the text the model actually saw; replay
  fidelity holds by construction. Old sessions holding JSON payloads
  are history; nothing parses them back.
- **UI** — tool rows render text excerpts instead of JSON excerpts.
  The UI's affordances come from event metadata, not payload
  parsing; implementation audits that assumption and fixes any spot
  found parsing payload JSON.
- **Compaction** — treats results as opaque strings; unaffected. The
  slice-2 masking pre-pass masks whole results regardless of shape.
- **Tool descriptions** — audited against the new shapes so the
  model is not told "returns JSON" while receiving text.

## Eval impact

Verifiers assert on file state and outcome-document fields, never
response prose, so the portfolio survives intact:

- `tool-result-dependence` stays valid unchanged: the token lives in
  a file, the read now returns it byte-exact, and the verifier still
  asserts `answer.txt` holds exactly the token. The task gets harder
  to fail for the right reason — no envelope remains to mistake for
  the answer.
- The board's note that most harnesses would fail this task as
  written (gutter-prefixed text in answers) belongs to the
  cross-harness comparison track and moves there; it is not a change
  to yach's own portfolio now.
- No new eval: the sweep's tasks already exercise every converted
  tool on every provider shape.

## Validation

1. Unit tests rewritten alongside each renderer (payload-shape
   assertions exist today and change with the shapes); workspace
   clippy strict; full suite green.
2. `just runtime-image`, then the gate: 7/7 + driver checks
   expected.
3. The 125-cell sweep against the 2026-08-01 rig-0.41 baseline
   (99/100 + 24/25 patch; both misses nemotron noise). Regression
   check at ceiling. One directional prediction: the envelope-echo
   class stays at zero — any response reproducing tool-result
   structure is a finding.

## Risks

- **Content that looks like notices.** A file ending in
  `[exit code 1]` is indistinguishable from a notice line. Accepted:
  notices are presentation, not a parsing contract; no consumer
  parses them back.
- **A model somewhere keyed on the JSON.** The sweep is the
  detector; every observed failure so far pointed the opposite
  direction (models tripping over structure, never leaning on it).
- **Empty-content edges.** Providers differ on empty tool-result
  strings; the `[empty file]` / `[no output; …]` notices mean yach
  never sends one.

## Non-goals

- No line gutter, no read summarization, no output schemas.
- No `is_error` wiring (unavailable through rig 0.41; upstream gap).
- No extension tool-result contract changes — the omp-inspired
  `useless`/pinning contract additions are their own slated design
  pass.
- No result-transform hook (omp shell-minimizer analog); separate
  slated item.

## Slice

One slice: all seven builders converted together with their tests,
measured once. The change is mechanical and uniform; splitting it
would ship two format families into the same transcript between
slices, which is the state this design exists to remove.
