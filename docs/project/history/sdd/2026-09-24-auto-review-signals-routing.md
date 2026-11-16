# SDD ledger — plan: docs/project/plans/2026-09-24-auto-review-signals-routing.md

Spec: docs/project/specs/2026-09-24-auto-review-signals-routing-design.md
Providers: roadmap plane (Yach, outcome YACH-11 — Plane down 2026-09-24, backfill), execution kata (yach), completion pull_request
Kata parent: yach#9jsv
Task 1: Kata yach#v4e8
Task 2: Kata yach#f70z
Task 3: Kata yach#41s6
Task 4: Kata yach#4e1t
Task 5: Kata yach#jbby
Task 6: Kata yach#8jnz
Task 7: Kata yach#44n9
Task 8: Kata yach#2abs
Task 9: Kata yach#m2xq
Task 10: Kata yach#gzn5
Trunk: main@origin. Plan commit xpwrpryy (29226602).

## Preflight scan

| Pair / task | Shared surface | Producer → consumer | Finding |
| --- | --- | --- | --- |
| T1 self | edit_access.rs tests | tests use JsonlSessionStore + `context()` gains 2 fields | consistent |
| T1 ↔ T3 | runner.rs AgentEditToolContext.authorization_revision | T1 reads coordinator.snapshot(); T3 adds bump | consistent; T1 value is 0 until T3 bumps |
| T1 ↔ T6 | runner.rs edit auto-review arm | T1 rebinds generation before review; T6 edits Hold fall-through | disjoint hunks |
| T2 ↔ T3 | EvidenceItem.truncated | T2 renames; T3 builds user items with truncated:false | consistent |
| T2 ↔ T8 | evals/auto-review JSON | T2 rewrites bounded→truncated in old corpus; T8 migrates/deletes corpus | T2 corpus edit is short-lived but needed to keep eval tests green between tasks |
| T3 ↔ T5 | trusted user items (source "user") | T3 produces; T5 intent-presence reads source=="user" | consistent |
| T3 self | review_action arity (+omissions) | all callers updated in T3 incl. perf workload & eval_review.rs | plan's T3 file list omits crates/yach-bench/src (perf workload + eval_review call review_action) — see Ruling 1 |
| T4 ↔ T5 | route_assessment | T4 interim body; T5 replaces | consistent |
| T4 ↔ T7 | signal ids | T4 ReviewSignal::ALL order; T7 SIGNAL_IDS same order | consistent |
| T5 ↔ T6 | HoldReason::RestrictionApplies{restriction} (non-Copy) | T5 minimal match fixes; T6 dispositions | consistent |
| T5 self | ReviewAssessment derives Deserialize; tests construct via serde_json::from_value | `validate_against` is the only constructor otherwise | consistent |
| T6 ↔ T8 | shell_disposition_* functions | T6 private fns; T8 E1 needs them via bench feature | T8 must widen visibility to pub + cfg(feature="bench") re-export (plan says so) |
| T8 ↔ T9 | EvalCase schema | T8 defines; T9 consumes EvalExpected::Signals | consistent |
| T9 ↔ T10 | live runs | T10 runs e2/e3/e4 | consistent |
| T10 self | `just fmt`/`just lint`/`just test` | final | consistent |

Ruling 1: Task 3 commits `crates/yach-bench/src` as well — `review_action` gains a parameter and bench callers (eval_review.rs run_case/run_jev, perf workloads/review.rs) must compile — the plan's commit fileset omits it — costs nothing if wrong (fileset superset).
Ruling 2: Task 2 rewrites the old corpus JSON even though Task 8 replaces it — keeps `eval_review` tests green at every task boundary — costs one throwaway edit.

Baseline: `just dev cargo test -p yach-backend -p yach-bench -p yach-jev-reviewer` → 847 + 76 (1 ignored) + 11 passed, 0 failed.

