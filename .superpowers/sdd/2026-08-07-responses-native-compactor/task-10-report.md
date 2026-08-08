# Task 10 — Responses native compactor final-fix report

## Scope

This fix wave is limited to the Responses compactor boundary: vendored Rig Responses streaming, yach native compaction/replay/runner and provider error surfaces, the Responses smoke command, measurement, and directly related regressions. Controller format, lint, full-suite, focused, and live-credential verification now pass through `4fdb2298`.

## Finding mapping

| Finding | Resolution | Regression / evidence |
| --- | --- | --- |
| FinalSpecReview: malformed compact window | `native_window_is_replayable` is the canonical validator. It requires a nonempty object-only window containing a compaction item. The compact endpoint maps invalid output to `InvalidOutput`; runner-side compactor results also fall back before checkpoint mutation. | `native_compactor_maps_transport_failures_to_redacted_errors` covers empty, null-item, and no-compaction-item responses. |
| FinalSpecReview: malformed resume window | Checkpoint loading applies the same canonical validator and refuses missing, malformed, wrong-provenance, or unreplayable native details before state activation. Native replay-artifact deserialization also rejects invalid windows. | Existing malformed-checkpoint replay coverage plus `replay_artifact_rejects_an_empty_window`. |
| FinalSpecReview / QS-RESP-006: terminal response absent output | Rig now reports a malformed response instead of normalizing a missing `response.output` to an empty array. | `live_stream_rejects_terminal_output_that_is_absent`. |
| FinalSpecReview: live smoke bypasses runner | The live smoke now starts `run_native_loop` over a private temporary project and persisted JSONL session, uses a capability-marked `ProviderConfig`, resumes a completed foldable turn, drives threshold compaction through the production runner, requires an atomic portable-summary/native checkpoint, then drives replay continuation. When configured, it also drives A → B → A re-selection. | Both real-key smoke variants pass: base lifecycle and optional model-switch replay. |
| FinalSpecReview: capability restoration | Replay has a distinct `CapabilityDisabled` state. Turning capability back on reloads the matching checkpoint; unsafe invalidation remains non-reloadable for that target. | Full backend suite and the 21-test Responses native-compaction focus pass. |
| FinalSpecReview: measurement revision scope | Measurement identifies the implementation range through final verified correction `4fdb2298`. | `docs/project/records/2026-08-07-responses-native-compactor-measurement.md`. |
| FinalSpecReview: empty terminal output after content | Terminal output validation rejects an empty raw terminal array when text or tools were emitted, before replay commit or tool execution. | `terminal_raw_output_requires_nonempty_and_exact_unique_tool_pairing`. |
| QS-RESP-001: typed-request trace body | Vendored Rig trace output is metadata-only and never serializes the caller-built request. | Trace source is content-omitting by construction. |
| QS-RESP-002: provider-body persistence/CLI | Yach maps Rig errors to bounded variant/status/type/code categories; raw chains and bodies are not retained. Persisted reasons and CLI rendering now use only the provider error kind. Direct HTTP smoke failures retain only categorical status/network metadata. | Full suite passes the provider-error and persisted-reason sentinel coverage. |
| QS-RESP-003: malformed window fail-open | Addressed by the shared native-window validator at compact-result and persisted-artifact boundaries. | See malformed compact/resume rows. |
| QS-RESP-004: cross-connection replay | Live replay target identity incorporates a stable digest of the selected connection and resolved endpoint provenance; persisted artifacts include matching credential-free provenance and mismatches rebuild from summary context. | Full backend suite passes same-model connection mismatch, resume, capability restoration, and A → B → A fixtures. |
| QS-RESP-005: tool/raw correlation | Before any tool execution, raw terminal `function_call` items must have unique IDs and exactly match completed typed calls in ordered ID/name/arguments/cardinality. | `terminal_raw_output_requires_nonempty_and_exact_unique_tool_pairing` covers valid, empty, mismatch, and duplicate paths. |
| QS-RESP-007: transparent Debug | Native envelopes, artifacts/outcomes, replay target/state/store, and provider request debug representations are metadata/count-only; connection identities are represented only as fingerprints. Tests avoid full opaque-envelope/outcome comparisons. | `native_envelope_serializes_raw_input_and_redacts_debug`, `native_artifact_debug_redacts_connection_and_window_content`, and `replay_target_debug_redacts_connection_identity`. |

## Publication gates

All local publication gates pass. The known residual-risk sentence and upstream-not-open statement remain unchanged.

## Post-checkpoint source correction

