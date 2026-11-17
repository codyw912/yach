# SDD ledger — plan: docs/project/plans/2026-09-30-session-diagnostics.md

Spec: docs/project/specs/2026-09-30-session-diagnostics-design.md (owner-approved 2026-09-30)
Providers: roadmap plane (YACH-15, In Progress), execution kata, completion pull_request
Kata parent: yach#2126
Task 1: Kata yach#jm6d
Task 2: Kata yach#zh3r
Task 3: Kata yach#b5xx
Task 4: Kata yach#r0n0
Task 5: Kata yach#hm6g
Task 6: Kata yach#a979

## Pre-flight scan

| Pair / task | Produces vs consumes | Finding |
|---|---|---|
| T1 → T2 | T1 `unix_ms_now()`; T2 `ToolTimer` uses it | consistent |
| T1 → T3 | T1 `SessionEvent::Unknown` + exhaustive arms; T3 adds `ProviderAttemptFinished` arms at the same matches | consistent; T3 must add arms at every match T1 touched |
| T1 → T5 | T1 `load_stamped_from_file`/`StampedSessionEvent`; T5 inspector consumes | consistent |
| T2 → T5 | T2 `ToolExecutionFinished.started_at_ms/duration_ms`; T5 renders | consistent |
| T3 → T4 | T3 `AttemptLabel{purpose}`, `AttemptDiagnostics`, requester hooks, native attempt site; T4 `AttemptRecorder`, `capture_path(label)`, `CompactionPreparation.recorder` read by T3 native event | T3 native event reads `recorder.diagnostics()` which T4 adds. Ruling below |
| T3 → T5 | T3 `ProviderAttemptSummary` (pub, flat) + `ProviderAttemptOutcome::Partial`; T5 renders | consistent |
| T4 → T6 | T4 `YACH_CAPTURE_REQUESTS`; T6 documents + smoke | consistent |
| T5 → T6 | T5 commands; T6 README + smoke | consistent |
| T1 self | tests use `test_jsonl_path`; encoder shared by both writers | consistent |
| T2 self | bash cancel test requires cancelling a running command; plan names the select arm | consistent |
| T3 self | tests 1–11 + compaction cases vs impl steps; `Partial` outcome case 5a | consistent |
| T4 self | error-path code block uses `from_chunks`; test 3 open-connection oversize | consistent |
| T5 self | CLI test strips YACH_*; case 7 ordering | consistent |
| T6 self | smoke uses secretspec `gateway` scope added in same task | consistent |

Ruling: T3 records native-compaction attempts without request ID/capture; T4 then adds `CompactionPreparation.recorder` and fills those fields — keeps each task self-contained and testable — cost if wrong: T4 touches the T3 native call site again (small).
Ruling: rig error path reads chunks and calls public `BoundedErrorBody::from_chunks`, no vendor patch — spec says the vendored patch does not change — cost if wrong: a Rig upgrade that changes the sentinel diverges silently; test 3 compares against Rig's reqwest path to catch it.

