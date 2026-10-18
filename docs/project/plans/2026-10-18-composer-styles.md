# Composer Styles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a theme file choose the composer layout (`box`, `rule`, `band`),
calm the status row's colors, let warnings and errors outrank the context
meter, show the key hint only on an empty draft, and keep a theme's layout
under `NO_COLOR`.

**Architecture:**
- `crates/yach-ui/src/theme.rs` owns the setting: `Theme.composer:
  ComposerStyle`, parsed from the theme file's `composer.style` and applied
  like `spacing`, plus a `statusBackground` color token.
  `Theme::composer_style()` returns the style a theme can actually draw
  (`band` without a band background becomes `rule`), so no widget repeats
  the fallback.
- `crates/yach-ui/src/status_bar.rs` gains a `StatusFill` (plain, rule, band)
  and an optional right-hand marker. It owns segment colors, priorities and
  fitting for every style.
- `crates/yach-ui/src/layout.rs` gets one `composer_areas` function that both
  `transcript_viewport_size` and `render` call, so the viewport and the drawn
  layout cannot disagree. `crates/yach-ui/src/input.rs` draws the box or the
  open (`›`-prefixed) prompt.
- `crates/yach-cli/src/main.rs` resolves the theme from its sources first,
  then applies `NO_COLOR` as "monochrome colors, same layout".

**Tech Stack:** Rust (ratatui 0.30, ratatui-textarea 0.9.1, serde), bash +
tmux + Node + Chromium for `just tui-themes`.

**Spec:** `docs/project/specs/2026-10-17-composer-styles-design.md`
(approved 2026-10-18).

**Source:** linear:CEW-266

## Global Constraints

- Workspace clippy: pedantic warn; `unwrap_used`, `expect_used`, `panic`,
  `panic_in_result_fn`, `allow_attributes`, `print_stdout`, `print_stderr`,
  `dbg_macro`, `todo`, `unimplemented` deny. `clippy.toml`:
  `too-many-lines-threshold = 100`, `cognitive-complexity-threshold = 15`.
  Tests follow each file's neighbouring test style (no `unwrap`; use
  `assert!(x.is_ok()); let Ok(x) = x else { return };` or `Result` tests).
- `style` is one of `box`, `rule`, `band`. Default `box`. `box` stays the
  default; this plan does not change it.
- No `config.toml` key and no new environment variable. The style lives in
  the theme file, next to `spacing`.
- A style is fixed per process: no in-TUI switching, no live reload.
- No change to what any segment says, only to color, position, and which
  segment survives on a narrow row.
- `NO_COLOR` present and non-empty: colors and mode are monochrome; `spacing`
  and `composer` come from the theme resolved exactly as without `NO_COLOR`;
  theme source errors and notices are the same with or without `NO_COLOR`.
- Contrast on `statusBackground`: `text` at least 4.5:1; `muted`, `warning`,
  `error` at least 3:1, for every painted preset and bundled palette.
- Priorities: model 100, approval 99, warning or error status 98, context 97,
  info status 95, reviewer 85, connection 80, compaction 60, tokens 50.
  `MIN_STATUS_WIDTH` (12) truncation rule unchanged.
- Line references are to `main` at `88431052`.
- Commit each task with `jj commit <paths> -m "<message>"` naming the files
  the task touched; never a bare `jj commit`.
- Run Rust commands through `just dev ...` (or `just test` / `just lint`).

---

### Task 1: Composer setting and status background token

**Files:**
- Modify: `crates/yach-ui/src/theme.rs` (`Theme` 42-47, `ThemeColors` 49-73,
  helpers after 165, `set_color` 323-351, `ThemeDocument` 409-456,
  `ThemeFile` 458-470, tests 539-)
