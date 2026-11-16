# SDD ledger — plan: docs/project/plans/2026-10-18-composer-styles.md
Fix-round budget: 3
Kata parent: yach#0z2j
Task 1: Kata yach#sygx
Task 2: Kata yach#d4dh
Task 3: Kata yach#ps1h
Task 4: Kata yach#qwh8
Task 5: Kata yach#x5kq
Spec: docs/project/specs/2026-10-17-composer-styles-design.md
Start: main 5e8a5bd2; Task 1 change ssrnpqntyovquspnxtklzvzkzuurvlpm; baseline `just test` green
Preflight: scan table at preflight.md (9 pair rows, 5 self-consistency, 3 hazards; Theme is Copy, StatusMessage.severity pub — H3 cleared).
Ruling: R1 (T4) keep `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` on InputStyles when making it pub(crate) with the new `marker` field — plan snippet omitted the derive by accident — none.
Ruling: R2 (T1/T2/T4/T5) README line numbers drift after earlier tasks; implementers locate passages by content, not number — line refs are to 88431052 — doc edit lands in a neighbouring paragraph.
Ruling: R3 (all) implementer `jj commit` message is the plan's message plus a blank line and `Kata: yach#<child>` trailer — keeps the association starting-a-change wrote — a lost trailer.
Task 1: BASE 5e8a5bd27f466a290a0ea5ed7d5fef554ce267cf; dispatched implementer
Task 1: implementer Task1Implementer DONE, change ssrnpqnt aab99638; review package review-5e8a5bd27f46..073757db2100.diff; reviewer Task1Reviewer dispatched
Task 1: ⚠️ resolved by controller: 18/18 palettes statusBackground == toolPendingBackground (jq), NOTICE.md untouched, Kata trailer present.
Task 1: minor (deferred): README token-list line and docs/extensions.md schema line exceed ~80-col wrap; reflow.
Task 1: minor (deferred): contrast.rs band `assert!(is_some)` + `unwrap_or(Reset)` pattern (brief-mandated) could be `let Some .. else`.
Task 1: complete (changes 5e8a5bd2..aab99638, review clean)
Task 2: change rpntrllvkzssprklzyouvrwkmvyttuup; BASE aab99638df8f033a71354a3ead3372ea7ddbbfae
Task 2: ⚠️ resolved by controller: Kata trailer present; sole production caller main.rs:644 propagates `?` (broken file under NO_COLOR now errors, per spec); RED/GREEN evidence in report.
Task 2: minor (deferred): theming spec item 1 still reads "overrides every theme source" above the amendment; consider inline wording.
Task 2: complete (changes aab99638..7b0bf813, review clean)
Task 3: change kptwkvwwwpzvknnqtrmmupwxokqnnkts; BASE 7b0bf8137d16b761ba2ef40178d9cd780000d06a
Ruling: R4 (T3) StatusFill::Rule/Band dead in non-test code until Task 4 fails `just lint` — Task 3 gives layout.rs a real consumer: fill chosen from theme.composer_style()/status_band() exactly as Task 4 plans (Box→None, Band+Some→Band, else Rule(theme.border(false))), marker None; Task 4 replaces it with the input_styles-based version — no allow attributes / no fake uses; spec style already parses — rule/band themes show the status fill one task before the composer layout lands (intermediate stack state only).
Task 3: ⚠️ resolved by controller: composer_style/status_band exist (Task 1, aab99638); band contrast covered in Task 1.
Task 3: minor (deferred, carried to Task 4 dispatch): status_bar.rs render marker_x can sit left of area.x when marker wider than area; clamp to area.x.
Task 3: minor (deferred): no narrow-width (<4 col / zero-width / marker wider than row) status render test.
Task 3: minor (deferred): layout fill selection untested directly; Task 4 band_without_a_band_background_draws_rule covers it.
Task 3: complete (changes 7b0bf813..29f91390, review clean)
Task 4: change nsnkuqyzuzltrwqwzuktvpqxvsstrlut; BASE 29f9139051d27a87611a1a0ad43cce06d4405e61
Ruling: R5 (T4) fold deferred T3 marker-clamp minor into Task 4 (first real marker): clamp marker x to area.x and cap width at area.width; status_bar.rs added to T4 paths — unreachable before T4 — one line.
Task 4: ⚠️ resolved by controller: README example JSON parses and loads (smoke: real `yach tui --backend fixture` with it in ~/.yach/theme.json renders rule layout; hint only on empty draft). Smoke also: band on dark paints 40,40,50 with bold text model; band+NO_COLOR draws rule monochrome; nord box unchanged.
Task 4: minor (deferred): unfocused status-row marker (band muted / rule dim title) untested; layout draw() hardcodes focused.
Task 4: minor (deferred): empty-draft cursor HIDDEN/REVERSED on open styles untested.
Task 4: minor (deferred): open-style hint sits two cells after cursor (cursor cell + OPEN_HINT leading space); brief-mandated.
Task 4: complete (changes 29f91390..0f30c34f, review clean)
Task 5: change opurxuqslpnpqrwyvrpqvutxnkkwyvru; BASE 0f30c34f7f90aa1c6410d0b498729bc314848ed4
Ruling: R6 (T5) before/after captures and PR screenshots move from Task 5 Step 5 to controller publication step — they need the base revision checked out and pr-screenshots hosting, outside a task change — none.
Task 5: controller visual check: grids from composer-check used proportional DejaVuSans (fc-match in devenv) → box borders misaligned in PNG only (.ansi rows exact, 120/80 cols); re-ran with YACH_SHOT_FONT=<dejavu_fonts>/DejaVuSansMono.ttf → target/tui-themes/composer-mono, all styles correct. Use YACH_SHOT_FONT for PR screenshots.
Task 5: review Needs fixes — 1 Important (theme-grid.sh launch env not shell-escaped; spaced TMPDIR breaks). Fix round 1 FIX_BASE 8bc3c5031c91a65f93a4a74ccf52ae03e3076b4e; resumed Task5Implementer.
Task 5: fix round 1/3 (1 addressed, 0 open — theme env shell-escaped; changes 8bc3c503..ed40bc45)
Task 5: minor (deferred): `just test` stdout carries four pre-existing warning diagnostics from malformed/legacy fixture tests (not from this stack).
Task 5: minor (deferred): devenv fc-match resolves proportional DejaVuSans; captures need YACH_SHOT_FONT=DejaVuSansMono.ttf for aligned box borders (pre-existing).
Task 5: complete (changes 0f30c34f..ed40bc45, review clean after 1 fix round)
Final review: With fixes — 1 Important (rule/band panic at terminal height 1-2: render_open/closing rule write into zero-height rect), minors: README/extensions reflow, render_open marker clip to area.width. Fix wave FIX_BASE ed40bc45b462bb02e92eb24605d0957a043a1c54.
Final fix wave: 3 addressed, 0 open (tiny-terminal panic guarded + test, marker clipped to area, README/extensions reflow); changes ed40bc45..3225966d.
Final review residual minors left per reviewer triage: theming spec item 1 wording, contrast let-else style, narrow-width status test, unfocused marker test, empty-draft cursor test, hint spacing (brief-mandated), pre-existing fixture warnings, devenv proportional font for captures.
Verdict: medium, behavior-tested, current at 5df89d8c (hk-check, just test, 4 verify-yach drives; evidence /tmp/vbr-composer.tPTz + ~/.local/state/sjujperpowers/evidence/yach/20261018T2018*).
Finish: PR https://github.com/codyw912/yach/pull/319; Codex clean (issuecomment-6254779835); Kata children + parent 0z2j closed with --pr. Workspace retained (PR outcome).

CI follow-up (2026-10-18): perf gates failed 3 headless #alloc_bytes rows at c1ca26df; bisect blamed Task 4 0f30c34f (second Layout solve in transcript_viewport_size). Fixed in kyqluwxl b896024a (row arithmetic). Deterministic A/B: all alloc rows improved vs main. Codex P1 (unsupported perf claim) addressed by citing CI runs; Codex clean at b896024a. Perf (latency) startup row failed once at b896024a (+17.2%, comment-only diff vs passing 85e74814); rerun denied (403).
