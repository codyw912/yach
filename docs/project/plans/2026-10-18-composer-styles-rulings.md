# Rulings: 2026-10-18-composer-styles

- Ruling: R1 (T4) keep `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` on InputStyles when making it pub(crate) with the new `marker` field — plan snippet omitted the derive by accident — none.
- Ruling: R2 (T1/T2/T4/T5) README line numbers drift after earlier tasks; implementers locate passages by content, not number — line refs are to 88431052 — doc edit lands in a neighbouring paragraph.
- Ruling: R3 (all) implementer `jj commit` message is the plan's message plus a blank line and `Kata: yach#<child>` trailer — keeps the association starting-a-change wrote — a lost trailer.
- Ruling: R4 (T3) StatusFill::Rule/Band dead in non-test code until Task 4 fails `just lint` — Task 3 gives layout.rs a real consumer: fill chosen from theme.composer_style()/status_band() exactly as Task 4 plans (Box→None, Band+Some→Band, else Rule(theme.border(false))), marker None; Task 4 replaces it with the input_styles-based version — no allow attributes / no fake uses; spec style already parses — rule/band themes show the status fill one task before the composer layout lands (intermediate stack state only).
- Ruling: R5 (T4) fold deferred T3 marker-clamp minor into Task 4 (first real marker): clamp marker x to area.x and cap width at area.width; status_bar.rs added to T4 paths — unreachable before T4 — one line.
- Ruling: R6 (T5) before/after captures and PR screenshots move from Task 5 Step 5 to controller publication step — they need the base revision checked out and pr-screenshots hosting, outside a task change — none.

## Over fix-round budget

None.
