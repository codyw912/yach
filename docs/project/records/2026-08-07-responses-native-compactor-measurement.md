# Responses Provider-Native Compactor Measurement

Date: 2026-08-07

## Scope and revisions

Accepted design: `docs/superpowers/specs/2026-08-06-responses-native-compactor-design.md`.
Accepted implementation plan revision: `65c83566` (`docs: plan Responses provider-native compactor implementation`).
The vendored Rig Responses passthrough prerequisite is `bdf7db08`; yach integration revisions span `4a3612de` through final verified correction `4fdb2298` (`fix: make Responses live smoke exercise threshold replay`). Task 8 live-smoke evidence was recorded at `c58de134`; Task 10 review and final corrections are included in this measurement scope. The slice adds capability-gated OpenAI `/responses/compact`, a mandatory portable-summary checkpoint, exact ordered native-window replay, and summary fallback.

The 125-cell provider matrix remains a pre-release requirement, not a per-slice measurement. Its sweep-driver credential re-resolution fix remains the next pre-release prerequisite.

## Recorded fixture evidence

Task 8 recorded these focused results before this Task 9 measurement pass:

```text
just dev cargo test -p yach-backend responses_native_compaction
21 passed; 0 failed; 604 filtered out

just dev cargo test -p yach smoke_responses_compaction
3 passed; 0 failed; 150 filtered out
```

The loopback fixture runs the production Rig adapter and `run_native_loop` with a JSONL session store. Its recorded scenarios cover:

- native `/responses/compact` request construction, raw-window retention, and a nonempty portable summary before checkpoint commit;
- HTTP, malformed JSON, malformed output, timeout, and empty-summary failures, each retaining the summary-only/no-checkpoint fallback as applicable;
- automatic threshold compaction, manual focus isolation, second-window kept-tail de-duplication, and A -> B -> A selection/replay behavior;
- exact continuation of function-call output, cancellation/restart recovery, and retry-prefix replay without duplicated text, raw output, lifecycle events, or tool execution;
- malformed persisted native artifacts declining to ordinary Responses context rather than being replayed.

The recorded selection observations are capability-gated: `Some(true)` for the active OpenAI Responses model permits `auto` or `openai-responses` to try native compaction; unsupported or unknown capability uses summary only. A forced `openai-responses` selection reports that native compaction was unavailable and uses the summary path; the fixture verifies that no compact-endpoint call occurs in that fallback case.

## Recorded live smoke

Task 8 first recorded direct compact/summary evidence. Final Task 10 verification reran the smoke through the production `run_native_loop` over a private temporary project and a resumed, completed foldable JSONL turn. Through 1Password credential resolution, with `YACH_RIG_OPENAI_MODEL=gpt-5.6-luna`, it reported pass for `responses_turn`, `native_compact`, `portable_summary`, and `replayed_continuation`, with three artifact items and 1,233 tokens. With `YACH_RIG_OPENAI_SMOKE_ALT_MODEL=gpt-5.6-terra`, it additionally reported `model_switch_replay`, with three artifact items and 1,045 tokens.

The smoke renderer exposes only pass/fail state, model ID, stage labels, artifact count, token count, and a missing-prerequisite label. This record does not reproduce credential material, request bodies, or opaque encrypted provider content.

## Final controller verification

Final verification covers source through `4fdb2298`:

```text
just fmt
exit 0

just fmt-check
exit 0

just lint
exit 0; clippy all targets/features with -D warnings

just test
exit 0; 1,063 unit tests total, 0 failed; doctests 0 failed

just dev cargo test -p yach-backend responses_native_compaction
21 passed; 0 failed; 609 filtered out

just dev cargo test -p yach smoke_responses_compaction
2 passed; 0 failed; 152 filtered out
```

## Residual risk and upstream status

Known residual risk: post-checkpoint raw suffixes are lossy across restart until the next native compaction.

The upstream Rig PR has not yet been opened. No upstream URL exists yet.
