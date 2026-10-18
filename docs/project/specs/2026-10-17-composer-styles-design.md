# Composer Styles

**Outcome:** linear:CEW-266 (kata `yach#cwte`)

Status: approved by the owner, 2026-10-18. Open questions resolved the same
day: `NO_COLOR` keeps the theme file's layout (option b below), and the key
hint shows only while the draft is empty.

Triggered by a TUI comparison on 2026-10-17 of yach, Codex, Claude Code, omp,
pi, and OpenCode v2, each captured idle, with a two-line draft, and after a
turn at 80 columns. Yach's composer was the busiest: a full bordered box, a key
hint in the frame at all times, and up to four bright colors in the status row
while nothing is wrong. Codex uses three colors in that row, Claude Code and pi
two; omp shows a quiet status line, and OpenCode a colored left bar beside a
shaded input. Owner direction, 2026-10-17: likes mockups B (status rule above,
closing rule below) and D (shaded status band above), undecided between them,
and wants the choice configurable "like omp", or at least a few options.

This spec builds on the status-row changes in PR #311: segment priorities
model 100, approval 99, context 98, status 95, and status text styled by
severity. Line references are to `main` after #314.

## Problem

1. **No choice.** The composer frame, the status row position, and the key
   hint are fixed in code (`crates/yach-ui/src/input.rs:57-83`,
   `crates/yach-ui/src/layout.rs:43-90`). The theme file controls colors and
   transcript spacing only (`crates/yach-ui/src/theme.rs:459-487`).
2. **Color on healthy states.** `segment_style`
   (`crates/yach-ui/src/status_bar.rs:255-294`) paints the connected dot
   green, every approval mode yellow or red, the model and reviewer in the
   accent color, and compaction in the harness color. A healthy idle screen
   shows four hues, so the ones that mean "look here" do not stand out.
3. **Errors lost at 80 columns.** Context (98) outranks the status message
   (95). With full-access on and a long model segment such as
   `gpt-5.1-codex-max · think:xhigh`, an 80-column bar keeps the context meter
   and drops a provider error, because fewer than 12 cells remain for it.
4. **Rows.** The box costs five rows for a one-line prompt: gap, top border,
   prompt, bottom border, status row.

## Non-goals

- No status segment selection or ordering (omp's `leftSegments` /
  `rightSegments`). Yach has eight segments with fixed priorities; that
  surface can follow if a real need appears.
- No in-TUI style switching or live reload. A style is fixed per process, like
  the theme (`2026-10-02-theming-engine-design.md`, Non-goals).
- No change to the default style. `box` stays the default until the owner picks
  another from real use; changing the default is a one-line follow-up.
- No `config.toml` key and no new environment variable. The style lives in the
  theme file, next to `spacing`.
- No change to what any segment says, only to color, position, and which
  segment survives on a narrow row.

## Design

### Setting

The theme file gains an optional `composer` object:

```json
{
  "base": "nord",
  "composer": { "style": "rule" }
}
```

- `style` is one of `box`, `rule`, `band`. Default `box`.
- Unknown keys or values fail like any other theme file error: a personal
  `~/.yach/theme.json` fails before the TUI opens; an explicit `YACH_THEME`
  file falls back with a notice (`README.md` Theme section).
- Precedence matches `spacing`: a user file overrides the base it names. An
  extension theme may set `composer`; it applies only when the user names that
  theme as `base`, the same consent rule as its colors
  (`2026-10-02-theming-engine-design.md`, Extension control).
- A theme file carrying `composer` fails to parse on an older yach
  (`deny_unknown_fields`), as any new theme field does.
- Under `NO_COLOR`, the theme keeps its layout and loses its colors. See
  `NO_COLOR` below.

`Theme` gains `pub composer: ComposerStyle` (`Box`, `Rule`, `Band`), set to
`Box` by every preset and bundled palette and overridden by
`ThemeDocument::apply`, in the same way as `ThemeSpacing`.

### `NO_COLOR`

