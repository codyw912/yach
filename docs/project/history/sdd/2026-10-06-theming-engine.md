# SDD ledger — plan: docs/project/plans/2026-10-06-theming-engine.md
Fix-round budget: 3
Kata parent: yach#n7kf
Task 1: Kata yach#9n61
Task 2: Kata yach#2d00
Task 3: Kata yach#j3h9
Task 4: Kata yach#4rew
Task 5: Kata yach#4yxn
Task 6: Kata yach#dg04
Task 7: Kata yach#pvnc
Spec: docs/project/specs/2026-10-02-theming-engine-design.md
Start: main 4de8fb28; Task 1 change urtzoynursrw
Ruling: Task 3 precondition (tui-markdown on main) is satisfied by #290 (43570cfd) — plan's gate text satisfied — none.
Preflight: scan table at preflight.md (21 pair rows, 7 self-consistency, 10 hazards, 20 anchors, 7 spec conflicts).
Ruling: F1 (T1) semantic-error tests run parse+apply and expect the preserved resolver's taxonomy (#xyz -> UnknownVariable) — plan tests contradict plan code; spec keeps resolver — a wrong error variant name in one test.
Ruling: F2 (T1) system focused border expects the accent (Cyan), not Reset — matches the focused-accent contract in the plan's own code — one assertion.
Ruling: F17 (T1) selected() routes through mode handling: System gets no background, Mono keeps REVERSED; add a custom base=system selectedBackground test — spec S728 every system bg Reset — small helper change.
Ruling: R8 (T1) named White maps to (255,255,255), contrast 21, equal to Indexed(15) — matches ratatui-crossterm — contrast table values.
Ruling: R9 (T1/T5) one shared test-only contrast assertion helper used by preset and palette tests, with an explicit pre-Task-5 exception for toolOutput removed in Task 5 — avoids duplicated partial coverage — test refactor.
Ruling: P1364 import (T1) use the imported ThemeDocument name or drop the import — no dead imports — none.
Ruling: F14 (T2) palette table gets a real production consumer in Task 2 (select::resolve_base lookup by name used by an existing code path such as ThemeDocument base resolution), no allow attributes; if no honest consumer exists before Task 4, Task 2's gate is clippy without the unused table and Task 2 and Task 4 commit together — no dead code / no allow — staging order.
Ruling: F15 (T2/T7) binary size measured after Task 4 links the palettes, in the actual cargo target dir (cargo metadata target_directory); docs call the existing gate the relative per-PR size gate, not YACH-13 — measurement location — doc wording.
Ruling: F3 (T3) tool_title() is not bold during Task 3's migration (pin unchanged); Task 5 adds the bold title as a declared visible change — pin must stay immutable — one modifier moves between tasks.
Ruling: F4/R5 (T3) pin viewport tall enough for every fixture row (or scrolled assertions per row); user marker identified by entry context, not a bare '›'; split pin helper under 100 lines; list fixture file in Files — pin must cover the diff row — test shape.
Ruling: F10/F11 (T3) selection helpers are theme-owned and mode-aware: Painted keeps today's dark cells, System uses attribute selection with no background, Mono keeps REVERSED; test final cells — spec S224-229, S606-613 — helper design.
Ruling: Staleness #6 (T3) migrate the assistant Markdown gutter accent read at transcript.rs:1151 and keep Markdown/alignment routing from #290/#292 — missing site — one site.
Ruling: F5 (T4) bare name vs path: a value containing '/' or starting with '.' or '~' or absolute is a path (read error reported, then auto); a bare unknown name gets a diagnostic and auto — spec S462-466 and S715 both hold — tests for both.
Ruling: F6 (T4) borrow the matches! scrutinee, add missing test imports, wire re-exports/options before green runs, list Cargo manifests in Files — compile order — none.
Ruling: F7/F9/F8 (T5) replace tool_state in place; panel_rows takes &[Line<'static>] with explicit lifetime; import TranscriptEntry; update all three named tests whose rule span bg assertions conflict (assert rule separately) — compile and test consistency — none.
Ruling: F13 (T5) split tool header/summary/body into tool_title (bold) / muted / tool_output roles without new tokens; review diff styling preserved — spec S287-288, S646-670 — visible styling.
Ruling: R1-R3 (T5) tests assert display width + full wrapped text, check every row of the known panel extent, and add TestBackend mono cells including review/user/tool rows — rubric — test depth.
Ruling: F12 (T6) themes load from user-scope records only, filtered before indexing/duplicate detection; add test: valid selected user theme + invalid/duplicate project manifest still applies — spec S509-512 — loader order.
Ruling: R4/F18 (T6) add real temp install cases: oversize reader, symlink containment, disabled, project-scope ignored — spec S733-756 — test scope.
Ruling: F18 (T2/T4) palette tests check the pinned variant; Task 4 tests actual legacy file startup lookup — spec — test scope.
Ruling: F16/R10 (T7) capture driver uses a PTY driver or delayed pipe ((sleep 3; printf '/quit\r') | script ...), preserves child exit status; scanner treats SGR 39/49 resets as non-color — spec — script.
Task 1: BASE 4de8fb2823f4bf2ba8637c7446fbaf74b5f2068e
Task 1: implementer T1Implementer-3; preflight rulings F1,F2,F17,R8,R9 carried
Task 1: review 1 — spec ❌ (1 Important: lib.rs re-export omits ThemeMode/ToolState/DiffKind, plan-mandated); quality Needs fixes. Reviewer T1Reviewer-3.
Ruling: Task 1 re-exports ThemeMode, ToolState, DiffKind beside Theme/ThemeDocument (Task 4's export list keeps them) — public methods take these types; unnameable outside the crate — none; no import-only API test (wiring test).
Ruling: ThemeLoadError::InvalidColor stays (pre-existing, brief retains it, F1 forbids manufacturing a path) — reviewer agreed — dead variant persists; final review may triage.
Task 1: FIX_BASE e268f3b15868da1fa34ac71443fe6e9491596db0
Task 1: fix round 1/3 (1 addressed, 0 open — enum re-exports; changes e268f3b1..d4e0517c)
Task 1: complete (changes 4de8fb28..d4e0517c, review clean) — changes urtzoynu, qpumntup
Task 2: change ntpwzqtzqkks BASE d4e0517c7da38ef6197be487ab0594e0d22e01aa
Ruling: F14 (T2) `bundled_palette` and `BundledPalette` are re-exported from yach-ui lib.rs in Task 2 (the plan already makes bundled_palette `pub` for the CLI), so the table has a public consumer and lint passes with no allow attributes — overrides the brief's "do not re-export in this task" — one extra public fn earlier than planned.
Ruling: F15 (T2) Step 25 size measurement moves to Task 4 (palettes are not linked into the yach binary until select.rs uses them); measure in the cargo metadata target_directory; Task 2 commit message drops N/M/D and says measurement follows in Task 4 — linker drops unused data — none.
Ruling: R6/F18 (T2) NOTICE test asserts each palette has its attribution row (name + upstream + license as stated), and the palette test asserts each palette's declared appearance matches its pinned upstream variant (light/dark) — substring check proves nothing — test depth.
Task 2: review 1 — spec ❌ (1 Important: NOTICE test accepts any URL/license; must compare to expected attribution per palette). Reviewer T2Reviewer-3. Palette JSON verbatim to fences (checked).
Task 2: FIX_BASE 9e581a1657669fc7477ac057eb287cb9ccc187f5
Ruling: rustfmt was never run in Tasks 1-2 (`just lint` is clippy only); controller ran `cargo fmt -p yach-ui` as a separate change suvynkqq "Format theme module" (format-only, 1 file) — controller edit without review is acceptable for pure formatting — none. Later dispatches add `just fmt-check` to their gates.
Task 2: fix round 1/3 (1 addressed, 0 open — NOTICE attribution pinned per palette; changes 9e581a16..a7e566d1)
Task 2: minor (deferred): palette_test.rs:198-204 no longer checks the scheme cell is non-empty.
Task 2: complete (changes d4e0517c..a7e566d1, review clean) — changes ntpwzqtz, zwkwqmky
Task 3: change nvlrwzmlxnrn BASE 3a3a0ddc6898aed066d14380eea9dc0c7f5d2100
Ruling: abandoned stray change wwkxqmnk "Track the sjujperpowers provider config" (made at 11:24 by another agent session, not a theming task; AGENTS.md keeps planning config local, untracked). Local config.json restored and still ignored — keeps the stack to theming work — the maintainer may want it tracked elsewhere.
Task 3: BASE d7785d3d (rebased; was 3a3a0ddc)
Task 3: implementer T3Implementer-3 DONE (dfe676c0); review dispatched to T3Reviewer-3 with named risk: painted confirm labels gained bold.
Task 3: review 1 — spec ❌ (1 Important: selected_on adds BOLD in Painted mode; 4 overlay cells change NONE→BOLD). Reviewer T3Reviewer-3. Pin provenance confirmed (fixture made before migration).
Task 3: minor (deferred): render_entry_lines (transcript.rs 1163-1284) and model_selector render (36-208) exceed 100 lines; pre-existing; workspace allows too_many_lines.
Task 3: FIX_BASE dfe676c00b3e005e37c2ba34def0a06a2dc56cd6
Task 3: fix round 1/3 (1 addressed, 0 open — painted selection unbolded; changes dfe676c0..33fa3ad3)
Task 3: complete (changes d7785d3d..33fa3ad3, review clean) — changes nvlrwzml, ntuvpqsv
Task 4: change usxvzoqxutws BASE 33fa3ad352738a6a99fa9dcea071544188e34065
Task 4: implementer T4Implementer-3 DONE (d31ef253); size +72,448 bytes (+0.249%); review T4Reviewer-3
Task 4: review 1 — spec ❌ (1 Important: --resume with no session defers startup notices that never show, then dump after a later hydration). Reviewer T4Reviewer-3.
Task 4: minor (deferred): main.rs:565 project-theme notice fires when cwd is $HOME (project file == personal file).
Task 4: minor (deferred): main.rs:490-494/530-534 duplicate the auto appearance->preset mapping from select.rs; use resolve_base("auto", ...).
Task 4: minor (deferred): main.rs:497-505 empty YACH_THEME reported as unknown theme ''; treat empty as unset.
Task 4: FIX_BASE d31ef253
Task 4: fix round 1/3 (1 addressed, 0 open: deferral follows actual session availability; load-failure statuses flush notices; changes d31ef253..b3e8a8f4). Re-reviewer T4ReReviewC (task agent; reviewer agent type failed twice with "No model selected").
Task 4: minor (deferred): app.rs:1377-1379 matches backend status text prefixes with no cross-layer test pinning both sides.
Task 4: minor (deferred): a transient initial load failure followed by a successful retry would skip transcript hydration (expectation already cleared); untested.
Task 4: complete (changes 33fa3ad3..b3e8a8f4, review clean): changes usxvzoqx, lqktnkkk
Task 5: change kznxxstownzlyzomlrzzwpmpvktkwyvv BASE b3e8a8f4a7e02dde273812b4404cdd4a3c8af0df
Ruling: Task 4 re-review ran as a 'task' agent under reviewer instructions; the 'reviewer' agent type failed twice at startup ('No model selected'; config maps reviewer -> @worker_hard = claude-sonnet-5-5, served by the gateway). Later reviews use 'task' until that resolves. Review rubric unchanged.
Task 5: implementer T5Implementer-3 dispatched with rulings F7/F8/F9, F13, R1-R3, R9 (remove dark tool_output contrast exception), and the pin update allowed
Task 5: implementer T5Implementer-3 DONE (0a7d21ba); pin 507 cells changed
Ruling: Task 4 implementer force-tracked task-4-report.md (gitignored .sjujperpowers/*) in usxvzoqx and lqktnkkk; controller removed it from both commits with jj restore (local file kept). Task 4 content otherwise unchanged; Task 5 BASE is now 8c601fb0b79626126f8ad84bb28635c4e6f4933c. Later dispatches: never commit SDD files.
Task 5: review 1: spec FAIL (2 Important: tiny-width content loss; CRLF breaks summary/body role offsets). Reviewer T5Reviewer-3. R9 contrast 8.02:1 pending / 7.56:1 error; 507 pin cells independently audited, zero unexplained.
Ruling: T5 review Important 1 (widths 3-6 drop content to padding/indent) downgraded to Minor (deferred). The brief's acceptance for narrow terminals is no panic and no empty rows, which holds; widths under 10 are not a usable transcript, and degrading padding there adds layout branches for no real terminal. Revisit if a narrow-pane use case appears.
Ruling: T5 review Minor 3 (theme.rs:223-226 stale "until Task 5" doc) folded into fix round 1; it is a one-line doc correction in a file this task already touches.
Task 5: FIX_BASE b07cd1d2
Task 5: fix round 1/3 (1 Important addressed, 1 Minor folded in, 0 open: roles assigned per source line before wrapping; changes b07cd1d2..2d7dbef3). Re-reviewer T5ReReview. Controller ran 37 tool/pin/contrast tests: pass.
Task 5: minor (deferred): widths 3-6 lose title/summary content to padding/indent (ruled Minor above).
Task 5: complete (changes 8c601fb0..2d7dbef3, review clean): changes kznxxsto, opqvqlun
Task 6: change xurzpumxlwkznxtwvlvrsynkmuozvysl BASE 2d7dbef3cca6b9b7d5d22aeecb4a375c1070f816
Task 6: implementer T6Implementer-2 dispatched with rulings F12, R4/F18, laziness test; wiring point main.rs:564 resolve_tui_options
Task 6: implementer T6Implementer-2 DONE (720e59b6)
Task 6: review 1: spec FAIL (2 Important: non-regular theme file (FIFO/device) can block TUI startup before fallback; production wiring makes YACH_THEME=<ext>:<theme> select an extension theme directly, contrary to spec 514-517). Reviewer T6Reviewer-2. F12, R4/F18, laziness, containment, manifest compat: pass.
Ruling: Task 6 added an extension-theme section to docs/extensions.md:60-92, overlapping Task 7 Step 6. Keep it. Task 7 Step 6 amends that section in place (adds the explicit "not via YACH_THEME=<qualified-name>" warning, the README link and the theme-file key list) instead of appending a second Themes section after line 183. This avoids duplicate docs, and the text needs the selection fix from this round anyway.
Task 6: FIX_BASE 720e59b6
Task 6: fix round 1/3 (2 addressed, 0 open: is_file check before open with guarded FIFO/dir tests; bare YACH_THEME resolves built-ins only; changes 720e59b6..57aa6718). Re-reviewer T6ReReview ran the five focused tests: pass.
Task 6: complete (changes 2d7dbef3..57aa6718, review clean): changes xurzpumx, knqnnpom
Task 7: change krnptnxywprywuzrlouywzrpmntxwwzs BASE 57aa671848135ba118b488081da0668c604cba6c
Task 7: implementer T7Implementer dispatched with rulings F16/R10 (scanner proven both ways), Step 6 amend-in-place, F15 size wording; PNGs copied to sdd visual/
Task 7: VHS 0.12.0 on this host can't render: the unchanged session.tape times out at /no model/ with no composer or status bar on main 4de8fb28 and on 9e581a16 (before the widget work), and standalone Screenshot/GIF commands write no file (strace shows no output write and no ffmpeg exec). This environment problem predates the theming stack, so it isn't a Task 1-6 defect.
Ruling: Task 7 continues without VHS pixels. The tapes are written as the brief specifies and committed unrendered. That is labeled in the report and the PR, and someone has to render them on a working VHS host before relying on them. The legibility checks run instead on a tmux capture with escape sequences kept (`capture-pane -p -e`) of the same fixture launch, in light (COLORFGBG=0;15) and NO_COLOR configs. Each item is judged from the actual SGR colors, with contrast computed from the RGB values. check-nocolor.sh uses script(1), not VHS, so it runs as planned. The tradeoff: color evidence instead of pixel evidence, and tapes that haven't been run.
Ruling: the brief's "failed row shows ✗" expectation is wrong. The fixture's malformed_patch row is a harness outcome, which renders `!` (by design since Task 5). The legibility item checks for `!` plus a failure label.
Ruling: The Task 7 commit 5eef4652 tightened every tape's status wait to /no model \(run \/connect\)/, but the fixture's status bar paints `default · think:off ... invalid_config` (status_bar.rs:261-268; the backend resolves model `default` and gets InvalidConfig), so on any host those tapes hang deterministically. The /no model/ wait was already stale on main before this stack, which probably accounts for the "baseline session.tape fails on main" observation instead of, or alongside, VHS. Fix: all four tapes wait on /approval:review/, which the fixture status bar paints in every tmux capture. Then retry a real VHS render once. If PNGs come out, the VHS-environment ruling above is withdrawn and the legibility checks run on pixels.
Task 7: implementer T7Implementer DONE (5eef4652); fix 1/3 tape waits -> /approval:review/ (4560b355); VHS retry still no PNGs (session timed out at approval:review: composer/status not painted under VHS on this host). Size 29,064,208 -> 29,641,968 (+1.99%, budget 5%).
Task 7: review 1 (T7Reviewer): spec PASS, quality Approved, no findings. Reviewer reran check-nocolor.sh: exit 0. Scanner verified on 7 accept / 37 reject cases.
Task 7: parked: pixel legibility unverified until the tapes render on a working VHS host (VHS on this host doesn't paint the composer/status bar and writes no PNGs).
Task 7: complete (changes 57aa6718..4560b355): changes krnptnxy, mkyyooln
Final review (FinalReviewer-3): With fixes. 0 Critical, 5 Important, 2 Minor. Reviewer ran 349 tests across the crates: pass.
Ruling: Final review Important 2 (invalid ~/.yach/theme.json fails TUI startup) is not a defect. The personal file kept its strict pre-theming behavior: the spec's JSON-compat table says deny_unknown_fields and error handling are "Unchanged", and README:330-332 documents "fail before the TUI opens"; only explicit YACH_THEME files fall back. The reviewer's stricter policy came from the controller's review prompt, not the spec. Changing it would silently ignore a user's broken file. Not fixed; the owner can revisit.
Ruling: Final fix wave scope = Important 1 (selected_on action labels unreadable on bundled palettes; tokyo-night-light ~1.0:1), Important 3 (startup load failure consumes the hydration expectation of a later successful queued load; promoted from Task 4 deferred minor), Important 4 (parse_color byte-slices non-ASCII hex and panics), Important 5 (explicit YACH_THEME path and home file can block on a FIFO; apply the Task 6 is_file policy). Folded in because each is small and lives in code this stack already touches: Minor 1 (trim the yach-ui exports that have no consumer), Minor 2 (omission marker gets muted, per spec), and the deferred Task 4 minors: empty YACH_THEME treated as unset, no project-theme notice when the project file is the personal file, a single auto appearance->preset mapping. Deferred minors still open: palette_test scheme-cell check, long render functions, status-prefix coupling (unless the Important 3 fix removes it), tiny widths, unrendered tapes.
Final: FIX_BASE cbccb4f4 (head 4560b355)
Ruling: Final fix item 1. Measured current selected_on contrast: dark White on success 2.01, error 3.71, warning 1.70; light 2.76 / 2.28 / 2.90; worst palettes tokyo-night-light error 1.002, catppuccin-mocha success 1.03, catppuccin-macchiato warning 1.03. "Keep dark byte-identical" was Task 3's migration constraint, and it would keep a legibility bug that predates this stack. Painted selected_on keeps the role color as background and uses selectedText as foreground when that clears 4.5:1; otherwise it uses whichever of black (Rgb 0,0,0) or white (Rgb 255,255,255) has the higher ratio. The black/white maximum is at least about 4.58:1 against any color, so every pair clears 4.5:1. This is a declared visible change: the dark confirm/reject label cells change foreground (for example White on yellow becomes black on yellow). Pins may change only in those cells, and the PR must call it out for the owner.
Final: fix wave FinalFixer-2 DONE (75b2ed82, 7f21df50). Gates: fmt/lint pass; UI 323, backend 943, CLI 248 + 43 integration. 26 selected-label fg cells changed across four overlay fixtures; dark transcript pin unchanged.
Ruling: The fix wave amended the approved spec (docs/project/specs/2026-10-02-theming-engine-design.md: selected_on contrast rule, empty YACH_THEME is unset, explicit paths must be regular files, no project notice when the project file is the personal file) and added two pub const status prefixes to yach-proto (no wire change). Accepted: the spec records the behavior these rulings decided, so spec and code don't drift. The PR calls out the spec amendment for owner review.
Final re-review (FinalReReview): 6 of 7 addressed. Gates rerun: UI 323, CLI theme 12, backend focused 2, all pass.
Ruling: Residual Important parked for the owner (no second fix wave). The fix keeps the hydration expectation armed after a failed load so that failure-then-success still hydrates. When the only load fails for good (unreadable session file at startup or on /resume), the expectation stays armed, and the next session-tree response clears and rebuilds the transcript from the log, which drops startup notices and transient status rows (app.rs:1381-1386, 1511-1513; runner.rs:3044-3054). Before the fix, the reverse edge applied: transient failure then success showed no restored history. Both are failure-path edges. The current behavior loses no conversation content (the rebuild comes from the log), while the old one hid restored history, so the current one is the lesser harm. Proper fix: correlate load completion with the request (a typed load-result event or a request id) instead of matching status text. That touches the protocol and is out of scope for theming.