The integrity checkpoint initially omitted the closing delimiter for the pre-existing raw-output collector test when adding the terminal-pairing regression, and contained one surplus delimiter after the provider-error metadata helper. Both delimiters were restored/removed after inspection of every delimiter hunk in `review-89f22df..9c64de6.diff`; no validation command was run in this correction step.

## Controller lint follow-up

Controller lint identified only source-level follow-ups from this wave: nested `Some` or-patterns and a `map` simplification in bounded provider-error categorization; omitted test-module imports for existing buffered-sink, agent-tool batch/round, and provider-config fixtures; the stale replay fixture field; and formatter-owned `main.rs` layout. The source now applies all listed corrections without suppressions. Controller must rerun validation; this follow-up ran no validation command.

The next controller lint round exposed the prior runner import replacement as incomplete. Every pre-existing explicit test import was restored (`ProviderConfig`, `ResourceRoot`, and `Role`), while retaining only the new symbols required by this slice. The remaining non-cascade provider metadata diagnostic uses `map_or` directly. No suppression or validation command was used; controller rerun remains required.

The final controller lint pass found six direct cleanup defects: a panic-style fixture unwrap, four obsolete direct-smoke helpers and their stale unit test/imports, and a redundant `Vec::len` closure. The fixture now asserts success before non-panicking extraction; the obsolete manual-smoke path is removed; the metadata-only runner smoke uses `Vec::len` directly. No validation command was run in this follow-up.

Controller full-suite execution then isolated three CLI fixture-persistence expectations that still asserted free-form provider text. Their behavioral contract now asserts the outcome and bounded persisted `provider_error kind` (`provider_internal`, `malformed_stream`, or `cancelled`) and explicitly asserts the fixture free-form text is absent. The test gate must be rerun by the controller; this correction ran no validation command.

## Controller compile follow-up

Controller compilation exposed two stale fixture splices made while adding replay provenance: an ordinary static-context argument was replaced by a replay target, and a compaction fallback fixture lost its pre-existing log and pending-event setup. The source restores those original setup boundaries, retains only the target provenance fields, and fixes metadata-only Debug formatting plus the unused test import. No validation command was run in this correction step; the controller must rerun validation.

The subsequent compiler pass required only explicit sized-reference coercion for the bounded connection-fingerprint slices and restoration of the borrowed token-estimate assertion. No validation command was run in this correction step; the controller must rerun validation.

The focused backend gate then confirmed the new identity and terminal validators by exposing stale test data rather than a product defect. Persisted checkpoint fixtures now derive their artifact provenance from the same fixture provider helper; capability restoration asserts `CapabilityDisabled` before reloading a matching checkpoint; and the tool-round fixture's terminal raw output exactly repeats the typed function call. No validation command was run in this correction step; the controller must rerun the gate.

## Final controller verification

The final source correction fixed an automatic-compaction edge case exposed by the production smoke: an oversized newest `EntryAppended` followed by zero-token `StaticContextIncluded` left the cut scan after the mandatory tail and returned `None`. Regression `cut_selection_keeps_oversized_newest_entry_before_zero_token_event` failed as `None` (`artifact://3438`) before the cut fallback and passed afterward (`artifact://3440`).

The live smoke now creates a private temporary project with a low kept-tail budget and a completed foldable JSONL turn. Its workspace tests were observed red before implementation (`artifact://3447`, `artifact://3460`) and green afterward (`artifact://3451`, `artifact://3462`). This avoids manufacturing a giant current user instruction while exercising the real resume, threshold, checkpoint, and replay path.

Final controller evidence (`artifact://3468`):

```text
just fmt
exit 0

just fmt-check
exit 0

just lint
exit 0; clippy all targets/features with -D warnings

just test
exit 0; 1,063 unit tests, 0 failed; doctests 0 failed

just dev cargo test -p yach-backend responses_native_compaction
21 passed; 0 failed; 609 filtered out

just dev cargo test -p yach smoke_responses_compaction
2 passed; 0 failed; 152 filtered out
```

Real-key production-runner smoke through 1Password:

```text
YACH_RIG_OPENAI_MODEL=gpt-5.6-luna
responses_compaction_smoke=passed
stage=responses_turn
stage=native_compact
stage=portable_summary
stage=replayed_continuation
artifact_item_count=3
token_count=1233

YACH_RIG_OPENAI_SMOKE_ALT_MODEL=gpt-5.6-terra
responses_compaction_smoke=passed
stage=model_switch_replay
artifact_item_count=3
token_count=1045
```