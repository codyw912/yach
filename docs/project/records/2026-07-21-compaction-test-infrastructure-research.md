# How The Cohort Tests Compaction

Date: 2026-07-21

Survey of compaction test infrastructure in Pi, opencode, and Codex CLI,
to decide what structured testing yach's compaction needs beyond the
synthetic loop tests shipped with slices 1 (#149–#151).

## Findings

- **Pi — real-session fixtures + gated live tests.** Commits captured
  real sessions as fixtures (`test/fixtures/large-session.jsonl`, ~1MB,
  100+ messages; `before-compaction.jsonl`, 2.4MB) and runs deterministic
  logic (cut-point selection, context rebuild, token estimation) against
  them. Live-LLM tests are gated on an API-key env
  (`describe.skipIf(!API_KEY)`) and run the real summarizer over the
  large fixture, printing the summary for human inspection and asserting
  the reloaded session is valid. Synthetic entry builders with
  predictable ids cover pure unit tests.
- **opencode — scripted stub-LLM queue.** History built by builders; a
  queue of scripted stream responses plays the LLM, with request capture
  so tests assert what the summarizer was asked ("anchors repeated
  compactions with the previous summary"). Keeps a permanent regression
  test named for a known bug ("BUG: no headroom when limit.input is
  set…"). Yach's `FakeProviderRequester` loop tests already occupy this
  niche.
- **Codex — golden history-shape snapshots.** Insta snapshot files of
  the numbered message-history layout before/after compaction
  (`mid_turn_compaction_shapes.snap`, pre-turn failure, remote
  variants), each line like `03:message/user:<COMPACTION_SUMMARY>`.

## Adopted for yach (2026-07-21)

1. **Shape assertions** (Codex's value, no snapshot dependency):
   `native_provider_message_shapes` renders provider context as
   `role:prefix` lines, asserted as plain expected strings in the
   compaction assembly test.
2. **`yach smoke-compaction <session.jsonl>`** (Pi's gated live test, in
   yach's smoke-command idiom): loads a session log, selects the cut,
   runs the real summary pass via provider env, and prints the summary
   plus accounting (estimated tokens before/after, fold size, anchoring)
   for human judgment. This is the tool for judging continuation quality
   and iterating on the summary prompt.
3. **Captured-session fixture** (pending): after a real long dogfood
   session exists, sanitize and commit its JSONL as a test fixture and
   run cut selection / context rebuild / serialization / estimation over
   it — real event interleavings (edit traces, permission decisions,
   multi-round tool loops) that synthetic seeded logs do not produce.
