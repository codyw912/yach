# SDD ledger — plan: docs/project/plans/2026-09-16-extension-capability-contract.md

Spec: docs/project/specs/2026-09-16-extension-capability-contract-design.md
Providers: roadmap plane (Yach), execution kata (yach), completion pull_request
Outcome: plane:YACH-10

## Kata refs

Parent: yach#528c
Task 1: Kata yach#vwhx
Task 2: Kata yach#djrb
Task 3: Kata yach#nmyb
Task 4: Kata yach#fp10
Task 5: Kata yach#vzzc
Task 6: Kata yach#xfh3
Task 7: Kata yach#wc3k
Task 8: Kata yach#25gh

## Pre-flight conflict scan

Task-pair rows — every pair sharing a file or interface:

| pair | shared | A produces | B consumes | finding |
|---|---|---|---|---|
| T1-T2 | extension.rs, tools.rs | `ExtensionToolRisk::{UsesNetwork,RunsProcess}` | same variants | agrees |
| T1-T4 | extension.rs | risk variants | `ExtensionToolContribution` (existing) | agrees |
| T1-T5 | extension.rs | risk variants | Task 3 fns | agrees; T5 depends on T1+T3 |
| T1-T6 | extension.rs | risk variants | Task 3 fns | agrees |
| T2-T4 | extension.rs, tools.rs | `network_execution` field, `with_network_execution` | existing types | disjoint edits in tools.rs vs extension.rs |
| T2-T5 | extension.rs, tools.rs | policy field | Task 3 fns | agrees |
| T2-T6 | extension.rs, tools.rs | policy field | Task 3 fns | agrees |
| T4-T5 | extension.rs | `initialize_and_register(&[ExtensionToolContribution])` | Task 3 fns | agrees; both edit extension.rs, T4 first by design |
| T4-T6 | extension.rs, main.rs | registration signature | Task 3 fns | agrees |
| T5-T6 | extension.rs, main.rs | `capability_block_reason` | Task 3 fns | agrees |
| T6-T7 | main.rs | diagnostic `capabilities:`/`grant:` lines | `store_grant`, `remove_grant` | **CONFLICT — see Ruling 1** |
| T3-T7 | — | `store_grant`, `remove_grant` | same | agrees |

Per-task internal consistency: T1-T8 each self-consistent — failing-test-first
step present (T1-T7), full `just test` verification present, commit step
present, no task creates a file it also modifies.

Global constraints: no plan code block uses `unwrap()` or `expect()`.
One exception found — see Ruling 2.

### Ruling 1 — diagnostic field naming

Task 6's Interfaces block says the diagnostic gains a `capabilities:` line
and a `grant:` line, but its own test asserts `capabilities=uses_network`
and `capability_grant=none`. `ExtensionDiagnosticRecord::render_line`
(crates/yach-cli/src/main.rs:862) emits `key=value` space-separated pairs,
so the colon form in the Interfaces prose is wrong for this surface.

Ruling: the asserted forms are authoritative — `capabilities=` and
`capability_grant=`, matching `render_line`'s existing `key=value` shape.
The Interfaces prose is loose wording, and the task itself already says
later tasks do not depend on the exact wording.
Cost if wrong: a diagnostic line reads `capabilities=` where someone
expected `capabilities:`; cosmetic, one-line fix, no dependent code.

### Ruling 2 — `panic!` in Task 5's test

Task 5's first test uses `panic!("expected an activation diagnostic")` in an
`else` branch. `panic!` appears nowhere in crates/yach-backend/src, which
consistently uses `let Some(x) = … else { return }`. The global constraint
names `unwrap()`/`expect()`, so `panic!` is not literally forbidden, but it
is a foreign pattern here and clippy's configuration may reject it.

Ruling: implementers use the codebase's `let … else { return }` form with a
preceding `assert!` that the value is present, as the plan's other tests do.
The assertion must still fail loudly when the diagnostic is absent — an
early `return` with no assert would make the test vacuous.
Cost if wrong: if `panic!` were in fact acceptable, this costs two extra
lines per site and nothing else.

## Progress

