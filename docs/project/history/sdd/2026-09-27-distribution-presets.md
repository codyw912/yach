# SDD ledger — plan: docs/project/plans/2026-09-27-distribution-presets.md
Spec: docs/project/specs/2026-09-27-distribution-presets-design.md
Kata parent: yach#3r07
Task 1: Kata yach#a4yf
Task 2: Kata yach#gkwx
Task 3: Kata yach#t3ap
Task 4: Kata yach#t7bj
Task 5: Kata yach#j8pd
Task 6: Kata yach#zadk
Task 7: Kata yach#d61d
Task 8: Kata yach#p7je
Task 9: Kata yach#9g9s
Task 10: Kata yach#6fqs
Task 1: change ykvokopqpspmyyxlunvkpxtskmllnkqo (described; Kata claimed)

## Pre-flight scan (2026-09-27)

| Pair / task | Shared surface | Finding |
|---|---|---|
| T1 -> T2 | `Component`, `Preset`, `ComponentSet` | T2 consumes `is_kernel`, `name`, `parse`, `from_preset`, `full`, `with`; all produced by T1. OK. |
| T1 -> T3/T4 | `ComponentSet::{project_tools, baseline_guidance, contains, with}` | Produced by T1. OK. |
| T2 -> T6/T7 | `persist_bundled_removed`, `persist_preset`, `bundled_removed`, `kernel_components` | Produced by T2. OK. |
| T3 -> T4 | `RunnerConfig.components`, round threading | T4 consumes the `components` field T3 threads into `ProviderAgentToolRound`; T4 must not re-add it. OK. |
| T3 -> T7 | `RunnerConfig.components`, `ExtensionActivationSnapshot::for_components` | T7 sets `components` on production constructors T3 left at `ComponentSet::full()`. OK. |
| T3 / T4 / T5 | `runner.rs` | Sequential edits of one file; no interface conflict. T5 touches only `:6504` and compaction status. OK. |
| T6 -> T7 | `bundled.rs`, `hashline_extension.rs` test | T6 shortens the CLI test; T7 restores it. Intentional handoff. OK. |
| T6/T7 -> T8 | `bundled::BUNDLED.manifest_json`, `materialize -> Option` | T6 defines `manifest_json: fn() -> Option<&str>` returning Some; T8 adds cfg. OK. |
| T3/T6/T7/T8 -> T9 | CLI behavior | T9 is acceptance only. OK. |
| T1 self | tests vs code | `from_preset(Full) == full()` holds: bits 0b1_1111 = all five. OK. |
| T2 self | tests use `set_private` | **Defect**: no `set_private` helper exists; existing tests inline `set_permissions(0o600)` under `#[cfg(unix)]` (user_config.rs:437-441). |
| T3 self | tests vs code | Tests reference `run_recording_agent_round_with_components` and `advertised_tool_names` helpers the task instructs the implementer to write. OK. |
| T3 self | commit fileset | `jj commit crates/yach-backend crates/yach-cli/src` includes all literal sites. OK. |
| T4 self | callers list | Lists every caller found by grep (runner.rs). OK. |
| T5 self | `load_from_paths` | Test uses it; Step 3 defines it as the testable core. OK. |
| T6 self | shortened CLI test | Consistent with T7 restoring it. OK. |
| T7 self | parser helper name `parse` | Plan tells implementer to use the existing helper. OK. |
| T8 self | test cfg | Test only compiles without `bundled-hashline`; run command uses `--no-default-features --features bundled-jev`. OK. |
| T9 self | scenario 1 | Tolerates hashline activation timing by asserting provider names. OK. |