## Progress
Task 1: dispatched (BASE f5293b4241b9b1f994da29693d2f4130f44e86f8, change kvkxrtqu, agent T1Implementer)
Task 1: implemented kvkxrtqu 7fb27ac8 (DONE, yach-backend 893 passed); review dispatched (T1Reviewer, package review-f5293b4241b9..7fb27ac8954a.diff)
Task 1: complete (changes f5293b42..7fb27ac8, review clean: Approved, 0 findings)
Task 2: dispatched (BASE 7fb27ac8954a, change zrmruvrk, agent T2Implementer)
Task 2: implemented zrmruvrk c802f87b (DONE, yach-backend 895 passed); review dispatched (T2Reviewer, package review-7fb27ac8954a..c802f87bb769.diff)
Task 2: fix round 1/5 (2 Important open: failed edit apply loses timing; extension EditProposal drops invocation timing)
Ruling: extension edit proposal timing = extension start; duration = extension exec + apply, never review wait; preview-fail/rejected keep extension timing — brief requires timing on every post-dispatch finish — cost if wrong: one duration semantics change in agent_edit_tools.
Task 2: fix round 1 implemented zrmruvrk 8952ec46 (898 passed); re-review dispatched (T2ReReviewer, package review-c802f87bb769..8952ec468239.diff)
Task 2: complete (changes 7fb27ac8..8952ec46, review clean after fix round 1: all findings addressed)
Task 3: dispatched (BASE 8952ec468239, change olwoqrmv, agent T3Implementer)
Task 3: implemented olwoqrmv ccafdfd8 (DONE_WITH_CONCERNS: 3 pre-existing tests adjusted for attempt events; RED transcript lost to compaction; 916 passed); review dispatched (T3Reviewer, package review-8952ec468239..ccafdfd8d52c.diff)
Task 3: fix round 1/5 (3 Important open: Completed-then-error Partial reported Succeeded; stream payload clones for settlement; vacuous privacy test). Existing-test adaptations verified sound by reviewer.
Ruling: result used despite trailing error → Partial with error fields — spec: no succeeded row carries an error — cost if wrong: outcome label only.
Task 3: fix round 1 implemented olwoqrmv 4dcb0972 (917 passed, RED/GREEN for Partial case); re-review dispatched (T3ReReviewer, package review-ccafdfd8d52c..4dcb09725565.diff)
Task 3: re-review attempt 1 failed (provider stream error, not a verdict); re-dispatched T3ReReviewer2
Task 3: complete (changes 8952ec46..4dcb0972, review clean after fix round 1: all findings addressed)
Task 4: dispatched (BASE 4dcb09725565, change noqltosl, agent T4Implementer)
Task 4: implemented noqltosl c808fd3c (DONE; concern: capture tests rely on RUST_TEST_THREADS=1 — CI does not set it; 930+235 passed); review dispatched (T4Reviewer, package review-4dcb09725565..c808fd3cf9f4.diff)
Task 4: fix round 1/5 (4 Important open: global disable flag shared across parallel tests; env mutation in test; native first_event_ms never set; chmod of existing capture root)
Ruling: capture-disable state injectable, one process-wide instance in production — CI runs tests in parallel — cost if wrong: small plumbing.
Ruling: native compaction first event = response headers received — single non-streamed response — cost if wrong: TTFB semantic differs from stream first event; documented in field meaning.
Ruling: never chmod an existing capture root; 0700 on created dirs and session dir — /tmp-style roots must keep sticky bit — cost if wrong: none.
Task 4: fix round 1 implemented noqltosl 37a4222a (931+235 passed; recording_http 11 passed at 8 threads); re-review dispatched (T4ReReviewer, package review-c808fd3cf9f4..37a4222ac394.diff)
Task 4: re-review output cut off by provider stream error (whitespace tool-args); asked T4ReReviewer to re-emit
Task 4: fix round 2/5 (findings 3,4 addressed; 1,2 partial: validator calls production warn_once; capture tests still on production policy)
Task 4: fix round 2 implemented noqltosl 97c770a6 (931 passed; recording_http 11 at 8 threads). Controller verified delta: validator returns CaptureDir enum, side-effect free; env reader warns; capture-path tests use CapturePolicy::isolated(); remaining AttemptRecorder::new(None) test uses never touch policy (capture_body returns before write with no path).
Task 4: complete (changes 4dcb0972..97c770a6, review clean after fix round 2)
Task 5: dispatched (BASE 97c770a6, change mvkwopvq, agent T5Implementer)
Task 5: implemented mvkwopvq 54f1fd28 (DONE; sessions 7/7, yach all passed); review dispatched (T5Reviewer, package review-97c770a63d81..54f1fd28f8eb.diff)
Task 5: fix round 1/5 (3 Important: turn origin ignores non-rendered event timings; untimed attempt lacks at_ms fallback; mtime truncated before sort. 2 Minor included: duplicated comparator, deep clones in reorder)
Ruling: turn origin = earliest known timing over all events in the turn — spec "turn's earliest known timing" — cost if wrong: offsets shift by user-entry lead time.
Task 5: fix round 1 implemented mvkwopvq 580bfbe8 (sessions 10/10). Controller verified delta: origin via note_timing over all events; attempt at_ms fallback; SystemTime sort in one shared comparator; drain/sort/extend without clones.
Task 5: complete (changes 97c770a6..580bfbe8, review clean after fix round 1)
Task 6: controller-owned (README, secretspec edited in uvpvxzvq); next: full gate, gateway smoke, evidence record
Task 6: complete uvpvxzvq fa5e4cbf (just lint clean; just test exit 0; parallel backend+cli pass; gateway smoke on openai-codex/gpt-6.1-sol met all expectations via --model; record committed)
Final review: dispatched (FinalReviewer, package review-f5293b4241b9..fa5e4cbf54a1.diff)
Final review: Not ready — 2 Important (interrupted extension edit-proposal review loses timing; sessions list drops load warnings), 1 Minor (retry_index missing in text rows), README over-claims capture scope/credential freedom. All accepted; fix dispatched in zplsrqwt (FinalFixer).
Final review: fixes landed zplsrqwt e77bcfb9 (controller verified: interrupted-review helper writes Cancelled finish with prior_timing, fallback finds it so no duplicate; list warnings; retry label; README scope/credential wording). Final gate: just lint 0, just test 0 (backend 944). Ready for finishing.
Finish: record command fixed and re-run; fmt-check clean; record list output refreshed with final code; fix commit described. Pushed session-diagnostics (commits re-signed on push: head 796a847a). PR https://github.com/codyw912/yach/pull/283. Kata 2126 + 6 children closed with PR. Plane YACH-15 roll-up commented; stays In Progress until merge. Recovery workspace retained.

Merged: PR #283 squash-merged to main as 2ff9457a (2026-10-01). Plane YACH-15 moved to Done with merge comment. Workspace retained for reference only.
