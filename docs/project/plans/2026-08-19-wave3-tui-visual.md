# Wave 3 TUI Visual Implementation Plan

Date: 2026-08-19
Design: `docs/project/specs/2026-08-19-wave3-tui-visual-design.md`

## Goal

Implement the accepted OpenCode-hierarchy/Pi-directness visual pass: balanced transcript grouping, quiet successful tool rows, structural exceptional-state rails, and an inset responsive composer that preserves transcript reading position while typing.

## Slice 1: composer and bottom dock geometry

Files:

- `crates/yach-ui/src/input.rs`
- `crates/yach-ui/src/layout.rs`
- relevant unit tests in those modules

Changes:

1. Add one geometry function that computes the centered composer/status bounds:
   - two-column gutters from 40 columns upward;
   - maximum width 112;
   - full-width narrow fallback.
2. Compute textarea wrapping and height from that actual card width.
3. Reserve one row between transcript and composer, while keeping saturating narrow/short behavior.
4. Render the status bar inside the same horizontal bounds.
5. Restyle the composer as a restrained docked card:
   - concise `message` top title;
   - muted bottom key hints when width permits;
   - cyan focused title, dark-gray border, dim unfocused state;
   - current reversed/hidden focus cursor behavior preserved.
6. Detect content beyond the eight-row cap and mark the title without changing stored input.
7. Add geometry and buffer-render assertions for wide, maximum-width, narrow, wrapped, capped, focused, and unfocused cases.

Acceptance:

- composer never overlays transcript content;
- ultrawide input is capped at 112 columns and centered;
- narrow terminals use the available width;
- multiline input grows from 3 to 8 rows, then signals overflow;
- transcript viewport calculations exactly match render reservations.

## Slice 2: transcript hierarchy and semantic grouping

File:

- `crates/yach-ui/src/transcript.rs`

Changes:

1. Replace opposing conversation arrows with:
   - cyan `│ ` user rail and bright text;
   - two-column inset gray assistant prose.
2. Normalize tool rows to a four-column gutter and keep semantic state glyphs.
3. Use continuation rails for live output, review detail, expanded detail, and multiline exceptional states.
4. Keep successful compact result summaries muted; retain explicit failed/denied/cancelled/interrupted labels.
5. Replace unconditional trailing blank rows with semantic spacing:
   - one blank row between conversation blocks and before the first tool in a group;
   - no blank rows between adjacent tools;
   - no gratuitous final blank row.
6. Preserve render-cache, wrapping, bottom alignment, and global expansion behavior.
7. Update tests to assert semantic line content and spacing instead of the old glyph layout.

Acceptance:

- conversation hierarchy is visible without boxes or role labels;
- adjacent tools read as a compact group;
- expanded/review/error rows have a visible structural rail;
- existing bounded detail and review controls remain exact.

## Slice 3: scrolled-composer behavior and integration

File:

- `crates/yach-ui/src/app.rs`

Changes:

1. Add a regression test proving ordinary prompt editing does not change `scroll_offset` after PageUp.
2. Confirm prompt submission still appends the user row and follows the live turn.
3. Adjust only code required by the new layout/input APIs; do not broaden input or follow-mode semantics.

Acceptance:

- a user can type while reading older transcript content;
- submitting returns to the active turn exactly as before.

## Slice 4: project record and verification

Files:

- `docs/project/board.md`
- `docs/project/next.md`

Changes:

1. Mark the owner taste decisions and implemented Wave 3 visual/composer behavior only after actual-TUI verification.
2. Record any intentionally deferred visual issues rather than silently broadening this slice.

Verification:

1. Run focused `yach-ui` tests during implementation.
2. Launch the actual fixture TUI and exercise the states named in the design.
3. Run `just fmt-check`, `just lint`, and `just test` once after the smoke test.
4. Review the complete branch diff against the accepted design before checkpointing.