Ruling: Task 2 tests use a local `set_private(path)` helper defined in the test module as the existing inline `#[cfg(unix)] set_permissions(0o600)` block — the plan names a helper that does not exist; the brief already allows substituting the existing pattern — cost if wrong: none beyond a helper name.
Task 1: dispatched implementer Task1Impl (BASE da872ff43fed458b05b55b8557d3fdd83267bc4f)
Task 1: implementer DONE (change ykvokopq f18ac3e4); review dispatched Task1Review
Task 1: complete (changes da872ff43fed..f18ac3e4f089, review clean; ⚠️ runtime bash presence deferred to Task 3 acceptance)
Task 2: change ovtwquvnmpokmusvtsyurrzqqkkxlzpu (described; Kata claimed); BASE f18ac3e4f089a6aa418ae5167659593fd16e6a94
Task 2: dispatched implementer Task2Impl. Ruling: brief's set_private/unwrap_or_default test helpers replaced by existing user_config test style (inline 0o600 permissions, assert!(...is_ok())) — plan named a nonexistent helper — cost if wrong: none
Task 2: minor (deferred): persist_bundled_removed filters non-string array entries that parse_snapshot already rejects (unreachable defensive code; implementer-noted)
Task 2: complete (changes f18ac3e4f089..245504862479, review clean; ⚠️ downstream diagnostic display/removal enforcement verified in later tasks)
Task 3: change pzuowtumpqolsyqrymvrsrkzkqoumrwv (described; Kata claimed); BASE 2455048624791cd77e28ca18bfee12f98637831f
Task 3: dispatched implementer Task3Impl (effort hi)
Ruling: background-activation inactive-bundle status reuses runner's turn_permission_policy (made pub(crate)) — plan named no policy source for extension_state.rs — cost if wrong: one visibility change to revert
Ruling: inactive-bundle dedup set is an Arc<Mutex<HashSet<(ext, bundle)>>> created in the runner loop and threaded beside components; background activation sends without dedup — plan left owner unspecified — cost if wrong: duplicate status lines, cosmetic
Task 3: implementer DONE (change pzuowtum 8a65be6d; runner suite 262 passed; CLI check ok); review dispatched Task3Review
Task 3: review — Important (plan-mandated): failed-bundle drop is two-pass, not a fixed point; overlapping bundle implementations can leave a newly failed bundle's members advertised standalone with its diagnostic discarded (extension.rs:902-914).
Ruling: fix the plan-mandated algorithm — iterate drop/resolve to a fixed point and keep every bundle's diagnostic; the spec's "failed bundle contributes no tools" is binding over the plan's two-pass text — cost if wrong: a few extra resolves per turn when bundles fail.
Ruling: pre-existing gap — standalone (non-replacement) extension tools are never in RigProviderRequester's fixed approved list on main (runner.rs provider_approved_tools only), so rig rejects them; this plan does not widen it (with full() behavior is unchanged) and does not fix it — out of spec scope — cost if wrong: minimal/full + a third-party standalone tool fails exactly as on main today. Follow-up to file for the user.
Task 3: fix round 1 dispatched to Task3Impl (FIX_BASE 8a65be6d25635b00b2f2f02c9495584fc9b0a2f1)
Task 3: fix round 1 implementer DONE (change mqvluwzu 35cabd3f)
Task 3: fix round 1/5 (0 addressed, 2 open — regression test's bundle B fails on pass one via ExtensionIdMismatch so cascade untested; per-bundle diagnostic filter drops extra member diagnostics; changes 8a65be6d..35cabd3f)
Task 3: fix round 2 dispatched to Task3Impl (FIX_BASE 35cabd3f46bb0862a5d457221cc75b21e3f9c710)
Task 3: fix round 2 implementer DONE (change mvwsslsl)
Task 3: fix round 2/5 (2 addressed, 0 open; changes 35cabd3f..dd3f97b6)
Task 3: complete (changes 245504862479..dd3f97b6a628, review clean after 2 fix rounds)
Task 4: change qzqqqsyxpxvmyzxulyuxqmzssqxqwynn (described; Kata claimed); BASE dd3f97b6a6281d610307cb9095d2f488b1f4e525
Task 4: dispatched implementer Task4Impl
Task 4: implementer DONE (change qzqqqsyx)
Task 4: complete (changes dd3f97b6a628..ebc38dced268, review clean; ⚠️ whole-request default equivalence -> Task 9.1)
Task 5: change wzszmyzslktrnpuulyossvkxmpqrunqx (described; Kata claimed); BASE ebc38dced268e579a15d9c3f3b6ee940bf66434d
Task 5: dispatched implementer Task5Impl
Ruling: summary_prompt fields are #[serde(skip)] on CompactionConfig and read from the raw user JSON only; files over 64 KiB are "unreadable" — plan specified a cap but not over-cap behavior — cost if wrong: large prompts rejected instead of truncated
Task 5: implementer DONE (change wzszmyzs 55880ea6)
Task 5: controller found yach crate build break before review: crates/yach-cli/src/main.rs:2252 calls build_summary_prompt with one arg (implementer ran -p yach-backend only). Review cancelled; fix dispatched to Task5Impl.
Ruling: every remaining task's verification includes `just dev cargo check -p yach --all-targets` — the plan's per-task commands never compiled the CLI crate — cost if wrong: one extra check per task
Task 5: minor (deferred): a summary_prompt key inside an invalid project config.json is undetectable (file fails closed to defaults; implementer-noted)
Task 5: complete (changes ebc38dced268..09f74d55e8f4, review clean after CLI-caller amend)
Task 6: change uylyzyznopzszsmmwuuzskxtkstskymt (described; Kata claimed); BASE 09f74d55e8f4faf49c5ce33f47fcd376d1387b7f
Task 6: dispatched implementer Task6Impl (effort hi)
Ruling: extension remove of a bundled record persists [bundled] removed after the store save; a config write failure reports command failure with the record already removed — plan did not order the two writes — cost if wrong: a removed record without a tombstone could be re-added by a later preset apply
Ruling: refresh_on_upgrade errors and unreadable user config degrade to a stderr warning with an empty removed set — matches the old ensure_* warning behavior — cost if wrong: a removed id could be refreshed if config is unreadable (refresh never creates records, so impact is limited to repointing an existing record)
Task 6: implementer DONE (change uylyzyzn fe26d525). Known intermediate break: tests/rpc_review.rs rpc_hashline_extension_recovers_from_malformed_edit_then_applies_reviewed_edit relied on per-read seeding; Task 7 first-run full apply restores it.
Ruling: Task 7 acceptance adds `just dev cargo test -p yach --test rpc_review` — the break is the designed Task 6/7 handoff, same as hashline_extension — cost if wrong: an unnoticed broken RPC test at Task 7
Task 6: review — Important: bundled.rs:10,134 #[allow(dead_code)] violates denied clippy::allow_attributes; use #[expect(dead_code, reason=...)]. Fix round 1 dispatched (FIX_BASE fe26d5259ad0116a322a02d21ea1d12568521ce3)
Task 6: fix round 1 implementer DONE (change vlmpopql 344315d9); clippy -p yach blocked by yach-backend clippy errors.
Controller clippy run (-p yach-backend --all-targets -D warnings): compaction.rs:110 needless deref, :113 redundant closure, :989 single_match_else (Task 5); extension.rs:3785 panic/assert in Result-returning test (Task 3); extension_install.rs:499 same (Task 6).
Ruling: one fix dispatch to Task6Impl covers all five; Task 6's own lint joins its fix range, Task 3/5 lints land as a separate commit in the same range; every remaining task runs `just dev cargo clippy -p yach-backend -p yach --all-targets -- -D warnings` — the plan's per-task commands never ran clippy, which CI enforces — cost if wrong: one extra lint pass per task
Task 6: fix round 2 dispatched (FIX_BASE 344315d9c3edce8022bc590eedcb3288438fe5ab)
Task 6: fix round 2 implementer DONE (changes rpvnwkot, luvoqkqk; clippy clean)
Task 6: fix round 2/5 (6 addressed, 0 open; changes fe26d525..721e6672)
Task 6: complete (changes 09f74d55e8f4..721e66721213, review clean after 2 fix rounds; rpc_review hashline scenario pending Task 7)
Task 7: change ltrzouluxzwlztsywuxymvpwmyosortu (described; Kata claimed); BASE 721e667212138e6a5e9a656f7b356ad46e965bbc
Task 7: dispatched implementer Task7Impl (effort hi)
Ruling: PresetApplyReport field `removed` renamed `disabled` — apply_preset only disables records (spec: removal is explicit only), so "removed" would misreport — cost if wrong: output key rename
Ruling: `component list`/`preset show` states: kernel enabled/disabled; bundled enabled/disabled/removed/not-installed; skill-index reserved; compiled_in from manifest_json() — plan named states but not not-installed — cost if wrong: output wording
Ruling: `component enable <bundled>` with no record installs it; `disable` with no record succeeds with message=not installed — plan unspecified — cost if wrong: UX wording
Ruling: unknown [components] keys warn on stderr at preset show/component list/session start — spec says "diagnostic", plan did not name the surface — cost if wrong: warning placement
Task 7: mid-flight steering sent to Task7Impl (3 corrections)
Ruling: explicit `preset use` installs and enables included bundled records; only ensure_first_run preserves existing enabled (apply_preset preserve_existing flag) — spec :117-118/:170-171 bind over plan Task 7 text that preserved on every apply — cost if wrong: explicit preset switches override manual enable/disable, which is the spec's "applying a preset is an action"
Ruling: parser test asserts the parser's existing bad-argument result (Command::Unknown), not a nonexistent Command::UsageError — plan defect — cost if wrong: none
Ruling (Task 6 regression, fixed in Task 7): refresh_on_upgrade always re-materializes existing non-removed bundled records; the version-only stale gate dropped the old per-read current_exe() rewrite, breaking same-version rebuilds — spec's upgrade rule is about repointing, not a license to skip exe refresh — cost if wrong: one byte-compare per bundled package per start (writes only on change)
Task 7: implementer DONE (change ltrzoulu fcebb0f1; corrections applied)
Task 7: review — 5 Important: (1) ephemeral loaders run persistent upgrade refresh (writes extensions.json); (2) ephemeral full cannot restore disabled/absent bundles (plan-mandated filter-only); (3) first-run marker persisted before bundle reconciliation (plan-mandated order); (4) component disable swallows store read errors; (5) ephemeral startup skips unknown-component warnings. ⚠️ omitted-bundle preference persistence -> Task 8.
Ruling: an ephemeral --preset session resolves roots in memory: persisted non-bundled records as-is (no refresh, no save) plus each bundled component the preset includes, materialized into ~/.yach/bundled (a cache, not user state) regardless of its persisted enabled/removed state — spec: ephemeral selection "uses a preset for that session only" so evaluation compares presets on one checkout — cost if wrong: an ephemeral full session loads an extension the user removed persistently; grants/trust still gate it
Ruling: apply_preset reconciles and saves the install store first, then writes config (marker and components); a failed store save leaves no marker so first run retries — spec first-run rule implies completion only when applied — cost if wrong: config and records can diverge if the config write fails after the store save (next explicit apply repairs)
Task 7: fix round 1 dispatched to Task7Impl (FIX_BASE fcebb0f142b4ff91e2521be5b83439354778e9f5)
Task 7: fix round 1 implementer DONE (change tprkkwlu a631630a)
Task 7: fix round 1/5 (5 addressed, 1 open — new: apply_preset reconciles before validating config, so malformed config + removed id reinstalls and saves store; changes fcebb0f1..a631630a)
Task 7: minor (deferred): first-run failure test exercises store load failure, not save failure (bundled.rs:563-578)
Task 7: fix round 2 dispatched (FIX_BASE a631630a0279640ec123c0780b6dcf242cbc84c8)
Task 7: fix round 2 implementer DONE (change uwyxqnys a5ddaf3e)
Task 7: fix round 2/5 (1 addressed, 0 open Important; changes a631630a..a5ddaf3e)
Controller verified reviewer Minor: unix 0500-dir save-failure test fails at materialize, not save (fs::write truncates existing file without dir write), and fails under root. Test replaced rather than deferred — it asserts a false claim and is privilege-sensitive.
Task 7: fix round 3 (test-only) dispatched (FIX_BASE a5ddaf3e37938b5567ad7e705575035299f0f1c0)
Task 7: fix round 3 implementer DONE (change rtkzumqp 877a9cff)
Task 8 carry-forward: bundled::tests that assert materialization (first_run_save_failure asserts ~/.yach/bundled exists; ephemeral_roots/full tests) must be cfg-gated on bundled-hashline/bundled-jev or tolerate not_compiled_in when Task 8 makes the crates optional
Task 7: fix round 3/5 (2 addressed, 0 open; changes a5ddaf3e..877a9cff)
Task 7: complete (changes 721e66721213..877a9cff47cf, review clean after 3 fix rounds)
Task 8: change wsttvlqlrpqowvlrstrxoxoporoqsznu (described; Kata claimed); BASE 877a9cff47cf8d9842135c453dd120e99cfbd767
Task 8: dispatched implementer Task8Impl (effort hi)
Ruling: core-build extension diagnostics — spec :220-221 requires "not compiled in" in extension diagnostics and sessions starting normally; plan brief omits it. Discovery skips Bundled records whose package is not compiled in (bundled::is_compiled_in); diagnostics report such records blocked with new ExtensionActivationErrorKind::NotCompiledIn ("not_compiled_in") — cost if wrong: one enum variant in yach-backend
Ruling: in a core build apply_preset records the preference on existing records (explicit use enables, exclusion disables) and reports every included not-compiled-in package under not_compiled_in; component enable/disable on a not-compiled-in bundled id records what it can (set_enabled on an existing record; enable with no record clears the removed marker) and succeeds with "<id> not compiled in; preference recorded" — spec :219-223 "records the preference ... and continues"
Ruling: materialized_version has no production consumer after the Task 7 gate removal; move it into bundled::tests instead of keeping cfg_attr(expect(dead_code))
Ruling: feature-dependent tests are cfg-gated (bundled::tests materialization cases, hashline_extension bundled tests, rpc_review hashline scenario); verification adds cargo test/clippy for yach under --no-default-features
Task 9 prep (controller verified brief against code):
- Scenario 4: `extension_background_activation_finished` is a trace mark, not a ServerEvent; the StatusUpdated `tool_replacement_bundle_inactive extension=yach.hashline` is sent immediately before it (runner/extension_state.rs:181-189) — wait_for that StatusUpdated as the sync point
- Scenario 8: approval mode persists at ~/.yach/permissions/<project-key>.json (permission.rs:109-121); shell allowlist at ~/.yach/config.json (shell.rs:95); capability grants under ~/.yach/extensions/ (extension_capability/store.rs:199) — snapshot all three
- Scenario 11: `yach extension list` never runs upgrade refresh (extension_diagnostics_result uses loaded_extension_install_records); refresh runs at session extension discovery (installed_extension_records via extension_package_root_loader). Scenario must start an RPC session with FirstRenderCompleted, then inspect extensions.json
- Hashline-dependent scenarios (1 tool-name claim holds regardless; 4, 6, 10, 11) cfg-gated on bundled-hashline after Task 8
Task 8: implementer DONE (change wsttvlql 68de9273; also fixed two Task 7 lint warnings)
Task 8: controller check — clippy -p yach --no-default-features --features bundled-hashline --all-targets fails: bundled.rs:382 unused ExtensionInstallScope import, :396 materialized_version never used (test gates cover both-features/neither but not hashline-only). bundled-jev-only clean.
Task 8: unauthorized test deletion — main.rs loop_provider_cancel_after_finish_does_not_duplicate_terminal_turn lost its persisted TurnFinished==1 assertion and temp-log cleanup (BASE 877a9cff); ruling: restore verbatim from BASE, keep 'store' binding, drop SessionEvent import removal
Task 8: review — 3 Important: (1) unauthorized deletion of persisted TurnFinished assertion + temp-log cleanup (main.rs:5732,5797); (2) hashline-only build: bundled.rs:382 import, :395-396 helper unused under warning-denied clippy; (3) hashline_extension.rs:422 gate admits hashline-only but :453 asserts extension_count=2. WARN: Acceptance 12 session start not exercised end-to-end in core build.
Ruling: Acceptance 12 "session starts normally" moves to Task 9 as a cfg(not(bundled-hashline)) RPC scenario with a stale full-build hashline record; Task 8 fix adds the extension-diagnostics half (doctor reports not_compiled_in) — spec :220-221 names both
Ruling: Task 8 verification covers all four feature combinations (clippy --all-targets -D warnings and cargo test -p yach) — hashline-only failure shows two-combination checks are insufficient
Pre-existing (not Task 8): yach depends on publish=false yach-jev-reviewer while yach is in publish_crates — cargo publish of yach cannot resolve it; predates this plan (non-optional at trunk). Report to user at handoff.
Task 8: fix round 1 dispatched (FIX_BASE 68de9273f8d1cbb7e27ff9e46ef38d7c50b41ff3)
Task 8: fix round 1 implementer DONE (change mrlnqmmw 9ea29976)
Task 8: fix round 1/5 (4 addressed, 0 open; changes 68de9273..9ea29976); controller re-ran hashline-only clippy clean + cargo test 221 passed
Task 8: complete (changes 877a9cff47cf..9ea29976cdf8, review clean after 1 fix round)
Task 9: change wmpykyloyutrvxlkkxvsxrtqkmtrlszq (described; Kata claimed); BASE 9ea29976cdf861b6d87be3df57f00169fb6adbe6
Task 9: dispatched implementer Task9Impl (effort hi)
Ruling: all CLI/RPC children run with current_dir(project) plus the rpc_review env scrub — extension_store_path(Project) reads current_dir, so an inherited cwd would read crates/yach-cli/.yach
Ruling: scenario 7 seeds persisted full state first (preset use full), then asserts byte-identity after rpc --preset minimal — a fresh HOME makes "writes nothing" trivially true for extensions.json
Ruling: scenario 8 pre-writes ~/.yach/config.json with a shell allow entry so byte-identity is meaningful; also asserts ~/.yach/extensions/ (grants) unchanged; event is ClientEvent::ApprovalModeSelected
Ruling: new scenario 12b core_build_session_starts_with_stale_bundled_record (cfg not bundled-hashline) closes the Task 8 review WARN for Acceptance 12
Ruling: a scenario failing because of product code is reported, not fixed in Task 9 — controller routes the fix to the owning task's files
Task 9: implementer DONE_WITH_CONCERNS (uncommitted presets.rs; 7/9 default scenarios pass)
Ruling (Defect A, real, owner Task 2/7): first-run apply must preserve existing [components] kernel toggles — persist_preset gains preserve_existing; with it, only absent kernel keys are written. Spec rule 3 "Only components with no prior record take the preset's default". Fix committed as a separate "Task 7 fix" change before the Task 9 change — cost if wrong: explicit toggles made before first session survive first run (the spec's intent)
Ruling (Defect B, not a preset defect): model-default auto-save on first activation with no configured default is pre-existing (runner.rs:2225 identical at trunk) and preset-independent; Acceptance 7 = preset-owned state unchanged (every session writes ~/.yach/sessions, so "no writes at all" is unsatisfiable). Scenario 7 byte-compares extensions.json and compares parsed [preset]/[components]/[bundled] tables of config.toml — this was the dispatch ruling; report the auto-save as an observation to the user
Task 9: implementer DONE (Task 7 fix wmpykylo; Task 9 pwxuqrvw)
Task 9: controller ran --test presets default 9/9 (1.13s) and --no-default-features 6/6 (0.71s)
Task 10: dispatched early (docs-only, parallel with Task 9 review) as Task10Impl — commits after Task 9 on its own change; if Task 9 fix rounds touch product behavior the docs reference, controller re-checks docs
Ruling (Task 10): docs describe shipped behavior — `extension install --bundled` takes ids yach.hashline|yach.jev-reviewer (usage line), component names for enable/disable; compaction.summary_prompt lives in ~/.yach/config.json under "compaction" (CompactionConfig::load_for_project), not config.toml; core-build `component enable` records the preference; publish caveat for --no-default-features is NOT documented as a crates.io install until yach-jev-reviewer publish=false is resolved — document `cargo install --path crates/yach-cli --no-default-features`
Task 10: change tpvrnpvnxwrvkmnsyknnxyoyyqlpmvyr (described; Kata claimed); BASE e69cdb4146da33df2ac79167769b68061d0ac361
Task 9: review — 2 Important: (1) 12b seeds an empty package dir (no manifest) so removing the is_compiled_in filter still passes; hashline_ prefix check meaningless (replacements keep native names); (2) scenario 8 grants dir is empty before and after — clearing grants would pass. Preserve fix (wmpykylo) confirmed by LSP refs.
Ruling: 12b seeds the real hashline manifest from crates/yach-hashline-extension/yach.extension.json (path read, no crate dep) with main.command=CARGO_BIN_EXE_yach; assertion must fail with the filter removed — implementer proves by local mutation (remove filter, see FAIL, restore)
Ruling: scenario 8 seeds a real grant via ExtensionAuthorityStore::in_home(home).grant_requested(...) with a uses_network tool (feature-independent) before snapshot
Task 9: fix round 1 dispatched (Task 9 change pwxuqrvw; FIX_BASE e69cdb4146da33df2ac79167769b68061d0ac361) — Task 10 docs change tpvrnpvn sits above; implementer edits presets.rs in pwxuqrvw via jj edit/squash, not in Task 10's change
Controller correction: Task 9 fix round held until Task 10 commits — both agents share one jj working copy; moving @ mid-edit would carry Task10Impl's uncommitted docs. Serialize jj mutations: Task 10 commit, then Task 9 fix via jj new pwxuqrvw / squash / return to tip.
Task 10: mid-flight corrections — (1) core-build "recorded preference applies" true only for existing records; record-less components stay uninstalled after moving to a full build (preset_applied already set, first run never re-fires); (2) summary_prompt path must be absolute (PathBuf::from, no ~ expansion, relative = process cwd)
HANDOFF FLAG (spec gap): spec :222-223 "If the same user state is later used by a build that includes the component, the recorded preference applies" is unimplementable for record-less components without re-applying a preset at startup, which rule 3 forbids ("Later starts never re-apply a preset"). Shipped behavior: existing records carry the preference; missing ones need explicit preset use / extension install --bundled. Needs a spec decision (e.g., record intent in [bundled] or accept the limitation).
Task 10: implementer DONE (change tpvrnpvn a843181e)
Task 9: fix round 1 implementer DONE (squashed into pwxuqrvw); mutation proof reported
Task 9: fix round 1/5 (grant seeding ADDRESSED; 12b NOT ADDRESSED — snapshot lacks activation-completion barrier)
Controller found flake: removed_bundled_extension_stays_removed timed out 2/3 runs at presets.rs:1154 — second session on same home takes home_has_connection branch and waits for DiscoveredModelsUpdated with MODEL_ID that never arrives (transcript: discovered_models only fixture-echo; StateUpdated.default_model already resolved with connection_id). Implementer's 9/9 x2 was schedule luck.
Ruling: stored-connection sessions take the activation target from StateUpdated.default_model (resolved), not discovery; no timeout increase. 12b waits for `extension_background_activation_finished` StatusUpdated before the snapshot. Evidence: scenario loop x20 + full suite x5 per build.
Task 9: fix round 2 dispatched
Task 10: review — 3 Important (controller verified): (1) openai-responses compactor still sends build_summary_prompt (runner.rs:6552-6558) — docs workaround false; (2) component enable restores removed bundled; --reset clears all removed markers but installs only included bundles; (3) minimal is not bash-only if non-bundled extensions / project context exist
HANDOFF FLAG (spec error): spec :283-285 says users can avoid yach's summarization prompt with compactor = "openai-responses"; runner always runs the portable summary pass after native compaction. Only compaction.enabled = false avoids it.
Task 10: fix round 1 dispatched — Task10Impl edits files only, no jj; controller squashes docs paths into tpvrnpvn after Task9Impl's squash lands (serialize jj mutations)
Task 9: fix round 2 implementer DONE (squashed into pwxuqrvw 7722fcf3); reports 20/20 scenario loop, 5+5 suite runs
Task 10: fix round 1 edits squashed by controller into tpvrnpvn (0fb9cc7c)
Task 9: controller evidence after fix round 2 — default presets binary: removed_bundled_extension_stays_removed 20/20, full suite 5/5 (9 tests); core suite via cargo 2/2 (6 tests). (Discarded a first loop that ran the core binary where scenario 6 is cfg'd out.)
Task 9: fix round 2/5 (2 addressed, 0 open); complete (Task 7 fix wmpykylo 847af187 + Task 9 pwxuqrvw 7722fcf36cfb, review clean after 2 fix rounds)
Task 10: fix round 1/5 (3 addressed, 1 new: compaction.enabled=false only gates automatic compaction; manual /compact ignores it — runner.rs CompactionRequested path loads config without enabled check)
Task 10: fix round 2 applied by controller directly (one sentence, docs/presets.md:160-164), squashed into tpvrnpvn
HANDOFF FLAG: compaction.enabled=false does not gate manual /compact (runner.rs ~1942-2025) — note for the user; not changed in this plan
Task 10: fix round 2/5 (1 addressed, 0 open); complete (tpvrnpvn b1c40836b71b, review clean after 2 fix rounds)
Final verification: started
Final verification: advisor-found CLI/docs mismatch fixed — core-build component enable/disable without a record said 'preference recorded'; now reports 'no install record (run yach extension install --bundled <id> ...)'. Committed as Task 8 fix above Task 10; core_build_reports_hashline_not_compiled_in asserts both branches.
Final verification (head xqwwktqt c7eeb903b6d7):
- just fmt: no changes; just lint (toolchain-check + clippy --all-targets --all-features -D warnings): clean
- clippy -p yach --no-default-features --all-targets -D warnings: clean
- cargo test -p yach-backend -p yach (default): backend 890, yach unit 203, integration 8/3/2/10/9(presets)/1 — all pass
- cargo test -p yach --no-default-features: unit 199, integration 8/2/6(presets)/10/1 — all pass
- cargo build -p yach --no-default-features: ok; cargo tree core: 0 matches for yach-hashline-extension|yach-jev-reviewer
- manual smoke (throwaway HOME): preset use minimal -> 3 note= lines; component list: project-tools/baseline-guidance disabled, skill-index reserved; preset use full installs both; extension remove yach.hashline then preset use full -> hashline state=removed, not re-added
- perf: not run locally (release A/B is heavy); CI perf-deterministic gate measures default-feature build on the PR
Pre-shape jj op: d15ab0c19ed1
Shaped: one change per task (13 changes); shaped tree == verified tree except bundled.rs:772 assert message + spec amendment (5 lines to shipped behavior)
Final head verification: clippy -D warnings x4 feature combos clean; cargo test -p yach --no-default-features all pass; per-commit cargo check on 9 code commits 0 errors
Checkpoint recorded.
PR: https://github.com/codyw912/yach/pull/279 (bookmark distribution-presets, head c1564e19, content == d57e95e9)
Kata: 10 children + parent 3r07 closed --pr. Plane YACH-12: roll-up comment posted; left open until merge.
Kata verification (post-close, proper field path): all 10 children + parent 3r07 status=closed; parent links show every child closed.
CI on PR #279 (head c1564e19): Rust workspace pass; Perf (deterministic) FAIL — turn/scripted/tools_4/builtin alloc_count +384 (+8.4%), alloc_bytes +46k; text_only alloc_count +1.
Root cause: resolve_provider_turn_catalog_dropping_failed_bundles (fixpoint) and the activation-finish diagnostic resolve ran with zero replacement bundles. Fix: early return when replacement_bundles is empty (both sites), folded into Task 3 (pzuowtum).
Local perf run --deterministic on fixed tree: text_only 2553/333406, tools_4/builtin 4586/593610 (== base). Full `just perf --deterministic` A/B vs main on e58791a5: 27 unchanged, 2 improved, 0 regressed.
Spec reconciliation moved out of tpunysqv (restored to its original amendment) into its own tip commit nvuvqzxs "Reconcile distribution presets spec with shipped behavior" with a Revision note under Status.
Final matrix on e58791a5: clippy -D warnings clean x4 feature combos (yach + yach-backend); just lint clean; default tests all pass (backend 890, yach 203 + integration); core tests all pass (199 + integration).
Push pending: rewritten history (all 14 hashes change) needs user OK for force-push + fresh publication approval.
Force-pushed with user approval: distribution-presets c1564e19 -> acef79c6 (content == verified e58791a5; sign-on-push rewrote metadata only). CI run 36371060523 started.
PR body REST PATCH denied: HTTP 403 (empty body) — needs operator pr-edit approval.
Checkpoint recorded.
Plane YACH-12: perf-fix/force-push update comment posted.
Final: CI run 36371060523 on acef79c6 — Rust workspace success, Perf (deterministic) success
PR body PATCH applied after pr-edit approval: HTTP 200 (contains perf fix + owner-decision framing). CI run 36371060523 on acef79c6: both jobs success.
