# SDD ledger — plan: docs/project/plans/2026-10-02-codex-release-client-version.md
Spec: docs/project/specs/2026-10-02-codex-release-client-version-design.md
Kata parent: yach#ngw3
Task 1: Kata yach#scwt
Task 2: Kata yach#y9gh
Task 3: Kata yach#1phc
Task 4: Kata yach#jbxp
Task 5: Kata yach#cm65
Task 6: Kata yach#0emg
Task 7: Kata yach#7vgq

## Preflight scan
| Pair / task | Shared file or interface | Finding |
|---|---|---|
| T1→T2 | baked_codex_release_version, release_tag_version, compare_dotted_versions (pub) | T1 produces all three; T2 consumes them. Consistent. |
| T1→T2 | caller sites provider_connections.rs:255,285,1523, main.rs:4090, model_discovery_cache.rs:331 | T1 switches to baked release; T2 replaces with effective version. Consistent, two passes. |
| T2→T3 | check_latest_release/CheckMode/ReleaseCheck; refresh_models gate | T2 adds Due-check gate in refresh_models; T3 changes refresh_models signature. T3 must keep T2's gate in Normal mode. |
| T2↔T3 | catalog_refresh.rs (REMOTE_CATALOG_REFRESH_INTERVAL_MS pub(crate), write_json_to; codex_refresh_due gains mode) | Different functions, compatible. |
| T3→T4 | RefreshMode, ModelDiscoveryOutcome::Forced, forced_refresh_status, refresh_models(.., mode) | T3 updates runner call sites with Normal; T4 threads mode through pending state. Consistent. |
| T3→T5 | cached_models, refresh_models(Forced), forced_refresh_status | Consistent. |
| T1→T6 | just catalog-codex-snapshot | Workflow reads tag from pin's first field; T1 format `<tag> <sha>`. Consistent. |
| T7 | all | Verification only. |
| T1 self | tests vs impl | parse_codex_pin requires exactly 2 fields + 40-hex; test includes `rust-v0.160.0 a956835d a956835d` (3 fields) rejected. Consistent. |
| T2 self | unreadable_cache_is_absent uses uuid | uuid v4 is already a yach-cli dependency. OK. |
| T2 self | has_chatgpt_subscription uses `store.list()` | Plan says verify actual method name. OK as instructed. |
| T3 self | forced_refresh_ignores_a_fresh_discovery_cache | Plan notes the fixture may lack a cache path; fallback given. OK. |
| T4 self | parser_rejects_arguments_for_alpha_commands edit | Plan explicitly drops the /model assertion and adds app-level rejection test. Consistent with spec §4. |
| T5 self | exit code rule | "1 only when every attempted step failed and no entries" — narrower than spec ("exits non-zero if every step failed"); same meaning. OK. |

Ruling: T2/T3 refresh_models gate ordering — T3 keeps T2's Due release check at the start of Normal refreshes and uses Forced for the forced path — spec §3 requires both — if wrong, Normal refreshes stop picking up releases until launch.
Ruling: tests that touch HOME-derived paths (codex_release cache) use explicit-path helpers (load_release_cache_from) rather than mutating HOME — env mutation is process-global and races under parallel tests — if wrong, a check_latest_release end-to-end path is untested in unit tests and covered only by Task 7 live acceptance.
Task 1: dispatched T1Implementer-2 (BASE 20d8c1f9b2e02497507690151f52717ae5815210, change ynrzzpyn)
Task 1: implementer DONE, change ynrzzpyn fa688d11; review dispatched T1Reviewer-2 (package review-20d8c1f9b2e0..1f042b78211f.diff)
Task 1: complete (changes 20d8c1f9..fa688d11, review clean; pin and snapshot verified against upstream by controller)
Task 2: dispatched (BASE fa688d11, change tpntxznwskrq)
Task 2: implementer DONE_WITH_CONCERNS, change tpntxznw f5b5454e; review dispatched T2Reviewer-2 (named risk: no-cache failure not recorded vs spec 'wait out the interval')
Task 2: review round 1 — 3 Important (no-cache failure not recorded; discovery cache stamped with publication-time version; overlapping release checks share tmp file/stale overwrite). All accepted. Fix round 1/5 dispatched to T2Implementer-2 (FIX_BASE f5b5454e).
Ruling: spec beats brief on failure throttling — a failed check with no cache persists baked version/tag, no ETag, checked_at=now — spec §2/§Error handling binding — if wrong, offline users pay 5s per /model refresh.
Task 2: fix round 1 DONE, change xkxlmkml ceaf0f3b; scoped re-review dispatched to T2Reviewer-2 (package review-f5b5454e06d4..4ec365c89b5b.diff)
Task 2: re-review round 1 — 1,3 addressed; 2 partial (discoverer re-reads version); new: tests mutate global effective version. Fix round 2/5 dispatched (FIX_BASE ceaf0f3b).
Task 2: fix round 2 DONE, change ruprvonq 9bb9483f; scoped re-review dispatched (package review-ceaf0f3bfabd..c223576baed1.diff)
Task 2: re-review round 2 — both addressed; new regression: EFFECTIVE published outside CHECK_LOCK. Fix round 3/5 dispatched (FIX_BASE 9bb9483f).
Task 2: fix round 3 DONE, change qvvsrzlt ac0337e1; re-review dispatched
Task 2: complete (fix rounds squashed into tpntxznw 15209a06; review clean after 3 rounds). Note: implementer saw one perf-bench flake in just test, passed on rerun.
Task 3: dispatched T3Implementer-2 (BASE 15209a06, change qmsnxtzo)
Task 3: implementer DONE, change qmsnxtzo 3dc389ea; review dispatched
Task 3: review round 1 — production approved; 1 must-fix test isolation (global catalog mutex in policy test). Fix round 1/5 dispatched (FIX_BASE 3dc389ea).
Task 3: fix round 1 DONE, change pqsyvokm fbd7d14d; re-review dispatched
Task 3: complete (fix squashed into qmsnxtzo ef13aac1; review clean after 1 round)
Task 4: dispatched (BASE ef13aac1)
Task 4: implementer DONE, change svkpqmrl; controller added protocol doc line (implementer missed the client-event list); review dispatched
Task 4: complete (svkpqmrl 6f2a87df, review clean; RED by mutation noted)
Task 5: dispatched (BASE 6f2a87df)
Task 5: implementer DONE, change qxvultnl; review dispatched
Task 6: dispatched (BASE 1838f81c, change lzmxlkmx)
Task 5: complete (qxvultnl 1838f81c, review clean)
Task 6: implementer DONE, change lzmxlkmx cf345eb5 (deviations: checkout@v4 to match ci.yml; --state all); review dispatched
Task 6: complete (lzmxlkmx cf345eb5, review clean)
Task 7: full checks pass; live acceptance pass (evidence .sjujperpowers/sdd/2026-10-02-codex-release-client-version/task-7-evidence.md); temp pin restored, rebuilt.
Final review round 1: Needs fixes — 4 Important, all accepted: (1) justfile local override must require CODEX_MODELS_TAG (spec 90-92); (2) forced status must carry discovery failures (spec 190-194); (3) CLI exit code from step outcomes, not fallback rows (spec 165-166; ledger preflight equivalence claim was wrong); (4) committed-pin tests must not hardcode 0.160.0 (breaks data-only re-pin PRs, spec §5). Fix dispatched (FIX_BASE cf345eb5).
Final fix: DONE, change yprwqypm 0d153155; re-review dispatched
Finish: final review Ready to merge; full checks green on 0d153155