Today `NO_COLOR` returns `Theme::monochrome()` before reading any theme
source (`crates/yach-cli/src/main.rs:498-508`), so a user who chose `rule`
would get `box` under `NO_COLOR`. The theme file's `spacing` is already
discarded the same way. The `NO_COLOR` standard covers color only
(https://no-color.org/), and layout is not color.

New rule: `resolve_theme` resolves the theme from its sources exactly as it
does without `NO_COLOR`. When `NO_COLOR` is set and non-empty, it then returns
`Theme::monochrome()` carrying the resolved theme's `spacing` and `composer`.
Consequences:

- Colors and mode are always monochrome under `NO_COLOR`, as today. `band`
  renders as `rule` because monochrome has no `statusBackground`.
- Theme source errors behave the same with or without `NO_COLOR`: a broken
  `~/.yach/theme.json` fails before the TUI opens, and an explicit
  `YACH_THEME` failure or unknown name shows the fallback notice. Today
  `NO_COLOR` hides these errors.
- This amends theming spec `NO_COLOR` semantics §1 ("overrides every theme
  source") to "overrides every theme color". The implementation adds a
  one-line note at that section pointing here, and updates the `NO_COLOR`
  and `YACH_THEME` rows of the `README.md` environment table and its
  "TUI appearance" paragraph, which say `NO_COLOR` overrides the theme
  sources.
- `resolve_theme_no_color_overrides_every_source`
  (`crates/yach-cli/src/main.rs:6566`) changes: it asserts monochrome colors
  and mode with the file's `spacing` and `composer`, and that a broken
  personal file is an error under `NO_COLOR` as well. Its check that an empty
  `NO_COLOR` is ignored stays.

### Styles

Rows from top to bottom. "Prompt rows" are the textarea, 1 to 6 rows,
wrapped at width − 2 in every style, so `input_metrics` keeps one wrap width.

| Style | Rows below the transcript | Rows for a one-line prompt |
|---|---|---|
| `box` | gap, top border, prompt rows, bottom border, status row | 5 |
| `rule` | status rule, prompt rows, closing rule | 3 |
| `band` | status band, prompt rows | 2 |

**`box`** is today's layout, unchanged apart from the shared changes below.

**`rule`** draws the status segments at the left of the top row, followed by a
space and `─` to the right edge. Prompt rows start with `› ` (the transcript's
user-message prefix); continuation rows are indented two cells. A full-width
`─` row closes the composer.

**`band`** fills the top row with the new `statusBackground` color and draws
the status segments on it, with one leading space. Prompt rows are the same as
`rule`. There is no closing row. When `statusBackground` resolves to
`default`/`Color::Reset`, or the theme mode is `system` or monochrome, `band`
renders as `rule`, since a band with no background is just a row of text.

In `rule` and `band`:

- The `running` and `more ↑` markers that `box` puts in its top border appear
  at the right end of the status row. Their width is taken from the segment
  budget before fitting.
- Rule glyphs and the `›` marker use `theme.border(false)`; the marker is bold.
  An unfocused terminal dims both and hides the cursor, as `input_styles` does
  for the box today (`crates/yach-ui/src/input.rs:36-55`).
- The textarea renders without a block into the prompt area offset two
  columns, so wrap-aware cursor movement still sees the area it renders into.

`layout::transcript_viewport_size` and `layout::render` take their reserved
rows from one per-style function, so the viewport and the drawn layout cannot
disagree.

### New color token

`statusBackground`: painted presets and every bundled palette set it to their
`toolPendingBackground` value; `system` and monochrome use `Color::Reset`.
Theme files may override it. It is documented with the other tokens in
`README.md`.

### Restrained status colors (every style)

| Segment | Today | New |
|---|---|---|
| Connection `●` | green connected, yellow connected with error, red disconnected | hidden while connected and the status is not an error; yellow connected with error; red disconnected |
| Model | accent | accent, bold |
| Approval `review`, `accept-edits`, `auto-review` | yellow | muted |
| Approval `full-access` | red | red, bold |
| Reviewer `auto:<id>` | accent | muted |
| Context | dim < 70%, yellow 70-89%, red ≥ 90% | unchanged |
| Compaction `⟲N` | harness | muted |
| Tokens | muted | unchanged |
| Status | muted / yellow / red by severity | unchanged |

Monochrome keeps its mapping: muted and dim become `DIM`, accent and error
`BOLD`. The connection dot follows the same hidden-while-healthy rule.

On the `band` row, `dim` and `accent` do not reliably reach 3:1 against
`statusBackground`: `dim` measures 1.36-3.36 across the bundled palettes, and
`accent` measures 2.74 on `gruvbox-dark`. The band therefore draws the model
in bold `text`, and anything the table colors `dim` (context below 70%,
separators) in `muted`. `text`, `muted`, `warning`, and `error` clear 3:1 on
`toolPendingBackground` in every bundled palette (lowest: error 3.01 on
`solarized-dark`).

### Alerts outrank context

Priorities become: model 100, approval 99, **warning or error status 98**,
context 97, info status 95, reviewer 85, connection 80, compaction 60,
tokens 50. An info status keeps today's position below context. The existing
truncation rule is unchanged: the status is shortened, not dropped, while at
least 12 cells fit (`MIN_STATUS_WIDTH`).

Acceptance case: 80 columns, full-access, context 78% of 200k, model
`gpt-5.1-codex-max · think:xhigh`, provider error `invalid_config; set the
provider environment and relaunch yach tui`. In every style the row shows the
model, `approval:full-access`, and at least 12 cells of the error. With the
#311 priorities the error is dropped in this case; with
`claude-opus-5-5 · think:high` it already survives, so that model does not
exercise the change.

### Key hint

`enter send · ctrl+j newline` shows only while the draft is empty, in every
style: `box` keeps it in the bottom border (still only at width ≥ 42); `rule`
and `band` show it dimmed after the empty prompt's cursor. The help overlay
already lists both keys (`crates/yach-ui/src/help_overlay.rs:45`).

### Preview

`just tui-themes` (`tests/visual/theme-grid.sh`) adds a composer section: each
style over `dark`, `light`, and `system`, at 120 and 80 columns. Each capture
writes a temporary theme file `{"base": "<theme>", "composer": {"style":
"<style>"}}` and points `YACH_THEME` at it, so no new environment variable is
needed. The `system` captures show `band` falling back to `rule`.

## Testing

- **Theme parsing:** `composer.style` accepts the three names, rejects other
  values and unknown keys; a user file overrides an extension base's style;
  omitted means `box`.
- **`NO_COLOR`:** with a personal file setting `rule` and custom spacing,
  `NO_COLOR` yields monochrome colors and mode with that style and spacing;
  a broken personal file is an error with or without `NO_COLOR`; an empty
  `NO_COLOR` is ignored.
- **Layout:** for each style and draft heights 1, 3, and 7 (overflow),
  `transcript_viewport_size` equals the rows `render` leaves the transcript;
  the wrapped glyph count matches `input_metrics` (extends
  `composer_glyph_wrap_matches_metrics_for_a_long_token` to every style).
- **Band fallback:** `band` under `system`, monochrome, and a theme whose
  `statusBackground` is `default` renders the same buffer as `rule`.
- **Focus:** unfocused `rule` and `band` hide the cursor and dim the marker.
- **Colors:** the table above, per segment and state, including the dot
  hidden while healthy and shown on a connected error.
- **Priorities:** the 80-column acceptance case in every style; an info status
  still drops before context; full-access still outlasts every non-model
  segment (`full_access_outlasts_every_non_model_segment`).
- **Contrast:** on `statusBackground`, `text` reaches 4.5:1 and `muted`,
  `warning`, and `error` reach 3:1 for every painted preset and bundled
  palette, alongside the existing panel checks.
- **Visual:** the PR includes `just tui-themes` before/after grids, per
  `AGENTS.md`.

## Decisions

1. **`NO_COLOR` and layout** (owner, 2026-10-18): option (b), keep layout and
   drop colors, as specified under `NO_COLOR`. Rejected: (a) keep the
   current rule, which changes layout when `NO_COLOR` is set; (c) a
   `[tui] composer` key in `~/.yach/config.toml`, which splits a theme's
   look across two files and leaves `spacing` discarded.
2. **Key hint** (owner, 2026-10-18): only while the draft is empty, in every
   style. Rejected: always visible (today), help screen only.