- Modify: `crates/yach-ui/src/theme/presets.rs` (four presets, 5-156)
- Modify: `crates/yach-ui/src/theme/contrast.rs:25-101`
- Modify: `crates/yach-ui/src/theme/palette_test.rs:8-31`
- Modify: `crates/yach-ui/src/theme/select.rs` (tests, after 153)
- Modify: `crates/yach-ui/themes/*.json` (18 palettes)
- Modify: `crates/yach-ui/src/lib.rs:26-30`
- Modify: `README.md:358-362`, `docs/extensions.md:80-81`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  // crates/yach-ui/src/theme.rs, re-exported from yach_ui
  #[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
  #[serde(rename_all = "lowercase")]
  pub enum ComposerStyle { #[default] Box, Rule, Band }

  pub struct Theme {
      colors: ThemeColors,          // gains status_background: Color
      pub spacing: ThemeSpacing,
      pub composer: ComposerStyle,  // the requested style
      mode: ThemeMode,
  }
  impl Theme {
      /// Band background, or None for system, monochrome, or `default`.
      pub fn status_band(&self) -> Option<Color>;
      /// The style this theme can draw: Band without a band becomes Rule.
      pub fn composer_style(&self) -> ComposerStyle;
  }
  ```
  Theme file: `"composer": { "style": "box" | "rule" | "band" }`; color token
  `statusBackground`.

- [ ] **Step 1: Write the failing theme tests**

Add to the `tests` module of `crates/yach-ui/src/theme.rs` (extend its
`use super::{...}` with `ComposerStyle`):

```rust
    #[test]
    fn composer_style_parses_names_and_defaults_to_box() {
        for (json, expected) in [
            (r"{}", ComposerStyle::Box),
            (r#"{"composer":{}}"#, ComposerStyle::Box),
            (r#"{"composer":{"style":"box"}}"#, ComposerStyle::Box),
            (r#"{"composer":{"style":"rule"}}"#, ComposerStyle::Rule),
            (r#"{"composer":{"style":"band"}}"#, ComposerStyle::Band),
        ] {
            let theme = ThemeDocument::parse(json).and_then(|document| document.apply(Theme::dark()));
            assert_eq!(theme.ok().map(|theme| theme.composer), Some(expected), "{json}");
        }
    }

    #[test]
    fn composer_rejects_unknown_styles_and_keys() {
        for json in [
            r#"{"composer":{"style":"boxed"}}"#,
            r#"{"composer":{"style":"Rule"}}"#,
            r#"{"composer":{"position":"top"}}"#,
            r#"{"composer":"rule"}"#,
        ] {
            assert!(
                matches!(ThemeDocument::parse(json), Err(ThemeLoadError::Json(_))),
                "{json}"
            );
        }
    }

    #[test]
    fn band_needs_a_painted_status_background() {
        assert_eq!(Theme::dark().status_band(), Some(Color::Rgb(40, 40, 50)));
        assert_eq!(Theme::light().status_band(), Some(Color::Rgb(236, 236, 242)));
        let mut band = Theme::dark();
        band.composer = ComposerStyle::Band;
        assert_eq!(band.composer_style(), ComposerStyle::Band);
        for base in [Theme::system(), Theme::monochrome()] {
            let mut theme = base;
            theme.composer = ComposerStyle::Band;
            assert_eq!(theme.status_band(), None, "{:?}", base.mode());
            assert_eq!(theme.composer_style(), ComposerStyle::Rule, "{:?}", base.mode());
        }
        for (json, expected) in [
            (
                r#"{"colors":{"statusBackground":"default"},"composer":{"style":"band"}}"#,
                None,
            ),
            (
                r##"{"colors":{"statusBackground":"#123456"},"composer":{"style":"band"}}"##,
                Some(Color::Rgb(18, 52, 86)),
            ),
        ] {
            let theme = ThemeDocument::parse(json).and_then(|document| document.apply(Theme::dark()));
            assert!(theme.is_ok(), "{json}");
            let Ok(theme) = theme else { return };
            assert_eq!(theme.status_band(), expected, "{json}");
            let style = if expected.is_some() { ComposerStyle::Band } else { ComposerStyle::Rule };
            assert_eq!(theme.composer_style(), style, "{json}");
        }
    }
```

In `legacy_file_with_all_22_tokens_round_trips` (theme.rs:675-738), after the
spacing assertion, add (a 22-token file keeps its meaning):

```rust
        assert_eq!(theme.colors.status_background, Theme::dark().colors.status_background);
        assert_eq!(theme.composer, ComposerStyle::Box);
```

In `monochrome_is_reset_plus_the_modifier_table` (theme.rs:926-), add
`colors.status_background,` to the color list after `colors.harness,`.

Add to `crates/yach-ui/src/theme/select.rs` tests (after `OneTheme`, extend
imports with `use crate::theme::{ComposerStyle, ThemeDocument};`):

```rust
    #[test]
    fn extension_composer_applies_only_until_the_user_file_sets_one() -> Result<(), String> {
        let extension = OneTheme(r#"{"base":"dark","composer":{"style":"band"}}"#);
        let base = resolve_base("nightowl:dim", Appearance::Dark, &extension)
            .map_err(|error| format!("{error:?}"))?;
        for (json, expected) in [
            (r#"{"base":"nightowl:dim"}"#, ComposerStyle::Band),
            (
                r#"{"base":"nightowl:dim","composer":{"style":"rule"}}"#,
                ComposerStyle::Rule,
            ),
        ] {
            let theme = ThemeDocument::parse(json)
                .and_then(|document| document.apply(base))
                .map_err(|error| format!("{json}: {error}"))?;
            if theme.composer != expected {
                return Err(format!("{json}: {:?} != {expected:?}", theme.composer));
            }
        }
        Ok(())
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `just dev cargo test -p yach-ui --lib theme::`
Expected: FAIL to compile with "cannot find type `ComposerStyle`" and "no
field `status_background`".

- [ ] **Step 3: Implement the setting and token**

In `crates/yach-ui/src/theme.rs`, above `pub struct Theme`:

```rust
/// How the composer and status row are laid out. Layout, not color, so it
/// survives `NO_COLOR`.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ComposerStyle {
    #[default]
    Box,
    Rule,
    Band,
}
```

Add `pub composer: ComposerStyle,` to `Theme` after `spacing`, and
`status_background: Color,` to `ThemeColors` after `harness`. In
`presets.rs`, add `status_background:` with the preset's
`tool_pending_background` value (dark `Color::Rgb(40, 40, 50)`, light
`Color::Rgb(236, 236, 242)`, system and monochrome `Color::Reset`) and
`composer: ComposerStyle::Box,` after each `spacing` block; import
`ComposerStyle` from `super`.

Add the `set_color` arm before the fallback:

```rust
            "statusBackground" => colors.status_background = color,
```

Add the helpers after `harness()`:

```rust
    /// The status band's background, or `None` when this theme cannot paint
    /// one: `system`, monochrome, or `statusBackground` set to `default`.
    #[must_use]
    pub fn status_band(&self) -> Option<Color> {
        (self.mode == ThemeMode::Painted && self.colors.status_background != Color::Reset)
            .then_some(self.colors.status_background)
    }

    /// The composer style this theme can draw. A band with no background is
    /// just a row of text, so `band` falls back to `rule`.
    #[must_use]
    pub fn composer_style(&self) -> ComposerStyle {
        match self.composer {
            ComposerStyle::Band if self.status_band().is_none() => ComposerStyle::Rule,
            style => style,
        }
    }
```

Parse and apply the `composer` object:

```rust
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ComposerOverrides {
    style: Option<ComposerStyle>,
}
```

Add `#[serde(default)] composer: ComposerOverrides,` to `ThemeFile`,
`composer: ComposerOverrides,` to `ThemeDocument`, `composer: file.composer,`
in `ThemeDocument::parse`, and in `ThemeDocument::apply` after
`base.spacing.apply(&self.spacing);`:

```rust
        if let Some(style) = self.composer.style {
            base.composer = style;
        }
```

Re-export from `crates/yach-ui/src/lib.rs`: add `ComposerStyle` to the
`pub use theme::{...}` list.

- [ ] **Step 4: Run the theme tests**

Run: `just dev cargo test -p yach-ui --lib theme::`
Expected: the new tests PASS. `bundled_palette_files_parse_and_cover_every_token`
still passes (palettes do not set the new token yet; the next step makes it
required).

- [ ] **Step 5: Require the token in every palette and check its contrast**

In `crates/yach-ui/src/theme/palette_test.rs`, change `COLOR_TOKENS` to
`[&str; 23]` and append `"statusBackground",` after `"harness",`.

Set every palette's band to its pending panel (the spec's measured pairs):

```bash
for f in crates/yach-ui/themes/*.json; do
  jq '.colors.statusBackground = .colors.toolPendingBackground' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done
jj diff --stat crates/yach-ui/themes
```

Expected: 18 files, each `2 insertions(+), 1 deletion(-)` (a trailing comma
on `harness` plus the new line); `NOTICE.md` untouched.

In `crates/yach-ui/src/theme/contrast.rs` `assert_preset_contrast`, after
the `text on error panel` assertion:

```rust
    let band = theme.status_band();
    assert!(band.is_some(), "painted theme has no status band");
    let band = band.unwrap_or(Color::Reset);
    assert_contrast_at_least("text on status band", text, band, 4.5);
    for (name, style) in [
        ("muted", theme.muted()),
        ("warning", theme.warning()),
        ("error", theme.error()),
    ] {
        let foreground = style.fg.unwrap_or(Color::Reset);
        assert_contrast_at_least(&format!("{name} on status band"), foreground, band, 3.0);
    }
```

- [ ] **Step 6: Document the token and field**

`README.md:358-362`: add `statusBackground` to the token list
(`` `diffContext`, `diffHunk`, `harness`, and `statusBackground`. ``).
`docs/extensions.md:80-81`: "The file uses the user theme JSON schema:
`base`, `vars`, `colors`, `spacing`, and `composer` (see ...)".

- [ ] **Step 7: Run the crate tests**

Run: `just dev cargo test -p yach-ui --lib theme`
Expected: PASS, including `dark_and_light_paint_explicit_pairs_at_spec_contrast`
and `bundled_palette_files_parse_and_cover_every_token`.

- [ ] **Step 8: Commit**

```bash
jj commit crates/yach-ui/src/theme.rs crates/yach-ui/src/theme crates/yach-ui/themes crates/yach-ui/src/lib.rs README.md docs/extensions.md -m "Add composer style setting and statusBackground token"
```

---

### Task 2: `NO_COLOR` keeps the theme's layout

**Files:**
- Modify: `crates/yach-cli/src/main.rs:498-535` (`resolve_theme`),
  `:6565-6590` (test)
- Modify: `docs/project/specs/2026-10-02-theming-engine-design.md:605-608`
- Modify: `README.md:186-187`, `README.md:258-267`

**Interfaces:**
- Consumes: `yach_ui::ComposerStyle`, `Theme.composer`, `Theme.spacing`
  (Task 1).
- Produces: `resolve_theme(env, extensions)` unchanged signature; under
  `NO_COLOR` returns `Theme::monochrome()` with the resolved `spacing` and
  `composer`, plus the same notices and errors as without `NO_COLOR`.

- [ ] **Step 1: Replace the `NO_COLOR` test**

Replace `resolve_theme_no_color_overrides_every_source`
(`crates/yach-cli/src/main.rs:6565-6590`) with the test below. Add
`ComposerStyle` to the `yach_ui` import of the module containing the
`resolve_theme_*` tests (verify which `use yach_ui::{...}` is in scope there
before editing).

```rust
    #[test]
    fn resolve_theme_no_color_keeps_layout_and_drops_color() -> Result<(), String> {
        let root = TestTempDir::new("theme-no-color")?;
        let home = root.path().join("home");
        theme_file(
            &home,
            r#"{"base":"nord","composer":{"style":"rule"},"spacing":{"toolGap":3}}"#,
        )?;
        let (painted, _) = resolve_theme(&theme_env(None, None, Some("0;15"), Some(&home)), &())?;
        expect_equal(&painted.spacing.tool_gap, &3)?;
        let mut expected = Theme::monochrome();
        expected.spacing = painted.spacing;
        expected.composer = ComposerStyle::Rule;
        for no_color in ["1", "0", " "] {
            let (theme, notices) = resolve_theme(
                &theme_env(None, Some(no_color), Some("0;15"), Some(&home)),
                &(),
            )?;
            expect_equal(&theme, &expected)?;
            expect_equal(&theme.mode(), &ThemeMode::Mono)?;
            expect_true(notices.is_empty(), "a valid file under NO_COLOR is silent")?;
        }
        // An explicit name that does not resolve still explains itself.
        let (theme, notices) = resolve_theme(
            &theme_env(Some("unknown:theme"), Some("1"), Some("0;15"), Some(&home)),
            &(),
        )?;
        expect_equal(&theme, &Theme::monochrome())?;
        expect_equal(&notices.len(), &1)?;
        // A broken personal file fails with or without NO_COLOR.
        theme_file(&home, "not json")?;
        for no_color in [None, Some("1")] {
            expect_true(
                resolve_theme(&theme_env(None, no_color, None, Some(&home)), &()).is_err(),
                "a broken personal file is an error",
            )?;
        }
        // Empty NO_COLOR is unset.
        let (theme, _) =
            resolve_theme(&theme_env(Some("light"), Some(""), None, Some(&home)), &())?;
        expect_equal(&theme, &Theme::light())?;
        Ok(())
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just dev cargo test -p yach resolve_theme_no_color`
Expected: FAIL: the theme equals `Theme::monochrome()` (composer `Box`,
default spacing) and the broken file is not an error under `NO_COLOR`.

- [ ] **Step 3: Resolve sources first, then drop color**

In `crates/yach-cli/src/main.rs`, rename the current `resolve_theme` body to
`resolve_theme_sources` with the `NO_COLOR` early return (lines 502-508)
removed, and add:

```rust
/// `NO_COLOR` removes color, not layout (https://no-color.org/): the theme
/// is resolved as usual, so its errors and notices still surface, and only
/// its colors are replaced.
fn resolve_theme(
    env: &ThemeEnv,
    extensions: &dyn yach_ui::ExtensionThemes,
) -> Result<(Theme, Vec<String>), String> {
    let (theme, notices) = resolve_theme_sources(env, extensions)?;
    if env
        .no_color
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        let mut monochrome = Theme::monochrome();
        monochrome.spacing = theme.spacing;
        monochrome.composer = theme.composer;
        return Ok((monochrome, notices));
    }
    Ok((theme, notices))
}
```

- [ ] **Step 4: Run the theme resolution tests**

Run: `just dev cargo test -p yach resolve_theme_`
Expected: PASS for all `resolve_theme_*` tests.

- [ ] **Step 5: Amend the theming spec and README**

`docs/project/specs/2026-10-02-theming-engine-design.md`, directly after
item 1 of "`NO_COLOR` semantics" (line 608), add:

```markdown
   Amended 2026-10-18 by `2026-10-17-composer-styles-design.md`: `NO_COLOR`
   overrides every theme **color**. The selected theme's `spacing` and
   `composer` still apply, and theme source errors are reported as they are
   without `NO_COLOR`.
```

`README.md:186-187`, replace the two rows:

```text
| `YACH_THEME` | user file, then `auto` | Preset (`dark`, `light`, `system`, `auto`), bundled palette name, or explicit path to a theme JSON file. A non-empty `NO_COLOR` replaces its colors and keeps its `spacing` and `composer`. |
| `NO_COLOR` | unset | Non-empty selects monochrome colors. The selected theme's `spacing` and `composer` still apply, and theme file errors are still reported. |
```

`README.md:258-267`, replace the paragraph and list with:

```markdown
### TUI appearance

TUI appearance is chosen once, at startup. The first match wins:

1. Non-empty `YACH_THEME`: a preset (`dark`, `light`, `system`, `auto`), a
   bundled palette name, or an explicit path to a theme file. Empty is unset.
2. `~/.yach/theme.json`.
3. `auto`.

`NO_COLOR` set to any non-empty value then replaces every color with the
monochrome theme's. The chosen theme's `spacing` and `composer` still apply,
and a broken theme file is reported exactly as it is without `NO_COLOR`.
```

(Keep whatever follows line 267 unchanged; check that the replaced block
ends where the original list ended.)

- [ ] **Step 6: Commit**

```bash
jj commit crates/yach-cli/src/main.rs docs/project/specs/2026-10-02-theming-engine-design.md README.md -m "Keep the theme's layout under NO_COLOR"
```

---

### Task 3: Restrained status row, alert priority, and status fills

**Files:**
- Modify: `crates/yach-ui/src/status_bar.rs` (priorities 13-25, `StatusBar`
  95-114, `segments` 117-168, `render` 172-193, `segment_style` 255-294,
  tests 340-)
- Modify: `crates/yach-ui/src/layout.rs:76-88` (keep compiling)

**Interfaces:**
- Consumes: `Theme::muted/dim/accent/text/warning/error`.
- Produces:
  ```rust
  // crates/yach-ui/src/status_bar.rs
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum StatusFill {
      /// `box`: segments only.
      None,
      /// `rule`: segments, a space, then `─` in this style up to the marker.
      Rule(Style),
      /// `band`: the row painted with this background, one leading space.
      Band(Color),
  }
  pub struct StatusBar<'a> {
      // existing fields, plus:
      pub fill: StatusFill,
      /// `running` / `more ↑`, drawn at the right end. Its width is taken
      /// from the segment budget before fitting.
      pub marker: Option<Span<'a>>,
  }
  ```

- [ ] **Step 1: Write the failing status tests**

In every existing `StatusBar { ... }` literal in the tests and in
`crates/yach-ui/src/layout.rs:76`, add `fill: StatusFill::None, marker: None,`.
Extend the test imports with `StatusFill` and
`ratatui::style::{Color, Modifier}`, `ratatui::text::Span`.

Delete `full_access_approval_segment_uses_danger_color` (the color table
below covers it). In `status_severity_colors_text_and_warns_on_connected_errors`,
delete the trailing `dot_style` block (the connection test below owns the
dot) and rename it `status_severity_colors_status_text`.

Add the tests and helpers:

```rust
    const ACCEPTANCE_ERROR: &str =
        "invalid_config; set the provider environment and relaunch yach tui";

    fn acceptance_bar<'a>(
        theme: &'a Theme,
        status: &'a StatusMessage,
        fill: StatusFill,
    ) -> StatusBar<'a> {
        StatusBar {
            model: "gpt-5.1-codex-max",
            thinking_level: "xhigh",
            approval_mode: "full-access",
            reviewer_id: None,
            status_message: status,
            is_connected: true,
            compaction_count: 0,
            total_tokens: None,
            context_used_percent: Some(78),
            context_window: Some((200_000, false)),
            theme,
            fill,
            marker: None,
        }
    }

    fn render_row(bar: StatusBar<'_>, width: u16) -> (Buffer, String) {
        let area = Rect::new(0, 0, width, 1);
        let mut buffer = Buffer::empty(area);
        Widget::render(bar, area, &mut buffer);
        let row = (0..width).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        (buffer, row)
    }

    fn cell_index(buffer: &Buffer, needle: &str) -> Option<u16> {
        let symbols: Vec<String> = needle.chars().map(String::from).collect();
        let width = buffer.area.width;
        (0..width).find(|&x| {
            (x..width)
                .zip(&symbols)
                .filter(|(cell, symbol)| buffer[(*cell, 0)].symbol() == symbol.as_str())
                .count()
                == symbols.len()
        })
    }

    #[test]
    fn alerts_outrank_context_at_80_columns_in_every_fill() {
        let theme = Theme::dark();
        let error = StatusMessage::error(ACCEPTANCE_ERROR);
        for fill in [
            StatusFill::None,
            StatusFill::Rule(theme.border(false)),
            StatusFill::Band(Color::Rgb(40, 40, 50)),
        ] {
            let (_, row) = render_row(acceptance_bar(&theme, &error, fill), 80);
            assert!(row.contains("gpt-5.1-codex-max · think:xhigh"), "{fill:?}: {row}");
            assert!(row.contains("approval:full-access"), "{fill:?}: {row}");
            // At least 12 cells: 11 characters and the ellipsis.
            assert!(row.contains("invalid_con"), "{fill:?}: {row}");
        }
    }

    #[test]
    fn info_status_still_drops_before_context() {
        let theme = Theme::dark();
        let info = StatusMessage::info(ACCEPTANCE_ERROR);
        let (_, row) = render_row(acceptance_bar(&theme, &info, StatusFill::None), 80);
        assert!(row.contains("ctx:78%/200k"), "{row}");
        assert!(!row.contains("invalid"), "{row}");
    }

    #[test]
    fn segment_colors_stay_quiet_while_healthy() {
        let theme = Theme::dark();
        let style = |id, connected, percent, mode, severity| {
            segment_style(id, connected, percent, mode, severity, false, &theme)
        };
        let info = StatusSeverity::Info;
        assert_eq!(
            style(SegmentId::Model, true, None, "review", info),
            theme.accent().add_modifier(Modifier::BOLD)
        );
        for mode in ["review", "accept-edits", "auto-review"] {
            assert_eq!(style(SegmentId::Approval, true, None, mode, info), theme.muted(), "{mode}");
        }
        assert_eq!(
            style(SegmentId::Approval, true, None, "full-access", info),
            theme.error().add_modifier(Modifier::BOLD)
        );
        for id in [SegmentId::Reviewer, SegmentId::Compaction, SegmentId::Tokens] {
            assert_eq!(style(id, true, None, "review", info), theme.muted(), "{id:?}");
        }
        for (percent, expected) in [
            (69, theme.dim()),
            (70, theme.warning()),
            (89, theme.warning()),
            (90, theme.error()),
        ] {
            assert_eq!(style(SegmentId::Context, true, Some(percent), "review", info), expected, "{percent}");
        }
        for (severity, expected) in [
            (StatusSeverity::Info, theme.muted()),
            (StatusSeverity::Warning, theme.warning()),
            (StatusSeverity::Error, theme.error()),
        ] {
            assert_eq!(style(SegmentId::Status, true, None, "review", severity), expected);
        }
        assert_eq!(
            style(SegmentId::Connection, true, None, "review", StatusSeverity::Error),
            theme.warning()
        );
        assert_eq!(style(SegmentId::Connection, false, None, "review", info), theme.error());
    }

    #[test]
    fn connection_dot_shows_only_when_something_is_wrong() {
        let theme = Theme::dark();
        for (connected, status, shown) in [
            (true, StatusMessage::info("ready"), false),
            (true, StatusMessage::warning("slow"), false),
            (true, StatusMessage::error("failed"), true),
            (false, StatusMessage::info("ready"), true),
        ] {
            let mut bar = acceptance_bar(&theme, &status, StatusFill::None);
            bar.is_connected = connected;
            let dot = bar
                .segments()
                .iter()
                .any(|segment| segment.id == SegmentId::Connection);
            assert_eq!(dot, shown, "connected={connected} {:?}", status.severity);
        }
    }

    #[test]
    fn band_paints_the_row_and_uses_band_safe_roles() {
        let theme = Theme::dark();
        let status = StatusMessage::info("ready");
        let mut bar = acceptance_bar(&theme, &status, StatusFill::Band(Color::Rgb(1, 2, 3)));
        bar.approval_mode = "review";
        bar.context_used_percent = Some(42);
        let (buffer, row) = render_row(bar, 120);
        assert!(row.starts_with(" gpt-5.1-codex-max"), "{row}");
        for x in 0..120 {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(1, 2, 3), "cell {x}");
        }
        assert_eq!(Some(buffer[(1, 0)].fg), theme.text().fg);
        assert!(buffer[(1, 0)].modifier.contains(Modifier::BOLD));
        let context = cell_index(&buffer, "ctx:");
        assert!(context.is_some(), "{row}");
        let Some(context) = context else { return };
        assert_eq!(Some(buffer[(context, 0)].fg), theme.muted().fg);
    }

    #[test]
    fn rule_fills_between_the_segments_and_the_marker() {
        let theme = Theme::dark();
        let status = StatusMessage::info("ready");
        let mut bar = acceptance_bar(&theme, &status, StatusFill::Rule(theme.border(false)));
        bar.model = "model";
        bar.thinking_level = "high";
        bar.approval_mode = "review";
        bar.context_used_percent = None;
        bar.marker = Some(Span::styled(" running ", theme.accent()));
        let (buffer, row) = render_row(bar, 60);
        assert!(row.ends_with("─ running "), "{row}");
        assert!(row.contains("ready ─"), "{row}");
        let rule = cell_index(&buffer, "─");
        assert!(rule.is_some(), "{row}");
        let Some(rule) = rule else { return };
        assert_eq!(Some(buffer[(rule, 0)].fg), theme.border(false).fg);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `just dev cargo test -p yach-ui --lib status_bar::`
Expected: FAIL to compile ("cannot find type `StatusFill`", `segment_style`
takes 6 arguments).

- [ ] **Step 3: Priorities and segments**

Replace the priority constants (status_bar.rs:13-25):

```rust
const PRIORITY_MODEL: u8 = 100;
const PRIORITY_APPROVAL: u8 = 99;
// A warning or error needs attention now: it outranks the context meter and
// is truncated, not dropped, while 12 cells fit. An info status stays below.
const PRIORITY_ALERT: u8 = 98;
const PRIORITY_CONTEXT: u8 = 97;
// A status answers the user's last action. Keep it above ambient details and
// truncate it before sacrificing approval, rather than losing that feedback.
const PRIORITY_STATUS: u8 = 95;
const PRIORITY_REVIEWER: u8 = 85;
const PRIORITY_CONNECTION: u8 = 80;
const PRIORITY_COMPACTION: u8 = 60;
// Below context: a narrow terminal should drop the cumulative total before
// the context-health meter, which is the actionable number.
const PRIORITY_TOKENS: u8 = 50;
```

In `segments`, start with an empty vector and push the connection dot only
when something is wrong; give the status its severity's priority:

```rust
        let mut segments = Vec::new();
        // A healthy connection needs no indicator; the dot is a warning sign.
        if !self.is_connected || self.status_message.severity == StatusSeverity::Error {
            segments.push(Segment::new(SegmentId::Connection, "●", PRIORITY_CONNECTION));
        }
        segments.push(Segment::new(
            SegmentId::Model,
            model_segment_text(self.model, self.thinking_level),
            PRIORITY_MODEL,
        ));
```

```rust
        if !self.status_message.text.is_empty() {
            let priority = if self.status_message.severity == StatusSeverity::Info {
                PRIORITY_STATUS
            } else {
                PRIORITY_ALERT
            };
            segments.push(Segment::new(
                SegmentId::Status,
                self.status_message.text.as_str(),
                priority,
            ));
        }
```

- [ ] **Step 4: Colors**

Replace `segment_style` (status_bar.rs:255-294):

```rust
/// Color only what asks for attention. On a band, `accent` and `dim` do not
/// reliably reach 3:1 against `statusBackground`, so the model uses bold
/// `text` and a healthy context meter uses `muted`.
fn segment_style(
    id: SegmentId,
    is_connected: bool,
    context_used_percent: Option<u8>,
    approval_mode: &str,
    severity: StatusSeverity,
    band: bool,
    theme: &Theme,
) -> Style {
    match id {
        // Present only when disconnected or showing an error (`segments`).
        SegmentId::Connection => {
            if is_connected {
                theme.warning()
            } else {
                theme.error()
            }
        }
        SegmentId::Model if band => theme.text().add_modifier(Modifier::BOLD),
        SegmentId::Model => theme.accent().add_modifier(Modifier::BOLD),
        SegmentId::Approval if approval_mode == "full-access" => {
            theme.error().add_modifier(Modifier::BOLD)
        }
        SegmentId::Approval
        | SegmentId::Reviewer
        | SegmentId::Compaction
        | SegmentId::Tokens => theme.muted(),
        SegmentId::Context => match context_used_percent.unwrap_or_default().min(100) {
            0..=69 if band => theme.muted(),
            0..=69 => theme.dim(),
            70..=89 => theme.warning(),
            _ => theme.error(),
        },
        SegmentId::Status => match severity {
            StatusSeverity::Info => theme.muted(),
            StatusSeverity::Warning => theme.warning(),
            StatusSeverity::Error => theme.error(),
        },
    }
}
```

- [ ] **Step 5: Fills and marker**

Add `StatusFill` (Interfaces above) and the two `StatusBar` fields. Replace
`render`:

```rust
impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        if area.is_empty() {
            return;
        }
        let band = matches!(self.fill, StatusFill::Band(_));
        if let StatusFill::Band(color) = self.fill {
            buf.set_style(area, Style::new().bg(color));
        }
        let lead = u16::from(band);
        // A rule needs a space and at least one `─` after the segments.
        let tail = if matches!(self.fill, StatusFill::Rule(_)) { 2 } else { 0 };
        let marker_width = self
            .marker
            .as_ref()
            .map_or(0, |marker| u16::try_from(marker.width()).unwrap_or(u16::MAX));
        let budget = area
            .width
            .saturating_sub(lead.saturating_add(tail).saturating_add(marker_width));
        let segments = fit_segments(self.segments(), budget);
        let used = u16::try_from(segment_width(&segments)).unwrap_or(u16::MAX);
        let mut spans = Vec::with_capacity(segments.len().saturating_mul(2));
        // The spec's band rule: separators take `muted`, not the default fg.
        let separator = if band { self.theme.muted() } else { Style::default() };
        for (index, segment) in segments.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled(SEGMENT_SEPARATOR, separator));
            }
            let style = segment_style(
                segment.id,
                self.is_connected,
                self.context_used_percent,
                self.approval_mode,
                self.status_message.severity,
                band,
                self.theme,
            );
            spans.push(Span::styled(segment.text, style));
        }
        let row = Rect {
            x: area.x.saturating_add(lead),
            width: area.width.saturating_sub(lead),
            ..area
        };
        Widget::render(Paragraph::new(Line::from(spans)), row, buf);
        let marker_x = area.right().saturating_sub(marker_width);
        if let StatusFill::Rule(style) = self.fill {
            for x in row.x.saturating_add(used).saturating_add(1)..marker_x {
                buf[(x, area.y)].set_symbol("─").set_style(style);
            }
        }
        if let Some(marker) = &self.marker {
            buf.set_span(marker_x, area.y, marker, marker_width);
        }
    }
}
```

Add `Color` and `Modifier` to the `ratatui::style` import. If clippy reports
`too_many_lines` or `cognitive_complexity` for `render`, move the span loop
into `fn styled_spans(&self, segments: Vec<Segment>, band: bool) -> Vec<Span<'static>>`.

- [ ] **Step 6: Run the status tests**

Run: `just dev cargo test -p yach-ui --lib status_bar::`
Expected: PASS. If an existing test asserts a `●` at column 0 for a healthy
bar, it encodes the old color rule: update it to the new rule, do not keep
the old behavior.

- [ ] **Step 7: Commit**

```bash
jj commit crates/yach-ui/src/status_bar.rs crates/yach-ui/src/layout.rs -m "Quiet status colors, rank alerts above context, add status fills"
```

---

### Task 4: Composer styles and empty-draft key hint

**Files:**
- Modify: `crates/yach-ui/src/input.rs` (constants 13-14, `InputComposer`
  16-22, `input_styles` 24-55, `render` 56-83, `input_metrics` 85-107, tests
  191-310)
- Modify: `crates/yach-ui/src/layout.rs` (whole file: 10-90, tests 92-160)
- Modify: `crates/yach-ui/src/app.rs:4332`, `:4514`
- Modify: `README.md:334-354` (theme example) and the paragraph after
  line 362

**Interfaces:**
- Consumes: `ComposerStyle`, `Theme::composer_style()`, `Theme::status_band()`
  (Task 1); `StatusFill`, `StatusBar.fill`, `StatusBar.marker` (Task 3).
- Produces:
  ```rust
  // crates/yach-ui/src/input.rs
  pub struct InputMetrics { pub prompt_rows: u16, pub overflowed: bool } // 1..=6 rows
  pub struct InputComposer<'a> { /* existing */ pub style: ComposerStyle }
  pub(crate) struct InputStyles { pub(crate) border, title, hint, marker, cursor: Style }
  pub(crate) fn input_styles(terminal_focused: bool, theme: &Theme) -> InputStyles;
  pub(crate) fn composer_marker(is_streaming: bool, overflowed: bool) -> Option<&'static str>;
  // crates/yach-ui/src/layout.rs
  pub fn transcript_viewport_size(area: Rect, input: &TextArea<'static>, theme: &Theme) -> (u16, u16);
  ```

- [ ] **Step 1: Write the failing input tests**

In `crates/yach-ui/src/input.rs` tests: add `style: ComposerStyle::Box,` to
every `InputComposer { ... }` literal; import `ComposerStyle` and
`ratatui::buffer::Cell`. Change the metric tests to prompt rows:

```rust
    #[test]
    fn prompt_rows_grow_for_wrapped_text() {
        assert_eq!(input_metrics(&TextArea::from(["hello"]), 20).prompt_rows, 1);
        assert_eq!(input_metrics(&TextArea::from(["abcdef"]), 4).prompt_rows, 3);
    }

    #[test]
    fn prompt_rows_count_explicit_newlines() {
        assert_eq!(input_metrics(&TextArea::from(["a", "b"]), 20).prompt_rows, 2);
    }
```

In `capped_input_title_reports_running_and_overflow`, assert
`metrics.prompt_rows == 6` and use `Rect::new(0, 0, 80, metrics.prompt_rows + 2)`.

Replace `composer_glyph_wrap_matches_metrics_for_a_long_token` and
`idle_composer_uses_a_clean_top_border_and_bottom_hint` with:

```rust
    fn text_of(buffer: &Buffer) -> String {
        buffer.content.iter().map(Cell::symbol).collect()
    }

    fn render_composer(input: &mut TextArea<'static>, style: ComposerStyle, area: Rect, focused: bool) -> Buffer {
        let mut buffer = Buffer::empty(area);
        Widget::render(
            InputComposer {
                textarea: input,
                is_streaming: false,
                terminal_focused: focused,
                overflowed: false,
                style,
                theme: &Theme::default(),
            },
            area,
            &mut buffer,
        );
        buffer
    }

    #[test]
    fn composer_glyph_wrap_matches_metrics_in_every_style() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let mut input = TextArea::from(["abcdefghij"]);
            let rows = input_metrics(&input, 6).prompt_rows;
            assert_eq!(rows, 3);
            let (area, x0, y0) = if style == ComposerStyle::Box {
                (Rect::new(0, 0, 6, rows + 2), 1, 1)
            } else {
                (Rect::new(0, 0, 6, rows), 2, 0)
            };
            let buffer = render_composer(&mut input, style, area, true);
            let content = (y0..y0 + rows)
                .map(|y| {
                    (x0..x0 + 4)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(content, ["abcd", "efgh", "ij"], "{style:?}");
        }
    }

    #[test]
    fn idle_box_keeps_a_clean_top_border() {
        let mut input = TextArea::default();
        let buffer = render_composer(&mut input, ComposerStyle::Box, Rect::new(0, 0, 48, 3), true);
        let top = (0..48).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        assert_eq!(top, format!("┌{}┐", "─".repeat(46)));
    }

    #[test]
    fn key_hint_shows_only_while_the_draft_is_empty() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let height = if style == ComposerStyle::Box { 3 } else { 1 };
            for (draft, shown) in [("", true), ("hello", false)] {
                let mut input = TextArea::from([draft]);
                let buffer = render_composer(&mut input, style, Rect::new(0, 0, 48, height), true);
                assert_eq!(
                    text_of(&buffer).contains("enter send · ctrl+j newline"),
                    shown,
                    "{style:?} draft={draft:?}"
                );
            }
        }
    }

    #[test]
    fn open_styles_mark_the_prompt_and_dim_it_when_unfocused() {
        for style in [ComposerStyle::Rule, ComposerStyle::Band] {
            for focused in [true, false] {
                let mut input = TextArea::from(["hi"]);
                let buffer = render_composer(&mut input, style, Rect::new(0, 0, 20, 1), focused);
                let marker = &buffer[(0, 0)];
                assert_eq!(marker.symbol(), "›", "{style:?}");
                assert!(marker.modifier.contains(Modifier::BOLD), "{style:?}");
                assert_eq!(marker.modifier.contains(Modifier::DIM), !focused, "{style:?} {focused}");
                assert_eq!(buffer[(2, 0)].symbol(), "h", "{style:?}");
                assert_eq!(
                    buffer[(2, 0)].modifier.contains(Modifier::HIDDEN),
                    !focused,
                    "{style:?} {focused}"
                );
            }
        }
    }
```

- [ ] **Step 2: Write the failing layout tests**

In `crates/yach-ui/src/layout.rs` tests, pass `&Theme::default()` as the new
third argument in the two `transcript_viewport_size` tests (expected values
unchanged: 19 and 18). Add, importing `ComposerStyle`, `ThemeDocument`,
`ratatui::buffer::Buffer` and `ratatui::style::{Color, Modifier}`:

```rust
    fn draw(theme: &Theme, input: &mut TextArea<'static>) -> Buffer {
        let backend = TestBackend::new(80, 24);
        let Ok(mut terminal) = Terminal::new(backend);
        let transcript = Transcript::new();
        let mut transcript_cache = TranscriptRenderCache::default();
        let status = StatusMessage::info("ready");
        let result = terminal.draw(|frame| {
            render(
                frame,
                &mut RenderParams {
                    transcript: &transcript,
                    transcript_cache: &mut transcript_cache,
                    scroll_offset: 0,
                    transcript_alignment: TranscriptAlignment::Bottom,
                    is_streaming: false,
                    input,
                    model: "model",
                    thinking_level: "high",
                    approval_mode: "review",
                    reviewer_id: None,
                    status_message: &status,
                    is_connected: true,
                    compaction_count: 0,
                    total_tokens: None,
                    context_used_percent: None,
                    context_window: None,
                    terminal_focused: true,
                    theme,
                },
            );
        });
        assert!(result.is_ok());
        terminal.backend().buffer().clone()
    }

    fn row(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
    }

    #[test]
    fn viewport_ends_where_each_style_starts_drawing() {
        for style in [ComposerStyle::Box, ComposerStyle::Rule, ComposerStyle::Band] {
            let mut theme = Theme::dark();
            theme.composer = style;
            for lines in [1_u16, 3, 7] {
                let mut input = TextArea::new(vec![String::from("line"); usize::from(lines)]);
                let (_, height) = transcript_viewport_size(Rect::new(0, 0, 80, 24), &input, &theme);
                let buffer = draw(&theme, &mut input);
                let prompt_rows = lines.min(6);
                let label = format!("{style:?}, {lines} lines");
                let first = row(&buffer, height);
                let status_end = if lines > 6 { "more ↑ " } else { "─" };
                match style {
                    ComposerStyle::Box => {
                        assert_eq!(height, 24 - prompt_rows - 4, "{label}");
                        assert!(first.trim().is_empty(), "{label}: gap row {first:?}");
                        assert!(row(&buffer, height + 1).starts_with('┌'), "{label}");
                        assert!(row(&buffer, 23).contains("approval:review"), "{label}");
                    }
                    ComposerStyle::Rule => {
                        assert_eq!(height, 24 - prompt_rows - 2, "{label}");
                        assert!(first.contains("approval:review"), "{label}: {first}");
                        assert!(first.ends_with(status_end), "{label}: {first}");
                        assert!(row(&buffer, height + 1).starts_with('›'), "{label}");
                        assert_eq!(row(&buffer, 23), "─".repeat(80), "{label}");
                    }
                    ComposerStyle::Band => {
                        assert_eq!(height, 24 - prompt_rows - 1, "{label}");
                        assert!(first.contains("approval:review"), "{label}: {first}");
                        assert_eq!(buffer[(0, height)].bg, Color::Rgb(40, 40, 50), "{label}");
                        assert!(row(&buffer, height + 1).starts_with('›'), "{label}");
                        if lines > 6 {
                            assert!(first.ends_with("more ↑ "), "{label}: {first}");
                            // ` more ↑ ` is 8 cells; `m` sits 7 from the right edge.
                            let marker = &buffer[(73, height)];
                            assert_eq!(Some(marker.fg), theme.text().fg, "{label}: band marker");
                            assert!(marker.modifier.contains(Modifier::BOLD), "{label}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn band_without_a_band_background_draws_rule() {
        let reset = ThemeDocument::parse(r#"{"colors":{"statusBackground":"default"}}"#)
            .and_then(|document| document.apply(Theme::dark()));
        assert!(reset.is_ok());
        let Ok(reset) = reset else { return };
        for base in [Theme::system(), Theme::monochrome(), reset] {
            let (mut band, mut rule) = (base, base);
            band.composer = ComposerStyle::Band;
            rule.composer = ComposerStyle::Rule;
            assert_eq!(
                draw(&band, &mut TextArea::from(["hello"])),
                draw(&rule, &mut TextArea::from(["hello"])),
                "{:?}",
                base.mode()
            );
        }
    }
```

- [ ] **Step 3: Run them to verify they fail**

Run: `just dev cargo test -p yach-ui --lib input:: layout::`
Expected: FAIL to compile (`prompt_rows`, `style` field, third argument).

- [ ] **Step 4: Prompt rows and composer rendering**

In `crates/yach-ui/src/input.rs`, replace the height constants with:

```rust
/// Prompt rows before the composer scrolls, in every style.
const MAX_PROMPT_ROWS: u16 = 6;
const PROMPT_MARKER: &str = "› ";
const BOX_HINT: &str = " enter send · ctrl+j newline ";
const OPEN_HINT: &str = " enter send · ctrl+j newline";
```

`input_metrics` keeps one wrap width (`area_width - 2`) for every style:

```rust
pub struct InputMetrics {
    pub prompt_rows: u16,
    pub overflowed: bool,
}

pub fn input_metrics(textarea: &TextArea<'_>, area_width: u16) -> InputMetrics {
    let inner_width = area_width.saturating_sub(2) as usize;
    let tab_length = textarea.tab_length();
    let content_rows = textarea
        .lines()
        .iter()
        .map(|line| wrapped_line_count(line, inner_width, tab_length))
        .sum::<usize>()
        .max(1);
    InputMetrics {
        prompt_rows: u16::try_from(content_rows)
            .unwrap_or(u16::MAX)
            .min(MAX_PROMPT_ROWS),
        overflowed: content_rows > usize::from(MAX_PROMPT_ROWS),
    }
}
```

Styles, marker and rendering:

```rust
pub struct InputComposer<'a> {
    pub textarea: &'a mut TextArea<'static>,
    pub is_streaming: bool,
    pub terminal_focused: bool,
    pub overflowed: bool,
    /// The drawn style, `Theme::composer_style()`, after the band fallback.
    pub style: ComposerStyle,
    pub theme: &'a Theme,
}

pub(crate) struct InputStyles {
    pub(crate) border: Style,
    pub(crate) title: Style,
    pub(crate) hint: Style,
    pub(crate) marker: Style,
    pub(crate) cursor: Style,
}

pub(crate) fn input_styles(terminal_focused: bool, theme: &Theme) -> InputStyles {
    if terminal_focused {
        InputStyles {
            border: theme.border(false),
            title: theme.accent().add_modifier(Modifier::BOLD),
            hint: theme.dim(),
            // Spec: the marker is chrome, like the rules, so it takes the
            // border color rather than the transcript prefix's accent.
            marker: theme.border(false).add_modifier(Modifier::BOLD),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
        }
    } else {
        // Unfocused dims the frame, title, hint and marker together.
        let dim = theme.dim().add_modifier(Modifier::DIM);
        InputStyles {
            border: dim,
            title: dim,
            hint: dim,
            marker: dim.add_modifier(Modifier::BOLD),
            cursor: Style::default().add_modifier(Modifier::HIDDEN),
        }
    }
}

/// `box` puts this in its top border; `rule` and `band` at the right end of
/// the status row.
pub(crate) fn composer_marker(is_streaming: bool, overflowed: bool) -> Option<&'static str> {
    match (is_streaming, overflowed) {
        (true, true) => Some(" running · more ↑ "),
        (true, false) => Some(" running "),
        (false, true) => Some(" more ↑ "),
        (false, false) => None,
    }
}

impl Widget for InputComposer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let styles = input_styles(self.terminal_focused, self.theme);
        self.textarea.set_wrap_mode(WrapMode::Glyph);
        self.textarea.set_style(self.theme.text());
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea.set_cursor_style(styles.cursor);
        match self.style {
            ComposerStyle::Box => self.render_box(&styles, area, buf),
            ComposerStyle::Rule | ComposerStyle::Band => self.render_open(&styles, area, buf),
        }
    }
}

impl InputComposer<'_> {
    fn render_box(self, styles: &InputStyles, area: Rect, buf: &mut Buffer) {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles.border);
        if let Some(title) = composer_marker(self.is_streaming, self.overflowed) {
            block = block.title(Line::styled(title, styles.title));
        }
        // The hint teaches the keys; once the user is typing it is noise.
        if area.width >= 42 && self.textarea.is_empty() {
            block = block.title_bottom(Line::styled(BOX_HINT, styles.hint));
        }
        self.textarea.set_block(block);
        Widget::render(&*self.textarea, area, buf);
    }

    /// `rule` and `band`: `› ` then the textarea, unblocked, in the area
    /// offset two columns, so wrap-aware cursor movement sees the area it
    /// renders into. Continuation rows inherit the two-column indent.
    fn render_open(self, styles: &InputStyles, area: Rect, buf: &mut Buffer) {
        buf.set_string(area.x, area.y, PROMPT_MARKER, styles.marker);
        let prompt = Rect {
            x: area.x.saturating_add(2),
            width: area.width.saturating_sub(2),
            ..area
        };
        self.textarea.remove_block();
        // The textarea shows its placeholder after the cursor, only while empty.
        if self.textarea.placeholder_text() != OPEN_HINT {
            self.textarea.set_placeholder_text(OPEN_HINT);
        }
        self.textarea.set_placeholder_style(styles.hint);
        Widget::render(&*self.textarea, prompt, buf);
    }
}
```

Imports: `ratatui::buffer::Buffer`, `ratatui::layout::Rect`,
`crate::theme::{ComposerStyle, Theme}`.

- [ ] **Step 5: One layout function for every style**

In `crates/yach-ui/src/layout.rs`, remove `STATUS_HEIGHT` and
`COMPOSER_GAP_HEIGHT` and add:

```rust
/// Where each part of the screen goes for one composer style. Both
/// `transcript_viewport_size` and `render` read this, so the viewport and
/// the drawn layout cannot disagree.
struct ComposerAreas {
    transcript: Rect,
    status: Rect,
    input: Rect,
    closing: Option<Rect>,
}

fn composer_areas(area: Rect, style: ComposerStyle, prompt_rows: u16) -> ComposerAreas {
    match style {
        ComposerStyle::Box => {
            let [transcript, _gap, input, status] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows.saturating_add(2)),
                Constraint::Length(1),
            ])
            .areas(area);
            ComposerAreas { transcript, status, input, closing: None }
        }
        ComposerStyle::Rule => {
            let [transcript, status, input, closing] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows),
                Constraint::Length(1),
            ])
            .areas(area);
            ComposerAreas { transcript, status, input, closing: Some(closing) }
        }
        ComposerStyle::Band => {
            let [transcript, status, input] = Layout::vertical([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(prompt_rows),
            ])
            .areas(area);
            ComposerAreas { transcript, status, input, closing: None }
        }
    }
}

pub fn transcript_viewport_size(
    area: Rect,
    input: &ratatui_textarea::TextArea<'static>,
    theme: &Theme,
) -> (u16, u16) {
    let prompt_rows = input_metrics(input, area.width).prompt_rows;
    let areas = composer_areas(area, theme.composer_style(), prompt_rows);
    (area.width, areas.transcript.height.max(1))
}
```

Replace `render`'s body after `let area = frame.area();`:

```rust
    let style = params.theme.composer_style();
    let metrics = input_metrics(params.input, area.width);
    let areas = composer_areas(area, style, metrics.prompt_rows);

    transcript::render(
        areas.transcript,
        frame.buffer_mut(),
        params.transcript,
        params.transcript_cache,
        params.scroll_offset,
        params.is_streaming,
        params.transcript_alignment,
    );

    frame.render_widget(
        InputComposer {
            textarea: &mut *params.input,
            is_streaming: params.is_streaming,
            terminal_focused: params.terminal_focused,
            overflowed: metrics.overflowed,
            style,
            theme: params.theme,
        },
        areas.input,
    );

    let styles = input_styles(params.terminal_focused, params.theme);
    let fill = match (style, params.theme.status_band()) {
        (ComposerStyle::Box, _) => StatusFill::None,
        (ComposerStyle::Band, Some(color)) => StatusFill::Band(color),
        (ComposerStyle::Rule | ComposerStyle::Band, _) => StatusFill::Rule(styles.border),
    };
    // On a band, `accent` and `dim` can fall below 3:1 against
    // `statusBackground`; the marker follows the band's rule for the model:
    // bold `text` while focused, `muted` (no DIM modifier) while unfocused.
    let marker_style = match (fill, params.terminal_focused) {
        (StatusFill::Band(_), true) => params.theme.text().add_modifier(Modifier::BOLD),
        (StatusFill::Band(_), false) => params.theme.muted(),
        _ => styles.title,
    };
    let marker = (style != ComposerStyle::Box)
        .then(|| composer_marker(params.is_streaming, metrics.overflowed))
        .flatten()
        .map(|marker| Span::styled(marker, marker_style));
    frame.render_widget(
        StatusBar {
            model: params.model,
            thinking_level: params.thinking_level,
            approval_mode: params.approval_mode,
            reviewer_id: params.reviewer_id,
            status_message: params.status_message,
            is_connected: params.is_connected,
            compaction_count: params.compaction_count,
            total_tokens: params.total_tokens,
            context_used_percent: params.context_used_percent,
            context_window: params.context_window,
            theme: params.theme,
            fill,
            marker,
        },
        areas.status,
    );
    if let Some(closing) = areas.closing {
        let rule = "─".repeat(usize::from(closing.width));
        frame.buffer_mut().set_string(closing.x, closing.y, rule, styles.border);
    }
```

Imports: `crate::input::{InputComposer, composer_marker, input_metrics, input_styles}`,
`crate::status_bar::{StatusBar, StatusFill, StatusMessage}`,
`crate::theme::{ComposerStyle, Theme}`, `ratatui::text::Span`,
`ratatui::style::Modifier`; drop
`Direction` if unused.

`crates/yach-ui/src/app.rs`: pass the theme at both call sites:
`layout::transcript_viewport_size(area, &self.app.prompt, &self.app.theme)`
(4332) and `layout::transcript_viewport_size(area, &app.prompt, &app.theme)`
(4514).

- [ ] **Step 6: Run the UI tests**

Run: `just dev cargo test -p yach-ui`
Expected: PASS, including `fresh_session_keeps_first_exchange_next_to_the_composer`
and `resumed_session_starts_next_to_the_composer_after_archived_scrollback`
(both render an empty prompt, so the box hint they locate still shows).

- [ ] **Step 7: Document the styles**

`README.md` theme example (334-354): add `"composer": { "style": "rule" },`
before `"spacing"`. After the token paragraph (ends line 362), add:

```markdown
`composer.style` picks the composer layout. `box` (the default) draws a
bordered input with the status row below it. `rule` draws the status row on
a `─` rule above the prompt and closes the composer with a second rule.
`band` draws the status row on a shaded `statusBackground` band above the
prompt; under `system`, `NO_COLOR`, or a `statusBackground` of `default` it
draws as `rule`. In every style the key hint shows only while the prompt is
empty, and the status row colors only what needs attention: a disconnected
or erroring connection, full-access approval, a filling context window, and
warning or error messages.
```

- [ ] **Step 8: Commit**

```bash
jj commit crates/yach-ui/src/input.rs crates/yach-ui/src/layout.rs crates/yach-ui/src/app.rs README.md -m "Add rule and band composer styles and show the key hint only on an empty draft"
```

---

### Task 5: Composer preview captures and final verification

**Files:**
- Modify: `tests/visual/theme-grid.sh:50-89`
- Modify: `tests/visual/ansi2html.mjs:77-85`, `:102-112`
- Modify: `tests/visual/ansi2html.test.mjs`
- Modify: `README.md:383-396` ("TUI design review")

**Interfaces:**
- Consumes: the `composer` theme field (Task 1) through a temporary theme
  file selected by `YACH_THEME`.
- Produces: manifest lines `{name, fg, bg, cols}`; panels named
  `composer-<style>-<theme>-<cols>` grouped on `composer` grid pages.

- [ ] **Step 1: Write the failing converter test**

Append to `tests/visual/ansi2html.test.mjs`:

```js
test('groups composer captures on their own pages after the palettes', () => {
  const panels = ['dark', 'composer-box-dark-120', 'composer-rule-dark-80', 'nord'];
  const pages = gridPages(panels.map(name => ({ name, body: `<pre>${name}</pre>` })));
  assert.deepEqual(pages.map(page => page.name), ['presets', 'nord-monokai', 'composer']);
  assert.ok(pages[2].body.includes('<pre>composer-rule-dark-80</pre>'));
});
```

Run: `node --test tests/visual/ansi2html.test.mjs`
Expected: FAIL (composer panels land on `other`).

- [ ] **Step 2: Group and size composer panels**

`tests/visual/ansi2html.mjs`: add `['composer', /^composer-/],` before
`['other', /./]`. In `render`, read `cols` from each manifest line
(`const { name, fg, bg, cols = 120 } = JSON.parse(line);`), add
`--cols:${cols}` to the section's `style`, and change the `pre` rule's
`width:120ch` to `width:calc(var(--cols)*1ch)`.

Run: `node --test tests/visual/ansi2html.test.mjs`
Expected: PASS.

- [ ] **Step 3: Capture the composer section**

`tests/visual/theme-grid.sh`: give `capture` an optional theme value and
width, and record the width:

```bash
capture() {
  local name="$1" fg="$2" bg="$3" theme="${4:-$1}" cols="${5:-120}" launch screen previous='' stable=0 settled=0
  local -a theme_env=("YACH_THEME=$theme")
```

Use `-x "$cols"` in `tmux new-session`, and write the manifest line with
`jq -nc --arg name "$name" --arg fg "$fg" --arg bg "$bg" --argjson cols "$cols" '{name:$name,fg:$fg,bg:$bg,cols:$cols}'`.
After the palette loop, before the `node` call:

```bash
# Each composer style over the presets, at the full and the narrow width.
# `system` shows `band` falling back to `rule`.
for style in box rule band; do
  for base in dark light system; do
    file="$work/composer-$style-$base.json"
    printf '{"base":"%s","composer":{"style":"%s"}}\n' "$base" "$style" > "$file"
    if [[ "$base" == light ]]; then fg='#303030' bg='#fafafa'; else fg='#d0d0d0' bg='#1c1c1c'; fi
    for cols in 120 80; do
      capture "composer-$style-$base-$cols" "$fg" "$bg" "$file" "$cols"
    done
  done
done
```

Verify that an absolute path in `YACH_THEME` takes the explicit-file branch
(`explicit_theme_path`, `crates/yach-cli/src/main.rs:474-489`) and that the
settle check's `Updated the greeting` stays on one row at 80 columns; if it
wraps, match a shorter prefix such as `Updated the`.

`README.md` "TUI design review" step 1: append "The capture also renders a
composer section: each composer style over `dark`, `light`, and `system`, at
120 and 80 columns."

- [ ] **Step 4: Run the capture**

Run: `just tui-themes composer-check`
Expected: `captured composer-<style>-<base>-<cols>` for 18 panels, and
`target/tui-themes/composer-check/grid-composer*.png` (5 pages). Open the
`composer` grids and confirm: `rule` shows a status rule above `› ` and a
closing rule; `band` on `dark`/`light` shows a shaded status row and no
closing row; `band` on `system` matches `rule`; the empty prompt shows the
hint in every style; 80-column panels fit without wrapping the status row.

- [ ] **Step 5: Full verification**

Run, one at a time:

```bash
just fmt-check
just lint
just test
node --test tests/visual/ansi2html.test.mjs
```

Expected: all pass with no warnings.

Then capture `just tui-themes before` on the base revision (`main`) and
`just tui-themes after` on this stack, and embed the affected grids
(presets, at least one palette family, and the `composer` pages) in the PR
description's Screenshots section, hosted on `pr-screenshots` per
`AGENTS.md`.

- [ ] **Step 6: Commit**

```bash
jj commit tests/visual/theme-grid.sh tests/visual/ansi2html.mjs tests/visual/ansi2html.test.mjs README.md -m "Capture composer styles in the theme preview"
```