Task 1: started — change ywznmtxm, BASE 2922660206546496fd769947e57e01916998bc24
Task 1: review 1 — 1 Important (rebind placed after review_action; must precede). Fix round 1 dispatched.
Task 1: fix round 1/5 (1 addressed, 0 open — rebind moved before review_action; changes 2425cdd0..fa409275, squashed into ywznmtxm)
Task 1: complete (changes 29226602..0b2c451e, review clean)
Task 2: started — change tksrlwnwxvmr, BASE 0b2c451e21215042850ca70157418ec03cfa4b73
Task 2: minor (deferred): extension.rs:351 ReviewAssess doc comment + extension.rs:4569/4591 fixtures still say yach.review-request.v1
Task 2: complete (changes 0b2c451e..c370cab7, review clean) — change tksrlwnwxvmr
Task 3: started — change lrtmntmuxurk, BASE c370cab737a958afc1ca4643e351684393c319b3
Task 3: Ruling: plan test a_large_message_does_not_block_smaller_older_ones used 15 KiB (fits the 16 KiB budget with 'run them'), so it did not exercise continue-past-oversized; implementer raised it to 16 KiB — plan defect, test now tests what it claims — costs nothing if wrong.
Task 3: minor (deferred): request.rs:189 second Role::User entry in issuing turn would be dropped without Unavailable marker (latent; one user entry per turn today)
Task 3: minor (deferred): runner.rs:2140 unconfigured-provider prompt branch appends user entry without bump (no coordinator there; no effect)
Task 3: complete (changes c370cab7..66af2049, review clean) — change lrtmntmuxurk
Task 4: started — change svyvmktmvrlx, BASE 66af20493d3c184e38c84299d0e3a03189b13da7
Task 4: Ruling: Jev adapter stays on assessment v1 until Task 7 — plan orders it that way; live eval is not run between T4 and T7 — costs a broken live path for 3 commits.
Task 4: Ruling: interim fixture eval maps all hold routes to all-0.9 signals and may relax eval_review_fixture_routes_corpus to safety-only gates until T8 rewrites the harness — interim router can only emit SignificantRisk — costs a temporarily weaker bench test.
Task 4: minor (deferred): assessment.rs:115 missing blank line between signal() and authorization_answer()
Task 4: complete (changes 66af2049..b1d81aa7, review clean) — change svyvmktmvrlx
Task 5: started — change kkwulwqwuqrw, BASE b1d81aa7daa53f28c3413bc4f4091edfb8956a8f
Task 5: minor (deferred): routing.rs:83 / request.rs:174 share "user" source literal via a pub const
Task 5: minor (deferred): perf workloads/review.rs:190 duplicated match on bound request (dead OverBudget arm)
Task 5: review 1 — 1 Important (policy clone not proven equal to validated revision). Fix round 1 dispatched.
Task 5: fix round 1/5 (1 addressed, 0 open — post-clone revision recheck → Stale; changes d11e69b2..0af2d5d1, squashed into kkwulwqw)
Task 5: complete (changes b1d81aa7..f28cb966, review clean) — change kkwulwqw
Task 6: started — change wuzurslywrts, BASE f28cb966830a4e37ef0b3b25eca35c86f44468e7
Task 6: Ruling: extend Task 6 to the edit deterministic HumanPerforms path (decide → restriction_human_performs → NeedsUserApproval → approvable LocalEdit row) — spec says an ordinary approve never erases human-performs; plan enumerated only the reviewer-Hold edit arm (plan gap) — costs a slightly larger Task 6 diff.
Task 6: review 1 — 2 Important (edit deterministic HumanPerforms still approvable; edit handoff evidence recorded as user_rejected/user_override). Fix round 1 dispatched.
Task 6: fix round 1/5 (2 addressed, 0 open — edit deterministic HumanPerforms handoff via EditAccessReviewState::HumanPerforms; handoff evidence reason restriction_human_performs, user_override false; squashed into wuzursly)
Task 6: minor (deferred): yach-proto LocalEditReviewState::HumanPerforms is unreachable wire surface without #[serde(other)] — future emitter would break older clients
Task 6: complete (changes f28cb966..c0cb5759, review clean) — change wuzursly
Task 7: started — change xkzkxznrztyt, BASE c0cb5759a4d38f428ae2d5a9f0f233688d14157a
Task 7: minor (deferred): questions.rs:152 authorization criterion texts not pinned by equality in test
Task 7: minor (deferred): main.rs:1 pub mod questions only to keep RUBRIC live; no external consumer
Task 7: complete (changes c0cb5759..84da0208, review clean) — change xkzkxznrztyt
Task 8: Ruling: E1 edit classification maps EditAccessReviewState::HumanPerforms → Hold(HumanPerforms) and NeedsUserApproval with restriction_ask_first → Hold(Restriction) (Task 6 fix added the HumanPerforms state after the plan was written) — costs nothing if wrong.
Task 8: Ruling: shell_disposition_* made pub + re-exported under cfg(feature="bench") for E1 classification per plan; ReviewCoordinator::new_fixture is already pub — no production visibility widening beyond the bench feature.
Task 8: started — change myrqmsowwszn, BASE 84da020894f3596e65d4d352ab4f46d67f8a5f81
Task 8: review 1 — Important: (1) run_routes uses gated ReviewCoordinator::new → every live route ReviewFailed(Disabled); (2) temp edit project path collides for same-id cases across e1/e3 under parallel tests; (3) E3 cwd evidence hard-coded /eval/project vs action.cwd.
Task 8: Ruling: reviewer-not-called is asserted, not just reported (reviewer's Minor #6) — brief says "Assert through the fixture's call counter"; a spec gap → enters the fix loop — costs a small schema addition.
Task 8: minor (deferred): e3/truncated-build-log.json expected reason risk should be clarify (opaque_effect)
Task 8: minor (deferred): run_routes reviewer_calls counts captured assessments, not invocations
Task 8: fix round 1 dispatched.
Task 8: Ruling: run_routes (E3/E4) must gate on route only; reason feeds reason_agreement — spec "hold reason reported, not gated"; expected_matches compares reason for all suites (advisor finding, verified eval_review.rs:1001) — E1 keeps reason gating (spec: E1 labels route and reason, 100%) — costs nothing if wrong.
Task 8: Ruling: add E1 deterministic human-performs EDIT case (HumanPerforms PathPrefix → Hold(HumanPerforms), reviewer_calls 0) — Task 6 deferred E2E proof of the edit handoff to E1; all E1 human_performs cases were shell — costs one case file.
Task 8: fix round 1 amended with findings 5 (route-only gating for live suites) and 6 (E1 edit human-performs case).
Task 8: fix round 1/5 (6 addressed, 0 open — new_fixture pre-gate routes; counter temp dirs; cwd from action; reviewer_calls asserted; reason gated only in E1; E1 edit human-performs case; squashed into myrqmsow)
Task 8: minor (deferred): e3_route_observes_pre_gate_execute runs under cfg(test) which already bypasses the gate — proves plumbing, not the bypass
Task 8: minor (deferred): run_routes compares aggregate reviewer_calls per run (inert: no E3/E4 case sets it)
Task 8: complete (changes 84da0208..bf0c71c0, review clean) — change myrqmsow
Task 9: started — change utnxvwwzqrtm, BASE bf0c71c064e67614ce3a5e38f5f8b39f9a7c0f55
Task 9: Ruling: every E2 case lists all 11 signals explicitly (true/false/null) — makes maintainer label review unambiguous; plan allowed sparse maps — costs larger JSON.
Task 9: Ruling: run_signals must reuse run_routes' live spawn/request construction (no duplicated spawn block) — review rubric treats duplication as a defect — costs a small extraction.
Task 9: review 1 attempt failed — reviewer agent's model (grok-4.7 via omp-subscriptions) returned 403 out-of-credits; re-dispatched on default task agent. Not a review verdict.
Task 9: review 1 — 3 Important: (1) duplicated Jev spawn block run_signals vs run_routes; (2) signal_thresholds re-parses routing.toml (duplicate of thresholds_from_toml) — fix via bench-feature accessor; (3) 8/12 dev edit cases lack diff_summary (spec: edit cases carry real diffs).
Task 9: minor (deferred): held-out gate keyed on dir name "held-out" (fragile)
Task 9: minor (deferred): E2 CaseReport hardcodes passed:true / synthesized expected_route Execute
Task 9: minor (deferred): #[expect(clippy::cast_precision_loss)] follows existing convention
Task 9: minor (deferred): e4-sed-delete-tests near-duplicates dev o-weaken-* (same file + message)
Task 9: minor (deferred): ho-secret-netrc vs ho-secret-kubeconfig scope_conflict labels inconsistent — maintainer label review
Task 9: fix round 1 dispatched.
Task 9: fix round 1/5 (3 addressed, 0 open — shared spawn_jev_reviewer; bench-gated backend signal_thresholds accessor; real diffs on 8 edit cases; squashed into utnxvwwz)
Task 9: minor (deferred → Task 10 lint pass): clippy type_complexity at eval_review.rs:1037 spawn_jev_reviewer return type
Task 9: complete (changes bf0c71c0..e434db35, review clean) — change utnxvwwz
Ruling: branch lint debt (clippy -D warnings red: permission.rs:618 needless_lifetimes from Task 5, eval_review.rs:1037 type_complexity from Task 9, possibly more masked) is fixed as the first step of Task 10, which runs just lint — one lint pass instead of reopening closed tasks — costs a lint-fix commit inside Task 10.
Task 10: started — change nxqwpqrnwxlv, BASE e434db35a69ddcd06f070260c01f0081dd733c2e; TypeSafe probe HTTP 200
Task 10: Ruling: fold cheap deferred minors in touched files (extension.rs v1 schema strings; assessment.rs blank line; e3 truncated-build-log reason → clarify) into Task 10 — same files, avoids a separate cleanup — costs a slightly broader Task 10 diff.
Task 10: Ruling: live suite gate failures are recorded as results, never "fixed" by changing thresholds/labels/rubric/routing — spec forbids tuning on held-out — costs a possibly red eval headline.
Task 10: Ruling: commit eval report JSONs under docs/project/records/2026-09-25-auto-review-signals-evaluation/ — record numbers must be reproducible from repo — costs repo size.
Task 10: review 1 — 1 Important: harness bug — prepare_edit_case does not create parent dir for CreateTextFile → routine-create-fixture fails ParentMissing before reviewer; record understates E3 routine rate. Minor folded into same fix: name opaque_effect 0.52–0.56 driver for routine-just-dev-check.
Task 10: Ruling: fix the harness and re-run E3 dev (and e3-held-out/E1 if they contain create ops) rather than only annotating — a recorded result distorted by a harness artifact must be corrected — costs one more live suite run (~140 calls).
Task 10: fix round 1 dispatched.
Task 10: fix round 1/5 (1 addressed + 1 minor folded, 0 open — CreateTextFile parent dir in harness; E3 re-run 27/28 routine 0.90; record corrected with Corrections note; squashed into nxqwpqrn)
Task 10: minor (deferred): create_text_file_in_nested_dir_reaches_reviewer_path asserts only is_ok()
Task 10: complete (changes e434db35..72aafa25, review clean) — change nxqwpqrn
Final review: With fixes — Important: (1) edit AskFirst restriction not covered in E1 (deterministic + reviewer-side), edit hold reason mapping ad-hoc; (2) extension.rs:390/4560(/4591) still say yach-review-assessment.v1; (3) authorization revision not bumped on unconfigured-provider prompt branch.
Ruling: fix wave includes all 3 Important + two cheap minors (shared USER_SOURCE const for "user"; issuing-turn duplicate user entries get Unavailable markers) — both close latent fail-open/fail-silent paths in authority code for a few lines — costs a slightly larger final fix diff. Held-out dir-name gate + E2 passed:true cosmetics documented in evals README instead of code changes.
Final fix wave dispatched.
Final review: fix wave 1 (3 Important + 2 minors + docs addressed, 0 open; change ktnkumyp); re-review clean.
Final: minor (deferred): decision_hold_reason maps every non-HumanPerforms NeedsUserReview reason to Restriction (future non-restriction edit reasons would be mislabeled in E1)
Final review: complete.
Checkpoint recorded (c2abd2d0). Awaiting user: stack shape + publication (public repo, approval required).
Post-final-review blocker (advisor, verified coordinator.rs:406-416): gate_execution rewrites Hold as well as Execute into ReviewFailed{Disabled} when AUTO_REVIEW_EXECUTION_ENABLED=false, so in production a reviewer-side Hold{RestrictionApplies{HumanPerforms}} becomes an approvable ReviewerError row (shell) / falls to LocalEdit row (edit) — Task 6 guarantee broken on the reviewer path; unit tests (cfg!(test)) and E1/E3 (fixture_reviewer) all bypass the gate so nothing caught it.
Ruling: gate rewrites only Execute; every Hold passes through (holds only tighten) — extracted as pure gate_route(enabled, route) with unit tests at enabled=false, plus a yach-bench test driving ReviewCoordinator::new (real gate, cfg(test) off in dependency builds) to prove a HumanPerforms hold survives — one extra fix dispatch + scoped re-review after the final review's single fix wave, justified because it is a safety defect — costs one more round before push.
Gate fix: gate_route blocks only Execute; holds survive; bench regression drives ReviewCoordinator::new (real gate) proving HumanPerforms hold survives + clean assessment → Disabled; re-review clean; squashed into ktnkumyp.
Checkpoint recorded (152244d8), stack head ktnkumyp e6f66032. Awaiting operator push/pr approval.
Published: pushed bookmark auto-review-signals-routing (ktnkumyp a4e248af after push re-sign); PR https://github.com/codyw912/yach/pull/278. Kata children v4e8 f70z 41s6 4e1t jbby 8jnz 44n9 2abs m2xq gzn5 and parent 9jsv closed with --pr. Plane YACH-11 roll-up not posted: Plane MCP returned HTTP 404 on retrieve_by_identifier.
Follow-up xtsonkuv (ff092f11): reports moved to yach-evals@21135bf5; SCRATCHPAD.md untracked (kept on disk); eval-report.json removed; PR diff +21940 -158. Push pending approval (previous grant expired).
Follow-up wsutsxxs (a66ce865, pushed): E2-E4 corpora moved to yach-evals@b44c51cc auto-review/corpora/<suite>/{dev,held-out}; load_corpus + eval-review validate; PR +15445 -158, 92 files. Checkpoint recorded.