Task 1: dispatched (Kata yach#vwhx claimed, change npwwnlpp, BASE dc3fb79dfaff64bc40aa45cfc9c4a9f73a583960, agent Task1RiskVocabulary, model med)
Task 1: review — spec ❌, 1 Important (partial registration reachable)
Task 1: Ruling: the atomicity finding is real and load-bearing — verified at
  extension.rs:1750-1765, where the batch pre-validates duplicates across all
  staged definitions (deliberate atomicity) but the commit loop registers
  sequentially and bails with `?`, leaving earlier tools registered. Task 1
  makes that path newly reachable: `uses_network` now parses, so it reaches a
  commit loop the native allowlist still rejects. Fixing it here rather than
  parking, because Task 2 widens that same allowlist and Task 4 adds two more
  rejection reasons to the same loop — every later task builds on this path.
  Decision: pre-validate risk admissibility alongside the existing duplicate
  pass, so the commit loop cannot fail midway; keep a test covering rejection
  of the now-recognized risk in a batch with a preceding valid tool.
  Cost if wrong: a slightly stricter pre-check than strictly needed, and one
  extra test. If left unfixed: a failed registration silently leaves partial
  tools in the registry, and two later tasks widen the same failure surface.
Task 1: fix round 1/5 (1 addressed, 0 open — batch pre-validates risk before commit; changes 3db5fda282ab..pending)
Task 1: Ruling: implementer's own concern accepted — the round-1 fix duplicated
  the owner/risk rules from ToolRegistry::register_extension_tool (tools.rs:2082-2098)
  into extension.rs. Two copies of one rule is the same defect shape as the
  emit_alloc bug fixed earlier in this project, and Task 2 widens exactly that
  allowlist while Task 4 adds two more rejection reasons to the same path, so the
  copies would drift on the next task. Decision: extract
  ToolRegistry::extension_tool_rejection as the single source of truth, consulted
  by both register_extension_tool and the batch pre-validation.
  Cost if wrong: one extra public method on ToolRegistry that only two callers use.
Task 1: carried to Task 4 — implementer notes initialize_and_register registers
  per-message rather than as a staged batch, so it lacks the atomicity
  process_extension_registration_messages now has. Not fixed in Task 1: Task 4
  rewrites that function's signature and validation. Must be addressed there.
Task 1: fix round 2/5 (1 addressed, 0 open — extracted ToolRegistry::extension_tool_rejection as single source of truth; changes 3db5fda282ab..d9b8f6629487)
Task 1: complete (changes dc3fb79dfaff..acff0f517e11, review clean)
Task 2: dispatched (Kata yach#djrb claimed, change vrsykxwmyost, BASE acff0f517e11b5f4c8bdb952c07b4254c873a0c9, agent Task2NetworkPolicy, model med)
Task 2: note carried into dispatch — the brief points at tools.rs:2088 for the
  registration allowlist, but Task 1 moved that rule into
  ToolRegistry::extension_tool_rejection. Dispatch tells the implementer to widen
  it there and verify only one copy remains. Also flagged that Task 1's atomicity
  regression test pins the OLD network rejection and must be rethought now that
  network risk is admissible; its real purpose was batch atomicity.
Task 2: implementer reported DONE_WITH_CONCERNS; first concern verified as a real gap.
Task 2: Ruling: project_provider_advertised_tool carries a FOURTH copy of the
  three-risk allowlist (tools.rs:803-806, extension branch). Confirmed empirically
  with a throwaway probe: a UsesNetwork extension tool returns
  Err(UnsupportedRisk { name: "fetch_url", risk: UsesNetwork }) from that function,
  so the tool is never advertised and the model never sees it. Task 2's own
  acceptance criterion says the tool "is advertised to providers", so this is a
  failed spec review, not an observation. Decision: fix within Task 2, routing the
  extension branch through the same shared rule rather than adding a fifth copy —
  the widened vocabulary must be admissible in exactly one place.
  Cost if wrong: the shared helper gains a caller it did not need. If left unfixed:
  every later task builds on a declared tool the provider can never call, and
  Task 8's end-to-end proof would fail at the last step.
Task 2: Ruling: the deleted Task 1 atomicity test is accepted. Its purpose was
  batch atomicity, which it asserted via a risk the native allowlist rejected;
  Task 2 makes that risk admissible, so the vehicle no longer exists. Implementer
  reports later_message_fails still covers atomicity. To be verified in review.
  Cost if wrong: batch atomicity loses coverage — review must confirm the
  replacement actually exercises a mid-batch failure.
Task 2: fix round 1/5 (1 addressed, 0 open — advertising routed through
  ToolRisk::is_extension_declarable; verified with the same throwaway probe that
  previously returned Err(UnsupportedRisk), now passing; zero hand-maintained
  three-risk lists remain in tools.rs; changes 8ed227b25775..0dec55ff39b9)
Task 2: review dispatched (agent Task2Review-2, full-task diff acff0f517e11..0dec55ff39b9)
Task 2: complete (changes acff0f517e11..0dec55ff39b9, review clean — spec OK, no findings)
Task 2: carried to Task 4 — reviewer notes the surviving atomicity test
  (extension_host_registration_is_atomic_when_later_message_fails) exercises a
  PARSE failure before registry mutation, not a native pre-validation failure.
  Task 4 adds UndeclaredTool and ToolRiskMismatch to that same path, so Task 4
  should cover a mid-batch failure that reaches pre-validation rather than
  failing at parse.
Task 3: dispatched (Kata yach#nmyb claimed, BASE 0dec55ff39b93107be340954647b5399ab8aed1d, agent Task3CapabilityModule, model lo — brief carries complete code for module and tests, so transcription plus the three file ops)
Task 3: review — spec OK, no Critical; 1 Important (process-global HOME mutation in tests)
Task 3: Ruling: the HOME finding is real. Verified five other backend modules read
  HOME (compaction.rs:107, permission.rs:99, runner.rs:869, sensitive_paths.rs:222,
  shell.rs:578) while ENV_LOCK is private to the extension_capability test module,
  so it serializes nothing outside it; cargo runs tests as threads in one process.
  Decision: refactor rather than lock harder — split the path-accepting core
  (load_grant_at/store_grant_at/remove_grant_at) from the HOME-resolving wrappers,
  and test the core against a temp path with no environment mutation at all. This
  also removes the `unsafe` set_var from the test and is the shape Task 7 will want
  when it grants and revokes.
  Cost if wrong: three extra thin wrapper functions. If left unfixed: unrelated
  tests intermittently resolve user config against a corrupt-grant fixture, which
  is a scheduling-dependent failure that would be very hard to diagnose later.
Task 3: fix round 1/5 (1 addressed, 0 open — path-accepting _at cores with
  HOME-resolving wrappers; zero set_var/ENV_LOCK/unsafe remain; changes
  784d16b74b71..79eed0e54113)
Task 3: complete (changes 0dec55ff39b9..79eed0e54113, review clean)
Task 4: dispatched (Kata yach#fp10 claimed, BASE 79eed0e541131da6b178c91577fb37b8d1ee49ac, agent Task4ManifestBinding, model hi — security-critical, and carries the two findings deferred from Tasks 1 and 2: per-message registration atomicity in initialize_and_register, and a mid-batch failure test reaching the new post-parse validation)
Task 4: implementer concern investigated — process_extension_registration_messages
  and its wrapper run_extension_host_registration_command still accept host
  name/risk verbatim. Verified this is NOT a live bypass: both are `pub` but
  neither is re-exported from lib.rs, no caller exists outside extension.rs, and
  their only non-test caller is each other. Production activation reaches
  initialize_and_register, which Task 4 bound to the manifest.
Task 4: Ruling: fix the inconsistency in Task 4 rather than defer or ignore it.
  Two public functions performing the same job with different strictness is how a
  future caller picks the unvalidated one by accident, and the manifest-binding
  check is now a shared helper so wiring it in is cheap. Decision: have the batch
  path take the declared tools and apply the same name/risk validation, or state
  in the report why it cannot. Not treated as a security finding, because it is
  unreachable from production today.
  Cost if wrong: an extra parameter on a function only tests call. If left
  unfixed: the contract holds only on the path that happens to be wired, and the
  weaker public API remains available to the next caller.
Task 4: fix round 1/5 (1 addressed, 0 open — bind_registration_to_manifest is the
  single name/risk check, called by both initialize_and_register (ext.rs:1858) and
  the JSONL batch path (ext.rs:1756); run_extension_host_registration_command now
  forwards declared tools; new test extension_host_registration_is_atomic_when_a_later_tool_mismatches_the_manifest)
Task 4: controller completed the commit — the implementer's agent settled after
  finishing the work but before committing. Verified on disk first: build clean,
  1399 tests pass, lint and fmt clean, shared helper called from both paths, report
  complete including the requested staged-batch reasoning. Committed as 739683bb
  with no code authored by the controller.
Task 4: review dispatched (agent Task4Review-2, model hi, diff 79eed0e54113..3be82ec42314)
Task 4: two implementer concerns passed to review rather than accepted —
  (a) description/provider_visible/input schema still host-supplied, not compared
  to the manifest; (b) initialize_and_register expects exactly declared_tools.len()
  messages while the JSONL path accepts any matching subset.
Task 4: CORRECTION to my own earlier ruling — I claimed
  process_extension_registration_messages and run_extension_host_registration_command
  were "pub but not re-exported from lib.rs, so not a live bypass". That was wrong.
  lib.rs:65 is `pub use extension::*`, a glob re-export, so both ARE public API of
  the crate. My check grepped lib.rs for the function names, which a glob
  re-export does not contain, so the method could not have found it. Verified by
  reading lib.rs:60-70 after the reviewer flagged it.
  Consequence: the fix round I ordered was load-bearing, not tidiness — it closed
  a reachable bypass on the crate's public surface rather than pre-empting a
  hypothetical future caller. The decision was right; my stated reason was not.
  Lesson for later tasks: grepping lib.rs for a symbol name cannot prove absence
  of export when the crate uses glob re-exports.
Task 4: complete (changes 79eed0e54113..3be82ec42314, review clean — spec OK, no findings)
Task 4: reviewer resolved both implementer concerns, neither a defect:
  (a) provider_visible IS more than presentation (it controls model advertising, so
      a host can expose a manifest-hidden tool), BUT requested_capabilities includes
      every declared tool's risk regardless of visibility/description/schema, so a
      host cannot strip a required network or process grant from the consent summary.
      Retaining host-supplied description/visibility/schema is consistent with this
      task's name-and-risk-scoped contract. Do not claim the whole advertised tool
      surface is manifest-equal.
  (b) the count asymmetry is safe: too few live registrations hit the per-receive
      timeout and commit nothing; trailing extra messages are rejected as Malformed
      by a later invocation; manifest parsing already rejects duplicate declared
      names (ext.rs:1549-1551) and both paths reject repeated host names via
      staged_names before mutation, so a host cannot satisfy the count by
      re-registering one tool. JSONL subset acceptance is a completeness asymmetry,
      not capability escalation.
Task 5: dispatched (Kata yach#vzzc claimed, BASE 3be82ec42314c8602ed55a3dd59cd034b35f58f2, agent Task5ActivationGate, model hi)
Task 5: dispatch carries three things the brief cannot: Ruling 2 (no panic!, use
  let-else with a preceding assert), the HOME-mutation prohibition with Task 3's
  _at-core solution as the pattern to follow, and explicit instruction to resolve
  the brief's todo_build_record()/error_kind()/message() placeholders against real
  API rather than committing them.
Task 5: implemented — one shared capability_block_reason (ext.rs:1147) called from
  reload_extension_from_record (ext.rs:878) and activate_background_metadata_extensions
  (ext.rs:1200). Controller verified independently: diff adds no set_var/remove_var/
  unsafe/panic!, all three gate tests pass, and removing ONLY the reload-path call
  makes reloading_an_extension_cannot_bypass_the_capability_gate FAIL — so the gate
  is load-bearing on the path the plan advisory identified as a bypass, proven by
  test rather than by reading.
Task 5: review dispatched (agent Task5Review-2, model hi, diff 3be82ec42314..f2dfff885ff8)
Task 5: review — production gate correct on both paths (pre-count, pre-spawn,
  manifest-derived, fails closed on corrupt grants); reviewer searched beyond the
  diff and found exactly two callers of activate_extension_host_record, both gated,
  and no third production activation route. Spec marked strictly-❌ on two points.
Task 5: Ruling: MY Ruling 2 was unsatisfiable and is hereby corrected. I required a
  preceding assert! before let-else, but Cargo.toml:22 sets
  panic_in_result_fn = "deny" and these tests return Result<(), String>, so assert!
  cannot be used in them. The implementer's early-Err-before-let-else fails just as
  loudly and satisfies the intent (no silent early return). Ruling 2 now reads: in
  Result-returning tests, return Err with a message; use assert! only in
  ()-returning tests. Not a finding against the implementer.
  Cost if wrong: none identified — both forms fail the test with a message.
Task 5: Ruling: the test-isolation finding is real and worth fixing. The tests use
  fixed extension ids and a comment asserting the grant file "cannot exist", which
  is false — a real grant under the developer's HOME for
  test.capability-gate.network would make the test fail while production behaviour
  is correct. Low probability, but the failure is confusing and the comment is
  wrong. Decision: fix by making the precondition independent of persistent user
  state, following the neighbouring TestPackageRoot per-run naming rather than
  fixed ids, and correct the comment. Do not mutate HOME.
  Cost if wrong: slightly noisier fixture names. If left unfixed: a rare, very
  confusing false failure, plus a comment that misstates why the test is safe.
Task 5: complete (changes 1489ddeb1769, re-review ADDRESSED, no findings)
Task 6/7 sequencing: both briefs modify crates/yach-cli/src/main.rs, so they run
  serially rather than in parallel - same-file concurrent edits are not guaranteed
  to merge. Task 6 first (smaller).
Task 6: dispatched (Kata yach#xfh3 claimed - note my first guess 'nvxg' was not a
  real id; kata list gave xfh3. BASE 1489ddeb1769352b7a3ed205d26af728966e8186,
  agent Task6Diagnostics, model hi)
Task 6: controller pre-verified that the brief understates the work. Neither
  ExtensionActivationDiagnostic (backend ext.rs:626-640) nor ExtensionDiagnosticRecord
  (cli main.rs:507-523) carries risk data - both hold tool NAMES only. So this is
  plumbing from the manifest down to render_line, not a render-only change.
Task 6: hazard flagged to the implementer - there are 6 construction sites of
  ExtensionActivationDiagnostic plus from_install_record and
  from_activation_diagnostic. A site left defaulting to an empty set would render
  capabilities=none for a network extension: a false statement in the exact surface
  the user consults before consenting. Required: audit every site, and render an
  explicit unknown state rather than 'none' where the manifest is unknowable.
Task 6: implementer reported complete with one concern: TUI /extension-status uses
  yach-proto's ExtensionDiagnosticRecord, which has no capability fields, so the TUI
  snapshot shows no declared risk. CLI list/doctor covered.
Task 6: Ruling: the concern is load-bearing, fix now rather than defer. Reasoning -
  Task 7 adds /extension-trust as a TUI slash command (it does not exist yet), so the
  user would grant consent in the TUI while unable to see declared capabilities in
  that same surface. Verified the fix is small and the data is already present at the
  dropping site: proto record at yach-proto/src:771-785 lacks the fields;
  extension_diagnostic_record_from_activation (runner/extension_state.rs:348-370)
  reads an ExtensionActivationDiagnostic that now carries them and simply does not
  copy them; TUI render at yach-ui/src/app.rs:104-121; one other construction site,
  a test fixture at app.rs:7115.
  Required same field names/formatting as the CLI render - two surfaces printing the
  same fact differently is itself a defect.
  Cost if wrong: two proto fields crossing the JSONL boundary. If left unfixed: the
  interactive consent surface is the blind one, which defeats the task's purpose.
Task 6: fix round 1/5 (1 addressed, 0 open - proto fields added, copied through
  extension_diagnostic_record_from_activation, rendered in yach-ui/app.rs:116 with
  the same field names as CLI main.rs:880). Agent settled mid-step without a final
  report again; controller verified (1407 pass, lint+fmt clean, both surfaces'
  wording identical) and committed on its behalf: changes 1489ddeb1769..1bd936baf58d.
Task 6: the 'unknown' answer to the false-none hazard is implemented and pinned by
  test - CLI test at main.rs:7448 asserts a discovered-but-unactivated row renders
  capabilities=unknown capability_grant=unknown rather than none.
Task 6: review dispatched (agent Task6Review-2, model hi) with the derivation
  question called out explicitly: capabilities must come from manifest declarations,
  not host-registered names, because a BLOCKED extension registers nothing and
  would render capabilities=none - exactly the extension the user most needs to see.
Task 6: complete (changes 1bd936baf58d, review clean - spec OK, no findings)
Task 7: dispatched (Kata yach#wc3k claimed, BASE 1bd936baf58dd5fbd0160af215cabaf99cf012d9,
  agent Task7GrantCommands, model hi)
Task 7: TWO brief corrections issued, both overriding the written brief:
  (a) The brief says "Point HOME at a temp directory" for the backend test. That
  directly contradicts the constraint Tasks 3 and 5 established - five backend
  modules read HOME and cargo runs tests as threads in one process. Overridden:
  use the _at path-accepting cores.
  (b) The brief says to record evidence, but PermissionDecisionRecorded requires
  session_id and turn_id (session.rs:374-378) and a CLI subcommand has neither.
  Flagged as the real design question, with an explicit prohibition on inventing a
  synthetic session/turn id to satisfy the type - that would put fiction in the
  audit trail. "CLI writes the grant without session evidence" declared acceptable
  if it is the honest answer.
Task 7: carried Task 6's empty-grant note forward - this is the task that WRITES
  grants, so it must not create an approved-but-empty grant that renders identically
  to no grant at all.
Task 7: implemented (changes 1bd936baf58d..04514a82c96c, 1418 pass, lint clean).
Task 7: session-evidence decision ACCEPTED - CLI and TUI both write the grant
  without PermissionDecisionRecorded. I verified the justification independently
  rather than taking it on report: handle_native_extension_lifecycle_request
  (runner/extension_state.rs:246-253) receives request_id, NOT session_id, so
  extension lifecycle requests are genuinely not session-scoped in this
  architecture. Fabricating an id to satisfy the type would have put fiction in the
  audit trail. Flagged to the reviewer as a possible gap for a later task, with the
  concrete consequence: a user grants network access and later cannot tell when or
  why it was granted.
Task 7: controller-verified before review (not taken on report):
  - closed-loop proof at extension.rs:4561 - blocked -> granted -> block clears ->
    revoked -> block restored, temp path + per-run id, no HOME mutation. Implementer
    added capability_block_reason_with_grant as the path-injectable seam.
  - Task 6's empty-grant carry-forward handled AND tested
    (extension_capability.rs:514-530): requesting nothing writes NO file, keeping
    'absent' distinct from 'approved but empty'. Correct resolution.
  - my one concern about the proof test (line 4598 re-derives with None rather than
    reloading) is covered elsewhere: extension_capability.rs:500-510 reloads after
    revoke and asserts it does not load.
  - grant_confirmation_message names the approved set and states Yach does not
    observe whether the host uses those capabilities. Honest wording.
Task 7: review dispatched (agent Task7Review-2, model hi) with the sharpest
  questions being manifest-change-after-grant (is version_at_grant actually used?),
  selector ambiguity, and whether revoke stops a running host.
Task 7: review - spec OK, quality NOT approved. One Critical finding, and I verified
  BOTH halves of it myself rather than trusting the description:
  (a) grant_path (extension_capability.rs:107-115) builds
      home/.yach/extensions/{id}.json with PathBuf::join. Rust join semantics: an
      ABSOLUTE component replaces the whole path. Demonstrated: selector
      '/etc/cron.d/evil' resolves to '/etc/cron.d/evil.json', outside the grant
      directory entirely. revoke_grant_at then calls remove_grant_at on it, so
      revoke can DELETE AN ARBITRARY .json FILE the user never intended.
  (b) Raw selectors genuinely reach that code on both paths - TUI
      extension_state.rs:307-309 map_or_else(|| selector.clone(), ...) and CLI
      main.rs:4476 Ok(None) => selector.to_owned(). Undiscovered selector = raw
      user string used as a filename component.
  Second consequence, quieter but also real: after uninstall,
  '/extension-revoke /path/to/pkg' looks for a grant named after the path, reports
  'no capability grant recorded', and LEAVES the real id-keyed grant in place - so
  reinstalling the extension is still silently covered by the old grant. That is a
  capability-contract failure, not just a UX wart.
  Ruling: fix now, in this task. This is the security-relevant surface the whole
  plan exists to establish; shipping a revoke that can miss the real grant while
  deleting an unrelated file would undo it.
Task 7: reviewer also resolved the questions I posed - version_at_grant is
  display-only (so adding another uses_network tool after a network grant stays
  covered, while adding runs_process is correctly blocked); selectors are exact
  matches with no prefix ambiguity; TUI revoke does stop the host but CLI revoke
  does not, and the messages only claim the grant, which is honest.
Task 7: gap noted for a later task - granted_at dies with the grant file, so a user
  cannot later tell when or why a capability was granted. Not in this task's scope.
Task 7: fix round 1/5 (1 Critical addressed, 0 open). Fix shape is good design, not
  just a patch: grant_path_in is a single choke point refusing any non-id, and
  grant_id_from_selector is one shared resolution rule (discovered manifest id wins,
  undiscovered accepted only if itself a valid id, else an actionable error telling
  the user to pass the id). Validator REUSED via crate::is_valid_extension_id - no
  second copy, which is the defect shape I had to fix twice earlier on this plan.
Task 7: controller verified the escape empirically with a throwaway probe against
  grant_path_in: '/etc/cron.d/evil', '/path/to/pkg', '../../escape' and 'a/b' all
  refused; 'example.toy-tools' still resolves. Probe removed. Sentinel test at
  extension_capability.rs:621-631 writes a file outside the grants dir and asserts
  it survives. 1421 pass, lint+fmt clean. Committed changes d69e3f3d091a..fe652c1b548a.
Task 7: scoped re-review dispatched (Task7ReReview) - key question is whether any
  remaining route lets a non-id string reach a grant filename, since grant_path is
  pub and callers outside the diff could still pass anything.
Task 7: complete (changes fe652c1b548a, re-review ADDRESSED, no new breakage)
Gate phase complete (Tasks 5, 6, 7). Moving to Proof phase.
Task 8: dispatched as TWO PARALLEL slices (Kata yach#25gh, BASE fe652c1b548a):
  Task8Proof owns test/fixture files, Task8Docs owns docs/ only. Disjoint file
  ownership, so genuine parallelism rather than serialized work - unlike Tasks 6/7
  which both touched yach-cli/src/main.rs and had to run serially.
Task 8: controller pre-resolved three things so neither agent has to guess:
  - The proof needs a REAL spawned extension. crates/yach-cli/tests/hashline_extension.rs
    :144 is the pattern; it also shows the local TestUnwrap convention and a Mutex
    for HOME-dependent work. Integration tests are their own process, so HOME there
    is acceptable - unlike the unit-test prohibition that has governed Tasks 3-7.
  - There is NO existing user-facing extension doc (docs/ has only README.md,
    benchmarks/, project/, protocol/), and specs/plans are explicitly historical.
    So the docs slice creates a new doc and registers it in the docs/README.md table.
  - docs/protocol/yach-proto-v0.md is flagged: Task 6 added capability fields to
    yach_proto::ExtensionDiagnosticRecord, which crosses the JSONL boundary, so the
    protocol doc may now be incomplete.
Task 8: scope boundary restated in full to both agents - consent, not enforcement.
  An extension can open a socket without declaring uses_network and Yach neither
  prevents nor detects it. That sentence is the one a reader must not miss.
Task 8: both slices landed in parallel with disjoint files, no coordination needed.
  Proof: zqoyootu (48d25029). Docs: vkvykzys. Combined fe652c1b548a..a408442eeed7.
Task 8: controller verification (done myself, not taken on report):
  - Fixture crates/yach-cli/tests/fixtures/capability-network/ is a REAL spawned host
    ("command": "sh", "args": ["host.sh"]), not a mock. Test
    network_fixture_is_blocked_until_granted_then_callable_and_survives_restart passes.
  - Gate-removal proof is concrete and is what gives the test teeth: deleting the
    capability_block_reason check produced 'left: 1, right: 0' on host_start_count,
    i.e. a host genuinely spawned. Restored, tree clean.
  - Smoke output is real observed output with real temp paths and real JSONL, showing
    capability_grant=none -> uses_network -> none after revoke. Not paraphrase.
  - Perf gate clean: no extension/* row regressed against main, thresholds untouched.
    So the per-extension grant read at activation costs nothing measurable.
  - Wire encoding checked at proto_capability_grant (extension_state.rs:447-453):
    null=unknown, []=absent, non-empty=approved. Unambiguous BECAUSE Task 7
    guarantees an approved-but-empty grant is never written - the two decisions
    compose correctly.
  - Doc claims spot-checked against source: subcommand names, block message verbatim,
    all three error labels, and version_at_grant display-only (so the doc's
    version-change claim is correct). Registered in docs/README.md table. Zero
    forbidden wording; the consent-not-enforcement boundary is paragraph two.
Task 8: 1422 pass, lint+fmt clean. Review dispatched (Task8Review-2) with the doc's
  honesty as the highest-value target and the restart-survival stage flagged - does
  it re-read from disk or observe cached in-process state?
Task 8: complete (changes 48d25029 proof + vkvykzys docs, review clean - spec OK,
  approved, no findings). Reviewer confirmed the two stages I flagged: restart
  survival genuinely re-reads disk (load_grant is fs::read, no cache) and the
  callable stage is a real stdio invoke returning {"ok":true}, not advertisement.
Controller test-quality action (mine, not a review finding): deleted
  extension_capability_grant_renders_approved_capabilities (yach-cli/src/main.rs:5813).
  It wrote a message string itself and asserted render_lines echoed it back - a
  forwarding test, not behaviour, and it pinned full wording that would break on any
  copy edit. The substance is covered properly in
  extension_capability.rs:577 (asserts the label names the tool and extension, and
  guards the no-overstatement constraint without pinning wording), and the
  render_lines field format is covered by the neighbouring install test at :5793.
  190 yach bin tests pass after deletion.
FINAL BRANCH REVIEW: NOT ready to land. One Critical emergent finding - exactly the
  class a whole-branch pass exists to catch, invisible to all eight per-task reviews.
  FINDING: the contract is unusable in production. Task 2 taught authorize() and
  allows_provider_advertising() to consult network_execution, but the only production
  policy builder (runner.rs:4856-4869) buckets active extension tools into
  metadata/content/edit and DROPS UsesNetwork/RunsProcess:
    ToolRisk::FixtureSafe | ToolRisk::UsesNetwork | ToolRisk::RunsProcess => {}
  with_network_execution has NO production caller - only the Task 2 unit test
  (tools.rs:2881). So after a grant the host starts and the tool registers, but the
  model never sees it and a call would be denied.
  Verified empirically with a throwaway probe reproducing the production builder:
    PROBE authorize=Denied advertised=false
  File-scoped extensions are unaffected (they hit the metadata/content/edit arms), so
  nothing regressed - but the entire network/process capability path this plan exists
  to deliver is silently dead in a real session.
  Why no per-task review caught it: Task 2 proved the policy CAN allow network risk,
  Task 5 proved activation is gated, Task 8 proved the host spawns and the tool is
  callable - but the Task 8 proof forges ToolValidation { permission: Allowed }
  (yach-cli/tests/extension_capability.rs:136-142) and never builds a turn policy.
  Each task's seam was correct; the join between Task 2 and the agent loop was never
  wired.
  Ruling: fix before landing. Shipping a consent contract whose granted capability
  cannot be exercised would be worse than not shipping it - the user would grant,
  see success, and the tool would silently not work.
FINAL FIX: Critical ADDRESSED, branch verdict READY TO LAND.
  Fix: extracted turn_permission_policy (runner.rs:4822-4862, production caller 4894)
  routing UsesNetwork -> with_network_execution and RunsProcess -> process list,
  seeded with "bash" then appended (with_process_tools REPLACES, so seeding matters).
  Extraction was deliberate, not incidental: the per-task proofs could not reach this
  join because each verified its own seam, so the fix needed a named seam a test could
  address. Regression test
  a_granted_network_extension_tool_is_advertised_and_allowed_in_a_turn asserts
  authorize/advertising observables plus the negative case, and I proved it
  load-bearing - reverting the fix yields 'left: Denied, right: Allowed'.
  Safety argument verified three ways: only production caller passes active_tool_names()
  (Active-only filter); both activation entry points call capability_block_reason
  before mark_active; and - the route I had not checked - Blocked/Failed diagnostics
  never populate registered_tools, so a non-consented name cannot reach the policy even
  in principle. The other production ToolPermissionPolicy site (runner.rs:4585) is the
  read-only round using with_project_read_only_tools(), which has no extension tools,
  so it is correctly unaffected.
  Deleted CLI echo test confirmed to lose no real coverage.
