# SDD ledger — plan: docs/project/plans/2026-09-17-extension-authority-history.md

Providers: plane Yach; kata yach; completion pull_request.
Parent: yach#eh53
Task 1: Kata yach#n09g
Task 2: Kata yach#w65j
Task 3: Kata yach#8yr3
Task 4: Kata yach#ncz8
Original evidence refs yach#wc3k and yach#528c remain open.
Baseline: just test passed, artifact://2396, 2026-09-17.
Task 1: started; change vltpuuymnxts (reused); immutable BASE da3206697d4170efe9502cdf2ff637cc3fcdcb78.

## Preflight contract scan

| Tasks | Producer / consumer boundary | Finding |
|---|---|---|
| 1 / 2 | Store typed errors and explicit decision surface consumed by native scheduling and RPC/CLI tests | Compatible; Task 1 cuts over callers, Task 2 adds locking order and runtime proof. |
| 1 / 3 | main.rs, extension.rs, CLI integration tests, docs | Shared files require serialized edits; no change to stored scope or authority query. |
| 1 / 4 | Authority schema and error semantics documented and measured | Compatible; docs must describe implementation, not stronger durability claims. |
| 2 / 3 | CLI subprocess tests and optional error-message correction in main.rs | Sequential ownership; Failed exit assertions become green in Task 3. |
| 2 / 4 | Existing lifecycle wire types and implemented scheduling | Compatible; native evidence must not invent authenticated identities. |
| 3 / 4 | Discovery/failure behavior in docs/extensions.md | Compatible; final docs distinguish fresh scans from live snapshots. |
| 1 | Test contracts versus storage/caller implementation and file list | Consistent; compile-only red is insufficient; atomic faults need behavioral red. |
| 2 | RPC fixture ownership, actual native backend, blocking lifecycle ordering | Consistent; snapshot serialization and external revoke cover distinct orderings. |
| 3 | Identity deduplication tests versus catalog negative controls | Consistent; validate pointers before skipping aliases; preserve stored scope. |
| 4 | Existing protocol fields versus docs, final validation and perf | Consistent; no new runtime types or permanent benchmark APIs. |

Execution policy: implementers skip formatters, linters and all validation; controller runs focused red/green checks at requested stage boundaries, then combined checks after all edits. Agents report changes ready for controller validation before committing. This follows harness delegation requirements; no speculative test-pass claims.

Task 1: implementer AuthorityStore; initial commit 859c4409; initial review AuthorityReview found five Important issues.
Task 1: fix round 1/5 (4 addressed, 1 partial — file-sync failure proof missing; change kpoqvolk 93386f23). RED artifact://2418; GREEN artifact://2420 (37 backend + 1 CLI).
Task 1: fix round 2/5 (remaining proof addressed, 0 open; no production diff). Real CLI fsync EIO probe artifact://2427 proves unchanged baseline and temporary cleanup. Reviewer AuthorityReview accepted all findings; no scoped breakage.
Task 1: complete (changes da3206697d4170efe9502cdf2ff637cc3fcdcb78..93386f23302fc63e4689d0ab3da0977df10b17de, review clean).
Task 2: next; exact API is implemented as brief; Task 1 intentionally leaves CLI Failed exit0 for Task 3.
Task 2: started; implementer NativeLifecycle; change wlxumwtswxoxuoqoryvzwxylsmoutvum; immutable BASE 93386f23302fc63e4689d0ab3da0977df10b17de. Child claimed and associated.
Task 2: initial implementation 73a8ec4e; reviewer LifecycleReview requested actual pending-lock contention and CLI output assertions.
Task 2: fix round 1/5 (2 addressed, 0 open; fix pumzwotu 3af99258). GREEN artifact://2454 sequence 1 + CLI 2; review accepted.
Task 2: complete (changes 93386f23302fc63e4689d0ab3da0977df10b17de..3af992586434a2ac51ea1021cbc0e7aadedfa1ef, review clean). Actual CLI/native smoke TASK2_SMOKE_OK; evidence in task-2-report.md.
Task 3: started; implementer DiscoveryIdentity; change vrxqtvuokyvvqylmzllnzkovqlxurynn; immutable BASE 3af992586434a2ac51ea1021cbc0e7aadedfa1ef. Child claimed and associated.
Task 3: complete (changes 3af992586434a2ac51ea1021cbc0e7aadedfa1ef..078854f9cbef4a1fc9849301f2a0379b38e5f1a8, review clean). DiscoveryReview no findings; GREEN2465 CLI8/backend6; original no-override identical/symlink HOME smoke passed. Source/error branches reviewed statically; combined validation next.
Task 4: started; implementer ProtocolDocs; change qvoxrupmqmrpyqnsvmnzovuqlxwpoput; immutable BASE078854f9cbef4a1fc9849301f2a0379b38e5f1a8. Child claimed and associated. Controller owns final tests/lint/performance/probe.
Task 4: docs committed 97d6dc09. Recovered ProtocolReview found Stop selector overclaim and unescaped Markdown table pipes; correction 5ed0b655; scoped re-review requested.
Resume: preserved later user build-resource change 1b1615e3. Prior perf result was absent; rerunning gate, not claiming prior success.
Idle history-load probe artifact://2493: 10,000 samples each; 1 decision 99,839,677 ns / 672 bytes; 1000 decisions 31,348,656,046 ns / 547,130 bytes. Equivalent approved set/version; actual decision timestamps/UUIDs. Authority/lock bytes and metadata and directory entries unchanged. Temporary example removed in ae1a508c.
Final reviewer FinalAuthorityReview running against full branch package review-ee5470f1035c..39e0fbdf9565.diff with later doc correction and probe cleanup supplied.
Task 4: complete (docs97d6dc09, correction5ed0b655, probe cleanupae1a508c; ProtocolReview accepted). Final proof in task-4-report.md.
Final review: FinalAuthorityReview sole Important existing-directory ancestor sync gap corrected42444478. RED2501, full testGREEN2503, lint/fmt/pinnedCI2505 pass. Scoped re-review accepted; zero open findings.
Performance: artifact://2499 all five extension rows unchanged, gate pass. Idle history probe2493 recorded; no read-path writes or locking seen by source review/metadata proof.
Ruling: use repository `just lint-ci` pinned1.98.1 for final CI reproduction instead of the plan's floating-stable command — repository policy explicitly requires the CI pin; wrong choice would miss compiler-specific findings. Both commands passed during this work.
Final committed-tree `just test` passed artifact://2509. Workspace clean, no stack conflicts. All implementation and review tasks accepted; publication not authorized yet. Existing measurement ancestor given descriptive subject without content changes.
