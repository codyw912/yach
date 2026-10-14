# Theming Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use sjujperpowers:subagent-driven-development (recommended) or sjujperpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace yach's fixed `pi_dark` token reads with a role-based theme
module: `dark`, `light`, `system` and `auto` presets, eighteen bundled named
palettes, `NO_COLOR` monochrome, a neutral tool panel with a state-colored
`┃` rule, and declarative extension themes.

**Architecture:**

- `crates/yach-ui/src/theme.rs` owns every color decision. Widgets call
  role helpers (`text()`, `tool_surface(state)`, `diff(kind)`, ...) that
  return complete `Style`s; `ThemeColors` fields become private, so a widget
  that bypasses a helper does not compile. A `ThemeMode` (`Painted`,
  `System`, `Mono`) decides backgrounds and attribute emphasis in one place.
- A theme is still the strict JSON document, now with an optional `base`.
  `ThemeDocument::apply` layers a file's `vars`/`colors`/`spacing` over a base
  `Theme`; presets are Rust constants, bundled palettes are embedded JSON
  files, extension themes are manifest-declared files. `select::resolve_base`
  resolves any `base` name; the CLI's pure `resolve_theme` applies the
  spec's single-winner precedence, `COLORFGBG` for `auto`, and `NO_COLOR`.
- Everything is resolved once at startup into the `Copy` `Theme`; rendering
  stays field reads.

**Tech Stack:** Rust 2024 workspace; `yach-ui` (ratatui), `yach` CLI crate at
`crates/yach-cli`, `yach-backend` (extension manifests), serde/serde_json,
VHS tapes under `tests/visual`.

**Spec:** `docs/project/specs/2026-10-02-theming-engine-design.md`
(approved 2026-10-06). Evidence: `docs/project/records/2026-10-06-theme-engine-survey.md`.

**Source:** plane:YACH-20

## Global Constraints

- Workspace clippy: pedantic warn; `unwrap_used`, `expect_used`, `panic`,
  `panic_in_result_fn`, `allow_attributes`, `print_stdout`, `print_stderr`,
  `dbg_macro`, `todo`, `unimplemented` deny. `clippy.toml`:
  `too-many-lines-threshold = 100`, `cognitive-complexity-threshold = 15`.
  Tests follow each file's neighbouring test style.
- No new color syntax (hex, ANSI names, indices 0-255, `default`, `vars`
  only). No 24-bit downgrade; limited-color terminals use `system`.
- A theme is fixed per process: no picker, no live reload, no runtime theme
  protocol.
- No project-scoped themes. `<project>/.yach/theme.json` is never applied,
  read for content, edited or deleted; its presence produces one startup
  notice: "project themes are not supported; move it to
  `~/.yach/theme.json` or set `YACH_THEME`".
- Any text on a painted background sets an explicit, non-`Reset` foreground.
- `NO_COLOR` present and non-empty selects monochrome and overrides every
  theme source.
- Contrast (WCAG 2.x relative luminance): body text at least 4.5:1 on every
  painted surface; `muted`, `success`, `warning`, `error`, `harness` at least
  3:1 against the tool panel, in `dark`, `light` and every bundled palette.
- Bundled palettes are pinned to Tinted Theming
  `a70da1dab18008023cfd55a94053f3b6cab4f86e`; NOTICE never claims a license
  an upstream does not state.
- Existing theme files keep their meaning: a file without `base` is `dark`
  plus its overrides; the 22 token names, `vars`, spacing and
  `deny_unknown_fields` are unchanged.
- Commands run through the dev shell: `just dev cargo test -p <crate>
  <filter>` (`yach-ui`, `yach`, `yach-backend`). Final gate: `just lint`
  and `just test`.
- Commit each task with `jj commit <paths> -m "<message>"` naming the files
  the task touched; never a bare `jj commit`.

## Sequencing and deviations from the spec's migration order

The spec orders migration 1, 2, 4, 3, 5, 6, 7 (its step numbers). This plan
keeps that intent with these explicit changes:

| Plan task | Spec step | Why |
|---|---|---|
| 1. Theme roles, presets, documents | 1 | Unchanged. `dark` keeps today's `pi_dark` values exactly. |
| 2. Bundled named palettes | 5 | Moved earlier: invisible until Task 4 names them, and Task 4's reserved-name set needs the table. |
| 3. Widget migration | 2 | Starts only after the `tui-markdown` change (bookmark `tui-markdown`) lands on `main`, because it migrates `markdown.rs` and edits `transcript.rs`. |
| 4. Theme selection | 4 | Unchanged. |
| 5. Tool row treatment | 3 | Also carries the two `dark` default changes (`toolSuccessBackground`, `toolOutput`). Tasks 1-4 change only what selection changes: `NO_COLOR`, reserved `YACH_THEME` names, and the default becoming `auto`, so a terminal reporting a light background via `COLORFGBG` gets `light` instead of `dark`. |
| 6. Extension themes | 6 | Unchanged. |
| 7. Visual tapes and docs | 7 | Unchanged. |

Two representation choices refine the spec without changing behavior: the
spec's `Theme.mono: bool` becomes `ThemeMode { Painted, System, Mono }`
(`system` also needs mode-specific helpers), and `Theme::from_json` /
`Theme::load` are replaced by `ThemeDocument` (file reading moves to the CLI,
which already owns the path).

---

### Task 1: Theme roles, presets and documents

**Files:**

- Create: `crates/yach-ui/src/theme/presets.rs`
- Create: `crates/yach-ui/src/theme/contrast.rs` (`#[cfg(test)]` only)
- Modify: `crates/yach-ui/src/theme.rs` (322 lines today; becomes the module root: types, helpers, `ThemeDocument`, tests)
- Modify: `crates/yach-ui/src/lib.rs:25` (re-exports)
- Modify: `crates/yach-cli/src/main.rs:425-432` (`load_tui_theme` only; `tui_theme_path` at `main.rs:434-450` is untouched)
- Modify: `crates/yach-ui/src/transcript.rs:1924-1978` (the one `Theme::from_json` caller)
- Test: `crates/yach-ui/src/theme.rs` (`mod tests`), `crates/yach-ui/src/theme/contrast.rs`

**Interfaces:**

- Consumes: today's `ThemeColors` (`theme.rs:16-39`), `ThemeSpacing` (`theme.rs:42-48`), `parse_color` (`theme.rs:244-268`), `resolve_color` (`theme.rs:220-242`), `ThemeSpacing::apply` (`theme.rs:141-159`), `set_color` (`theme.rs:104-132`).
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode { Painted, System, Mono }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState { Pending, Success, Error, Harness }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind { Added, Removed, Context, Hunk }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub colors: ThemeColors, // pub until Task 3
    pub spacing: ThemeSpacing,
    mode: ThemeMode,
}

impl Theme {
    pub const fn dark() -> Self;      // == today's pi_dark(), mode Painted
    pub const fn light() -> Self;     // mode Painted
    pub const fn system() -> Self;    // mode System
    pub const fn monochrome() -> Self; // mode Mono
    pub const fn mode(&self) -> ThemeMode;

    pub fn text(&self) -> Style;
    pub fn muted(&self) -> Style;
    pub fn dim(&self) -> Style;
    pub fn accent(&self) -> Style;
    pub fn border(&self, focused: bool) -> Style;
    pub fn success(&self) -> Style;
    pub fn warning(&self) -> Style;
    pub fn error(&self) -> Style;
    pub fn harness(&self) -> Style;
    pub fn selected(&self) -> Style;
    pub fn user_message(&self) -> Style;
    pub fn tool_surface(&self, state: ToolState) -> Style;
    pub fn tool_rule(&self, state: ToolState) -> Style;
    pub fn tool_title(&self) -> Style;
    pub fn tool_output(&self, state: ToolState) -> Style;
    pub fn diff(&self, kind: DiffKind) -> Style;
}
impl Default for Theme { fn default() -> Self { Self::dark() } }

pub struct ThemeDocument { /* private */ }
impl ThemeDocument {
    pub fn parse(json: &str) -> Result<Self, ThemeLoadError>;
    pub fn base(&self) -> Option<&str>;
    /// Color-token keys as written. Task 2 asserts these.
    pub fn color_tokens(&self) -> impl Iterator<Item = &str>;
    /// `vars` keys as written. Task 2 asserts these.
    pub fn var_names(&self) -> impl Iterator<Item = &str>;
    /// Applies vars/colors/spacing over `base`, keeping `base`'s mode.
    pub fn apply(&self, base: Theme) -> Result<Theme, ThemeLoadError>;
}

// ThemeLoadError: Json, InvalidColor, UnknownColorToken, UnknownVariable,
// VariableCycle. No Io variant.

// theme/contrast.rs, cfg(test):
pub(crate) fn contrast_ratio(a: Color, b: Color) -> Option<f64>;
```

`Theme::pi_dark`, `Theme::from_json`, and `Theme::load` are deleted. `mod palettes;` and `mod select;` are not declared here; Task 2 and Task 4 add them.

- [ ] **Step 1: Write the failing document and preset tests**

Replace the `tests` module at `theme.rs:270-322` with the tests below. They call `ThemeDocument`, `Theme::dark/light/system/monochrome`, and `contrast_ratio`, none of which exist yet, so this step fails to compile. Keep the existing `default_theme_matches_pi_message_and_tool_surfaces` assertion values: they move onto `Theme::dark()` and must still see `Rgb(52, 53, 65)` and `Rgb(40, 50, 40)`.

```rust
#[cfg(test)]
mod tests {
    use super::{DiffKind, Theme, ThemeDocument, ThemeLoadError, ThemeMode, ToolState};
    use crate::theme::contrast::contrast_ratio;
    use ratatui::style::{Color, Modifier, Style};

    fn fg(style: Style) -> Color {
        style.fg.unwrap_or(Color::Reset)
    }

    fn bg(style: Style) -> Color {
        style.bg.unwrap_or(Color::Reset)
    }

    fn is_reset(color: Color) -> bool {
        color == Color::Reset
    }

    /// Every surface a painted preset must pair explicitly. Pending and
    /// Success are listed separately: dark still paints them different
    /// greens, and Task 5 is what makes them equal.
    fn painted_surfaces(theme: &Theme) -> [(&'static str, Style); 5] {
        [
            ("selected", theme.selected()),
            ("user_message", theme.user_message()),
            ("tool_pending", theme.tool_surface(ToolState::Pending)),
            ("tool_success", theme.tool_surface(ToolState::Success)),
            ("tool_error", theme.tool_surface(ToolState::Error)),
        ]
    }

    #[test]
    fn dark_preset_keeps_todays_pi_dark_values() {
        let theme = Theme::dark();
        assert_eq!(theme.mode(), ThemeMode::Painted);
        assert_eq!(Theme::default(), theme);

        assert_eq!(theme.colors.accent, Color::Rgb(0, 215, 255));
        assert_eq!(theme.colors.border, Color::Rgb(95, 135, 255));
        assert_eq!(theme.colors.success, Color::Rgb(181, 189, 104));
        assert_eq!(theme.colors.error, Color::Rgb(204, 102, 102));
        assert_eq!(theme.colors.warning, Color::Yellow);
        assert_eq!(theme.colors.muted, Color::Rgb(128, 128, 128));
        assert_eq!(theme.colors.dim, Color::Rgb(102, 102, 102));
        assert_eq!(theme.colors.text, Color::White);
        assert_eq!(theme.colors.selected_background, Color::Rgb(58, 58, 74));
        assert_eq!(theme.colors.selected_text, Color::White);
        assert_eq!(theme.colors.user_message_background, Color::Rgb(52, 53, 65));
        assert_eq!(theme.colors.user_message_text, Color::White);
        assert_eq!(theme.colors.tool_pending_background, Color::Rgb(40, 40, 50));
        assert_eq!(theme.colors.tool_success_background, Color::Rgb(40, 50, 40));
        assert_eq!(theme.colors.tool_error_background, Color::Rgb(60, 40, 40));
        assert_eq!(theme.colors.tool_title, Color::White);
        assert_eq!(theme.colors.tool_output, Color::Rgb(128, 128, 128));
        assert_eq!(theme.colors.diff_added, Color::Rgb(181, 189, 104));
        assert_eq!(theme.colors.diff_removed, Color::Rgb(204, 102, 102));
        assert_eq!(theme.colors.diff_context, Color::Rgb(128, 128, 128));
        assert_eq!(theme.colors.diff_hunk, Color::Rgb(0, 215, 255));
        assert_eq!(theme.colors.harness, Color::Magenta);
        assert_eq!(theme.spacing.user_message_horizontal_padding, 1);
        assert_eq!(theme.spacing.user_message_vertical_padding, 1);
        assert_eq!(theme.spacing.tool_horizontal_padding, 1);
        assert_eq!(theme.spacing.tool_vertical_padding, 1);
        assert_eq!(theme.spacing.tool_gap, 1);

        // The two values Task 5 will change are still the old ones here.
        assert_ne!(
            theme.colors.tool_success_background,
            theme.colors.tool_pending_background
        );
        assert_eq!(theme.colors.tool_output, Color::Rgb(128, 128, 128));
    }

    #[test]
    fn legacy_file_with_all_22_tokens_round_trips() {
        // Every token set to a value that is not the dark default, so a
        // dropped or mis-named token fails this test instead of hiding
        // behind the preset.
        let json = r##"{
            "colors": {
                "accent": "#010101",
                "border": "#020202",
                "success": "#030303",
                "error": "#040404",
                "warning": "#050505",
                "muted": "#060606",
                "dim": "#070707",
                "text": "#080808",
                "selectedBackground": "#090909",
                "selectedText": "#0a0a0a",
                "userMessageBackground": "#0b0b0b",
                "userMessageText": "#0c0c0c",
                "toolPendingBackground": "#0d0d0d",
                "toolSuccessBackground": "#0e0e0e",
                "toolErrorBackground": "#0f0f0f",
                "toolTitle": "#101010",
                "toolOutput": "#111111",
                "diffAdded": "#121212",
                "diffRemoved": "#131313",
                "diffContext": "#141414",
                "diffHunk": "#151515",
                "harness": "#161616"
            }
        }"##;
        let document = ThemeDocument::parse(json);
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        assert_eq!(document.base(), None);
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else { return };

        assert_eq!(theme.mode(), ThemeMode::Painted);
        assert_eq!(theme.colors.accent, Color::Rgb(1, 1, 1));
        assert_eq!(theme.colors.border, Color::Rgb(2, 2, 2));
        assert_eq!(theme.colors.success, Color::Rgb(3, 3, 3));
        assert_eq!(theme.colors.error, Color::Rgb(4, 4, 4));
        assert_eq!(theme.colors.warning, Color::Rgb(5, 5, 5));
        assert_eq!(theme.colors.muted, Color::Rgb(6, 6, 6));
        assert_eq!(theme.colors.dim, Color::Rgb(7, 7, 7));
        assert_eq!(theme.colors.text, Color::Rgb(8, 8, 8));
        assert_eq!(theme.colors.selected_background, Color::Rgb(9, 9, 9));
        assert_eq!(theme.colors.selected_text, Color::Rgb(10, 10, 10));
        assert_eq!(theme.colors.user_message_background, Color::Rgb(11, 11, 11));
        assert_eq!(theme.colors.user_message_text, Color::Rgb(12, 12, 12));
        assert_eq!(theme.colors.tool_pending_background, Color::Rgb(13, 13, 13));
        assert_eq!(theme.colors.tool_success_background, Color::Rgb(14, 14, 14));
        assert_eq!(theme.colors.tool_error_background, Color::Rgb(15, 15, 15));
        assert_eq!(theme.colors.tool_title, Color::Rgb(16, 16, 16));
        assert_eq!(theme.colors.tool_output, Color::Rgb(17, 17, 17));
        assert_eq!(theme.colors.diff_added, Color::Rgb(18, 18, 18));
        assert_eq!(theme.colors.diff_removed, Color::Rgb(19, 19, 19));
        assert_eq!(theme.colors.diff_context, Color::Rgb(20, 20, 20));
        assert_eq!(theme.colors.diff_hunk, Color::Rgb(21, 21, 21));
        assert_eq!(theme.colors.harness, Color::Rgb(22, 22, 22));
        // Spacing is untouched when the file omits it.
        assert_eq!(theme.spacing, Theme::dark().spacing);
    }

    #[test]
    fn file_without_base_is_dark_plus_overrides() {
        let document = ThemeDocument::parse(
            r##"{
                "vars": { "surface": "#101820" },
                "colors": {
                    "userMessageBackground": "surface",
                    "toolSuccessBackground": 22,
                    "accent": "darkGray"
                },
                "spacing": { "userMessageVerticalPadding": 2, "toolGap": 3 }
            }"##,
        );
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        assert_eq!(document.base(), None);
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else { return };

        assert_eq!(theme.colors.user_message_background, Color::Rgb(16, 24, 32));
        assert_eq!(theme.colors.tool_success_background, Color::Indexed(22));
        assert_eq!(theme.colors.accent, Color::DarkGray);
        assert_eq!(theme.spacing.user_message_vertical_padding, 2);
        assert_eq!(theme.spacing.tool_gap, 3);
        // An omitted token keeps the dark value, not a zeroed one.
        assert_eq!(theme.colors.text, Theme::dark().colors.text);
        assert_eq!(theme.colors.error, Theme::dark().colors.error);
        assert_eq!(theme.mode(), ThemeMode::Painted);
    }

    #[test]
    fn base_key_parses_and_is_exposed_but_not_resolved() {
        // Resolution of the name is Task 4. Task 1 only records it and
        // applies tokens over whatever base the caller hands in.
        let document = ThemeDocument::parse(r##"{ "base": "light", "colors": { "accent": "#abcdef" } }"##);
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        assert_eq!(document.base(), Some("light"));

        let over_dark = document.apply(Theme::dark());
        assert!(over_dark.is_ok());
        let Ok(over_dark) = over_dark else { return };
        assert_eq!(over_dark.mode(), ThemeMode::Painted);
        assert_eq!(over_dark.colors.accent, Color::Rgb(0xab, 0xcd, 0xef));
        assert_eq!(over_dark.colors.text, Theme::dark().colors.text);

        let over_system = document.apply(Theme::system());
        assert!(over_system.is_ok());
        let Ok(over_system) = over_system else { return };
        // apply keeps the base theme's mode rather than inventing one.
        assert_eq!(over_system.mode(), ThemeMode::System);
        assert_eq!(over_system.colors.text, Color::Reset);
    }

    #[test]
    fn document_exposes_color_token_and_var_names() {
        let document = ThemeDocument::parse(
            r##"{ "vars": { "base00": "#111111", "base0F": "#ffffff" },
                 "colors": { "text": "base00", "accent": "#abcdef" } }"##,
        );
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        let mut tokens: Vec<&str> = document.color_tokens().collect();
        tokens.sort_unstable();
        assert_eq!(tokens, ["accent", "text"]);
        let mut vars: Vec<&str> = document.var_names().collect();
        vars.sort_unstable();
        assert_eq!(vars, ["base00", "base0F"]);
    }

    #[test]
    fn unknown_token_unknown_var_and_cycle_are_rejected() {
        let unknown = ThemeDocument::parse(r#"{"colors":{"unknown":"red"}}"#);
        assert!(matches!(
            unknown,
            Err(ThemeLoadError::UnknownColorToken(token)) if token == "unknown"
        ));

        let missing = ThemeDocument::parse(r#"{"colors":{"accent":"nope"}}"#);
        assert!(matches!(
            missing,
            Err(ThemeLoadError::UnknownVariable(name)) if name == "nope"
        ));

        let cycle = ThemeDocument::parse(
            r#"{ "vars": { "a": "b", "b": "a" }, "colors": { "accent": "a" } }"#,
        );
        assert!(matches!(cycle, Err(ThemeLoadError::VariableCycle(_))));

        // deny_unknown_fields survives the new `base` key.
        let extra = ThemeDocument::parse(r#"{ "name": "nope" }"#);
        assert!(matches!(extra, Err(ThemeLoadError::Json(_))));

        let bad_color = ThemeDocument::parse(r##"{"colors":{"accent":"#xyz"}}"##);
        assert!(matches!(bad_color, Err(ThemeLoadError::InvalidColor(_))));
    }

    #[test]
    fn dark_and_light_paint_explicit_pairs_at_spec_contrast() {
        for theme in [Theme::dark(), Theme::light()] {
            for (name, surface) in painted_surfaces(&theme) {
                assert!(!is_reset(fg(surface)), "{name} fg is Reset");
                assert!(!is_reset(bg(surface)), "{name} bg is Reset");
                let ratio = contrast_ratio(fg(surface), bg(surface));
                assert!(ratio.is_some(), "{name} contrast not measurable");
                let Some(ratio) = ratio else { return };
                assert!(
                    ratio >= 4.5,
                    "{name} text/panel contrast {ratio} is below 4.5:1"
                );
            }
            // muted is metadata on the panel, not body text: 3:1, not 4.5.
            let muted = contrast_ratio(fg(theme.muted()), bg(theme.tool_surface(ToolState::Pending)));
            assert!(muted.is_some());
            let Some(muted) = muted else { return };
            assert!(muted >= 3.0, "muted contrast {muted} is below 3:1");

            // Body text also has to clear 4.5:1 on the terminal-default
            // assumption the tests use: measured against the panel, because
            // that is the painted surface it can land on.
            let text_on_panel = contrast_ratio(
                fg(theme.text()),
                bg(theme.tool_surface(ToolState::Pending)),
            );
            assert!(text_on_panel.is_some_and(|ratio| ratio >= 4.5));
        }
    }

    #[test]
    fn system_paints_no_background_and_uses_ansi_status() {
        let theme = Theme::system();
        assert_eq!(theme.mode(), ThemeMode::System);
        for (_, surface) in painted_surfaces(&theme) {
            assert!(is_reset(bg(surface)), "system surface painted a background");
        }
        assert!(is_reset(bg(theme.tool_surface(ToolState::Harness))));
        assert!(is_reset(bg(theme.tool_output(ToolState::Pending))));
        assert!(is_reset(fg(theme.text())));
        assert!(is_reset(fg(theme.muted())));
        assert!(is_reset(fg(theme.dim())));
        assert!(is_reset(fg(theme.border(true))));
        assert_eq!(fg(theme.success()), Color::Green);
        assert_eq!(fg(theme.error()), Color::Red);
        assert_eq!(fg(theme.warning()), Color::Yellow);
        assert_eq!(fg(theme.harness()), Color::Magenta);
        assert_eq!(fg(theme.accent()), Color::Cyan);
        assert_eq!(fg(theme.diff(DiffKind::Added)), Color::Green);
        assert_eq!(fg(theme.diff(DiffKind::Removed)), Color::Red);
        assert_eq!(fg(theme.diff(DiffKind::Hunk)), Color::Cyan);
        // System status colors are terminal-relative, so helpers dim the
        // roles that would otherwise be indistinguishable from body text.
        assert!(theme.muted().add_modifier.contains(Modifier::DIM));
        assert!(theme.dim().add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn monochrome_is_reset_plus_the_modifier_table() {
        let theme = Theme::monochrome();
        assert_eq!(theme.mode(), ThemeMode::Mono);
        let colors = theme.colors;
        for color in [
            colors.accent, colors.border, colors.success, colors.error, colors.warning,
            colors.muted, colors.dim, colors.text, colors.selected_background,
            colors.selected_text, colors.user_message_background, colors.user_message_text,
            colors.tool_pending_background, colors.tool_success_background,
            colors.tool_error_background, colors.tool_title, colors.tool_output,
            colors.diff_added, colors.diff_removed, colors.diff_context, colors.diff_hunk,
            colors.harness,
        ] {
            assert_eq!(color, Color::Reset);
        }

        assert!(theme.muted().add_modifier.contains(Modifier::DIM));
        assert!(theme.dim().add_modifier.contains(Modifier::DIM));
        assert!(theme.accent().add_modifier.contains(Modifier::BOLD));
        assert!(theme.tool_title().add_modifier.contains(Modifier::BOLD));
        assert!(theme.error().add_modifier.contains(Modifier::BOLD));
        assert!(theme.selected().add_modifier.contains(Modifier::REVERSED));
        assert!(is_reset(bg(theme.selected())));
        assert!(is_reset(bg(theme.user_message())));
        assert!(is_reset(bg(theme.tool_surface(ToolState::Error))));

        // Glyphs and prefixes carry these, not hue.
        assert!(!theme.success().add_modifier.contains(Modifier::BOLD));
        assert!(!theme.warning().add_modifier.contains(Modifier::BOLD));
        assert!(!theme.harness().add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.diff(DiffKind::Added).add_modifier, Modifier::empty());
        assert_eq!(theme.diff(DiffKind::Removed).add_modifier, Modifier::empty());
    }

    #[test]
    fn light_and_system_share_pending_and_success_surfaces() {
        for theme in [Theme::light()] {
            assert_eq!(
                theme.tool_surface(ToolState::Pending),
                theme.tool_surface(ToolState::Success),
            );
            assert_ne!(
                theme.tool_surface(ToolState::Error),
                theme.tool_surface(ToolState::Pending),
            );
        }
        // `system` paints no panel in any state; the rule carries the state.
        let system = Theme::system();
        for state in [
            ToolState::Pending,
            ToolState::Success,
            ToolState::Error,
            ToolState::Harness,
        ] {
            assert_eq!(system.tool_surface(state).bg, Some(Color::Reset));
        }
        // dark is deliberately NOT asserted here. Its success surface is
        // still the green Rgb(40, 50, 40); Task 5 changes that default.
        assert_ne!(
            Theme::dark().tool_surface(ToolState::Pending),
            Theme::dark().tool_surface(ToolState::Success),
        );
    }

    #[test]
    fn helpers_follow_the_role_table() {
        let theme = Theme::dark();
        assert_eq!(fg(theme.text()), theme.colors.text);
        assert_eq!(bg(theme.text()), Color::Reset);
        assert_eq!(fg(theme.border(true)), theme.colors.accent);
        assert_eq!(fg(theme.border(false)), theme.colors.border);
        assert_eq!(fg(theme.selected()), theme.colors.selected_text);
        assert_eq!(bg(theme.selected()), theme.colors.selected_background);
        assert_eq!(fg(theme.user_message()), theme.colors.user_message_text);
        assert_eq!(bg(theme.user_message()), theme.colors.user_message_background);

        assert_eq!(bg(theme.tool_surface(ToolState::Pending)), theme.colors.tool_pending_background);
        assert_eq!(bg(theme.tool_surface(ToolState::Success)), theme.colors.tool_success_background);
        assert_eq!(bg(theme.tool_surface(ToolState::Error)), theme.colors.tool_error_background);
        assert_eq!(bg(theme.tool_surface(ToolState::Harness)), theme.colors.tool_error_background);
        assert_eq!(fg(theme.tool_surface(ToolState::Pending)), theme.colors.text);

        assert_eq!(fg(theme.tool_rule(ToolState::Pending)), theme.colors.warning);
        assert_eq!(fg(theme.tool_rule(ToolState::Success)), theme.colors.dim);
        assert_eq!(fg(theme.tool_rule(ToolState::Error)), theme.colors.error);
        assert_eq!(fg(theme.tool_rule(ToolState::Harness)), theme.colors.harness);

        assert!(theme.tool_title().add_modifier.contains(Modifier::BOLD));
        assert_eq!(fg(theme.tool_title()), theme.colors.tool_title);
        assert_eq!(fg(theme.tool_output(ToolState::Success)), theme.colors.tool_output);
        assert_eq!(
            bg(theme.tool_output(ToolState::Success)),
            bg(theme.tool_surface(ToolState::Success))
        );
        assert_eq!(fg(theme.diff(DiffKind::Added)), theme.colors.diff_added);
        assert_eq!(fg(theme.diff(DiffKind::Context)), theme.colors.diff_context);
    }

    #[test]
    fn contrast_ratio_covers_reset_named_and_indexed() {
        // Reset is not measurable; a caller must not treat it as passing.
        assert_eq!(contrast_ratio(Color::Reset, Color::Rgb(0, 0, 0)), None);
        assert_eq!(contrast_ratio(Color::White, Color::Reset), None);

        // Black on white is 21:1. White here is the named ANSI color, which
        // maps to xterm's (229, 229, 229), not pure (255, 255, 255).
        let named = contrast_ratio(Color::White, Color::Black);
        assert!(named.is_some());
        let Some(named) = named else { return };
        assert!((named - 17.3).abs() < 0.2, "named white/black was {named}");

        let pure = contrast_ratio(Color::Rgb(255, 255, 255), Color::Rgb(0, 0, 0));
        assert!(pure.is_some_and(|ratio| (ratio - 21.0).abs() < 0.01));

        // Indexed 16 is the cube origin (0, 0, 0); 231 is (255, 255, 255);
        // 232 is gray 8; 255 is gray 238.
        assert_eq!(
            contrast_ratio(Color::Indexed(231), Color::Indexed(16)),
            contrast_ratio(Color::Rgb(255, 255, 255), Color::Rgb(0, 0, 0)),
        );
        let gray = contrast_ratio(Color::Indexed(255), Color::Indexed(232));
        assert!(gray.is_some_and(|ratio| ratio > 1.0));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-ui --lib theme::tests`

Expected: FAIL to compile. `ThemeDocument`, `Theme::dark`, `ThemeMode`, `ToolState`, `DiffKind`, and `crate::theme::contrast::contrast_ratio` are unresolved. Do not "fix" this by stubbing the assertions.

- [ ] **Step 3: Add the presets module**

Create `crates/yach-ui/src/theme/presets.rs`. `dark()` copies `theme.rs:52-86` exactly, including `tool_success_background: Color::Rgb(40, 50, 40)` and `tool_output: Color::Rgb(128, 128, 128)`. Task 5 changes those two; this task must not.

```rust
use ratatui::style::Color;

use super::{Theme, ThemeColors, ThemeMode, ThemeSpacing};

pub(super) const fn dark() -> Theme {
    Theme {
        colors: ThemeColors {
            accent: Color::Rgb(0, 215, 255),
            border: Color::Rgb(95, 135, 255),
            success: Color::Rgb(181, 189, 104),
            error: Color::Rgb(204, 102, 102),
            warning: Color::Yellow,
            muted: Color::Rgb(128, 128, 128),
            dim: Color::Rgb(102, 102, 102),
            text: Color::White,
            selected_background: Color::Rgb(58, 58, 74),
            selected_text: Color::White,
            user_message_background: Color::Rgb(52, 53, 65),
            user_message_text: Color::White,
            tool_pending_background: Color::Rgb(40, 40, 50),
            tool_success_background: Color::Rgb(40, 50, 40),
            tool_error_background: Color::Rgb(60, 40, 40),
            tool_title: Color::White,
            tool_output: Color::Rgb(128, 128, 128),
            diff_added: Color::Rgb(181, 189, 104),
            diff_removed: Color::Rgb(204, 102, 102),
            diff_context: Color::Rgb(128, 128, 128),
            diff_hunk: Color::Rgb(0, 215, 255),
            harness: Color::Magenta,
        },
        spacing: ThemeSpacing {
            user_message_horizontal_padding: 1,
            user_message_vertical_padding: 1,
            tool_horizontal_padding: 1,
            tool_vertical_padding: 1,
            tool_gap: 1,
        },
        mode: ThemeMode::Painted,
    }
}

/// Measured against WCAG relative luminance, 2026-10-06. Every body-text
/// role is at least 4.5:1 on every painted surface; accents are at least
/// 3:1 on the panel. Do not "tidy" a channel without re-measuring.
pub(super) const fn light() -> Theme {
    Theme {
        colors: ThemeColors {
            text: Color::Rgb(36, 36, 48),
            muted: Color::Rgb(98, 98, 112),
            dim: Color::Rgb(150, 150, 160),
            accent: Color::Rgb(0, 95, 175),
            border: Color::Rgb(120, 130, 170),
            success: Color::Rgb(30, 120, 50),
            error: Color::Rgb(176, 36, 36),
            warning: Color::Rgb(150, 96, 0),
            harness: Color::Rgb(140, 40, 150),
            selected_background: Color::Rgb(208, 216, 236),
            selected_text: Color::Rgb(36, 36, 48),
            user_message_background: Color::Rgb(228, 231, 240),
            user_message_text: Color::Rgb(36, 36, 48),
            tool_pending_background: Color::Rgb(236, 236, 242),
            tool_success_background: Color::Rgb(236, 236, 242),
            tool_error_background: Color::Rgb(248, 228, 228),
            tool_title: Color::Rgb(36, 36, 48),
            tool_output: Color::Rgb(64, 64, 78),
            diff_added: Color::Rgb(30, 120, 50),
            diff_removed: Color::Rgb(176, 36, 36),
            diff_context: Color::Rgb(98, 98, 112),
            diff_hunk: Color::Rgb(0, 95, 175),
        },
        spacing: ThemeSpacing {
            user_message_horizontal_padding: 1,
            user_message_vertical_padding: 1,
            tool_horizontal_padding: 1,
            tool_vertical_padding: 1,
            tool_gap: 1,
        },
        mode: ThemeMode::Painted,
    }
}

/// No painted surface. Status uses the terminal's ANSI colors; everything
/// else is `Reset`, so a light or dark terminal stays legible.
pub(super) const fn system() -> Theme {
    Theme {
        colors: ThemeColors {
            accent: Color::Cyan,
            border: Color::Reset,
            success: Color::Green,
            error: Color::Red,
            warning: Color::Yellow,
            muted: Color::Reset,
            dim: Color::Reset,
            text: Color::Reset,
            selected_background: Color::Reset,
            selected_text: Color::Reset,
            user_message_background: Color::Reset,
            user_message_text: Color::Reset,
            tool_pending_background: Color::Reset,
            tool_success_background: Color::Reset,
            tool_error_background: Color::Reset,
            tool_title: Color::Reset,
            tool_output: Color::Reset,
            diff_added: Color::Green,
            diff_removed: Color::Red,
            diff_context: Color::Reset,
            diff_hunk: Color::Cyan,
            harness: Color::Magenta,
        },
        spacing: ThemeSpacing {
            user_message_horizontal_padding: 1,
            user_message_vertical_padding: 1,
            tool_horizontal_padding: 1,
            tool_vertical_padding: 1,
            tool_gap: 1,
        },
        mode: ThemeMode::System,
    }
}

pub(super) const fn monochrome() -> Theme {
    Theme {
        colors: ThemeColors {
            accent: Color::Reset,
            border: Color::Reset,
            success: Color::Reset,
            error: Color::Reset,
            warning: Color::Reset,
            muted: Color::Reset,
            dim: Color::Reset,
            text: Color::Reset,
            selected_background: Color::Reset,
            selected_text: Color::Reset,
            user_message_background: Color::Reset,
            user_message_text: Color::Reset,
            tool_pending_background: Color::Reset,
            tool_success_background: Color::Reset,
            tool_error_background: Color::Reset,
            tool_title: Color::Reset,
            tool_output: Color::Reset,
            diff_added: Color::Reset,
            diff_removed: Color::Reset,
            diff_context: Color::Reset,
            diff_hunk: Color::Reset,
            harness: Color::Reset,
        },
        spacing: ThemeSpacing {
            user_message_horizontal_padding: 1,
            user_message_vertical_padding: 1,
            tool_horizontal_padding: 1,
            tool_vertical_padding: 1,
            tool_gap: 1,
        },
        mode: ThemeMode::Mono,
    }
}
```

- [ ] **Step 4: Add the contrast module**

Create `crates/yach-ui/src/theme/contrast.rs`. The 16 named colors are the xterm default ANSI palette (not the VGA palette: Blue is `(0, 0, 238)`, not `(0, 0, 205)`). Cube and gray ramp follow xterm's `256colres.pl`: cube channel `n * 40 + 55` for `n > 0`, else 0; gray `8 + 10 * (index - 232)`.

```rust
//! WCAG 2.x contrast over the colors a theme can name.
//!
//! Test-only: presets and palettes assert contrast here, widgets do not.

use ratatui::style::Color;

/// Contrast ratio of `a` against `b`. `None` when either color is `Reset`
/// or has no RGB, so an unmeasurable pair cannot pass a `>=` check by
/// accident.
#[must_use]
pub(crate) fn contrast_ratio(a: Color, b: Color) -> Option<f64> {
    let a = relative_luminance(a)?;
    let b = relative_luminance(b)?;
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    Some((lighter + 0.05) / (darker + 0.05))
}

fn relative_luminance(color: Color) -> Option<f64> {
    let (red, green, blue) = to_rgb(color)?;
    Some(
        0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue),
    )
}

fn linearize(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn to_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Reset => None,
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        Color::Indexed(index) => Some(indexed_rgb(index)),
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((205, 0, 0)),
        Color::Green => Some((0, 205, 0)),
        Color::Yellow => Some((205, 205, 0)),
        Color::Blue => Some((0, 0, 238)),
        Color::Magenta => Some((205, 0, 205)),
        Color::Cyan => Some((0, 205, 205)),
        Color::Gray => Some((229, 229, 229)),
        Color::DarkGray => Some((127, 127, 127)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((92, 92, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((229, 229, 229)),
    }
}

/// xterm 256-color RGB. 0-15 are the same named ANSI defaults; 16-231 are
/// the 6x6x6 cube; 232-255 are the gray ramp that deliberately skips black
/// and white.
fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0 => (0, 0, 0),
        1 => (205, 0, 0),
        2 => (0, 205, 0),
        3 => (205, 205, 0),
        4 => (0, 0, 238),
        5 => (205, 0, 205),
        6 => (0, 205, 205),
        7 => (229, 229, 229),
        8 => (127, 127, 127),
        9 => (255, 0, 0),
        10 => (0, 255, 0),
        11 => (255, 255, 0),
        12 => (92, 92, 255),
        13 => (255, 0, 255),
        14 => (0, 255, 255),
        15 => (255, 255, 255),
        16..=231 => cube_rgb(index),
        232..=255 => {
            let level = index.saturating_sub(232).saturating_mul(10).saturating_add(8);
            (level, level, level)
        }
    }
}

fn cube_rgb(index: u8) -> (u8, u8, u8) {
    let offset = index - 16;
    let red = offset / 36;
    let green = (offset % 36) / 6;
    let blue = offset % 6;
    (cube_channel(red), cube_channel(green), cube_channel(blue))
}

fn cube_channel(level: u8) -> u8 {
    if level == 0 {
        0
    } else {
        level.saturating_mul(40).saturating_add(55)
    }
}
```

- [ ] **Step 5: Rewrite `theme.rs` around the document and the helpers**

Replace `crates/yach-ui/src/theme.rs` with the module below. It keeps `parse_color` (`theme.rs:244-268`), `resolve_color` (`theme.rs:220-242`), `ThemeSpacing::apply` (`theme.rs:141-159`), and the token match in `set_color` (`theme.rs:104-132`) verbatim apart from moving them onto `ThemeDocument::apply`. Delete `Theme::pi_dark`, `Theme::from_json`, `Theme::load`, the `Io` variant, and the `fs`/`Path` imports: nothing in this crate reads a file after this step.

`border(focused)` returns the accent color when focused and the border color otherwise, because that is what `input_styles` does today (`input.rs:38-55`): the focused border is `colors.border`, the focused title is `colors.accent` bold, and the unfocused border is `colors.dim` plus `DIM`. Widgets that draw a frame call `border(false)`; the focused title is `accent()`. Task 3 documents the two sites that do not fit a helper.

```rust
mod presets;
#[cfg(test)]
mod contrast;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Painted,
    System,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Pending,
    Success,
    Error,
    Harness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Removed,
    Context,
    Hunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub colors: ThemeColors,
    pub spacing: ThemeSpacing,
    mode: ThemeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    pub accent: Color,
    pub border: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub muted: Color,
    pub dim: Color,
    pub text: Color,
    pub selected_background: Color,
    pub selected_text: Color,
    pub user_message_background: Color,
    pub user_message_text: Color,
    pub tool_pending_background: Color,
    pub tool_success_background: Color,
    pub tool_error_background: Color,
    pub tool_title: Color,
    pub tool_output: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_context: Color,
    pub diff_hunk: Color,
    pub harness: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeSpacing {
    pub user_message_horizontal_padding: u16,
    pub user_message_vertical_padding: u16,
    pub tool_horizontal_padding: u16,
    pub tool_vertical_padding: u16,
    pub tool_gap: u16,
}

impl Theme {
    #[must_use]
    pub const fn dark() -> Self {
        presets::dark()
    }

    #[must_use]
    pub const fn light() -> Self {
        presets::light()
    }

    #[must_use]
    pub const fn system() -> Self {
        presets::system()
    }

    #[must_use]
    pub const fn monochrome() -> Self {
        presets::monochrome()
    }

    #[must_use]
    pub const fn mode(&self) -> ThemeMode {
        self.mode
    }

    #[must_use]
    pub fn text(&self) -> Style {
        self.foreground(self.colors.text, Modifier::empty())
    }

    #[must_use]
    pub fn muted(&self) -> Style {
        self.foreground(self.colors.muted, self.dim_modifier())
    }

    #[must_use]
    pub fn dim(&self) -> Style {
        self.foreground(self.colors.dim, self.dim_modifier())
    }

    #[must_use]
    pub fn accent(&self) -> Style {
        self.foreground(self.colors.accent, self.bold_unless_painted())
    }

    /// Focused frames use the accent color; unfocused frames use `border`.
    /// Monochrome drops both colors and bolds only the focused frame.
    #[must_use]
    pub fn border(&self, focused: bool) -> Style {
        let color = if focused { self.colors.accent } else { self.colors.border };
        let modifier = if focused { self.bold_unless_painted() } else { Modifier::empty() };
        self.foreground(color, modifier)
    }

    #[must_use]
    pub fn success(&self) -> Style {
        self.foreground(self.colors.success, Modifier::empty())
    }

    #[must_use]
    pub fn warning(&self) -> Style {
        self.foreground(self.colors.warning, Modifier::empty())
    }

    #[must_use]
    pub fn error(&self) -> Style {
        self.foreground(self.colors.error, self.bold_unless_painted())
    }

    #[must_use]
    pub fn harness(&self) -> Style {
        self.foreground(self.colors.harness, Modifier::empty())
    }

    #[must_use]
    pub fn selected(&self) -> Style {
        if self.mode == ThemeMode::Mono {
            return Style::new()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .add_modifier(Modifier::REVERSED);
        }
        Style::new()
            .fg(self.colors.selected_text)
            .bg(self.colors.selected_background)
    }

    #[must_use]
    pub fn user_message(&self) -> Style {
        self.surface(self.colors.user_message_text, self.colors.user_message_background)
    }

    /// Pending and Success share a background only when the preset says so.
    /// `dark` still has a distinct success green until Task 5. Error and
    /// Harness share the error background, matching today's harness outcome
    /// (`transcript.rs:1254-1263`).
    #[must_use]
    pub fn tool_surface(&self, state: ToolState) -> Style {
        let background = match state {
            ToolState::Pending => self.colors.tool_pending_background,
            ToolState::Success => self.colors.tool_success_background,
            ToolState::Error | ToolState::Harness => self.colors.tool_error_background,
        };
        self.surface(self.colors.text, background)
    }

    #[must_use]
    pub fn tool_rule(&self, state: ToolState) -> Style {
        let color = match state {
            ToolState::Pending => self.colors.warning,
            ToolState::Success => self.colors.dim,
            ToolState::Error => self.colors.error,
            ToolState::Harness => self.colors.harness,
        };
        self.foreground(color, Modifier::empty())
    }

    #[must_use]
    pub fn tool_title(&self) -> Style {
        self.foreground(self.colors.tool_title, Modifier::BOLD)
    }

    #[must_use]
    pub fn tool_output(&self, state: ToolState) -> Style {
        Style::new()
            .fg(self.colors.tool_output)
            .bg(self.tool_surface(state).bg.unwrap_or(Color::Reset))
    }

    #[must_use]
    pub fn diff(&self, kind: DiffKind) -> Style {
        let color = match kind {
            DiffKind::Added => self.colors.diff_added,
            DiffKind::Removed => self.colors.diff_removed,
            DiffKind::Context => self.colors.diff_context,
            DiffKind::Hunk => self.colors.diff_hunk,
        };
        self.foreground(color, Modifier::empty())
    }

    fn foreground(&self, color: Color, modifier: Modifier) -> Style {
        Style::new().fg(color).add_modifier(modifier)
    }

    fn surface(&self, foreground: Color, background: Color) -> Style {
        if self.mode == ThemeMode::Mono || self.mode == ThemeMode::System {
            return Style::new().fg(foreground).bg(Color::Reset);
        }
        Style::new().fg(foreground).bg(background)
    }

    fn dim_modifier(&self) -> Modifier {
        if self.mode == ThemeMode::Painted {
            Modifier::empty()
        } else {
            Modifier::DIM
        }
    }

    /// Painted presets already distinguish these roles by hue, so they stay
    /// regular weight. System and Mono have nothing but the attribute.
    fn bold_unless_painted(&self) -> Modifier {
        if self.mode == ThemeMode::Painted {
            Modifier::empty()
        } else {
            Modifier::BOLD
        }
    }

    fn set_color(&mut self, token: &str, color: Color) -> Result<(), ThemeLoadError> {
        let colors = &mut self.colors;
        match token {
            "accent" => colors.accent = color,
            "border" => colors.border = color,
            "success" => colors.success = color,
            "error" => colors.error = color,
            "warning" => colors.warning = color,
            "muted" => colors.muted = color,
            "dim" => colors.dim = color,
            "text" => colors.text = color,
            "selectedBackground" => colors.selected_background = color,
            "selectedText" => colors.selected_text = color,
            "userMessageBackground" => colors.user_message_background = color,
            "userMessageText" => colors.user_message_text = color,
            "toolPendingBackground" => colors.tool_pending_background = color,
            "toolSuccessBackground" => colors.tool_success_background = color,
            "toolErrorBackground" => colors.tool_error_background = color,
            "toolTitle" => colors.tool_title = color,
            "toolOutput" => colors.tool_output = color,
            "diffAdded" => colors.diff_added = color,
            "diffRemoved" => colors.diff_removed = color,
            "diffContext" => colors.diff_context = color,
            "diffHunk" => colors.diff_hunk = color,
            "harness" => colors.harness = color,
            _ => return Err(ThemeLoadError::UnknownColorToken(token.to_owned())),
        }
        Ok(())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl ThemeSpacing {
    fn apply(&mut self, overrides: &ThemeSpacingOverrides) {
        if let Some(value) = overrides.user_message_horizontal_padding {
            self.user_message_horizontal_padding = value;
        }
        if let Some(value) = overrides.user_message_vertical_padding {
            self.user_message_vertical_padding = value;
        }
        if let Some(value) = overrides.tool_horizontal_padding {
            self.tool_horizontal_padding = value;
        }
        if let Some(value) = overrides.tool_vertical_padding {
            self.tool_vertical_padding = value;
        }
        if let Some(value) = overrides.tool_gap {
            self.tool_gap = value;
        }
    }
}

#[derive(Debug)]
pub enum ThemeLoadError {
    Json(serde_json::Error),
    InvalidColor(String),
    UnknownColorToken(String),
    UnknownVariable(String),
    VariableCycle(String),
}

impl fmt::Display for ThemeLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "theme JSON is invalid: {error}"),
            Self::InvalidColor(value) => write!(formatter, "invalid theme color: {value}"),
            Self::UnknownColorToken(token) => {
                write!(formatter, "unknown theme color token: {token}")
            }
            Self::UnknownVariable(variable) => {
                write!(formatter, "unknown theme color or variable: {variable}")
            }
            Self::VariableCycle(variable) => {
                write!(formatter, "theme variable cycle includes: {variable}")
            }
        }
    }
}

impl std::error::Error for ThemeLoadError {}

/// A parsed theme file. `base` is recorded and not resolved: the caller
/// decides which `Theme` the tokens apply over (Task 4 owns name resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeDocument {
    base: Option<String>,
    vars: BTreeMap<String, ColorValue>,
    colors: BTreeMap<String, ColorValue>,
    spacing: ThemeSpacingOverrides,
}

impl ThemeDocument {
    pub fn parse(json: &str) -> Result<Self, ThemeLoadError> {
        let file: ThemeFile = serde_json::from_str(json).map_err(ThemeLoadError::Json)?;
        Ok(Self {
            base: file.base,
            vars: file.vars,
            colors: file.colors,
            spacing: file.spacing,
        })
    }

    #[must_use]
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    pub fn color_tokens(&self) -> impl Iterator<Item = &str> {
        self.colors.keys().map(String::as_str)
    }

    pub fn var_names(&self) -> impl Iterator<Item = &str> {
        self.vars.keys().map(String::as_str)
    }

    /// Overrides `base`'s tokens. The result keeps `base.mode`, so a file
    /// applied over `system` stays `system` even when it sets a color.
    pub fn apply(&self, mut base: Theme) -> Result<Theme, ThemeLoadError> {
        for (token, value) in &self.colors {
            let color = resolve_color(value, &self.vars, &mut BTreeSet::new())?;
            base.set_color(token, color)?;
        }
        base.spacing.apply(&self.spacing);
        Ok(base)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, ColorValue>,
    #[serde(default)]
    colors: BTreeMap<String, ColorValue>,
    #[serde(default)]
    spacing: ThemeSpacingOverrides,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum ColorValue {
    Name(String),
    Indexed(u8),
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct ThemeSpacingOverrides {
    user_message_horizontal_padding: Option<u16>,
    user_message_vertical_padding: Option<u16>,
    tool_horizontal_padding: Option<u16>,
    tool_vertical_padding: Option<u16>,
    tool_gap: Option<u16>,
}

fn resolve_color(
    value: &ColorValue,
    vars: &BTreeMap<String, ColorValue>,
    resolving: &mut BTreeSet<String>,
) -> Result<Color, ThemeLoadError> {
    match value {
        ColorValue::Indexed(index) => Ok(Color::Indexed(*index)),
        ColorValue::Name(value) => {
            if let Some(color) = parse_color(value) {
                return Ok(color);
            }
            let Some(variable) = vars.get(value) else {
                return Err(ThemeLoadError::UnknownVariable(value.clone()));
            };
            if !resolving.insert(value.clone()) {
                return Err(ThemeLoadError::VariableCycle(value.clone()));
            }
            let resolved = resolve_color(variable, vars, resolving);
            resolving.remove(value);
            resolved
        }
    }
}

fn parse_color(value: &str) -> Option<Color> {
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
        return Some(Color::Rgb(red, green, blue));
    }
    match value {
        "default" | "reset" | "" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "darkGray" => Some(Color::DarkGray),
        "white" => Some(Color::White),
        _ => None,
    }
}
```

Append the `tests` module from Step 1 at the bottom of this file.

- [ ] **Step 6: Migrate the two callers**

`crates/yach-cli/src/main.rs:425-432`. The file is still found by `tui_theme_path` (`main.rs:434-450`); only the load changes. A missing file still returns `Theme::default()`, which is now `Theme::dark()`.

```rust
fn load_tui_theme(project_root: Option<&Path>) -> Result<Theme, String> {
    let explicit = std::env::var_os("YACH_THEME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(path) = tui_theme_path(project_root, explicit.as_deref(), home.as_deref()) else {
        return Ok(Theme::default());
    };
    let json = std::fs::read_to_string(&path)
        .map_err(|error| format!("failed to load theme {}: {error}", path.display()))?;
    let document = yach_ui::ThemeDocument::parse(&json)
        .map_err(|error| format!("failed to load theme {}: {error}", path.display()))?;
    document
        .apply(Theme::dark())
        .map_err(|error| format!("failed to load theme {}: {error}", path.display()))
}
```

Add `ThemeDocument` to the `yach_ui` import at `main.rs:48-52`, and to the re-export at `lib.rs:25`:

```rust
pub use theme::{Theme, ThemeColors, ThemeDocument, ThemeLoadError, ThemeSpacing};
```

`crates/yach-ui/src/transcript.rs:1924-1936`, inside `custom_theme_colors_user_and_tool_surfaces`. The test asserts rendered backgrounds, so it must apply the document rather than read colors off it:

```rust
    fn custom_theme_colors_user_and_tool_surfaces() {
        let parsed = ThemeDocument::parse(
            r##"{
                "colors": {
                    "userMessageBackground": "#010203",
                    "userMessageText": "#040506",
                    "toolPendingBackground": "#070809"
                },
                "spacing": {
                    "userMessageVerticalPadding": 0,
                    "toolVerticalPadding": 0
                }
            }"##,
        );
        assert!(parsed.is_ok());
        let Ok(document) = parsed else {
            return;
        };
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else {
            return;
        };
```

Add `ThemeDocument` to that test module's `use crate::theme::Theme;` import. The rest of the test (`transcript.rs:1937-1978`) is unchanged and still reads `theme.colors`, which is legal until Task 3.

- [ ] **Step 7: Run to verify pass**

Run: `just dev cargo test -p yach-ui --lib theme::tests`

Expected: PASS. `dark_preset_keeps_todays_pi_dark_values` and `file_without_base_is_dark_plus_overrides` are the regression for today's `from_json`.

Run: `just dev cargo test -p yach-ui --lib transcript::tests::custom_theme_colors_user_and_tool_surfaces`

Expected: PASS. The custom backgrounds `#010203` / `#070809` still reach the rendered spans.

Run: `just dev cargo test -p yach tui_theme_path_prefers_explicit_then_project_then_user`

Expected: PASS. That test (`main.rs:6016`) checks path precedence only, which this task did not change.

- [ ] **Step 8: Lint**

Run: `just lint`

Expected: PASS. `theme.rs` stays under the 100-line function limit (`too-many-lines-threshold` in `clippy.toml`); `apply` and the helpers are each short. If `border` or `tool_surface` trips `cognitive-complexity-threshold` (15), split the match into a private color function rather than allowing the lint.

- [ ] **Step 9: Commit**

```bash
jj commit crates/yach-ui/src/theme.rs crates/yach-ui/src/theme crates/yach-ui/src/lib.rs crates/yach-ui/src/transcript.rs crates/yach-cli/src/main.rs -m "Add theme roles, presets, and theme documents"
```

---

### Task 2: Bundled named palettes

**Files:**
- Create: `crates/yach-ui/themes/catppuccin-latte.json`
- Create: `crates/yach-ui/themes/catppuccin-frappe.json`
- Create: `crates/yach-ui/themes/catppuccin-macchiato.json`
- Create: `crates/yach-ui/themes/catppuccin-mocha.json`
- Create: `crates/yach-ui/themes/rose-pine.json`
- Create: `crates/yach-ui/themes/rose-pine-moon.json`
- Create: `crates/yach-ui/themes/rose-pine-dawn.json`
- Create: `crates/yach-ui/themes/gruvbox-dark.json`
- Create: `crates/yach-ui/themes/gruvbox-light.json`
- Create: `crates/yach-ui/themes/tokyo-night-dark.json`
- Create: `crates/yach-ui/themes/tokyo-night-storm.json`
- Create: `crates/yach-ui/themes/tokyo-night-moon.json`
- Create: `crates/yach-ui/themes/tokyo-night-light.json`
- Create: `crates/yach-ui/themes/solarized-dark.json`
- Create: `crates/yach-ui/themes/solarized-light.json`
- Create: `crates/yach-ui/themes/dracula.json`
- Create: `crates/yach-ui/themes/nord.json`
- Create: `crates/yach-ui/themes/monokai.json`
- Create: `crates/yach-ui/themes/NOTICE.md`
- Create: `crates/yach-ui/src/theme/palettes.rs`
- Modify: `crates/yach-ui/src/theme.rs` (add `mod palettes;` next to `mod presets;`; Task 1 does not declare this module)
- Test: `crates/yach-ui/src/theme/palettes.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `ThemeDocument::parse(json: &str) -> Result<ThemeDocument, ThemeLoadError>`; `ThemeDocument::base(&self) -> Option<&str>`; `ThemeDocument::color_tokens(&self) -> impl Iterator<Item = &str>` (keys as written); `ThemeDocument::var_names(&self) -> impl Iterator<Item = &str>`; `ThemeDocument::apply(&self, base: Theme) -> Result<Theme, ThemeLoadError>`; `Theme::dark() -> Theme`; `Theme::light() -> Theme`; `Theme::tool_surface(&self, state: ToolState) -> Style`; `Theme::text/selected/user_message/tool_title/tool_output/muted/success/warning/error/harness(&self) -> Style`; `ToolState::{Pending, Success, Error}`; `contrast_ratio(a: Color, b: Color) -> Option<f64>` in `theme/contrast.rs` (Task 1; `#[cfg(test)] pub(crate)`). `Style::fg` / `Style::bg` return `Option<Color>`. `ThemeColors` fields stay `pub` until Task 3, but these tests must not read them.
- Produces:

```rust
pub struct BundledPalette {
    pub name: &'static str,
    pub json: &'static str,
}

/// Spec order: Catppuccin, Rosé Pine, Gruvbox, Tokyo Night, Solarized, then
/// `dracula`, `nord`, `monokai`.
pub(crate) const BUNDLED_PALETTES: [BundledPalette; 18];

#[must_use]
pub fn bundled_palette(name: &str) -> Option<&'static BundledPalette>;
```

The eighteen JSON files and `NOTICE.md` are the data Task 4's name set and the contrast tests read. Nothing in this task changes a rendered color: no caller names a palette until Task 4.

- [ ] **Step 1: Create `crates/yach-ui/themes/catppuccin-latte.json`** (1093 bytes, verbatim from the validated prototype; do not reformat)

Write exactly:

```json
{
  "base": "light",
  "vars": {
    "base00": "#eff1f5",
    "base01": "#e6e9ef",
    "base02": "#ccd0da",
    "base03": "#bcc0cc",
    "base04": "#acb0be",
    "base05": "#4c4f69",
    "base06": "#dc8a78",
    "base07": "#7287fd",
    "base08": "#d20f39",
    "base09": "#fe640b",
    "base0A": "#df8e1d",
    "base0B": "#40a02b",
    "base0C": "#179299",
    "base0D": "#1e66f5",
    "base0E": "#8839ef",
    "base0F": "#dd7878"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "#429434",
    "error": "base08",
    "warning": "#ac7838",
    "muted": "#818498",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#e3c8d4",
    "toolTitle": "base05",
    "toolOutput": "base05",
    "diffAdded": "#429434",
    "diffRemoved": "base08",
    "diffContext": "#818498",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 2: Create `crates/yach-ui/themes/catppuccin-frappe.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#303446",
    "base01": "#292c3c",
    "base02": "#414559",
    "base03": "#51576d",
    "base04": "#626880",
    "base05": "#c6d0f5",
    "base06": "#f2d5cf",
    "base07": "#babbf1",
    "base08": "#e78284",
    "base09": "#ef9f76",
    "base0A": "#e5c890",
    "base0B": "#a6d189",
    "base0C": "#81c8be",
    "base0D": "#8caaee",
    "base0E": "#ca9ee6",
    "base0F": "#eebebe"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "#717892",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#463947",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "#717892",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 3: Create `crates/yach-ui/themes/catppuccin-macchiato.json`** (1093 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#24273a",
    "base01": "#1e2030",
    "base02": "#363a4f",
    "base03": "#494d64",
    "base04": "#5b6078",
    "base05": "#cad3f5",
    "base06": "#f4dbd6",
    "base07": "#b7bdf8",
    "base08": "#ed8796",
    "base09": "#f5a97f",
    "base0A": "#eed49f",
    "base0B": "#a6da95",
    "base0C": "#8bd5ca",
    "base0D": "#8aadf4",
    "base0E": "#c6a0f6",
    "base0F": "#f0c6c6"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "#666c84",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#3d2f3f",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "#666c84",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 4: Create `crates/yach-ui/themes/catppuccin-mocha.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#1e1e2e",
    "base01": "#181825",
    "base02": "#313244",
    "base03": "#45475a",
    "base04": "#585b70",
    "base05": "#cdd6f4",
    "base06": "#f5e0dc",
    "base07": "#b4befe",
    "base08": "#f38ba8",
    "base09": "#fab387",
    "base0A": "#f9e2af",
    "base0B": "#a6e3a1",
    "base0C": "#94e2d5",
    "base0D": "#89b4fa",
    "base0E": "#cba6f7",
    "base0F": "#f2cdcd"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "#64677d",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#392939",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "#64677d",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 5: Create `crates/yach-ui/themes/rose-pine.json`** (1087 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#191724",
    "base01": "#1f1d2e",
    "base02": "#26233a",
    "base03": "#6e6a86",
    "base04": "#908caa",
    "base05": "#e0def4",
    "base06": "#e0def4",
    "base07": "#524f67",
    "base08": "#eb6f92",
    "base09": "#f6c177",
    "base0A": "#ebbcba",
    "base0B": "#31748f",
    "base0C": "#9ccfd8",
    "base0D": "#c4a7e7",
    "base0E": "#f6c177",
    "base0F": "#524f67"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#3e293d",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 6: Create `crates/yach-ui/themes/rose-pine-moon.json`** (1087 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#232136",
    "base01": "#2a273f",
    "base02": "#393552",
    "base03": "#6e6a86",
    "base04": "#908caa",
    "base05": "#e0def4",
    "base06": "#e0def4",
    "base07": "#56526e",
    "base08": "#eb6f92",
    "base09": "#f6c177",
    "base0A": "#ea9a97",
    "base0B": "#3e8fb0",
    "base0C": "#9ccfd8",
    "base0D": "#c4a7e7",
    "base0E": "#f6c177",
    "base0F": "#56526e"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#47324b",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 7: Create `crates/yach-ui/themes/rose-pine-dawn.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "light",
  "vars": {
    "base00": "#faf4ed",
    "base01": "#fffaf3",
    "base02": "#f2e9de",
    "base03": "#9893a5",
    "base04": "#797593",
    "base05": "#575279",
    "base06": "#575279",
    "base07": "#cecacd",
    "base08": "#b4637a",
    "base09": "#ea9d34",
    "base0A": "#d7827e",
    "base0B": "#286983",
    "base0C": "#56949f",
    "base0D": "#907aa9",
    "base0E": "#ea9d34",
    "base0F": "#cecacd"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "#c47b7d",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#f4e3e1",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 8: Create `crates/yach-ui/themes/gruvbox-dark.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#282828",
    "base01": "#3c3836",
    "base02": "#504945",
    "base03": "#665c54",
    "base04": "#928374",
    "base05": "#ebdbb2",
    "base06": "#fbf1c7",
    "base07": "#f9f5d7",
    "base08": "#cc241d",
    "base09": "#d65d0e",
    "base0A": "#d79921",
    "base0B": "#98971a",
    "base0C": "#689d6a",
    "base0D": "#458588",
    "base0E": "#b16286",
    "base0F": "#9d0006"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "#d76451",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#523532",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "#d76451",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 9: Create `crates/yach-ui/themes/gruvbox-light.json`** (1096 bytes)

Write exactly:

```json
{
  "base": "light",
  "vars": {
    "base00": "#fbf1c7",
    "base01": "#ebdbb2",
    "base02": "#d5c4a1",
    "base03": "#bdae93",
    "base04": "#7c6f64",
    "base05": "#3c3836",
    "base06": "#282828",
    "base07": "#1d2021",
    "base08": "#cc241d",
    "base09": "#d65d0e",
    "base0A": "#d79921",
    "base0B": "#98971a",
    "base0C": "#689d6a",
    "base0D": "#458588",
    "base0E": "#b16286",
    "base0F": "#9d0006"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "#817f21",
    "error": "base08",
    "warning": "#997229",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#e6c09c",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "#817f21",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "#5d845d"
  }
}
```

- [ ] **Step 10: Create `crates/yach-ui/themes/tokyo-night-dark.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#1a1b26",
    "base01": "#16161e",
    "base02": "#2f3549",
    "base03": "#444b6a",
    "base04": "#787c99",
    "base05": "#a9b1d6",
    "base06": "#cbccd1",
    "base07": "#d5d6db",
    "base08": "#c0caf5",
    "base09": "#a9b1d6",
    "base0A": "#0db9d7",
    "base0B": "#9ece6a",
    "base0C": "#b4f9f8",
    "base0D": "#2ac3de",
    "base0E": "#bb9af7",
    "base0F": "#f7768e"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#30313e",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 11: Create `crates/yach-ui/themes/tokyo-night-storm.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#24283b",
    "base01": "#16161e",
    "base02": "#343a52",
    "base03": "#444b6a",
    "base04": "#787c99",
    "base05": "#a9b1d6",
    "base06": "#cbccd1",
    "base07": "#d5d6db",
    "base08": "#c0caf5",
    "base09": "#a9b1d6",
    "base0A": "#0db9d7",
    "base0B": "#9ece6a",
    "base0C": "#b4f9f8",
    "base0D": "#2ac3de",
    "base0E": "#bb9af7",
    "base0F": "#f7768e"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#30313e",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 12: Create `crates/yach-ui/themes/tokyo-night-moon.json`** (1093 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#222436",
    "base01": "#1e2030",
    "base02": "#2d3f76",
    "base03": "#3b4261",
    "base04": "#636da6",
    "base05": "#828bb8",
    "base06": "#aeb4d1",
    "base07": "#c8d3f5",
    "base08": "#ff757f",
    "base09": "#ffc777",
    "base0A": "#ffdf77",
    "base0B": "#c3e88d",
    "base0C": "#86e1fc",
    "base0D": "#82aaff",
    "base0E": "#fca7ea",
    "base0F": "#c53b53"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base06",
    "selectedBackground": "base02",
    "selectedText": "base06",
    "userMessageBackground": "base01",
    "userMessageText": "base06",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#402d3c",
    "toolTitle": "base06",
    "toolOutput": "base06",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 13: Create `crates/yach-ui/themes/tokyo-night-light.json`** (1090 bytes)

Write exactly:

```json
{
  "base": "light",
  "vars": {
    "base00": "#d5d6db",
    "base01": "#cbccd1",
    "base02": "#dfe0e5",
    "base03": "#9699a3",
    "base04": "#4c505e",
    "base05": "#343b59",
    "base06": "#1a1b26",
    "base07": "#1a1b26",
    "base08": "#343b58",
    "base09": "#965027",
    "base0A": "#166775",
    "base0B": "#485e30",
    "base0C": "#3e6968",
    "base0D": "#34548a",
    "base0E": "#5a4a78",
    "base0F": "#8c4351"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#b4b6bf",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 14: Create `crates/yach-ui/themes/solarized-dark.json`** (1093 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#002b36",
    "base01": "#073642",
    "base02": "#586e75",
    "base03": "#657b83",
    "base04": "#839496",
    "base05": "#93a1a1",
    "base06": "#eee8d5",
    "base07": "#fdf6e3",
    "base08": "#dc322f",
    "base09": "#cb4b16",
    "base0A": "#b58900",
    "base0B": "#859900",
    "base0C": "#2aa198",
    "base0D": "#268bd2",
    "base0E": "#6c71c4",
    "base0F": "#d33682"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "#c25957",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base07",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#27353f",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "#c25957",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 15: Create `crates/yach-ui/themes/solarized-light.json`** (1099 bytes)

Write exactly:

```json
{
  "base": "light",
  "vars": {
    "base00": "#fdf6e3",
    "base01": "#eee8d5",
    "base02": "#93a1a1",
    "base03": "#839496",
    "base04": "#657b83",
    "base05": "#586e75",
    "base06": "#073642",
    "base07": "#002b36",
    "base08": "#dc322f",
    "base09": "#cb4b16",
    "base0A": "#b58900",
    "base0B": "#859900",
    "base0C": "#2aa198",
    "base0D": "#268bd2",
    "base0E": "#6c71c4",
    "base0F": "#d33682"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "#788c23",
    "error": "base08",
    "warning": "#9e821d",
    "muted": "base04",
    "dim": "base03",
    "text": "base06",
    "selectedBackground": "base02",
    "selectedText": "base06",
    "userMessageBackground": "base01",
    "userMessageText": "base06",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#ebcdbc",
    "toolTitle": "base06",
    "toolOutput": "base06",
    "diffAdded": "#788c23",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "#38928e"
  }
}
```

- [ ] **Step 16: Create `crates/yach-ui/themes/dracula.json`** (1084 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#282a36",
    "base01": "#21222c",
    "base02": "#44475a",
    "base03": "#6272a4",
    "base04": "#9ea8c7",
    "base05": "#f8f8f2",
    "base06": "#f8f8f2",
    "base07": "#ffffff",
    "base08": "#ff5555",
    "base09": "#ffb86c",
    "base0A": "#f1fa8c",
    "base0B": "#50fa7b",
    "base0C": "#8be9fd",
    "base0D": "#bd93f9",
    "base0E": "#ff79c6",
    "base0F": "#993333"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#422a32",
    "toolTitle": "base06",
    "toolOutput": "base04",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 17: Create `crates/yach-ui/themes/nord.json`** (1084 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#2e3440",
    "base01": "#3b4252",
    "base02": "#434c5e",
    "base03": "#4c566a",
    "base04": "#d8dee9",
    "base05": "#e5e9f0",
    "base06": "#eceff4",
    "base07": "#8fbcbb",
    "base08": "#bf616a",
    "base09": "#d08770",
    "base0A": "#ebcb8b",
    "base0B": "#a3be8c",
    "base0C": "#88c0d0",
    "base0D": "#81a1c1",
    "base0E": "#b48ead",
    "base0F": "#5e81ac"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "#c77c85",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#4f4756",
    "toolTitle": "base06",
    "toolOutput": "base04",
    "diffAdded": "base0B",
    "diffRemoved": "#c77c85",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

- [ ] **Step 18: Create `crates/yach-ui/themes/monokai.json`** (1087 bytes)

Write exactly:

```json
{
  "base": "dark",
  "vars": {
    "base00": "#272822",
    "base01": "#383830",
    "base02": "#49483e",
    "base03": "#75715e",
    "base04": "#a59f85",
    "base05": "#f8f8f2",
    "base06": "#f5f4f1",
    "base07": "#f9f8f5",
    "base08": "#f92672",
    "base09": "#fd971f",
    "base0A": "#f4bf75",
    "base0B": "#a6e22e",
    "base0C": "#a1efe4",
    "base0D": "#66d9ef",
    "base0E": "#ae81ff",
    "base0F": "#cc6633"
  },
  "colors": {
    "accent": "base0D",
    "border": "base04",
    "success": "base0B",
    "error": "base08",
    "warning": "base0A",
    "muted": "base04",
    "dim": "base03",
    "text": "base05",
    "selectedBackground": "base02",
    "selectedText": "base05",
    "userMessageBackground": "base01",
    "userMessageText": "base05",
    "toolPendingBackground": "base01",
    "toolSuccessBackground": "base01",
    "toolErrorBackground": "#55353a",
    "toolTitle": "base06",
    "toolOutput": "base05",
    "diffAdded": "base0B",
    "diffRemoved": "base08",
    "diffContext": "base04",
    "diffHunk": "base0D",
    "harness": "base0C"
  }
}
```

Each file is one Base16 scheme from Tinted Theming at commit `a70da1dab18008023cfd55a94053f3b6cab4f86e`, with the spec's default mapping already applied and the measured contrast adjustments baked in as hex (`docs/project/specs/2026-10-02-theming-engine-design.md:383-431`). Do not regenerate them. A value that differs from the slot named in the default mapping is an authored deviation, not a typo: Catppuccin's `muted`/`diffContext`, the blended `toolErrorBackground` in every file, `gruvbox-dark`'s `error`/`diffRemoved` `#d76451`, `solarized-dark`'s `selectedText: base07` and `error` `#c25957`, `tokyo-night-moon`'s `text`/`selectedText`/`userMessageText`/`toolOutput` all `base06`, `solarized-light`'s `text` family on `base06` plus blended `success`/`warning`/`harness`, and the other blends listed in the spec's measured table.

- [ ] **Step 19: Create `crates/yach-ui/themes/NOTICE.md`**

Write exactly:

```markdown
# Bundled palette attribution

Each file in this directory is a yach theme document derived from one Base16
scheme. Slot values (`base00`..`base0F`) are copied verbatim from Tinted
Theming's scheme repository, commit `a70da1dab18008023cfd55a94053f3b6cab4f86e`:

https://github.com/tinted-theming/schemes

Tinted Theming schemes are MIT licensed. A yach `colors` entry that is a hex
literal rather than a `baseNN` name is a precomputed contrast adjustment (a
blend toward `base05`, or a `base01` panel blended 15% toward `base08`); it is
not an upstream slot.

| Palette | Scheme | Author | Upstream project | Upstream license |
|---|---|---|---|---|
| `catppuccin-latte` | Catppuccin Latte | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-frappe` | Catppuccin Frappe | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-macchiato` | Catppuccin Macchiato | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-mocha` | Catppuccin Mocha | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `rose-pine` | Rosé Pine | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `rose-pine-moon` | Rosé Pine Moon | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `rose-pine-dawn` | Rosé Pine Dawn | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `gruvbox-dark` | Gruvbox dark | Tinted Theming (https://github.com/tinted-theming), morhetz (https://github.com/morhetz/gruvbox) | https://github.com/morhetz/gruvbox | none on file; README states MIT/X11, repository has no LICENSE file |
| `gruvbox-light` | Gruvbox Light | Tinted Theming (https://github.com/tinted-theming), morhetz (https://github.com/morhetz/gruvbox) | https://github.com/morhetz/gruvbox | none on file; README states MIT/X11, repository has no LICENSE file |
| `tokyo-night-dark` | Tokyo Night Dark | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-storm` | Tokyo Night Storm | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-moon` | Tokyo Night Moon | Ólafur Bjarki Bogason | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-light` | Tokyo Night Light | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `solarized-dark` | Solarized Dark | Ethan Schoonover (modified by aramisgithub) | https://github.com/altercation/solarized | MIT |
| `solarized-light` | Solarized Light | Ethan Schoonover (modified by aramisgithub) | https://github.com/altercation/solarized | MIT |
| `dracula` | Dracula | clach04 (https://github.com/clach04) | https://github.com/dracula/dracula-theme | MIT |
| `nord` | Nord | arcticicestudio | https://github.com/nordtheme/nord | MIT |
| `monokai` | Monokai | Wimer Hazenberg (http://www.monokai.nl) | http://www.monokai.nl | none stated |

Classic Monokai states no license, and Gruvbox's README claims MIT/X11 without
a LICENSE file. They ship on the basis that color values are generally not
copyrightable, and NOTICE says so rather than claiming a license. Monokai Pro
is a separate commercial product; only classic Monokai ships, under that name.
```

The test in Step 20 checks that every bundled name appears in this file as a backticked token. Do not rename a heading without updating that test's expectation, and do not drop a row.

- [ ] **Step 20: Write the failing palette tests**

Create `crates/yach-ui/src/theme/palettes.rs` with only the tests module, so the build fails on the missing table rather than on a test that cannot name it. `theme.rs` does not declare this module yet; Step 22 adds `mod palettes;`.

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use ratatui::style::Color;

    use super::{BUNDLED_PALETTES, bundled_palette};
    use crate::theme::contrast::contrast_ratio;
    use crate::theme::{Theme, ThemeDocument, ToolState};

    const COLOR_TOKENS: [&str; 22] = [
        "accent",
        "border",
        "success",
        "error",
        "warning",
        "muted",
        "dim",
        "text",
        "selectedBackground",
        "selectedText",
        "userMessageBackground",
        "userMessageText",
        "toolPendingBackground",
        "toolSuccessBackground",
        "toolErrorBackground",
        "toolTitle",
        "toolOutput",
        "diffAdded",
        "diffRemoved",
        "diffContext",
        "diffHunk",
        "harness",
    ];

    fn themes_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes")
    }

    fn paint(style: ratatui::style::Style) -> Result<(Color, Color), String> {
        match (style.fg, style.bg) {
            (Some(fg), Some(bg)) if fg != Color::Reset && bg != Color::Reset => Ok((fg, bg)),
            (fg, bg) => Err(format!("surface is not an explicit pair: fg={fg:?} bg={bg:?}")),
        }
    }

    /// Body text on every painted surface must be at least 4.5:1, and `muted`
    /// at least 3:1 against the tool panel. Same bar the `dark`/`light` preset
    /// tests use; a palette that only parses still fails here.
    fn assert_painted_contrast(theme: &Theme, name: &str) -> Result<(), String> {
        let panel = paint(theme.tool_surface(ToolState::Pending))?.1;
        let error_panel = paint(theme.tool_surface(ToolState::Error))?.1;
        let selected = paint(theme.selected())?;
        let user = paint(theme.user_message())?;
        let pairs = [
            ("text on tool panel", theme.text().fg, Some(panel)),
            ("text on error panel", theme.text().fg, Some(error_panel)),
            ("selected text", Some(selected.0), Some(selected.1)),
            ("user message text", Some(user.0), Some(user.1)),
            ("tool title on panel", theme.tool_title().fg, Some(panel)),
            (
                "tool title on error panel",
                theme.tool_title().fg,
                Some(error_panel),
            ),
            (
                "tool output on panel",
                theme.tool_output(ToolState::Pending).fg,
                Some(panel),
            ),
            (
                "tool output on error panel",
                theme.tool_output(ToolState::Error).fg,
                Some(error_panel),
            ),
        ];
        for (label, fg, bg) in pairs {
            let (Some(fg), Some(bg)) = (fg, bg) else {
                return Err(format!("{name}: {label} has no color"));
            };
            let ratio = contrast_ratio(fg, bg).ok_or_else(|| {
                format!("{name}: {label} is not measurable ({fg:?} on {bg:?})")
            })?;
            if ratio < 4.5 {
                return Err(format!("{name}: {label} contrast {ratio:.2} < 4.5"));
            }
        }
        for (label, style) in [
            ("muted", theme.muted()),
            ("success", theme.success()),
            ("warning", theme.warning()),
            ("error", theme.error()),
            ("harness", theme.harness()),
        ] {
            let fg = style.fg.ok_or_else(|| format!("{name}: {label} has no fg"))?;
            let ratio = contrast_ratio(fg, panel)
                .ok_or_else(|| format!("{name}: {label} is not measurable"))?;
            if ratio < 3.0 {
                return Err(format!("{name}: {label} contrast {ratio:.2} < 3"));
            }
        }
        Ok(())
    }

    #[test]
    fn bundled_palette_files_parse_and_cover_every_token() -> Result<(), String> {
        if BUNDLED_PALETTES.len() != 18 {
            return Err(format!("expected 18 palettes, got {}", BUNDLED_PALETTES.len()));
        }
        let mut seen = BTreeSet::new();
        for palette in &BUNDLED_PALETTES {
            if !seen.insert(palette.name) {
                return Err(format!("duplicate palette name {}", palette.name));
            }
            let document = ThemeDocument::parse(palette.json)
                .map_err(|error| format!("{}: {error}", palette.name))?;
            let base = document
                .base()
                .ok_or_else(|| format!("{}: missing base", palette.name))?;
            if base != "dark" && base != "light" {
                return Err(format!("{}: base {base} is not dark or light", palette.name));
            }
            let vars: BTreeSet<&str> = document.var_names().collect();
            for slot in (0..16).map(|index| format!("base0{index:X}")) {
                if !vars.contains(slot.as_str()) {
                    return Err(format!("{}: vars missing {slot}", palette.name));
                }
            }
            let tokens: BTreeSet<&str> = document.color_tokens().collect();
            for token in COLOR_TOKENS {
                if !tokens.contains(token) {
                    return Err(format!("{}: colors missing {token}", palette.name));
                }
            }
            if tokens.len() != COLOR_TOKENS.len() {
                return Err(format!(
                    "{}: colors has {} keys, expected {}",
                    palette.name,
                    tokens.len(),
                    COLOR_TOKENS.len()
                ));
            }
            let preset = if base == "light" {
                Theme::light()
            } else {
                Theme::dark()
            };
            let theme = document
                .apply(preset)
                .map_err(|error| format!("{}: apply {error}", palette.name))?;
            assert_painted_contrast(&theme, palette.name)?;
        }
        Ok(())
    }

    #[test]
    fn bundled_table_matches_files_on_disk_and_notice() -> Result<(), String> {
        let dir = themes_dir();
        let mut on_disk = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).map_err(|error| format!("read themes/: {error}"))? {
            let entry = entry.map_err(|error| format!("themes entry: {error}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(String::from("themes/ has a non-UTF-8 name"));
            };
            if let Some(stem) = name.strip_suffix(".json") {
                on_disk.insert(stem.to_owned());
            }
        }
        let table: BTreeSet<&str> = BUNDLED_PALETTES.iter().map(|palette| palette.name).collect();
        let on_disk_ref: BTreeSet<&str> = on_disk.iter().map(String::as_str).collect();
        if table != on_disk_ref {
            return Err(format!("table {table:?} != themes/*.json {on_disk_ref:?}"));
        }
        let notice = std::fs::read_to_string(dir.join("NOTICE.md"))
            .map_err(|error| format!("read NOTICE.md: {error}"))?;
        for palette in &BUNDLED_PALETTES {
            let needle = format!("`{}`", palette.name);
            if !notice.contains(&needle) {
                return Err(format!("NOTICE.md has no entry for {}", palette.name));
            }
            let file = std::fs::read_to_string(dir.join(format!("{}.json", palette.name)))
                .map_err(|error| format!("read {}: {error}", palette.name))?;
            if file != palette.json {
                return Err(format!(
                    "{}: embedded JSON differs from themes/{}.json",
                    palette.name, palette.name
                ));
            }
            if bundled_palette(palette.name).map(|found| found.name) != Some(palette.name) {
                return Err(format!("bundled_palette({}) missed", palette.name));
            }
        }
        if bundled_palette("dark").is_some() || bundled_palette("auto").is_some() {
            return Err(String::from("preset names must not resolve as palettes"));
        }
        Ok(())
    }
}
```

`assert_painted_contrast` is this task's copy of the preset contrast bar. Task 1 ships `contrast_ratio` and no shared helper (see Contract notes), so calling a function Task 1 does not define would not compile. The first test fails a palette whose `colors` map omits a token even when `apply` would still produce a `Theme`, which is the bug a later edit introduces by deleting a line and relying on the preset fallback. The second fails when someone adds a JSON file without a table row, edits a file without updating the embedded string, or drops a NOTICE row.

- [ ] **Step 21: Run to verify failure**

Run: `just dev cargo test -p yach-ui theme::palettes::tests -- --test-threads=1`

Expected: FAIL at compile time. `mod palettes` is not declared, so the filter matches nothing and cargo reports `error: no test target matches` is not the signal; the signal is that `palettes.rs` is not a module yet. If cargo compiles the crate and then says no test matched the filter, that is the same failure: the module is not wired. Do not "fix" this by renaming the filter.

- [ ] **Step 22: Implement the palette table**

In `crates/yach-ui/src/theme.rs`, next to the `mod presets;` Task 1 added (and not inside `#[cfg(test)]`):

```rust
mod palettes;
```

Replace the body of `crates/yach-ui/src/theme/palettes.rs` above the tests module with:

```rust
/// One bundled named palette. `json` is the theme file embedded from
/// `crates/yach-ui/themes/<name>.json`; startup parses only the selected one.
pub struct BundledPalette {
    pub name: &'static str,
    pub json: &'static str,
}

/// Spec order (`docs/project/specs/2026-10-02-theming-engine-design.md:353-360`).
pub(crate) const BUNDLED_PALETTES: [BundledPalette; 18] = [
    palette("catppuccin-latte", include_str!("../../themes/catppuccin-latte.json")),
    palette("catppuccin-frappe", include_str!("../../themes/catppuccin-frappe.json")),
    palette("catppuccin-macchiato", include_str!("../../themes/catppuccin-macchiato.json")),
    palette("catppuccin-mocha", include_str!("../../themes/catppuccin-mocha.json")),
    palette("rose-pine", include_str!("../../themes/rose-pine.json")),
    palette("rose-pine-moon", include_str!("../../themes/rose-pine-moon.json")),
    palette("rose-pine-dawn", include_str!("../../themes/rose-pine-dawn.json")),
    palette("gruvbox-dark", include_str!("../../themes/gruvbox-dark.json")),
    palette("gruvbox-light", include_str!("../../themes/gruvbox-light.json")),
    palette("tokyo-night-dark", include_str!("../../themes/tokyo-night-dark.json")),
    palette("tokyo-night-storm", include_str!("../../themes/tokyo-night-storm.json")),
    palette("tokyo-night-moon", include_str!("../../themes/tokyo-night-moon.json")),
    palette("tokyo-night-light", include_str!("../../themes/tokyo-night-light.json")),
    palette("solarized-dark", include_str!("../../themes/solarized-dark.json")),
    palette("solarized-light", include_str!("../../themes/solarized-light.json")),
    palette("dracula", include_str!("../../themes/dracula.json")),
    palette("nord", include_str!("../../themes/nord.json")),
    palette("monokai", include_str!("../../themes/monokai.json")),
];

const fn palette(name: &'static str, json: &'static str) -> BundledPalette {
    BundledPalette { name, json }
}

/// Returns the bundled palette with this exact name, or `None` for a preset
/// name, an extension theme name, or anything else.
#[must_use]
pub fn bundled_palette(name: &str) -> Option<&'static BundledPalette> {
    BUNDLED_PALETTES.iter().find(|palette| palette.name == name)
}
```

`include_str!` follows the existing data-embedding pattern (`crates/yach-catalog/src/lib.rs:238`), but parsing stays lazy: nothing in this file calls `ThemeDocument::parse` outside tests. `BUNDLED_PALETTES` is `pub(crate)` because Task 4's `select.rs` is a sibling module and is the only other reader; `bundled_palette` is `pub` so the CLI diagnostic can ask the crate for a name. Do not re-export either from `yach-ui` `lib.rs` in this task; Task 4's `builtin_theme_names` is the public name list.

Keep the tests module from Step 20 at the bottom of the same file.

- [ ] **Step 23: Run to verify pass**

Run: `just dev cargo test -p yach-ui theme::palettes::tests -- --test-threads=1`

Expected: PASS, both tests.

- [ ] **Step 24: Run lint**

Run: `just lint`

Expected: PASS. `palettes.rs` is a data table plus two tests; if clippy flags `too_many_lines` on the test module, that limit is 100 (`clippy.toml`) and the fix is to keep the two tests as they are and not to split them into string-matching tests. A contrast failure here means a JSON file was edited away from the prototype; restore the verbatim file rather than loosening `4.5` or `3.0`.

- [ ] **Step 25: Record the binary-size delta**

`cargo bloat` is not installed and is not a project tool. There is no YACH-13 size gate to satisfy: that issue is an unbuilt follow-up (`docs/project/specs/2026-09-27-distribution-presets-design.md:58`), and `crates/yach-bench/perf-thresholds.toml:43-45` only budgets a PR against its own base at 5%. Record the raw delta instead.

Before this task's files exist, the parent revision's release binary is the baseline. From the repo root:

```bash
jj new -m 'size baseline' <parent-of-this-change>
just dev cargo build --release --locked -p yach
ls -l target/release/yach
```

Record that byte count. Then return to this change (`jj edit <this-change>`) and rebuild:

```bash
just dev cargo build --release --locked -p yach
ls -l target/release/yach
```

The spec estimates roughly 25 KiB of embedded JSON (`docs/project/specs/2026-10-02-theming-engine-design.md:449-450`); the eighteen files are 19,597 bytes of source, so a release delta far above that (hundreds of KiB) means something other than `include_str!` was linked and must be explained in the commit message, not ignored. Do not add a threshold row.

- [ ] **Step 26: Commit**

```bash
jj commit crates/yach-ui/themes crates/yach-ui/src/theme.rs crates/yach-ui/src/theme/palettes.rs -m "Bundle eighteen named palettes

Embedded JSON is N bytes in target/release/yach (baseline M, delta D). No YACH-13 size gate exists; the relative binary/size_bytes budget is 5%."
```

Replace `N`, `M`, and `D` with the `ls -l` byte counts from Step 25. Name only these paths so a sibling task's uncommitted edits stay out of the commit.

### Task 3: Widget migration

**Depends on:** Task 1 merged, and branch `tui-markdown` merged to the revision this task starts from. Do not start the markdown step against the pre-merge tree: `crates/yach-ui/src/markdown.rs` does not exist there. Confirm `lib.rs` has `mod markdown;` before Step 3. Line numbers for `markdown.rs` below are from `tui-markdown` at planning time; re-read the file after the merge and trust the file if a number moved. Widget line numbers are from the current working copy and do not move unless that merge edits them (it edits `transcript.rs` assistant rendering; re-read `transcript.rs` before Step 5 and keep the site list's behavior, not a stale number).

**Files:**

- Modify: `crates/yach-ui/src/theme.rs` (`Theme.colors` and every `ThemeColors` field become private)
- Modify: `crates/yach-ui/src/transcript.rs` (production reads at `985`, `1026-1145`, `1163-1168`, `1254-1263`, `1270-1293`, `1310-1311`; tests at `1617`, `1904`, `1909`, `1913`, `1919`, `1964`, `1970`, `1976`, `2003`)
- Modify: `crates/yach-ui/src/app.rs:4562-4890` (dialog, local-edit, and diff helpers)
- Modify: `crates/yach-ui/src/approval_selector.rs:25-153`
- Modify: `crates/yach-ui/src/model_selector.rs:42-220`
- Modify: `crates/yach-ui/src/fork_picker.rs:22-69`
- Modify: `crates/yach-ui/src/slash_popup.rs:29-64`
- Modify: `crates/yach-ui/src/session_picker.rs:24-72`
- Modify: `crates/yach-ui/src/help_overlay.rs:20-79`
- Modify: `crates/yach-ui/src/thinking_selector.rs:26-53`
- Modify: `crates/yach-ui/src/input.rs:38-81` and the assertion at `input.rs:223-228`
- Modify: `crates/yach-ui/src/perf_overlay.rs:34-44`
- Modify: `crates/yach-ui/src/status_bar.rs:217-247` and the assertion at `status_bar.rs:423`
- Modify: `crates/yach-ui/src/markdown.rs` (`MarkdownStyles::from_theme` and the five tests that read `theme.colors`)
- Modify: `crates/yach-ui/src/lib.rs:25` (drop `ThemeColors` from the public re-export)
- Test: `crates/yach-ui/src/transcript.rs` (`mod tests`, the buffer pin from Step 1)

**Interfaces:**

- Consumes: every `Theme` helper from Task 1; `ToolState`; `DiffKind`.
- Produces: no new public API. `Theme.colors` and every `ThemeColors` field are private (no visibility modifier), so a sibling module that writes `theme.colors.text` fails to compile. Private is enough for `theme/presets.rs`, `theme/contrast.rs` and the `theme::tests` module: Rust privacy admits the defining module and its descendants. Do not use `pub(super)`: on a field declared in `theme.rs` that means visible to the crate root, i.e. to every widget.

- [ ] **Step 1: Pin today's transcript buffer before any migration**

This test renders through `TestBackend` against the current renderer and records the cell grid. It must be written and passing *before* Steps 3-6, so it pins the pre-migration output. After the migration it must still pass with no expected-value edit. That is the behavior-preserving check for `dark`.

Add to `crates/yach-ui/src/transcript.rs` tests module (`transcript.rs:1384`):

The tests module imports `Theme` (`transcript.rs:1391`) but not `ToolState`, `DiffKind`, or `Color`. Add those to the imports, or import them inside the test. `Color` is `ratatui::style::Color`.

```rust
#[test]
fn dark_transcript_buffer_is_stable_across_helper_migration() {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    let mut transcript = Transcript::new();
    transcript.append_user_message("ship the theme");
    transcript.append_tool_call(Some("call-1"), "read_text_file", Some("src/lib.rs"));
    transcript.append_tool_result(Some("call-2"), "read_text_file", "completed: 1 line", false);
    transcript.append_tool_result(Some("call-3"), "bash", "failed: exit 1", true);
    transcript.append_tool_call(Some("call-4"), "edit_text_file", Some("src/lib.rs"));
    transcript.begin_tool_review(
        "call-4",
        "edit_text_file",
        ToolReviewPayload::LocalEdit {
            preview: LocalEditPreviewSummary {
                preview_id: String::from("preview-1"),
                transaction_id: String::from("transaction-1"),
                permission_decision_id: String::from("permission-1"),
                path: String::from("src/lib.rs"),
                operation: String::from("replace"),
                review_state: LocalEditReviewState::NeedsUserApproval,
                diff_summary: String::from("@@ -1 +1 @@\n-old line\n+new line\n context"),
                diff_summary_truncated: false,
                review_origin: None,
            },
        },
    );

    let area = Rect::new(0, 0, 80, 24);
    let backend = TestBackend::new(area.width, area.height);
    let terminal = Terminal::new(backend);
    assert!(terminal.is_ok());
    let Ok(mut terminal) = terminal else { return };
    // render_uncached is the pre-migration entry point (transcript.rs:936).
    // It draws with Theme::default(), which Task 1 made equal to Theme::dark().
    let drawn = terminal.draw(|frame| {
        render_uncached(area, frame.buffer_mut(), transcript.entries(), 0, false);
    });
    assert!(drawn.is_ok());
    let buffer = terminal.backend().buffer().clone();

    // Pinned from the pre-migration render. A helper that changes a fg, bg,
    // or modifier on `dark` fails here. Update this table only in Task 5,
    // where the success background and tool output color change on purpose.
    // Helpers, not `theme.colors`: Step 7 makes the fields private, and this
    // test has to keep compiling. On dark the helper backgrounds equal the
    // fields, so the pin still catches a behavior change.
    let theme = Theme::dark();
    let pending = theme.tool_surface(ToolState::Pending).bg.unwrap_or(Color::Reset);
    let success = theme.tool_surface(ToolState::Success).bg.unwrap_or(Color::Reset);
    let error = theme.tool_surface(ToolState::Error).bg.unwrap_or(Color::Reset);
    let user_bg = theme.user_message().bg.unwrap_or(Color::Reset);

    let mut saw_user = false;
    let mut saw_pending = false;
    let mut saw_success = false;
    let mut saw_error = false;
    let mut saw_diff_added = false;
    let mut saw_diff_removed = false;
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            if cell.symbol() == "›" {
                assert_eq!(cell.bg, user_bg, "user marker lost its background");
                saw_user = true;
            }
            if cell.symbol() == "⚙" {
                assert_eq!(cell.fg, theme.warning().fg.unwrap_or(Color::Reset));
                assert_eq!(cell.bg, pending);
                saw_pending = true;
            }
            if cell.symbol() == "✓" {
                assert_eq!(cell.fg, theme.success().fg.unwrap_or(Color::Reset));
                assert_eq!(cell.bg, success, "success row must keep today's green band");
                saw_success = true;
            }
            if cell.symbol() == "✗" {
                assert_eq!(cell.fg, theme.error().fg.unwrap_or(Color::Reset));
                assert_eq!(cell.bg, error);
                saw_error = true;
            }
            if cell.symbol() == "+" {
                assert_eq!(cell.fg, theme.diff(DiffKind::Added).fg.unwrap_or(Color::Reset));
                saw_diff_added = true;
            }
            if cell.symbol() == "-" {
                assert_eq!(cell.fg, theme.diff(DiffKind::Removed).fg.unwrap_or(Color::Reset));
                saw_diff_removed = true;
            }
        }
    }
    assert!(saw_user && saw_pending && saw_success && saw_error);
    assert!(saw_diff_added && saw_diff_removed);

    // Full-buffer pin. Serialize every cell so a padding, gutter, or modifier
    // change fails even when the spot checks above still pass.
    let dumped = (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| {
                    let cell = &buffer[(x, y)];
                    format!(
                        "{:?}|{:?}|{:?}|{:?}",
                        cell.symbol(),
                        cell.fg,
                        cell.bg,
                        cell.modifier
                    )
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dark-transcript-buffer.txt");
    if std::env::var_os("UPDATE_THEME_PIN").is_some() {
        std::fs::write(&pin_path, &dumped).ok();
    }
    let pinned = std::fs::read_to_string(&pin_path).unwrap_or_default();
    assert_eq!(
        dumped, pinned,
        "dark transcript buffer changed; this task must not change it"
    );
}
```

The test uses `assert!` plus `let Ok(x) = x else { return };`, which is the style `transcript.rs` tests already use (`transcript.rs:1945-1951`). Do not use `unwrap` or `expect`: the workspace denies both. The pin file does not exist yet, so the first run fails on the empty compare; that is Step 2.

The test module already imports `ToolReviewPayload`, `LocalEditPreviewSummary`, and `LocalEditReviewState` (`transcript.rs:1394-1399`); use those names, not the `yach_proto::` prefix. The call above mirrors `command_review_row_renders_command_and_inline_selector` (`transcript.rs:1543` area) and the local-edit review constructed near `transcript.rs:1540`. If the struct fields differ after the markdown merge, copy the field list from the existing test rather than inventing one. The diff summary must contain a `+` line and a `-` line so the pin covers `diff_added` and `diff_removed`.

- [ ] **Step 2: Record the pin, then prove the test fails when output changes**

Run once to write the fixture from the current renderer:

```bash
mkdir -p crates/yach-ui/tests/fixtures
UPDATE_THEME_PIN=1 just dev cargo test -p yach-ui --lib transcript::tests::dark_transcript_buffer_is_stable_across_helper_migration -- --exact
```

Expected: PASS, and `crates/yach-ui/tests/fixtures/dark-transcript-buffer.txt` exists. Inspect the first lines and confirm they contain the user marker and a tool row before trusting the file.

Then prove the pin catches a real change. Temporarily change `tool_success_background` in `theme/presets.rs` `dark()` from `Color::Rgb(40, 50, 40)` to `Color::Rgb(40, 40, 50)` and rerun without `UPDATE_THEME_PIN`:

Run: `just dev cargo test -p yach-ui --lib transcript::tests::dark_transcript_buffer_is_stable_across_helper_migration -- --exact`

Expected: FAIL, with `dark transcript buffer changed` and a diff mentioning `Rgb(40, 50, 40)`. Revert the preset edit. Do not update the fixture. Commit the fixture with the test in Step 9.

- [ ] **Step 3: Migrate `markdown.rs`**

`MarkdownStyles::from_theme` (`markdown.rs:59-81` on `tui-markdown`) reads six tokens. Replace the body:

```rust
fn from_theme(theme: &Theme) -> Self {
    let text = theme.text();
    let muted = theme.muted();
    let dim = theme.dim();
    let accent = theme.accent();
    let warning = theme.warning();
    let code_bg = theme.tool_surface(crate::theme::ToolState::Pending);
    Self {
        text,
        quote_text: muted,
        quote_bar: dim,
        heading_major: accent.add_modifier(ratatui::style::Modifier::BOLD),
        heading_minor: Style::new().add_modifier(ratatui::style::Modifier::BOLD),
        strong: Style::new().add_modifier(ratatui::style::Modifier::BOLD),
        emphasis: Style::new().add_modifier(ratatui::style::Modifier::ITALIC),
        strikethrough: Style::new().add_modifier(ratatui::style::Modifier::CROSSED_OUT),
        inline_code: warning,
        code_block: text.bg(code_bg.bg.unwrap_or(ratatui::style::Color::Reset)),
        link: accent.add_modifier(ratatui::style::Modifier::UNDERLINED),
        link_url: dim,
        list_marker: accent,
        rule: dim,
        table_separator: dim,
        table_header: Style::new().add_modifier(ratatui::style::Modifier::BOLD),
    }
}
```

On `dark` this is identical: `text()` is `fg(White)`, `muted()` is `fg(Rgb(128,128,128))` with no modifier, `dim()` is `fg(Rgb(102,102,102))`, `accent()` is `fg(Rgb(0,215,255))`, `warning()` is `fg(Yellow)`, and `tool_surface(Pending)` bg is `Rgb(40,40,50)`. `heading_minor`, `strong`, `emphasis`, `strikethrough`, and `table_header` never read a color today and still must not.

Tests that read `theme.colors` move to helpers. Each compares a rendered fg/bg with the helper, so a helper that returns the wrong role fails:

| Test | Line | Old | New |
|---|---|---|---|
| `inline_delimiters_are_removed_and_styles_applied` | `markdown.rs:879` | `Some(theme.colors.warning)` | `theme.warning().fg` |
| same | `markdown.rs:880` | `Some(theme.colors.text)` | `theme.text().fg` |
| `headings_drop_hashes_and_are_styled` | `markdown.rs:905` | `Some(theme.colors.accent)` | `theme.accent().fg` |
| `fenced_code_preserves_whitespace_and_markup` | `markdown.rs:976` | `Some(theme.colors.tool_pending_background)` | `theme.tool_surface(ToolState::Pending).bg` |
| `block_quotes_get_a_bar_and_wrap_inside_it` | `markdown.rs:1078` | `Some(theme.colors.dim)` | `theme.dim().fg` |
| same | `markdown.rs:1079` | `Some(theme.colors.muted)` | `theme.muted().fg` |
| `links_show_label_and_url_only_when_they_differ` | `markdown.rs:1095` | `Some(theme.colors.accent)` | `theme.accent().fg` |
| same | `markdown.rs:1099` | `Some(theme.colors.dim)` | `theme.dim().fg` |

`colors_come_from_the_theme` (`markdown.rs:1223-1228`) mutates `theme.colors.accent`. After the fields are private that does not compile, and a test that only compiles against a public field does not protect a consumer. Replace it with a document apply, which is the real way a theme changes:

```rust
#[test]
fn colors_come_from_the_theme() {
    let document = crate::theme::ThemeDocument::parse(
        r##"{"colors":{"accent":"#010203"}}"##,
    );
    assert!(document.is_ok());
    let Ok(document) = document else { return };
    let theme = document.apply(Theme::dark());
    assert!(theme.is_ok());
    let Ok(theme) = theme else { return };
    let lines = render("# Head", 20, &theme);
    assert_eq!(style_at(&lines, "Head").fg, Some(Color::Rgb(1, 2, 3)));
}
```

- [ ] **Step 4: Migrate `app.rs`**

36 reads, all inside `render_dialog_overlay` (`app.rs:4562`), `render_local_edit_compose_overlay` (`app.rs:4680`), `render_local_edit_review_overlay` (`app.rs:4748`), `review_diff_line_style` (`app.rs:4821`), and `render_dialog_textarea` (`app.rs:4871`). Representative replacements:

Dialog frame and title (`app.rs:4561-4568`):

```rust
let block = Block::default()
    .borders(Borders::ALL)
    .border_style(theme.border(false))
    .title(dialog_summary(&dialog.request))
    .title_style(theme.accent().add_modifier(Modifier::BOLD));
```

Selected dialog row (`app.rs:4638-4646`):

```rust
let style = if is_selected {
    theme.selected().add_modifier(Modifier::BOLD)
} else {
    theme.muted()
};
```

Body text (`app.rs:4661`, `4716`, `4815`, `4871`, `4884`): `Style::new().fg(theme.colors.text)` becomes `theme.text()`.

`review_diff_line_style` (`app.rs:4821-4837`) is the one diff helper that does not match `transcript_line_style`. Migrate it as:

```rust
fn review_diff_line_style(line: &str, theme: &Theme) -> ratatui::style::Style {
    if line.starts_with("+++ ") || line.starts_with("--- ") {
        return theme.muted().add_modifier(ratatui::style::Modifier::BOLD);
    }
    if line.starts_with('+') {
        return theme.diff(crate::theme::DiffKind::Added);
    }
    if line.starts_with('-') {
        return theme.diff(crate::theme::DiffKind::Removed);
    }
    if line.starts_with("@@") {
        return theme.accent();
    }
    theme.diff(crate::theme::DiffKind::Context)
}
```

Complete site list for `app.rs`:

| Line | Old | New |
|---|---|---|
| 4562 | `fg(theme.colors.border)` | `theme.border(false)` |
| 4566 | `fg(theme.colors.accent)` + BOLD | `theme.accent().add_modifier(BOLD)` |
| 4585-4586 | `fg(selected_text).bg(success)` | `selected_on(theme.success(), theme.selected())` (divergence 1) |
| 4588 | `fg(success)` | `theme.success()` |
| 4591 | `fg(error)` | `theme.error()` |
| 4594-4595 | `fg(selected_text).bg(error)` | `selected_on(theme.error(), theme.selected())` (divergence 1) |
| 4641-4642 | `fg(selected_text).bg(selected_background)` + BOLD | `theme.selected().add_modifier(BOLD)` |
| 4645 | `fg(muted)` | `theme.muted()` |
| 4661 | `fg(text)` | `theme.text()` |
| 4680 | `fg(border)` | `theme.border(false)` |
| 4684 | `fg(accent)` + BOLD | `theme.accent().add_modifier(BOLD)` |
| 4693, 4697 | `fg(warning)` | `theme.warning()` |
| 4716 | `fg(text)` | `theme.text()` |
| 4748 | `fg(border)` | `theme.border(false)` |
| 4752 | `fg(accent)` + BOLD | `theme.accent().add_modifier(BOLD)` |
| 4760-4761 | `fg(selected_text).bg(success)` | `selected_on(theme.success(), theme.selected())` (divergence 1) |
| 4763 | `fg(success)` | `theme.success()` |
| 4767-4768 | `fg(selected_text).bg(error)` | `selected_on(theme.error(), theme.selected())` (divergence 1) |
| 4770 | `fg(error)` | `theme.error()` |
| 4815 | `fg(text)` | `theme.text()` |
| 4825 | `fg(muted)` + BOLD | `theme.muted().add_modifier(BOLD)` |
| 4828 | `fg(diff_added)` | `theme.diff(Added)` |
| 4831 | `fg(diff_removed)` | `theme.diff(Removed)` |
| 4834 | `fg(accent)` | `theme.accent()` |
| 4836 | `fg(diff_context)` | `theme.diff(Context)` |
| 4871 | `fg(text)` | `theme.text()` |
| 4879 | `fg(border)` | `theme.border(false)` |
| 4881 | `fg(warning)` | `theme.warning()` |
| 4884 | `fg(text)` | `theme.text()` |
| 4890 | `fg(dim)` | `theme.dim()` |

**Divergence 1 (no helper reproduces it).** `app.rs:4584-4595` and `app.rs:4759-4770` paint the *selected* confirm/apply/reject label with `selected_text` foreground on the `success` or `error` color as background. `selected()` pairs `selected_text` with `selected_background`, and `success()`/`error()` set no background. Combining them as `theme.success().bg(theme.success().fg.unwrap_or(Color::Reset)).fg(theme.selected().fg.unwrap_or(Color::Reset))` preserves today's cells on `dark` (selected text `White`, success bg `Rgb(181, 189, 104)`, error bg `Rgb(204, 102, 102)`) and is what Task 3 ships. Write it once:

```rust
fn selected_on(role: Style, selected_text: Style) -> Style {
    role.bg(role.fg.unwrap_or(ratatui::style::Color::Reset))
        .fg(selected_text.fg.unwrap_or(ratatui::style::Color::Reset))
}
```

and call `selected_on(theme.success(), theme.selected())` / `selected_on(theme.error(), theme.selected())`. This is a behavior-preserving encoding of a pairing the role set does not name. Do not add a new theme token for it.

- [ ] **Step 5: Migrate `transcript.rs`**

Tool rows keep today's band. `tool_background` (`transcript.rs:1254-1263`) maps to `ToolState` and the renderer keeps using `render_tool_surface` (`transcript.rs:1209`) with that background. The `┃` rule column is Task 5; do not add it here. The dim `│ ` continuation rail at `transcript.rs:1045,1058,1080` stays.

```rust
fn tool_state(entry: &TranscriptEntry) -> crate::theme::ToolState {
    match entry.kind {
        EntryKind::ToolResult { is_error: true, .. }
        | EntryKind::ToolResult { outcome_kind: Some(_), .. } => crate::theme::ToolState::Error,
        EntryKind::ToolResult { .. } => crate::theme::ToolState::Success,
        _ => crate::theme::ToolState::Pending,
    }
}

fn tool_background(entry: &TranscriptEntry, theme: &Theme) -> Color {
    theme
        .tool_surface(tool_state(entry))
        .bg
        .unwrap_or(Color::Reset)
}
```

On `dark`, `tool_surface(Pending).bg` is `Rgb(40, 40, 50)`, `Success` is `Rgb(40, 50, 40)`, and `Error` is `Rgb(60, 40, 40)`, so the band is unchanged. Harness outcomes keep the error band, matching the current match arm.

`render_entry_lines` (`transcript.rs:1024`) drops `let colors = theme.colors;` and uses helpers. Representative arm, the tool call (`transcript.rs:1043-1049`):

```rust
EntryKind::ToolCall { name, .. } => (
    Span::styled("⚙ ", theme.warning().add_modifier(Modifier::BOLD)),
    Span::styled("│ ", theme.dim()),
    name_and_detail(name, &display_text),
    theme.tool_title(),
    2,
),
```

`tool_title()` is already bold, and on `dark` its fg is `White`, so the title cell matches today. The failed tool arm (`transcript.rs:1062-1076`):

```rust
let (marker, marker_style, content_style) = if *is_error {
    (
        "✗ ",
        theme.error().add_modifier(Modifier::BOLD),
        theme.error(),
    )
} else {
    (
        "✓ ",
        theme.success(),
        theme.tool_output(tool_state(entry)),
    )
};
```

`tool_output` sets a background; `render_tool_surface` (`transcript.rs:1224`) overwrites every span background with the band, so the visible bg is still the band. The fg stays `Rgb(128, 128, 128)` on `dark`.

User message (`transcript.rs:1162-1168`):

```rust
let surface = theme.user_message();
let background_style = Style::new().bg(surface.bg.unwrap_or(Color::Reset));
let prefix_style = theme
    .accent()
    .bg(surface.bg.unwrap_or(Color::Reset))
    .add_modifier(Modifier::BOLD);
let content_style = surface;
```

**Divergence 2.** The `› ` prefix is `accent` bold on the user-message background (`transcript.rs:1165`). `user_message()` returns `user_message_text` on that background, not `accent`. No helper pairs accent fg with the user-message bg. The replacement above preserves it: on `dark` the prefix is `Rgb(0, 215, 255)` bold on `Rgb(52, 53, 65)`. Do not switch the prefix to `user_message()`.

**Divergence 3.** `transcript_line_style` (`transcript.rs:1276` and `1285`) bolds `diff_context` for `+++`/`---` and bolds `diff_hunk` for `@@`/`›`. `diff()` returns no modifier. Keep the bold by appending `.add_modifier(Modifier::BOLD)` at those two returns only. `app.rs:4825` bolds `muted` for the same file headers and is the same exception (already in the app.rs table).

**Divergence 4.** `app.rs:4834` styles a hunk header with `accent`, while `transcript.rs:1285` styles it with `diff_hunk`. On `dark` those colors are equal (`Rgb(0, 215, 255)`), so the buffer pin cannot see it, but a custom theme can set them apart. Keep each call site on the token it uses today: transcript goes to `theme.diff(DiffKind::Hunk)`, app goes to `theme.accent()`. Do not "unify" them.

Complete production site list for `transcript.rs`:

| Line | Old | New |
|---|---|---|
| 985 | `theme.colors.text` | `theme.text().fg.unwrap_or(Color::Reset)` |
| 1026 | `let colors = theme.colors` | delete |
| 1030 | `fg(colors.accent)` | `theme.accent()` |
| 1033 | `fg(colors.text)` | `theme.text()` |
| 1037 | `fg(colors.accent).bold()` | `theme.accent().add_modifier(BOLD)` |
| 1040 | `fg(colors.muted)` | `theme.muted()` |
| 1044 | `fg(colors.warning).bold()` | `theme.warning().add_modifier(BOLD)` |
| 1045 | `fg(colors.dim)` | `theme.dim()` |
| 1047 | `fg(colors.tool_title)` | `theme.tool_title()` |
| 1058 | `fg(colors.dim)` | `theme.dim()` |
| 1068 | `fg(colors.error).bold()` | `theme.error().add_modifier(BOLD)` |
| 1069 | `fg(colors.error)` | `theme.error()` |
| 1074 | `fg(colors.success)` | `theme.success()` |
| 1075 | `fg(colors.tool_output)` | `theme.tool_output(tool_state(entry))` |
| 1080 | `fg(colors.dim)` | `theme.dim()` |
| 1097 | `fg(colors.accent).bold()` | `theme.accent().add_modifier(BOLD)` |
| 1100 | `fg(colors.muted)` | `theme.muted()` |
| 1104 | `fg(colors.error).bold()` | `theme.error().add_modifier(BOLD)` |
| 1107 | `fg(colors.error)` | `theme.error()` |
| 1138 | `fg(colors.dim)` | `theme.dim()` |
| 1163 | `user_message_background` | `theme.user_message().bg` (divergence 2) |
| 1165 | `fg(accent).bg(background).bold()` | divergence 2 |
| 1167 | `fg(user_message_text)` | `theme.user_message()` |
| 1260 | `tool_error_background` | `tool_surface(Error).bg` via `tool_state` |
| 1261 | `tool_success_background` | `tool_surface(Success).bg` via `tool_state` |
| 1262 | `tool_pending_background` | `tool_surface(Pending).bg` via `tool_state` |
| 1276 | `fg(diff_context).bold()` | `theme.diff(Context).add_modifier(BOLD)` |
| 1279 | `fg(diff_added)` | `theme.diff(Added)` |
| 1282 | `fg(diff_removed)` | `theme.diff(Removed)` |
| 1285 | `fg(diff_hunk).bold()` | `theme.diff(Hunk).add_modifier(BOLD)` |
| 1291 | `fg(dim)` | `theme.dim()` |
| 1311 | `fg(harness).bold()` | `theme.harness().add_modifier(BOLD)` |

Test assertions move to the same helpers, so they fail if a row paints the wrong surface:

| Line | Old | New |
|---|---|---|
| 1617 | `Theme::default().colors.tool_pending_background` | `Theme::dark().tool_surface(ToolState::Pending).bg` |
| 1904 | `theme.colors.user_message_background` | `theme.user_message().bg` |
| 1909 | `theme.colors.user_message_text` | `theme.user_message().fg` |
| 1913 | `theme.colors.text` | `theme.text().fg` |
| 1919 | `theme.colors.tool_pending_background` | `theme.tool_surface(ToolState::Pending).bg` |
| 1964 | `theme.colors.user_message_background` | `theme.user_message().bg` |
| 1970 | `theme.colors.user_message_text` | `theme.user_message().fg` |
| 1976 | `theme.colors.tool_pending_background` | `theme.tool_surface(ToolState::Pending).bg` |
| 2003 | `Theme::default().colors.tool_success_background` | `Theme::dark().tool_surface(ToolState::Success).bg` |

`custom_theme_colors_user_and_tool_surfaces` (`transcript.rs:1924`) already applies a `ThemeDocument` after Task 1. Its assertions compare rendered spans with `theme.user_message().bg` and `theme.tool_surface(ToolState::Pending).bg`, which is the consumer-visible check: a document that fails to override the pending band fails the test.

- [ ] **Step 6: Migrate the remaining widgets**

Every file below follows one pattern: a frame border is `theme.border(false)`, a title is `theme.accent().add_modifier(Modifier::BOLD)`, body text is `theme.text()`, a selected row is `theme.selected().add_modifier(Modifier::BOLD)` where the old code used `selected_text` without a background, an unselected row is `theme.muted()`, the current/active row is `theme.warning()` or `theme.success()` as the old code chose, and hints are `theme.dim()`. Selected rows in these files set no background today (`fork_picker.rs:41-43`, `model_selector.rs:171-173`, `session_picker.rs:46-48`, `slash_popup.rs:47-49`, `thinking_selector.rs:40-42`, `approval_selector.rs:35-37`), so `selected()` would add `selected_background` and change the buffer. Use the foreground only:

```rust
fn selected_label(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.selected().fg.unwrap_or(ratatui::style::Color::Reset))
        .add_modifier(Modifier::BOLD)
}
```

**Divergence 5.** Those six widgets style the selected row with `selected_text` and no background. `selected()` also sets `selected_background`. Using `selected()` would paint a band behind the row that is not there today. `selected_label` keeps today's look on `dark` (`White` bold, no bg). This is the same gap as divergence 1, in the other direction.

`approval_selector.rs` (21 reads):

| Line | Old | New |
|---|---|---|
| 25 | `fg(border)` | `theme.border(false)` |
| 27 | `fg(accent)` | `theme.accent()` |
| 36 | `fg(selected_text)` + BOLD | `selected_label(theme)` |
| 39 | `fg(success)` | `theme.success()` |
| 41 | `fg(muted)` | `theme.muted()` |
| 69 | `fg(error)` | `theme.error()` |
| 73 | `fg(error)` + BOLD | `theme.error().add_modifier(BOLD)` |
| 78-79 | `fg(selected_text).bg(error)` + BOLD | `selected_on(theme.error(), theme.selected()).add_modifier(BOLD)` |
| 82 | `fg(error)` | `theme.error()` |
| 85 | `fg(muted)` | `theme.muted()` |
| 88-89 | `fg(selected_text).bg(success)` + BOLD | `selected_on(theme.success(), theme.selected()).add_modifier(BOLD)` |
| 134 | `fg(warning)` | `theme.warning()` |
| 138 | `fg(warning)` + BOLD | `theme.warning().add_modifier(BOLD)` |
| 143-144 | `fg(selected_text).bg(warning)` + BOLD | `selected_on(theme.warning(), theme.selected()).add_modifier(BOLD)` |
| 147 | `fg(warning)` | `theme.warning()` |
| 150 | `fg(muted)` | `theme.muted()` |
| 153-154 | `fg(selected_text).bg(success)` + BOLD | `selected_on(theme.success(), theme.selected()).add_modifier(BOLD)` |

`model_selector.rs` (16 reads):

| Line | Old | New |
|---|---|---|
| 42 | `fg(border)` | `theme.border(false)` |
| 46 | `fg(accent)` + BOLD | `theme.accent().add_modifier(BOLD)` |
| 58, 67 | `fg(warning)` | `theme.warning()` |
| 72, 88, 93, 130, 197 | `fg(dim)` | `theme.dim()` |
| 94 | `fg(text)` | `theme.text()` |
| 172 | `fg(selected_text)` + BOLD | `selected_label(theme)` |
| 175 | `fg(warning)` | `theme.warning()` |
| 177 | `fg(muted)` | `theme.muted()` |
| 203 | `fg(selected_text)` + BOLD | `selected_label(theme)` |
| 206 | `fg(accent)` | `theme.accent()` |
| 220 | `fg(text)` | `theme.text()` |

`fork_picker.rs` (9 reads): 22 `border(false)`; 26 `accent()` + BOLD; 42 `selected_label`; 45 `muted()`; 59 and 64 `warning()`; 62 and 65 `dim()`; 69 `text()`.

`slash_popup.rs` (7 reads): 29 `border(false)`; 33 `accent()` + BOLD; 48 `selected_label`; 51 `muted()`; 56 `accent()`; 57 `dim()`; 64 `text()`.

`session_picker.rs` (7 reads): 24 `border(false)`; 28 `accent()` + BOLD; 47 `selected_label`; 50 `warning()`; 52 `muted()`; 67 `dim()`; 72 `text()`.

`help_overlay.rs` (7 reads): 20 `border(false)`; 24 `accent()` + BOLD; 33 and 52 `text()` + BOLD; 41 `warning()`; 74 `dim()`; 79 `text()`.

`thinking_selector.rs` (6 reads): 26 `border(false)`; 28 `accent()`; 41 `selected_label`; 44 `success()`; 46 `muted()`; 53 `text()`.

`input.rs` (6 reads). `input_styles` (`input.rs:37-54`) is the focused/unfocused split `border` exists for, but the unfocused path dims *every* role, not just the border:

```rust
fn input_styles(terminal_focused: bool, theme: &Theme) -> InputStyles {
    if terminal_focused {
        InputStyles {
            border: theme.border(false),
            title: theme.accent().add_modifier(Modifier::BOLD),
            hint: theme.dim(),
            cursor: Style::default().add_modifier(Modifier::REVERSED),
        }
    } else {
        let dim = theme.dim().add_modifier(Modifier::DIM);
        InputStyles { border: dim, title: dim, hint: dim, cursor: Style::default().add_modifier(Modifier::HIDDEN) }
    }
}
```

Line 81 `fg(text)` becomes `theme.text()`. The test at `input.rs:223-228` compares `focused.title` with `theme.accent().add_modifier(Modifier::BOLD)` instead of `theme.colors.accent`.

**Divergence 6.** Unfocused input uses `dim` plus `DIM` for the border, the title, and the hint together (`input.rs:47-53`). `border(false)` would return the border color with no modifier, which is the focused border, not the unfocused one. The unfocused branch stays `theme.dim().add_modifier(Modifier::DIM)`.

`perf_overlay.rs` (4 reads): 34 `border(false)`; 36 `accent()`; 40 `muted()`; 44 `text()`.

`status_bar.rs`. The one `theme.colors` read is line 224; the nine `colors.X` uses under it are the real migration. `segment_style` (`status_bar.rs:217-247`):

```rust
fn segment_style(
    id: SegmentId,
    is_connected: bool,
    context_used_percent: Option<u8>,
    approval_mode: &str,
    theme: &Theme,
) -> Style {
    match id {
        SegmentId::Connection => {
            if is_connected { theme.success() } else { theme.error() }
        }
        SegmentId::Model | SegmentId::Reviewer => theme.accent(),
        SegmentId::Approval => {
            if approval_mode == "full-access" { theme.error() } else { theme.warning() }
        }
        SegmentId::Context => match context_used_percent.unwrap_or_default().min(100) {
            0..=69 => theme.dim(),
            70..=89 => theme.warning(),
            _ => theme.error(),
        },
        SegmentId::Compaction => theme.harness(),
        SegmentId::Tokens | SegmentId::Status => theme.muted(),
    }
}
```

The test at `status_bar.rs:420-423` becomes:

```rust
#[test]
fn full_access_approval_segment_uses_danger_color() {
    let theme = Theme::default();
    let style = segment_style(SegmentId::Approval, true, None, "full-access", &theme);
    assert_eq!(style.fg, theme.error().fg);
}
```

That assertion fails if the full-access segment stops using the error role, which is the consumer-visible bug. Comparing against `theme.colors.error` would not compile after Step 7 and would not catch a helper returning the right field through the wrong role.

- [ ] **Step 7: Make `ThemeColors` private and delete the bypass**

In `theme.rs`, change `pub colors: ThemeColors` to `colors: ThemeColors`, and remove `pub` from every field of `ThemeColors`. The `theme::presets`, `theme::contrast` and `theme::tests` modules are descendants of `theme`, so they still read and construct the fields; every sibling module loses access.

In `lib.rs:25`, drop `ThemeColors`:

```rust
pub use theme::{Theme, ThemeDocument, ThemeLoadError, ThemeSpacing};
```

Grep to confirm nothing remains:

```bash
rg -n "theme\.colors|colors\.(accent|border|text|muted|dim|warning|error|success|harness|tool_|diff_|selected_|user_message)" crates/yach-ui/src --glob '!theme.rs' --glob '!theme/**'
```

Expected: no matches. A match is a site the lists above missed; migrate it the same way before continuing. Do not make a field public again to silence it.

- [ ] **Step 8: Run to verify pass**

Run: `just dev cargo test -p yach-ui --lib transcript::tests::dark_transcript_buffer_is_stable_across_helper_migration -- --exact`

Expected: PASS against the fixture recorded in Step 2, with no fixture update. A failure here means a migration step changed a `dark` cell; fix the call site, do not regenerate the pin.

Run: `just dev cargo test -p yach-ui --lib`

Expected: PASS, including `render_lines_preserves_representative_entries`, `custom_theme_colors_user_and_tool_surfaces`, `adjacent_tool_rows_use_separate_success_surfaces`, `command_review_row_renders_command_and_inline_selector`, the markdown style tests, `input_styles_dim_and_hide_cursor_when_unfocused`, and `full_access_approval_segment_uses_danger_color`.

Run: `just dev cargo test -p yach-ui --lib theme::tests`

Expected: PASS. Preset tests still read `theme.colors` from inside the theme module, which privacy allows for descendants.

- [ ] **Step 9: Lint and commit**

Run: `just lint`

Expected: PASS. `selected_on` and `selected_label` live next to their only caller (or in `theme.rs` as `pub(crate)` if two files need them). If both `app.rs` and `approval_selector.rs` need `selected_on`, put it in `theme.rs` as `pub(crate) fn selected_on(role: Style, selected: Style) -> Style` rather than copying it. Two copies is how the pairing drifts.

```bash
jj commit crates/yach-ui/src crates/yach-ui/tests/fixtures/dark-transcript-buffer.txt -m "Render widgets through theme helpers"
```

### Task 4: Theme selection

**Files:**
- Create: `crates/yach-ui/src/theme/select.rs`
- Modify: `crates/yach-ui/src/theme.rs` (add `mod select;` next to `mod palettes;`; re-export `Appearance`, `ExtensionThemes`, `BaseError`, `appearance_from_colorfgbg`, `is_builtin_theme_name`, `builtin_theme_names`, `resolve_base` from `lib.rs` — see Step 5)
- Modify: `crates/yach-ui/src/lib.rs:25` (the `pub use theme::...` line)
- Modify: `crates/yach-ui/src/app.rs:33-37` (`RunTuiOptions`), `app.rs:788-857` (`App::new_with_theme`), `app.rs:4229` (the one `App::new_with_theme` call)
- Modify: `crates/yach-cli/src/main.rs:425-450` (delete `load_tui_theme` and `tui_theme_path`), `main.rs:3494-3498` and `main.rs:3757-3772` (`RunTuiOptions` literals), `main.rs:5856-5865` (test imports), `main.rs:6016-6046` (replace the theme-path test)
- Test: `crates/yach-ui/src/theme/select.rs` (`#[cfg(test)] mod tests`), `crates/yach-ui/src/app.rs` tests module (startup notices), `crates/yach-cli/src/main.rs` tests module (`resolve_theme` table)

**Interfaces:**
- Consumes: `Theme::{dark, light, system, monochrome}`; `ThemeDocument::{parse, base, apply}`; `BundledPalette`, `BUNDLED_PALETTES`, `bundled_palette` (Task 2); `ThemeMode` (Task 1; `Theme::mode(&self) -> ThemeMode`). `ThemeLoadError` has no `Io` variant after Task 1.
- Produces (yach-ui):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance { Dark, Light }

#[must_use]
pub fn appearance_from_colorfgbg(value: Option<&str>) -> Appearance;

pub const PRESET_NAMES: [&str; 4];

#[must_use]
pub fn is_builtin_theme_name(name: &str) -> bool;

#[must_use]
pub fn builtin_theme_names() -> Vec<&'static str>;

pub trait ExtensionThemes {
    fn theme_json(&self, qualified_name: &str) -> Option<String>;
}
impl ExtensionThemes for ();

#[derive(Debug)]
pub enum BaseError {
    Unknown(String),
    ExtensionBaseNotBuiltin { theme: String, base: String },
    Invalid { name: String, error: ThemeLoadError },
}

pub fn resolve_base(
    name: &str,
    appearance: Appearance,
    extensions: &dyn ExtensionThemes,
) -> Result<Theme, BaseError>;
```

Produces (yach-cli, private):

```rust
struct ThemeEnv {
    yach_theme: Option<std::ffi::OsString>,
    no_color: Option<std::ffi::OsString>,
    colorfgbg: Option<String>,
    home: Option<PathBuf>,
}

impl ThemeEnv {
    fn from_process() -> Self;
}

fn resolve_theme(
    env: &ThemeEnv,
    extensions: &dyn yach_ui::ExtensionThemes,
) -> Result<(Theme, Vec<String>), String>;
```

`resolve_theme` returns `Err` only when an explicit file or the user file exists but cannot be read, or is not valid theme JSON. An unknown name, or a file whose `base` does not resolve, is `Ok` plus one diagnostic and the `auto` theme (file tokens applied over `auto` when the file parsed). `NO_COLOR` set to a non-empty value returns `Theme::monochrome()` and no diagnostics, ignoring every other source.

`RunTuiOptions` gains `pub startup_notices: Vec<String>` and loses `Copy`. `App` appends each notice once, as its own transcript status line, before the first event.

- [ ] **Step 1: Write the failing selection tests**

Create `crates/yach-ui/src/theme/select.rs` containing only this tests module. It does not compile until Step 3 adds the module and the functions.

```rust
#[cfg(test)]
mod tests {
    use super::{
        Appearance, BaseError, ExtensionThemes, appearance_from_colorfgbg, builtin_theme_names,
        is_builtin_theme_name, resolve_base,
    };
    use crate::theme::{Theme, ThemeMode};

    #[test]
    fn colorfgbg_last_field_picks_appearance() {
        // Last field is the background. Index below 8 is dark; anything else
        // that parses as an integer is light. Absent, empty, and garbage are
        // dark so a terminal that sets nothing does not flip to light.
        let cases = [
            (None, Appearance::Dark),
            (Some(""), Appearance::Dark),
            (Some("nope"), Appearance::Dark),
            (Some("15;0"), Appearance::Dark),
            (Some("0;default;15"), Appearance::Light),
            (Some("default;7"), Appearance::Dark),
            (Some("15;8"), Appearance::Light),
            (Some("0;15;"), Appearance::Dark),
            (Some("0;default;default"), Appearance::Dark),
        ];
        for (value, expected) in cases {
            assert_eq!(
                appearance_from_colorfgbg(value),
                expected,
                "COLORFGBG={value:?}"
            );
        }
    }

    struct OneTheme {
        name: &'static str,
        json: &'static str,
    }

    impl ExtensionThemes for OneTheme {
        fn theme_json(&self, qualified_name: &str) -> Option<String> {
            (qualified_name == self.name).then(|| self.json.to_owned())
        }
    }

    #[test]
    fn resolve_base_presets_follow_appearance_and_keep_mode() -> Result<(), String> {
        let none = &();
        let dark = resolve_base("dark", Appearance::Light, none)
            .map_err(|error| format!("{error:?}"))?;
        if dark != Theme::dark() || dark.mode() != ThemeMode::Painted {
            return Err(String::from("dark preset ignored appearance but lost its value"));
        }
        let light = resolve_base("light", Appearance::Dark, none)
            .map_err(|error| format!("{error:?}"))?;
        if light != Theme::light() {
            return Err(String::from("light preset did not return Theme::light()"));
        }
        let auto_dark = resolve_base("auto", Appearance::Dark, none)
            .map_err(|error| format!("{error:?}"))?;
        let auto_light = resolve_base("auto", Appearance::Light, none)
            .map_err(|error| format!("{error:?}"))?;
        if auto_dark != Theme::dark() || auto_light != Theme::light() {
            return Err(String::from("auto did not follow appearance"));
        }
        let system = resolve_base("system", Appearance::Dark, none)
            .map_err(|error| format!("{error:?}"))?;
        if system.mode() != ThemeMode::System {
            return Err(String::from("system preset is not System mode"));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_palette_applies_over_its_own_base() -> Result<(), String> {
        let theme = resolve_base("catppuccin-mocha", Appearance::Light, &())
            .map_err(|error| format!("{error:?}"))?;
        // Mocha is a dark scheme. Appearance must not retarget it, and its
        // text color is the scheme's base05 (#cdd6f4), not the dark preset's
        // White and not the light preset's text.
        if theme.mode() != ThemeMode::Painted {
            return Err(String::from("palette lost the preset mode"));
        }
        let fg = theme.text().fg;
        if fg != Some(ratatui::style::Color::Rgb(205, 214, 244)) {
            return Err(format!("mocha text resolved to {fg:?}, not #cdd6f4"));
        }
        if theme == Theme::dark() || theme == Theme::light() {
            return Err(String::from("palette resolved to an unmodified preset"));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_unknown_name_is_unknown() {
        let error = resolve_base("not-a-theme", Appearance::Dark, &());
        assert!(
            matches!(error, Err(BaseError::Unknown(name)) if name == "not-a-theme"),
            "{error:?}"
        );
    }

    #[test]
    fn resolve_base_extension_theme_applies_over_builtin_base() -> Result<(), String> {
        let extensions = OneTheme {
            name: "nightowl:dim",
            json: r##"{"base":"catppuccin-mocha","colors":{"accent":"#112233"}}"##,
        };
        let theme = resolve_base("nightowl:dim", Appearance::Dark, &extensions)
            .map_err(|error| format!("{error:?}"))?;
        if theme.text().fg != Some(ratatui::style::Color::Rgb(205, 214, 244)) {
            return Err(String::from("extension theme did not keep the palette text"));
        }
        if theme.accent().fg != Some(ratatui::style::Color::Rgb(17, 34, 51)) {
            return Err(String::from("extension accent override was dropped"));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_rejects_extension_theme_whose_base_is_not_builtin() {
        let chained = OneTheme {
            name: "nightowl:dim",
            json: r#"{"base":"other:theme"}"#,
        };
        let error = resolve_base("nightowl:dim", Appearance::Dark, &chained);
        assert!(
            matches!(
                error,
                Err(BaseError::ExtensionBaseNotBuiltin { theme, base })
                    if theme == "nightowl:dim" && base == "other:theme"
            ),
            "{error:?}"
        );

        let missing = OneTheme {
            name: "nightowl:dim",
            json: r#"{"base":"not-a-palette"}"#,
        };
        let error = resolve_base("nightowl:dim", Appearance::Dark, &missing);
        assert!(
            matches!(
                error,
                Err(BaseError::ExtensionBaseNotBuiltin { base, .. }) if base == "not-a-palette"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn resolve_base_extension_theme_defaults_base_to_dark() -> Result<(), String> {
        let extensions = OneTheme {
            name: "nightowl:dim",
            json: r##"{"colors":{"accent":"#112233"}}"##,
        };
        let theme = resolve_base("nightowl:dim", Appearance::Light, &extensions)
            .map_err(|error| format!("{error:?}"))?;
        if theme.text().fg != Theme::dark().text().fg {
            return Err(String::from("absent extension base did not start from dark"));
        }
        if theme.accent().fg != Some(ratatui::style::Color::Rgb(17, 34, 51)) {
            return Err(String::from("override on a defaulted extension base was dropped"));
        }
        Ok(())
    }

    #[test]
    fn builtin_names_cover_presets_and_palettes_only() {
        assert!(is_builtin_theme_name("auto"));
        assert!(is_builtin_theme_name("catppuccin-latte"));
        assert!(!is_builtin_theme_name("nightowl:dim"));
        assert!(!is_builtin_theme_name("./dark"));
        let names = builtin_theme_names();
        assert_eq!(names.len(), 4 + 18);
        assert!(names.contains(&"dark"));
        assert!(names.contains(&"monokai"));
        assert!(!names.contains(&"nightowl:dim"));
    }
}
```

`colorfgbg_last_field_picks_appearance` fails a parser that takes the first field (so `15;0` becomes light) or that treats `default` as light. `resolve_base_palette_applies_over_its_own_base` fails a resolver that honors `Appearance` for palette names, which would paint Mocha light on a light terminal. The extension tests fail a resolver that accepts `other:theme` as a base (a cycle) or that drops the extension's `colors` when applying over a palette.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-ui theme::select::tests -- --test-threads=1`

Expected: FAIL at compile time, because `mod select` is not declared and `appearance_from_colorfgbg` does not exist. Same reading as Task 2 Step 21: "no test matched the filter" after a clean compile is the failure, not a pass.

- [ ] **Step 3: Implement `theme/select.rs`**

In `crates/yach-ui/src/theme.rs`, next to `mod palettes;`:

```rust
mod select;
```

`crates/yach-ui/src/theme/select.rs`, above the tests module:

```rust
use crate::theme::palettes::{BUNDLED_PALETTES, bundled_palette};
use crate::theme::{Theme, ThemeDocument, ThemeLoadError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

pub const PRESET_NAMES: [&str; 4] = ["dark", "light", "system", "auto"];

/// omp's rule on the last `;`-separated field: an integer below 8 is dark,
/// any other integer is light. Absent, empty, or non-integer is dark.
#[must_use]
pub fn appearance_from_colorfgbg(value: Option<&str>) -> Appearance {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Appearance::Dark;
    };
    let Some(last) = value.rsplit(';').next() else {
        return Appearance::Dark;
    };
    match last.trim().parse::<i64>() {
        Ok(index) if index < 8 => Appearance::Dark,
        Ok(_) => Appearance::Light,
        Err(_) => Appearance::Dark,
    }
}

#[must_use]
pub fn is_builtin_theme_name(name: &str) -> bool {
    PRESET_NAMES.contains(&name) || bundled_palette(name).is_some()
}

#[must_use]
pub fn builtin_theme_names() -> Vec<&'static str> {
    PRESET_NAMES
        .into_iter()
        .chain(BUNDLED_PALETTES.iter().map(|palette| palette.name))
        .collect()
}

pub trait ExtensionThemes {
    fn theme_json(&self, qualified_name: &str) -> Option<String>;
}

impl ExtensionThemes for () {
    fn theme_json(&self, _qualified_name: &str) -> Option<String> {
        None
    }
}

#[derive(Debug)]
pub enum BaseError {
    Unknown(String),
    ExtensionBaseNotBuiltin { theme: String, base: String },
    Invalid { name: String, error: ThemeLoadError },
}

fn preset(name: &str, appearance: Appearance) -> Option<Theme> {
    match name {
        "dark" => Some(Theme::dark()),
        "light" => Some(Theme::light()),
        "system" => Some(Theme::system()),
        "auto" => Some(match appearance {
            Appearance::Dark => Theme::dark(),
            Appearance::Light => Theme::light(),
        }),
        _ => None,
    }
}

fn apply_palette(palette: &crate::theme::palettes::BundledPalette) -> Result<Theme, BaseError> {
    let document = ThemeDocument::parse(palette.json).map_err(|error| BaseError::Invalid {
        name: palette.name.to_owned(),
        error,
    })?;
    let base = match document.base() {
        Some("light") => Theme::light(),
        _ => Theme::dark(),
    };
    document.apply(base).map_err(|error| BaseError::Invalid {
        name: palette.name.to_owned(),
        error,
    })
}

/// Resolves a `base` value. Presets resolve directly. `auto` follows
/// `appearance`. A bundled palette is parsed and applied over its own `base`
/// preset, ignoring `appearance`. `<ext>:<id>` loads JSON from `extensions`;
/// that document's `base` must itself be a preset or bundled palette (default
/// `dark` when absent) and is never another extension theme.
pub fn resolve_base(
    name: &str,
    appearance: Appearance,
    extensions: &dyn ExtensionThemes,
) -> Result<Theme, BaseError> {
    if let Some(theme) = preset(name, appearance) {
        return Ok(theme);
    }
    if let Some(palette) = bundled_palette(name) {
        return apply_palette(palette);
    }
    let Some(json) = extensions.theme_json(name) else {
        return Err(BaseError::Unknown(name.to_owned()));
    };
    let document = ThemeDocument::parse(&json).map_err(|error| BaseError::Invalid {
        name: name.to_owned(),
        error,
    })?;
    let base_name = match document.base() {
        Some(name) => name,
        None => "dark",
    };
    let base = if let Some(theme) = preset(base_name, appearance) {
        theme
    } else if let Some(palette) = bundled_palette(base_name) {
        apply_palette(palette)?
    } else {
        return Err(BaseError::ExtensionBaseNotBuiltin {
            theme: name.to_owned(),
            base: base_name.to_owned(),
        });
    };
    document.apply(base).map_err(|error| BaseError::Invalid {
        name: name.to_owned(),
        error,
    })
}
```

`appearance_from_colorfgbg` uses `rsplit` so `0;default;15` reads `15` and `15;0` reads `0`. The `let Some` on `rsplit` is unreachable (`rsplit` always yields one piece); it is kept so the function returns `Dark` on every non-integer path without a separate default. An extension theme naming `auto` resolves that `auto` against `appearance`; an extension theme naming a palette does not. Both are accepted behavior (the spec allows any built-in preset as an extension theme's `base`).

- [ ] **Step 4: Run to verify pass**

Run: `just dev cargo test -p yach-ui theme::select::tests -- --test-threads=1`

Expected: PASS, all eight tests.

- [ ] **Step 5: Re-export the selection API**

`crates/yach-ui/src/lib.rs:25` is `pub use theme::{Theme, ThemeDocument, ThemeLoadError, ThemeSpacing};` after Task 3 (Task 3 Step 7 removed `ThemeColors`; do not re-add it, its fields are private). The CLI resolver lives in another crate, so the selection API, `ThemeDocument` (the CLI reads and parses theme files), and `ThemeMode` (the CLI tests assert it) have to leave `yach-ui`. Replace that line with:

```rust
pub use theme::{
    Appearance, BaseError, ExtensionThemes, Theme, ThemeDocument, ThemeLoadError, ThemeMode,
    ThemeSpacing, appearance_from_colorfgbg, builtin_theme_names, is_builtin_theme_name,
    resolve_base,
};
```

`theme.rs` re-exports the selection items from the private `select` module. Next to `mod select;`:

```rust
pub use select::{
    Appearance, BaseError, ExtensionThemes, appearance_from_colorfgbg, builtin_theme_names,
    is_builtin_theme_name, resolve_base,
};
```

`ThemeDocument` and `ThemeMode` are already `pub` in `theme.rs` (Task 1). Do not re-export `BUNDLED_PALETTES`; callers outside the crate resolve by name.

- [ ] **Step 6: Write the failing CLI precedence tests**

In `crates/yach-cli/src/main.rs`, delete `load_tui_theme` (`main.rs:425-432`) and `tui_theme_path` (`main.rs:434-450`) only after the tests below are in place and the old test is removed; cargo will not compile the crate with both the old function and a tests module that still imports it. Order within this step: add the new tests, remove the import of `tui_theme_path` from the tests module (`main.rs:5864`), replace `tui_theme_path_prefers_explicit_then_project_then_user` (`main.rs:6016-6046`) with the tests below, then delete the two functions. The expected compile failure is `resolve_theme` not found.

Add `ThemeEnv` and `resolve_theme` to the `use super::{...}` list at `main.rs:5856-5865` and remove `tui_theme_path` from it. The tests module does not see the parent's `yach_ui` import (`main.rs:43-46`); extend its existing `use yach_ui::negotiate_with;` (`main.rs:5887`) to `use yach_ui::{Theme, ThemeMode, negotiate_with};`.

The tests compare rendered colors, and the `yach` crate has no `ratatui` dependency. Add it as a dev-dependency only (the version `yach-ui` already locks, so `Cargo.lock` gains no new package) to `crates/yach-cli/Cargo.toml`:

```toml
[dev-dependencies]
ratatui = "0.30"
```

Replace the test at `main.rs:6016-6046` with:

```rust
    fn theme_env(
        yach_theme: Option<&str>,
        no_color: Option<&str>,
        colorfgbg: Option<&str>,
        home: Option<&Path>,
    ) -> ThemeEnv {
        ThemeEnv {
            yach_theme: yach_theme.map(std::ffi::OsString::from),
            no_color: no_color.map(std::ffi::OsString::from),
            colorfgbg: colorfgbg.map(str::to_owned),
            home: home.map(Path::to_path_buf),
        }
    }

    fn theme_file(dir: &Path, body: &str) -> Result<PathBuf, String> {
        expect_ok(std::fs::create_dir_all(dir.join(".yach")))?;
        let path = dir.join(".yach/theme.json");
        expect_ok(std::fs::write(&path, body))?;
        Ok(path)
    }

    #[test]
    fn resolve_theme_precedence() -> Result<(), String> {
        let root = TestTempDir::new("theme-precedence")?;
        let home = root.path().join("home");
        let explicit = root.path().join("chosen.json");
        expect_ok(std::fs::write(&explicit, r##"{"colors":{"accent":"#010101"}}"##))?;
        theme_file(&home, r##"{"colors":{"accent":"#020202"}}"##)?;
        let explicit_string = path_string(&explicit)?;

        // A preset name beats the user file and is not read as a path.
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(Some("light"), None, None, Some(&home)),
            &(),
        ))?;
        expect_true(notices.is_empty(), "preset name is silent")?;
        expect_equal(&theme, &Theme::light())?;

        // A path whose file name is the reserved word `dark` is still a path.
        // `YACH_THEME=./dark` is the user-facing spelling (spec: a reserved
        // name is reachable as `./<name>`); `resolve_theme` takes the string
        // as a path relative to the process cwd, which a unit test must not
        // change, so the test passes the absolute path of a file named `dark`.
        let dot_dark = root.path().join("dark");
        expect_ok(std::fs::write(&dot_dark, r##"{"colors":{"accent":"#030303"}}"##))?;
        let (theme, _) = expect_ok(resolve_theme(
            &theme_env(Some(&path_string(&dot_dark)?), None, None, Some(&home)),
            &(),
        ))?;
        expect_equal(
            &theme.accent().fg,
            &Some(ratatui::style::Color::Rgb(3, 3, 3)),
        )?;

        // Any other value is a path and beats the user file.
        let (theme, _) = expect_ok(resolve_theme(
            &theme_env(Some(&explicit_string), None, None, Some(&home)),
            &(),
        ))?;
        expect_equal(
            &theme.accent().fg,
            &Some(ratatui::style::Color::Rgb(1, 1, 1)),
        )?;

        // No YACH_THEME: the user file wins over the default.
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(None, None, Some("0;15"), Some(&home)),
            &(),
        ))?;
        expect_true(notices.is_empty(), "user file is silent")?;
        expect_equal(
            &theme.accent().fg,
            &Some(ratatui::style::Color::Rgb(2, 2, 2)),
        )?;

        // No file: `auto`. COLORFGBG light selects light; absent stays dark.
        let empty_home = root.path().join("empty-home");
        expect_ok(std::fs::create_dir_all(&empty_home))?;
        let (theme, _) = expect_ok(resolve_theme(
            &theme_env(None, None, Some("0;15"), Some(&empty_home)),
            &(),
        ))?;
        expect_equal(&theme, &Theme::light())?;
        let (theme, _) = expect_ok(resolve_theme(
            &theme_env(None, None, None, Some(&empty_home)),
            &(),
        ))?;
        expect_equal(&theme, &Theme::dark())?;

        // A bundled palette name is not a path.
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(Some("nord"), None, Some("0;15"), Some(&home)),
            &(),
        ))?;
        expect_true(notices.is_empty(), "palette name is silent")?;
        expect_true(theme != Theme::light(), "palette must not follow COLORFGBG")?;
        expect_true(theme.mode() == ThemeMode::Painted, "palette keeps painted mode")?;
        Ok(())
    }

    #[test]
    fn resolve_theme_unknown_name_lists_builtins_and_uses_auto() -> Result<(), String> {
        let root = TestTempDir::new("theme-unknown")?;
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(Some("not-a-theme"), None, Some("0;15"), Some(root.path())),
            &(),
        ))?;
        expect_equal(&theme, &Theme::light())?;
        expect_equal(&notices.len(), &1)?;
        let notice = notices.first().ok_or("missing diagnostic")?;
        expect_true(notice.contains("not-a-theme"), "diagnostic names the request")?;
        expect_true(notice.contains("dark"), "diagnostic lists presets")?;
        expect_true(notice.contains("catppuccin-mocha"), "diagnostic lists palettes")?;
        expect_true(notice.contains("auto"), "diagnostic lists auto")?;
        Ok(())
    }

    #[test]
    fn resolve_theme_file_with_unknown_base_keeps_file_tokens_over_auto() -> Result<(), String> {
        let root = TestTempDir::new("theme-bad-base")?;
        let path = root.path().join("custom.json");
        expect_ok(std::fs::write(
            &path,
            r##"{"base":"not-a-palette","colors":{"accent":"#040404"}}"##,
        ))?;
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(Some(&path_string(&path)?), None, Some("0;15"), None),
            &(),
        ))?;
        expect_equal(&notices.len(), &1)?;
        let notice = notices.first().ok_or("missing diagnostic")?;
        expect_true(notice.contains("not-a-palette"), "diagnostic names the base")?;
        expect_equal(
            &theme.accent().fg,
            &Some(ratatui::style::Color::Rgb(4, 4, 4)),
        )?;
        // `auto` on COLORFGBG=0;15 is light, so an unresolved base must not
        // silently become the dark preset underneath the override.
        expect_equal(&theme.text().fg, &Theme::light().text().fg)?;
        Ok(())
    }

    #[test]
    fn resolve_theme_no_color_overrides_every_source() -> Result<(), String> {
        let root = TestTempDir::new("theme-no-color")?;
        let home = root.path().join("home");
        theme_file(&home, r##"{"colors":{"accent":"#050505"}}"##)?;
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(Some("light"), Some("1"), Some("0;15"), Some(&home)),
            &(),
        ))?;
        expect_true(notices.is_empty(), "NO_COLOR is silent")?;
        expect_equal(&theme, &Theme::monochrome())?;
        expect_true(
            theme.mode() == ThemeMode::Mono,
            "NO_COLOR selects Mono",
        )?;

        // Present but empty does not count (https://no-color.org/).
        let (theme, _) = expect_ok(resolve_theme(
            &theme_env(Some("light"), Some(""), None, Some(&home)),
            &(),
        ))?;
        expect_equal(&theme, &Theme::light())?;
        Ok(())
    }

    #[test]
    fn resolve_theme_unreadable_explicit_file_is_usage_error() -> Result<(), String> {
        let root = TestTempDir::new("theme-missing")?;
        let missing = root.path().join("missing.json");
        let error = resolve_theme(
            &theme_env(Some(&path_string(&missing)?), None, None, None),
            &(),
        );
        expect_true(error.is_err(), "missing explicit file must be Err")?;
        let Err(message) = error else {
            return Err(String::from("expected Err"));
        };
        expect_true(
            message.contains("missing.json"),
            "error names the file the user asked for",
        )?;

        let home = root.path().join("home");
        theme_file(&home, "not json")?;
        let error = resolve_theme(&theme_env(None, None, None, Some(&home)), &());
        expect_true(error.is_err(), "invalid user file must be Err")?;
        Ok(())
    }
```

These replace `tui_theme_path_prefers_explicit_then_project_then_user`, whose assertion that `<project>/.yach/theme.json` wins (`main.rs:6031-6034`) is the behavior this spec removes. A project file must not appear in any of the new tests as an input to `resolve_theme`: the resolver has no project-root parameter. The notice for that file is the next step.

`resolve_theme_file_with_unknown_base_keeps_file_tokens_over_auto` fails an implementation that returns `Theme::dark()` on a bad base (dropping the file's accent) or that returns `Err` (today's `Theme::load` failure mode, `main.rs:431`). `resolve_theme_no_color_overrides_every_source` fails an implementation that lets `YACH_THEME=light` beat `NO_COLOR`. `resolve_theme_unreadable_explicit_file_is_usage_error` fails an implementation that turns a missing explicit file into the `auto` fallback; that file was named by the user and today's code returns `UsageError` (`main.rs:3765-3767`).

- [ ] **Step 7: Run to verify failure**

Run: `just dev cargo test -p yach resolve_theme -- --test-threads=1`

Expected: FAIL at compile time with `cannot find function 'resolve_theme'` (and `ThemeEnv`). Not an assertion failure.

- [ ] **Step 8: Implement `ThemeEnv` and `resolve_theme`**

In `crates/yach-cli/src/main.rs`, where `load_tui_theme` and `tui_theme_path` were (`main.rs:425-450`), insert:

```rust
struct ThemeEnv {
    yach_theme: Option<std::ffi::OsString>,
    no_color: Option<std::ffi::OsString>,
    colorfgbg: Option<String>,
    home: Option<PathBuf>,
}

impl ThemeEnv {
    fn from_process() -> Self {
        Self {
            yach_theme: std::env::var_os("YACH_THEME"),
            no_color: std::env::var_os("NO_COLOR"),
            colorfgbg: std::env::var("COLORFGBG").ok(),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

fn no_color_set(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

fn read_theme_file(path: &Path) -> Result<yach_ui::ThemeDocument, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to load theme {}: {error}", path.display()))?;
    yach_ui::ThemeDocument::parse(&json)
        .map_err(|error| format!("failed to load theme {}: {error}", path.display()))
}

fn fallback_notice(requested: &str) -> String {
    format!(
        "unknown theme '{requested}'; using auto. built-in themes: {}",
        yach_ui::builtin_theme_names().join(", ")
    )
}

fn resolve_theme(
    env: &ThemeEnv,
    extensions: &dyn yach_ui::ExtensionThemes,
) -> Result<(Theme, Vec<String>), String> {
    if no_color_set(env.no_color.as_deref()) {
        return Ok((Theme::monochrome(), Vec::new()));
    }
    let appearance = yach_ui::appearance_from_colorfgbg(env.colorfgbg.as_deref());
    let auto = yach_ui::resolve_base("auto", appearance, extensions)
        .map_err(|error| format!("failed to resolve auto theme: {error:?}"))?;

    if let Some(requested) = env.yach_theme.as_deref() {
        if let Some(name) = requested.to_str()
            && yach_ui::is_builtin_theme_name(name)
        {
            let theme = yach_ui::resolve_base(name, appearance, extensions)
                .map_err(|error| format!("failed to resolve theme {name}: {error:?}"))?;
            return Ok((theme, Vec::new()));
        }
        let path = PathBuf::from(requested);
        // Not a file: the value was a name that matched nothing. A file that
        // exists but cannot be read still fails below, as today's UsageError.
        if !path.is_file() {
            let requested = requested.to_string_lossy();
            return Ok((auto, vec![fallback_notice(requested.as_ref())]));
        }
        let document = read_theme_file(&path)?;
        return apply_document(&path, document, appearance, extensions);
    }

    if let Some(home) = env.home.as_deref() {
        let path = home.join(".yach/theme.json");
        if path.is_file() {
            let document = read_theme_file(&path)?;
            return apply_document(&path, document, appearance, extensions);
        }
    }
    Ok((auto, Vec::new()))
}

fn apply_document(
    path: &Path,
    document: yach_ui::ThemeDocument,
    appearance: yach_ui::Appearance,
    extensions: &dyn yach_ui::ExtensionThemes,
) -> Result<(Theme, Vec<String>), String> {
    let base_name = match document.base() {
        Some(name) => name,
        None => "dark",
    };
    match yach_ui::resolve_base(base_name, appearance, extensions) {
        Ok(base) => document
            .apply(base)
            .map(|theme| (theme, Vec::new()))
            .map_err(|error| format!("failed to load theme {}: {error}", path.display())),
        Err(error) => {
            let auto = yach_ui::resolve_base("auto", appearance, extensions)
                .map_err(|error| format!("failed to resolve auto theme: {error:?}"))?;
            let theme = document
                .apply(auto)
                .map_err(|error| format!("failed to load theme {}: {error}", path.display()))?;
            Ok((theme, vec![base_error_notice(&error)]))
        }
    }
}

fn base_error_notice(error: &yach_ui::BaseError) -> String {
    match error {
        yach_ui::BaseError::Unknown(name) => fallback_notice(name),
        yach_ui::BaseError::ExtensionBaseNotBuiltin { theme, base } => format!(
            "theme '{theme}' cannot use base '{base}': an extension theme's base must be a built-in theme; using auto"
        ),
        yach_ui::BaseError::Invalid { name, error } => {
            format!("theme '{name}' is invalid: {error}; using auto")
        }
    }
}
```

Every `BaseError` falls back the same way: the file's own tokens over `auto`, plus one diagnostic naming what failed. The spec's fallback rule covers a missing, disabled, or invalid selected theme (`docs/project/specs/2026-10-02-theming-engine-design.md:546-551`), so none of them is a `UsageError`. Only a file the user named that cannot be read or parsed is.

`read_theme_file` preserves today's error text shape (`main.rs:431`: `failed to load theme {}: {error}`), which `run_tui_command` puts in `CommandResult::UsageError` (`main.rs:3765-3767`). A path that does not exist fails `read_to_string`, so an explicit missing file is `Err`. A user file is only read when `is_file()` is true, so a missing user file falls through to `auto`.

`NO_COLOR` is checked before any file read, so a broken user file does not surface when `NO_COLOR=1`. That matches "overrides every theme source" (`docs/project/specs/2026-10-02-theming-engine-design.md:595-598`).

- [ ] **Step 9: Wire `run_tui_command` and drop the project lookup**

`run_tui_command` (`main.rs:3757-3772`) currently resolves the theme from `project_root`. Replace the theme block only; leave the `project_root` binding, it is still used for model overrides at `main.rs:3788`.

```rust
    let env = ThemeEnv::from_process();
    let (theme, mut startup_notices) = match resolve_theme(&env, &()) {
        Ok(resolved) => resolved,
        Err(error) => {
            return CommandResult::UsageError { message: error };
        }
    };
    if let Some(root) = project_root.as_deref() {
        let legacy = root.join(".yach/theme.json");
        if legacy.is_file() {
            startup_notices.push(String::from(concat!(
                "project themes are not supported; move it to `~/.yach/theme.json` ",
                "or set `YACH_THEME`"
            )));
        }
    }
    let ui_options = RunTuiOptions {
        resume_session: resume,
        theme,
        startup_notices,
    };
```

The notice string is the spec text (`docs/project/specs/2026-10-02-theming-engine-design.md:476-477`), including the backticks. The file is not read, deleted, or rewritten.

`RunTuiOptions` is built in one other place, the connection-smoke fixture at `main.rs:3494-3498`. Add the field there:

```rust
                ui_options: RunTuiOptions {
                    resume_session: false,
                    theme: Theme::default(),
                    startup_notices: Vec::new(),
                },
```

`Vec` is already in scope through `std` prelude. No other `RunTuiOptions {` literal exists (`app.rs:4209` uses `Default`).

- [ ] **Step 10: Run the CLI tests**

Run: `just dev cargo test -p yach resolve_theme -- --test-threads=1`

Expected: PASS, five tests. `tui_theme_path_prefers_explicit_then_project_then_user` no longer exists; a failure naming it means Step 6 did not delete it.

- [ ] **Step 11: Write the failing startup-notice test**

`RunTuiOptions` does not carry notices yet. `BenchmarkApp` (`app.rs:4077-4090`) builds `App` through `App::new`, which does not see options, so a `BenchmarkApp` test cannot reach this path. The production construction is the single call at `app.rs:4229`. Add a crate-visible constructor that takes the options and test it directly.

Resume matters here. With `--resume`, the first `SessionMessagesUpdated` calls `hydrate_transcript_from_session_messages` (`app.rs:2119-2175`), which starts with `self.transcript.clear()`. A notice appended at construction would be wiped before the user sees it. So the constructor appends notices immediately for a fresh session, and holds them for a resumed one until hydration has rebuilt the transcript.

In the `app.rs` tests module (the module containing `session_messages_hydrate_empty_transcript_after_explicit_resume_request`, `app.rs:5551`), add:

```rust
    const LEGACY_NOTICE: &str = concat!(
        "project themes are not supported; move it to `~/.yach/theme.json` ",
        "or set `YACH_THEME`"
    );

    fn notice_options(resume_session: bool) -> RunTuiOptions {
        RunTuiOptions {
            resume_session,
            theme: Theme::default(),
            startup_notices: vec![
                String::from(LEGACY_NOTICE),
                String::from("unknown theme 'nope'; using auto"),
            ],
        }
    }

    fn status_contents(app: &App) -> Vec<&str> {
        app.transcript
            .entries()
            .iter()
            .filter(|entry| matches!(entry.kind, transcript::EntryKind::Status))
            .map(|entry| entry.content.as_str())
            .collect()
    }

    #[test]
    fn startup_notices_land_in_the_transcript_before_the_first_event() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let app = App::new_with_options(tx, &notice_options(false));

        assert_eq!(app.transcript.entries().len(), 2);
        assert_eq!(
            status_contents(&app),
            vec![LEGACY_NOTICE, "unknown theme 'nope'; using auto"]
        );
    }

    #[test]
    fn startup_notices_survive_resume_hydration_once() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new_with_options(tx, &notice_options(true));
        assert_eq!(app.session_message_hydration, SessionMessageHydration::ExplicitResume);

        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![
                session_message("user", "u1", "Start"),
                session_message("assistant", "a1", "Answer"),
            ],
        });
        assert_eq!(app.transcript.entries()[0].content, "Start");
        assert_eq!(
            status_contents(&app),
            vec![LEGACY_NOTICE, "unknown theme 'nope'; using auto"]
        );

        // A later `/resume` of another session must not repeat them.
        app.session_message_hydration = SessionMessageHydration::ExplicitResume;
        app.handle_server_event(ServerEvent::SessionMessagesUpdated {
            messages: vec![session_message("user", "u2", "Other")],
        });
        assert!(status_contents(&app).is_empty());
    }
```

The first test fails an implementation that stores the notice in `status_message` (the transient status bar, `app.rs:743`) or joins both notices into one line; each source stays attributable. The second fails one that appends at construction under `--resume` (hydration clears it) or re-appends on every hydration.

- [ ] **Step 12: Run to verify failure**

Run: `just dev cargo test -p yach-ui startup_notices -- --test-threads=1`

Expected: FAIL at compile time: `new_with_options` not found and `RunTuiOptions` has no `startup_notices`.

- [ ] **Step 13: Carry notices from `RunTuiOptions` into the transcript**

`RunTuiOptions` (`app.rs:33-37`) becomes:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunTuiOptions {
    pub resume_session: bool,
    pub theme: Theme,
    pub startup_notices: Vec<String>,
}
```

Drop `Copy`. `Default` still derives because every field is `Default`. Every CLI use moves `ui_options` exactly once (each `match` arm in `run_tui_command`, `main.rs:3795-3833`), and `run_tui_with_native_backend_config_observed` reads the `bool` `resume_session` (`main.rs:4021`) before the single move at `main.rs:4203`, so nothing else changes.

Add a field to `App` next to `session_message_hydration` (`app.rs:766`) and initialize it in `new_with_theme` (`app.rs:836`):

```rust
    /// Startup notices held until `--resume` hydration rebuilds the transcript.
    pending_startup_notices: Vec<String>,
```

```rust
            pending_startup_notices: Vec::new(),
```

Add the constructor next to `App::new_with_theme` (`app.rs:793`):

```rust
    fn new_with_options(
        client_tx: mpsc::UnboundedSender<ClientEvent>,
        options: &RunTuiOptions,
    ) -> Self {
        let mut app = Self::new_with_theme(client_tx, options.theme);
        if options.resume_session {
            app.session_message_hydration = SessionMessageHydration::ExplicitResume;
            app.pending_startup_notices.clone_from(&options.startup_notices);
        } else {
            for notice in &options.startup_notices {
                app.transcript.append_status(notice);
            }
        }
        app
    }
```

At the end of `hydrate_transcript_from_session_messages`, after the `for message in messages` loop (`app.rs:2174`):

```rust
        for notice in std::mem::take(&mut self.pending_startup_notices) {
            self.transcript.append_status(&notice);
        }
```

`append_status` (`transcript.rs:187-191`) pushes one `EntryKind::Status` per call. `mem::take` empties the field, so a later `/resume` hydration appends nothing.

Replace the construction and the resume branch at `app.rs:4229-4232`:

```rust
    let mut app = App::new_with_options(client_tx, &options);
```

The constructor now sets `ExplicitResume`, so the separate `if options.resume_session { ... }` block is deleted.

- [ ] **Step 14: Run to verify pass**

Run: `just dev cargo test -p yach-ui startup_notices -- --test-threads=1`

Expected: PASS.

Run: `just dev cargo test -p yach resolve_theme -- --test-threads=1`

Expected: PASS. The CLI crate rebuilds against the non-`Copy` `RunTuiOptions`; a compile error at `main.rs:3495` means Step 9 missed that literal.

- [ ] **Step 15: Run lint**

Run: `just lint`

Expected: PASS. `resolve_theme`, `apply_document` and `base_error_notice` each stay under cognitive complexity 15 (`clippy.toml`): `apply_document` has one success arm and one fallback arm, and `base_error_notice` is one flat `match`. If one trips the limit, split it; do not allow the lint. No `unwrap_or`/`expect` remains in the new code; the `rsplit` arm in `appearance_from_colorfgbg` is the unreachable empty case and returns `Dark`.

- [ ] **Step 16: Commit**

```bash
jj commit crates/yach-ui/src/theme.rs crates/yach-ui/src/theme/select.rs crates/yach-ui/src/lib.rs crates/yach-ui/src/app.rs crates/yach-cli/src/main.rs crates/yach-cli/Cargo.toml Cargo.lock -m "Resolve themes by name, COLORFGBG, and NO_COLOR

Project theme files no longer apply; startup reports the spec notice when one exists."
```

### Task 5: Tool row treatment

**Files:**
- Modify: `crates/yach-ui/src/theme/presets.rs` (the `dark()` preset's `tool_success_background` and `tool_output`)
- Modify: `crates/yach-ui/src/transcript.rs:1024-1263` (`render_entry_lines`, `render_tool_surface`, `tool_background`)
- Test: `crates/yach-ui/src/transcript.rs` tests module (starts at line 1384)
- Test: `crates/yach-ui/src/theme.rs` tests module (the Task 3 regression that pins the dark tool defaults)

This task starts from the post-Task-3 shape. Task 3 made `ThemeColors` private and replaced every `theme.colors.X` read with a helper, so the functions below are written against that shape: `tool_surface(state)`, `tool_rule(state)`, `tool_output(state)`, `tool_title()`, `success()`, `warning()`, `error()`, `harness()`, `dim()`, `muted()`, `text()`, `accent()`, `user_message()`, `diff(kind)`, and `mode()`. Do not reintroduce a `theme.colors` read.

**Interfaces:**
- Consumes: `Theme::dark()`, `Theme::system()`, `Theme::monochrome()`, `Theme::mode() -> ThemeMode`, `ThemeMode::{Painted, System, Mono}`, `ToolState::{Pending, Success, Error, Harness}`, `DiffKind`, and the helpers above (Tasks 1 and 3). `tool_surface` returns the pending background for `Pending` and `Success` and the error background for `Error` and `Harness`; `tool_rule` colors `Pending` from `warning`, `Success` from `dim`, `Error` from `error`, `Harness` from `harness`; `tool_output(state)` uses the `toolOutput` foreground on that state's surface background.
- Produces: no new public type. `render_entry_lines` draws one `┃` (U+2503) rule column on every row of a tool panel, including the padding rows, and drops the dim `│ ` continuation rail.

- [ ] **Step 1: Write the failing render tests**

Add these tests to the `transcript.rs` tests module. They render through `render_lines_with_theme`, which returns the `Line`s before the viewport paragraph paints them, so column 0 of each panel row is the first span and the assertion does not depend on `TestBackend` cell width. The narrow-width test goes through `TestBackend` because overflow is a buffer property.

```rust
/// Column 0 of a tool panel is the rule; everything after it is the surface.
fn panel_rows(lines: &[ratatui::text::Line<'_>]) -> Vec<&ratatui::text::Line<'_>> {
    lines
        .iter()
        .filter(|line| {
            line.spans
                .first()
                .is_some_and(|span| span.content.starts_with('┃'))
        })
        .collect()
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn tool_panel_draws_one_rule_on_every_row_including_padding() {
    let mut transcript = Transcript::new();
    transcript.append_tool_call(Some("call-1"), "bash", Some("cargo test"));
    transcript.append_tool_call_output("call-1", "line one\nline two\n");

    let lines = render_lines_with_theme(transcript.entries(), 40, &Theme::dark());
    let panel = panel_rows(&lines);

    // tool_vertical_padding is 1, so the panel is padding, content, tail, padding.
    assert!(panel.len() >= 4, "panel rows: {panel:?}");
    assert!(panel.iter().all(|line| line.spans[0].content == "┃"));
    let rendered = panel
        .iter()
        .map(|line| line_text(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains('│'),
        "the continuation rail must be gone, got: {rendered}"
    );
}

#[test]
fn tool_rule_color_follows_the_entry_state() {
    let theme = Theme::dark();
    let cases = [
        (
            EntryKind::ToolCall {
                id: None,
                name: String::from("read"),
            },
            theme.tool_rule(crate::theme::ToolState::Pending).fg,
        ),
        (
            EntryKind::ToolResult {
                id: None,
                name: String::from("read"),
                is_error: false,
                outcome_kind: None,
            },
            theme.tool_rule(crate::theme::ToolState::Success).fg,
        ),
        (
            EntryKind::ToolResult {
                id: None,
                name: String::from("read"),
                is_error: true,
                outcome_kind: None,
            },
            theme.tool_rule(crate::theme::ToolState::Error).fg,
        ),
        (
            EntryKind::ToolResult {
                id: None,
                name: String::from("read"),
                is_error: true,
                outcome_kind: Some(HarnessOutcomeKind::Denied),
            },
            theme.tool_rule(crate::theme::ToolState::Harness).fg,
        ),
    ];

    for (kind, expected) in cases {
        let mut transcript = Transcript::new();
        transcript.entries.push(TranscriptEntry::new(String::new(), kind));
        let lines = render_lines_with_theme(transcript.entries(), 40, &theme);
        let rule = &panel_rows(&lines)[0].spans[0];
        assert_eq!(rule.style.fg, expected);
    }
}

#[test]
fn tool_state_is_carried_by_glyph_and_label() {
    let theme = Theme::dark();
    let pending = render_lines_with_theme(
        &[TranscriptEntry::new(
            String::from("src/lib.rs"),
            EntryKind::ToolCall {
                id: None,
                name: String::from("read"),
            },
        )],
        60,
        &theme,
    );
    assert!(line_text(&panel_rows(&pending)[1]).contains("⚙ read"));

    let mut success = Transcript::new();
    success.append_tool_result(None, "read", "completed: 1 line", false);
    assert!(line_text(&panel_rows(&render_lines_with_theme(
        success.entries(),
        60,
        &theme
    ))[1])
    .contains("✓ read"));

    let mut failed = Transcript::new();
    failed.append_tool_result(None, "read", "missing", true);
    assert!(line_text(&panel_rows(&render_lines_with_theme(
        failed.entries(),
        60,
        &theme
    ))[1])
    .contains("✗ read"));

    let mut denied = Transcript::new();
    denied.append_tool_result_with_kind(
        None,
        "read",
        "blocked",
        true,
        Some(HarnessOutcomeKind::Denied),
    );
    let denied_text = line_text(&panel_rows(&render_lines_with_theme(
        denied.entries(),
        60,
        &theme,
    ))[1]);
    assert!(
        denied_text.contains("! denied read"),
        "harness outcome must keep its label, got: {denied_text}"
    );
}

#[test]
fn dark_success_surface_matches_pending_and_is_not_green() {
    let theme = Theme::dark();
    let pending = theme.tool_surface(crate::theme::ToolState::Pending);
    let success = theme.tool_surface(crate::theme::ToolState::Success);
    assert_eq!(success.bg, pending.bg);
    assert_eq!(success.bg, Some(ratatui::style::Color::Rgb(40, 40, 50)));
    assert_ne!(success.bg, Some(ratatui::style::Color::Rgb(40, 50, 40)));

    let error = theme.tool_surface(crate::theme::ToolState::Error);
    let harness = theme.tool_surface(crate::theme::ToolState::Harness);
    assert_ne!(error.bg, pending.bg);
    assert_ne!(harness.bg, pending.bg);
    assert_ne!(error.bg, success.bg);
}

#[test]
fn monochrome_tool_rows_carry_no_color() {
    let theme = Theme::monochrome();
    let mut transcript = Transcript::new();
    transcript.append_user_message("ask");
    transcript.append_tool_call(Some("call-1"), "read", Some("src/lib.rs"));
    transcript.append_tool_result(None, "read", "completed", false);
    transcript.append_tool_result(None, "read", "missing", true);
    transcript.append_tool_result_with_kind(
        None,
        "read",
        "blocked",
        true,
        Some(HarnessOutcomeKind::Denied),
    );
    transcript.entries.push(TranscriptEntry::new(
        String::from("turn stopped"),
        EntryKind::HarnessOutcome {
            kind: HarnessOutcomeKind::Limit,
        },
    ));

    let lines = render_lines_with_theme(transcript.entries(), 60, &theme);
    for line in &lines {
        for span in &line.spans {
            // `None` renders as the terminal default, the same as `Reset`.
            assert!(matches!(span.style.fg, None | Some(ratatui::style::Color::Reset)));
            assert!(matches!(span.style.bg, None | Some(ratatui::style::Color::Reset)));
        }
    }
}

#[test]
fn narrow_tool_row_wraps_without_overflowing() {
    let mut transcript = Transcript::new();
    transcript.append_tool_result(
        None,
        "read_text_file",
        "completed: a very long summary that must wrap inside twenty columns",
        false,
    );

    let backend = ratatui::backend::TestBackend::new(20, 12);
    let mut terminal = match ratatui::Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(infallible) => match infallible {},
    };
    let area = ratatui::layout::Rect::new(0, 0, 20, 12);
    let lines = render_lines_with_theme(transcript.entries(), area.width, &Theme::dark());
    let drawn = terminal.draw(|frame| {
        frame.render_widget(ratatui::widgets::Paragraph::new(lines), area);
    });
    assert!(drawn.is_ok());

    let buffer = terminal.backend().buffer();
    for row in 0..area.height {
        for column in 0..area.width {
            let cell = &buffer[(column, row)];
            assert!(
                cell.symbol().chars().count() <= 2,
                "cell ({column},{row}) overflowed: {:?}",
                cell.symbol()
            );
        }
    }
}
```

`Terminal::new` over `TestBackend` is infallible in ratatui 0.30; the `match infallible {}` and `assert!(…is_ok())` shapes are the ones `app.rs:5163-5169` already uses, because the workspace denies `expect_used` in tests too.

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-ui transcript::tests::tool_panel_draws_one_rule_on_every_row_including_padding -- --exact`

Expected: FAIL. The panel rows still start with the padding space, so `panel_rows` is empty and `panel.len() >= 4` fails.

- [ ] **Step 3: Update the dark preset**

In `crates/yach-ui/src/theme/presets.rs`, change the two dark defaults. Every other dark token stays exactly as it is.

```rust
tool_success_background: Color::Rgb(40, 40, 50),
tool_output: Color::Rgb(192, 192, 192),
```

`tool_success_background` now equals `tool_pending_background`. A theme file that sets either token explicitly still wins, because `ThemeDocument::apply` overrides after the preset.

- [ ] **Step 4: Update the tests that pinned the old dark tool defaults**

Three earlier tests encode the green success surface and gray-128 output on purpose; Task 5 changes both.

In the `theme.rs` tests module, `dark_preset_keeps_todays_pi_dark_values` (Task 1): change the two token assertions to

```rust
        assert_eq!(theme.colors.tool_success_background, Color::Rgb(40, 40, 50));
        assert_eq!(theme.colors.tool_output, Color::Rgb(192, 192, 192));
```

and replace the trailing block that begins `// The two values Task 5 will change are still the old ones here.` with

```rust
        assert_eq!(
            theme.colors.tool_success_background,
            theme.colors.tool_pending_background
        );
```

In `light_and_system_share_pending_and_success_surfaces` (Task 1), change `for theme in [Theme::light()]` to `for theme in [Theme::light(), Theme::dark()]` and delete the trailing `// dark is deliberately NOT asserted here` comment and its `assert_ne!`. The `system` loop is unchanged.

In the `transcript.rs` tests module, `adjacent_tool_rows_use_separate_success_surfaces` asserts every span of rows `4..=6` and `8..=10` carries the success background (after Task 3, `background` is `Theme::dark().tool_surface(ToolState::Success).bg`, an `Option<Color>`). Column 0 is now the rule, which sets only a foreground. Skip it:

```rust
        assert!(lines[4..=6].iter().all(|line| {
            line.spans[1..]
                .iter()
                .all(|span| span.style.bg == background)
        }));
```

(same for `8..=10`). The line count (11) and the blank separators at `3` and `7` are unchanged.

Delete `dark_transcript_buffer_is_stable_across_helper_migration` and `crates/yach-ui/tests/fixtures/dark-transcript-buffer.txt` (Task 3). That pin existed to prove the helper migration changed no `dark` cell; this task changes cells by design, and regenerating it would pin incidental layout instead of behavior. The tests in Step 1 replace it.

- [ ] **Step 5: Draw the rule and drop the continuation rail**

In `render_entry_lines` (`transcript.rs:1024`), the tool arms build a `continuation` span of `"│ "` styled with `dim`. Delete that span from all three tool arms (`ToolCall`, `ToolResult { outcome_kind: Some(_) }`, `ToolResult { .. }`) and from the `prefix_width` accounting. The rule replaces it, so an expanded row has one rail, not two.

Add the state mapping next to `tool_background`:

```rust
fn tool_state(entry: &TranscriptEntry) -> crate::theme::ToolState {
    match entry.kind {
        EntryKind::ToolResult {
            outcome_kind: Some(_),
            ..
        } => crate::theme::ToolState::Harness,
        EntryKind::ToolResult { is_error: true, .. } => crate::theme::ToolState::Error,
        EntryKind::ToolResult { .. } => crate::theme::ToolState::Success,
        _ => crate::theme::ToolState::Pending,
    }
}
```

`ToolCall` falls through to `Pending`. A harness-authored tool result (`outcome_kind: Some(_)`) is `Harness` even when `is_error` is also set, which is what `tool_background` already did by matching that arm first (`transcript.rs:1256-1260`). `EntryKind::HarnessOutcome` is not a tool entry and never reaches this function's tool path.

Subtract the rule column wherever tool output is wrapped. That is the one `available_width` calculation (`transcript.rs:1118-1121`), shared by the content wrap and the stream-tail wrap:

```rust
const TOOL_RULE_WIDTH: usize = 1;

let available_width = usize::from(width)
    .saturating_sub(horizontal_padding.saturating_mul(2))
    .saturating_sub(prefix_width)
    .saturating_sub(usize::from(is_tool) * TOOL_RULE_WIDTH)
    .max(1);
```

`render_tool_surface` (`transcript.rs:1209`) pads to `width` and does not wrap, so it keeps its arithmetic. It gains the rule as its first span on every row, including both padding loops:

```rust
fn render_tool_surface(
    lines: Vec<Line<'static>>,
    width: u16,
    state: crate::theme::ToolState,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let surface = theme.tool_surface(state);
    let rule = Span::styled("┃", theme.tool_rule(state));
    let horizontal_padding = usize::from(theme.spacing.tool_horizontal_padding)
        .min(width.saturating_sub(1) / 2);
    let inner_width = width
        .saturating_sub(TOOL_RULE_WIDTH)
        .saturating_sub(horizontal_padding.saturating_mul(2));
    let background_style = surface;
    let vertical_padding = usize::from(theme.spacing.tool_vertical_padding);
    let padding_row = |width: usize| {
        Line::from(vec![
            rule.clone(),
            Span::styled(" ".repeat(width.saturating_sub(TOOL_RULE_WIDTH)), background_style),
        ])
    };
    let mut rendered = Vec::with_capacity(lines.len() + vertical_padding.saturating_mul(2));
    rendered.extend(std::iter::repeat_with(|| padding_row(width)).take(vertical_padding));
    for mut line in lines {
        let line_width = line.width().min(inner_width);
        for span in &mut line.spans {
            span.style.bg = surface.bg;
        }
        let mut spans = Vec::with_capacity(line.spans.len() + 3);
        spans.push(rule.clone());
        spans.push(Span::styled(" ".repeat(horizontal_padding), background_style));
        spans.extend(line.spans);
        spans.push(Span::styled(
            " ".repeat(
                inner_width
                    .saturating_sub(line_width)
                    .saturating_add(horizontal_padding),
            ),
            background_style,
        ));
        rendered.push(Line::from(spans));
    }
    rendered.extend(std::iter::repeat_with(|| padding_row(width)).take(vertical_padding));
    rendered
}
```

`System` and `Mono` paint no panel background because `tool_surface` returns a `Reset` background in those modes (Task 1). The rule is still drawn, and `tool_rule` supplies its color, so a no-background theme keeps its separator.

The call site (`transcript.rs:1146`) becomes:

```rust
if is_tool {
    render_tool_surface(result, width, tool_state(entry), theme)
} else {
    result
}
```

Delete `tool_background`. Its only caller was that line, and the state mapping replaces it.

- [ ] **Step 6: Run to verify pass**

Run: `just dev cargo test -p yach-ui transcript::tests`

Expected: PASS. `render_lines_preserves_representative_entries` counts lines, which the rule column does not change. `failed_tool_results_render_failure_marker` still finds `✗ Read`.

- [ ] **Step 7: Lint and commit**

Run: `just lint`

Expected: no new warnings. `tool_state` and `render_tool_surface` stay under the cognitive-complexity limit of 15 because each is one match or one loop.

```bash
jj commit crates/yach-ui/src/theme/presets.rs crates/yach-ui/src/transcript.rs crates/yach-ui/src/theme.rs crates/yach-ui/tests/fixtures -m "Draw a state-colored rule on tool rows and neutralize the success surface"
```

### Task 6: Extension theme contributions

**Files:**
- Modify: `crates/yach-backend/src/extension.rs:68-73` (`ExtensionContributions`), `141-176` (`ExtensionManifestError`), `1662-1725` (raw contribution structs), `1802-1922` (`parse_extension_manifest`), `2697-2760` (`ExtensionManifestIndex`), `3001-3031` (the static-context parser this copies), `3154-3170` (the validators it reuses)
- Modify: `crates/yach-cli/src/main.rs:3757-3772` (`run_tui_command`), `4383-4396` (`extension_package_roots_from_env_and_install_records`), `5149-5155` (`loaded_extension_install_records`)
- Test: `crates/yach-backend/src/extension.rs` tests module (the `extension_manifest_accepts_packaged_background_static_context_contribution` test at line 4219 is the template)
- Test: `crates/yach-cli/src/main.rs` tests module (starts at line 5853)

**Interfaces:**
- Consumes: `yach_ui::ExtensionThemes` (`fn theme_json(&self, qualified_name: &str) -> Option<String>`), `resolve_theme(env: &ThemeEnv, extensions: &dyn ExtensionThemes) -> Result<(Theme, Vec<String>), String>`, `BaseError::ExtensionBaseNotBuiltin` (Task 4). `parse_extension_manifest`, `is_valid_static_context_id`, `is_valid_static_context_path`, `ExtensionManifestIndex::from_package_roots`, `loaded_extension_install_records`, `extension_package_roots_from_env_and_install_records`.
- Produces:

```rust
pub struct ExtensionThemeContribution {
    pub id: String,
    pub title: String,
    pub source: ExtensionThemeSource,
    pub max_bytes: u64,
}
pub enum ExtensionThemeSource { ExtensionFile { path: String } }
pub struct ExtensionThemeFile {
    pub extension_id: String,
    pub theme_id: String,
    pub package_root: PathBuf,
    pub relative_path: String,
    pub max_bytes: u64,
}
impl ExtensionManifestIndex {
    pub fn user_theme_files(&self) -> Vec<ExtensionThemeFile>;
}
```

`ExtensionContributions` gains `pub themes: Vec<ExtensionThemeContribution>`. The CLI produces `InstalledExtensionThemes`, which implements `yach_ui::ExtensionThemes` and reads install records only when `theme_json` is called.

- [ ] **Step 1: Write the failing manifest tests**

Add these to the `extension.rs` tests module. `parse_valid_manifest`, `expect_equal`, and `TestPackageRoot` already exist (`extension.rs:3300`, `3547`, `3376`).

```rust
fn theme_manifest(id: &str, path: &str, max_bytes: u64) -> serde_json::Value {
    serde_json::json!({
        "schema": "yach.extension.v1",
        "id": "example.themes",
        "version": "0.1.0",
        "main": { "command": "node", "args": ["./extension.js"] },
        "contributes": {
            "themes": [{
                "id": id,
                "title": "Night",
                "source": { "type": "extension_file", "path": path },
                "max_bytes": max_bytes
            }]
        }
    })
}

#[test]
fn extension_manifest_accepts_a_theme_contribution() -> Result<(), String> {
    let manifest = parse_valid_manifest(theme_manifest("night", "themes/night.json", 16384))?;
    expect_equal(
        &manifest.contributes.themes,
        &vec![ExtensionThemeContribution {
            id: String::from("night"),
            title: String::from("Night"),
            source: ExtensionThemeSource::ExtensionFile {
                path: String::from("themes/night.json"),
            },
            max_bytes: 16384,
        }],
    )
}

#[test]
fn extension_manifest_rejects_a_bad_theme_id() {
    let error = parse_extension_manifest(theme_manifest("Night", "themes/night.json", 16384));
    assert_eq!(
        error,
        Err(ExtensionManifestError::InvalidThemeId {
            id: String::from("Night"),
        })
    );
}

#[test]
fn extension_manifest_rejects_an_escaping_theme_path() {
    let error = parse_extension_manifest(theme_manifest("night", "../night.json", 16384));
    assert_eq!(
        error,
        Err(ExtensionManifestError::InvalidThemePath {
            path: String::from("../night.json"),
        })
    );
}

#[test]
fn extension_manifest_rejects_a_zero_theme_cap() {
    let error = parse_extension_manifest(theme_manifest("night", "themes/night.json", 0));
    assert_eq!(error, Err(ExtensionManifestError::ThemeMaxBytesTooSmall));
}

#[test]
fn extension_manifest_rejects_a_reserved_theme_id() {
    for id in ["dark", "light", "system", "auto"] {
        let error = parse_extension_manifest(theme_manifest(id, "themes/night.json", 16384));
        assert_eq!(
            error,
            Err(ExtensionManifestError::ReservedThemeId {
                id: String::from(id),
            })
        );
    }
}

#[test]
fn user_theme_files_skip_project_scope() -> Result<(), String> {
    let package = TestPackageRoot::new("user_theme_files")?;
    package.write_json_file(
        "yach.extension.json",
        &theme_manifest("night", "themes/night.json", 16384),
    )?;

    let project = ExtensionManifestIndex::from_package_roots([package
        .package_root(ExtensionInstallScope::Project)])
    .map_err(|error| format!("{error:?}"))?;
    expect_equal(&project.user_theme_files(), &Vec::new())?;

    let user = ExtensionManifestIndex::from_package_roots([package
        .package_root(ExtensionInstallScope::User)])
    .map_err(|error| format!("{error:?}"))?;
    expect_equal(
        &user.user_theme_files(),
        &vec![ExtensionThemeFile {
            extension_id: String::from("example.themes"),
            theme_id: String::from("night"),
            package_root: package.path.clone(),
            relative_path: String::from("themes/night.json"),
            max_bytes: 16384,
        }],
    )
}
```

- [ ] **Step 2: Run to verify failure**

Run: `just dev cargo test -p yach-backend extension::tests::extension_manifest_accepts_a_theme_contribution -- --exact`

Expected: FAIL. `contributes.themes` is an unknown field, so the manifest parses as `Malformed` rather than producing a theme contribution.

- [ ] **Step 3: Parse the contribution**

Add the error variants next to the static-context ones (`extension.rs:161-163`):

```rust
InvalidThemeId { id: String },
ReservedThemeId { id: String },
InvalidThemePath { path: String },
ThemeMaxBytesTooSmall,
```

Add the raw structs next to `RawExtensionStaticContextContribution` (`extension.rs:1706-1724`):

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionThemeContribution {
    id: String,
    title: String,
    source: RawExtensionThemeSource,
    max_bytes: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum RawExtensionThemeSource {
    #[serde(rename = "extension_file")]
    ExtensionFile { path: String },
}
```

`RawExtensionContributions` (`extension.rs:1662`) gains `#[serde(default)] themes: Vec<RawExtensionThemeContribution>`.

Add the parsed types next to `ExtensionStaticContextContribution` (`extension.rs:128-138`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionThemeContribution {
    pub id: String,
    pub title: String,
    pub source: ExtensionThemeSource,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionThemeSource {
    ExtensionFile { path: String },
}
```

`ExtensionContributions` (`extension.rs:68`) gains `pub themes: Vec<ExtensionThemeContribution>`. Every struct literal of `ExtensionContributions` needs the new field; `cargo check` lists them, and each one is `themes: Vec::new()`.

Parse beside `parse_static_context_contribution` (`extension.rs:3001`). The id and path validators are the static-context ones (`extension.rs:3154-3170`); reuse them, do not copy them.

```rust
const RESERVED_THEME_IDS: [&str; 4] = ["dark", "light", "system", "auto"];

fn parse_theme_contribution(
    contribution: RawExtensionThemeContribution,
) -> Result<ExtensionThemeContribution, ExtensionManifestError> {
    if !is_valid_static_context_id(&contribution.id) {
        return Err(ExtensionManifestError::InvalidThemeId {
            id: contribution.id,
        });
    }
    if RESERVED_THEME_IDS.contains(&contribution.id.as_str()) {
        return Err(ExtensionManifestError::ReservedThemeId {
            id: contribution.id,
        });
    }
    if contribution.max_bytes == 0 {
        return Err(ExtensionManifestError::ThemeMaxBytesTooSmall);
    }
    let source = match contribution.source {
        RawExtensionThemeSource::ExtensionFile { path } => {
            if !is_valid_static_context_path(&path) {
                return Err(ExtensionManifestError::InvalidThemePath { path });
            }
            ExtensionThemeSource::ExtensionFile { path }
        }
    };
    Ok(ExtensionThemeContribution {
        id: contribution.id,
        title: contribution.title,
        source,
        max_bytes: contribution.max_bytes,
    })
}
```

The four presets are the whole reserved set, as the spec states (`docs/project/specs/2026-10-02-theming-engine-design.md:541-542`). A theme id that matches a bundled palette name needs no rejection: the qualified name `<extension-id>:<palette>` never equals a built-in name, and `resolve_base` checks presets and palettes by exact name before it asks extensions, so `catppuccin-mocha` always means the bundled palette.

In `parse_extension_manifest` (`extension.rs:1888`), collect themes the same way as static context:

```rust
let themes = raw
    .contributes
    .themes
    .into_iter()
    .map(parse_theme_contribution)
    .collect::<Result<Vec<_>, _>>()?;
```

and pass `themes` into the `ExtensionContributions` literal (`extension.rs:1913`).

- [ ] **Step 4: Expose user-scope theme files**

Add the record type next to `ExtensionStaticContextFile` and the index method next to `static_context_files` (`extension.rs:2721`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionThemeFile {
    pub extension_id: String,
    pub theme_id: String,
    pub package_root: PathBuf,
    pub relative_path: String,
    pub max_bytes: u64,
}

impl ExtensionManifestIndex {
    pub fn user_theme_files(&self) -> Vec<ExtensionThemeFile> {
        self.records
            .iter()
            .filter(|record| record.scope == ExtensionInstallScope::User)
            .flat_map(|record| {
                record
                    .manifest
                    .contributes
                    .themes
                    .iter()
                    .map(|contribution| {
                        let ExtensionThemeSource::ExtensionFile { path } = &contribution.source;
                        ExtensionThemeFile {
                            extension_id: record.manifest.id.0.clone(),
                            theme_id: contribution.id.clone(),
                            package_root: record.package_root.clone(),
                            relative_path: path.clone(),
                            max_bytes: contribution.max_bytes,
                        }
                    })
            })
            .collect()
    }
}
```

`from_package_roots` already rejects a duplicate extension id through `ExtensionCatalog::from_manifests` (`extension.rs:2712`), so two user-scope extensions cannot offer the same `<extension-id>:<theme-id>`.

- [ ] **Step 5: Run to verify pass**

Run: `just dev cargo test -p yach-backend -- --exact extension::tests::extension_manifest_accepts_a_theme_contribution extension::tests::user_theme_files_skip_project_scope`

Expected: PASS.

- [ ] **Step 6: Write the failing resolver tests**

Add these to the `yach` tests module (`main.rs:5853`), next to the Task 4 `resolve_theme` tests. Task 4 already imported `ThemeEnv` and `resolve_theme`; these tests need no new `super` import (they use a test double, not `InstalledExtensionThemes`, and an unused import fails `just lint`). Reuse Task 4's `theme_env(yach_theme, no_color, colorfgbg, home)` and `theme_file(dir, body)` helpers; defining a second `theme_env` in the same module does not compile. `expect_ok`, `expect_equal`, `expect_true` and `TestTempDir` are the helpers at `main.rs:7989-8050`.

The spec selects an extension theme only through a file's `base` (`docs/project/specs/2026-10-02-theming-engine-design.md:514-517`), so each test writes `~/.yach/theme.json` naming it; `YACH_THEME=<ext>:<id>` is a path like any other non-built-in value.

```rust
    struct StaticThemes {
        themes: std::collections::BTreeMap<String, String>,
    }

    impl yach_ui::ExtensionThemes for StaticThemes {
        fn theme_json(&self, qualified_name: &str) -> Option<String> {
            self.themes.get(qualified_name).cloned()
        }
    }

    #[test]
    fn extension_theme_named_as_base_applies_over_its_own_base() -> Result<(), String> {
        let root = TestTempDir::new("ext-theme-applies")?;
        let home = root.path().join("home");
        theme_file(&home, r#"{"base":"example.themes:night"}"#)?;
        let mut themes = std::collections::BTreeMap::new();
        themes.insert(
            String::from("example.themes:night"),
            String::from(r##"{"base":"light","colors":{"accent":"#112233"}}"##),
        );
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(None, None, None, Some(&home)),
            &StaticThemes { themes },
        ))?;
        expect_true(notices.is_empty(), "a resolved extension theme is silent")?;
        expect_equal(
            &theme.accent().fg,
            &Some(ratatui::style::Color::Rgb(0x11, 0x22, 0x33)),
        )?;
        expect_equal(&theme.text().fg, &Theme::light().text().fg)
    }

    #[test]
    fn disabled_extension_theme_falls_back_to_auto() -> Result<(), String> {
        let root = TestTempDir::new("ext-theme-disabled")?;
        let home = root.path().join("home");
        theme_file(&home, r#"{"base":"example.themes:night"}"#)?;
        // A disabled or removed extension contributes nothing.
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(None, None, Some("0;15"), Some(&home)),
            &StaticThemes {
                themes: std::collections::BTreeMap::new(),
            },
        ))?;
        expect_equal(&theme, &Theme::light())?;
        expect_equal(&notices.len(), &1)?;
        expect_true(
            notices.iter().any(|line| line.contains("example.themes:night")),
            "the missing theme is named in the diagnostic",
        )
    }

    #[test]
    fn extension_theme_cannot_base_on_another_extension_theme() -> Result<(), String> {
        let root = TestTempDir::new("ext-theme-chain")?;
        let home = root.path().join("home");
        theme_file(&home, r#"{"base":"example.themes:night"}"#)?;
        let mut themes = std::collections::BTreeMap::new();
        themes.insert(
            String::from("example.themes:night"),
            String::from(r#"{"base":"other.pack:day"}"#),
        );
        let (theme, notices) = expect_ok(resolve_theme(
            &theme_env(None, None, None, Some(&home)),
            &StaticThemes { themes },
        ))?;
        expect_equal(&theme, &Theme::dark())?;
        expect_true(
            notices.iter().any(|line| line.contains("other.pack:day")),
            "the rejected base is named in the diagnostic",
        )
    }
```

`disabled_extension_theme_falls_back_to_auto` fails an implementation that returns `Err` for an unresolved extension base (the spec's fallback is the file over `auto` plus one diagnostic). `extension_theme_cannot_base_on_another_extension_theme` fails one that follows the chain or drops the rejected name from the diagnostic; it depends on Task 4's `base_error_notice`.

- [ ] **Step 7: Load extension themes lazily**

`InstalledExtensionThemes` holds the loaded index behind a `OnceLock`, so constructing it in `run_tui_command` reads nothing. The first `theme_json` call reads the install records and builds the index.

```rust
struct InstalledExtensionThemes {
    index: std::sync::OnceLock<Option<yach_backend::ExtensionManifestIndex>>,
}

impl InstalledExtensionThemes {
    fn new() -> Self {
        Self {
            index: std::sync::OnceLock::new(),
        }
    }

    fn index(&self) -> Option<&yach_backend::ExtensionManifestIndex> {
        self.index
            .get_or_init(|| {
                let records = loaded_extension_install_records().ok()?;
                // Enabled records only. `extension_package_roots_from_env_and_install_records`
                // also re-adds disabled bundled roots (main.rs:4387-4397), which would let a
                // disabled bundled extension's theme still resolve.
                let roots = extension_package_roots_from_install_records(&records);
                yach_backend::ExtensionManifestIndex::from_package_roots(roots).ok()
            })
            .as_ref()
    }
}

impl yach_ui::ExtensionThemes for InstalledExtensionThemes {
    fn theme_json(&self, qualified_name: &str) -> Option<String> {
        let (extension_id, theme_id) = qualified_name.split_once(':')?;
        let file = self.index()?.user_theme_files().into_iter().find(|file| {
            file.extension_id == extension_id && file.theme_id == theme_id
        })?;
        read_extension_theme_file(&file)
    }
}

fn read_extension_theme_file(file: &yach_backend::ExtensionThemeFile) -> Option<String> {
    let root = std::fs::canonicalize(&file.package_root).ok()?;
    let candidate = root.join(&file.relative_path);
    let canonical = std::fs::canonicalize(&candidate).ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    let opened = std::fs::File::open(&canonical).ok()?;
    let mut bytes = Vec::new();
    std::io::Read::take(opened, file.max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > file.max_bytes {
        return None;
    }
    String::from_utf8(bytes).ok()
}
```

`user_theme_files` already drops project scope, and `extension_package_roots_from_install_records` (`main.rs:4407`) already drops disabled records, so a disabled or project-scope extension never reaches `theme_json`.

In `run_tui_command`, the Task 4 block passes `&()` to `resolve_theme`. Pass the installed set instead; the legacy project-file notice from Task 4 stays as it is:

```rust
    let extensions = InstalledExtensionThemes::new();
    let (theme, mut startup_notices) = match resolve_theme(&ThemeEnv::from_process(), &extensions) {
        Ok(resolved) => resolved,
        Err(error) => {
            return CommandResult::UsageError { message: error };
        }
    };
```

`resolve_theme` returns `Err` only when an explicit or user theme file exists but cannot be read or is invalid JSON (Task 4). An unresolved, disabled, or chained extension base comes back as `Ok` with the file over `auto` and one diagnostic, which the tests above pin.

The theme is resolved before the UI exists. `run_tui_with_native_backend` forwards `ui_options` to `run_tui_with_trace_and_options` (`main.rs:4203`), which constructs the app with `App::new_with_options` (Task 4) and only then enables raw mode (`app.rs:4237`). Nothing paints before that, so an extension theme cannot flash the default and repaint; the spec's first-paint `[INFERENCE]` does not arise.

- [ ] **Step 8: Run to verify pass**

Run: `just dev cargo test -p yach -- --exact tests::extension_theme_named_as_base_applies_over_its_own_base tests::disabled_extension_theme_falls_back_to_auto tests::extension_theme_cannot_base_on_another_extension_theme`

Expected: PASS.

- [ ] **Step 9: Lint and commit**

Run: `just lint`

Expected: no new warnings. `parse_theme_contribution` is one validation sequence and stays under the cognitive-complexity limit of 15.

```bash
jj commit crates/yach-backend/src/extension.rs crates/yach-cli/src/main.rs -m "Let user-scope extensions contribute themes"
```

### Task 7: Visual tapes and docs

**Files:**

- Create: `tests/visual/light.tape`
- Create: `tests/visual/nocolor.tape`
- Create: `tests/visual/check-nocolor.sh`
- Modify: `tests/visual/session.tape:17`
- Modify: `tests/visual/narrow.tape:17`
- Modify: `README.md:167` (environment row) and `README.md:237-271` (theme section, replaced)
- Modify: `docs/extensions.md` (append a Themes section after line 183)
- Modify: `docs/project/specs/2026-08-19-wave3-tui-visual-design.md:1-5` (supersession note at the top)

**Interfaces:**

- Consumes: Task 4 `resolve_theme` (`NO_COLOR` non-empty overrides every source; `COLORFGBG=0;15` makes `auto` resolve to `light`); Task 5 tool row (neutral panel, `┃` rule, failure tint only, `toolOutput` = `Rgb(192,192,192)` on `dark`); Task 6 extension theme qualified name `<extension-id>:<theme-id>`.
- Produces: no Rust API. Visual artifacts under `target/tui-visual/`: `light.png`, `nocolor.png`, `session.png`, `narrow.png`, and `nocolor.typescript`.

This task starts only after Tasks 1-6 have landed and `tui-markdown` has merged (contract). Do not change widget code here. A tape that fails because the resumed fixture never paints is a Task 3/5 defect; stop and report it rather than weakening the wait.

- [ ] **Step 1: Confirm a light VHS theme, then write the tapes**

Run, from the repo root, inside the dev shell (`vhs` is a devenv package, `devenv.nix:19`):

```bash
just dev vhs themes
```

Expected: one theme name per line. Pick the first of these that appears, in this order: `Catppuccin Latte`, `Builtin Light`, `Alabaster`. Record the chosen name; the rest of this step uses `Catppuccin Latte` and must be replaced with the chosen name if it was not listed. Do not invent a name that was not printed.

`Set Theme` is a VHS setting (`vhs(1)` SETTINGS), not a yach setting. It paints the terminal page behind the inline viewport. Yach still resolves its own theme from `COLORFGBG` because the launch uses `env -i` and does not set `YACH_THEME` (`tests/visual/session.tape:14`).

Create `tests/visual/light.tape`. Same geometry and fixture launch as `session.tape`, plus the light terminal environment and the light VHS theme:

```tape
Output "target/tui-visual/light.gif"

Set Shell "bash"
Set Width 1200
Set Height 720
Set FontSize 16
Set TypingSpeed 1ms
Set Framerate 30
Set CursorBlink false
Set Padding 0
Set Margin 0
Set Theme "Catppuccin Latte"

Hide
Type `cd "$YACH_VISUAL_ROOT/workspace" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color COLORTERM=truecolor COLORFGBG=0\;15 YACH_SESSION_DIR="$YACH_SESSION_DIR" "$YACH_VISUAL_BINARY" tui --backend fixture --resume`
Enter
Wait+Screen /Updated the greeting/
Wait+Screen /no model \(run \/connect\)/
Show
Sleep 500ms
Screenshot "target/tui-visual/light.png"
Sleep 1s
Hide

Ctrl+C
Sleep 500ms
```

`COLORFGBG=0\;15` is the shell-quoted form of `COLORFGBG=0;15`. The last field is `15`, which is not below 8, so `auto` resolves to `light`. The wait is the full current status string (`status_bar.rs:266`), not the stale `/no model/` substring.

Create `tests/visual/nocolor.tape`. Same session, `NO_COLOR=1`, no `COLORFGBG` (monochrome overrides it anyway). VHS theme stays unset: a themed page background would hide whether the screenshot is actually unpainted.

```tape
Output "target/tui-visual/nocolor.gif"

Set Shell "bash"
Set Width 1200
Set Height 720
Set FontSize 16
Set TypingSpeed 1ms
Set Framerate 30
Set CursorBlink false
Set Padding 0
Set Margin 0

Hide
Type `cd "$YACH_VISUAL_ROOT/workspace" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color COLORTERM=truecolor NO_COLOR=1 YACH_SESSION_DIR="$YACH_SESSION_DIR" "$YACH_VISUAL_BINARY" tui --backend fixture --resume`
Enter
Wait+Screen /Updated the greeting/
Wait+Screen /no model \(run \/connect\)/
Show
Sleep 500ms
Screenshot "target/tui-visual/nocolor.png"
Sleep 1s
Hide

Ctrl+C
Sleep 500ms
```

`render.sh:21-23` runs `session` and `narrow` when given no names, and runs any basename that has a `.tape` (`render.sh:25-32`). No justfile change: `just tui-visual light` and `just tui-visual nocolor` already reach these files (`justfile:38-40`).

- [ ] **Step 2: Render the new tapes**

Run:

```bash
just tui-visual light nocolor
```

Expected: both tapes reach `Wait+Screen`, write `target/tui-visual/light.png` and `target/tui-visual/nocolor.png`, and the script prints `visual artifacts:`. A timeout on `/Updated the greeting/` means the fixture session did not resume; do not proceed to the screenshot review.

`render.sh:12` deletes `target/tui-visual` at the start, so this step's screenshots exist only until the next `just tui-visual`. Review them in this step, before Step 3.

Open `target/tui-visual/light.png` and check, against the light page background:

- The user message "Inspect src/lib.rs" is dark text on a pale surface, not white-on-white and not the dark preset's dark surface.
- Both tool panels (the completed `read_text_file` row and the failed `edit_text_file` row) are pale, not the dark green band from the reported bug. Each has a visible `┃` rule.
- The failed row is distinguishable from the successful row by a tinted error surface and the `✗` glyph, and its text is readable on that surface.
- Assistant prose "Updated the greeting" is dark on the light page, not light-on-light.

Open `target/tui-visual/nocolor.png` and check:

- No painted user or tool background. Rows sit on the terminal page.
- `┃` is still present on every tool row, and `✓` / `✗` / `›` still distinguish the rows. The tool name is bold.
- The failed row is still labeled as a failure in text, not only by color.

A failed legibility check is a Task 1 or Task 5 defect. Record which check failed and stop; do not edit the tape to hide it.

- [ ] **Step 3: Add the raw-byte `NO_COLOR` check**

VHS does not capture the terminal byte stream (`vhs(1)` OUTPUT: `.gif`, `.webm`, `.mp4` only). A ratatui `TestBackend` asserts cell colors, which Task 5 already covers, and never sees the SGR crossterm writes. `script(1)` is installed and logs the child stdout with `-O`, which is the captured stream the spec asks for.

Create `tests/visual/check-nocolor.sh` (mode `0755`):

```bash
#!/usr/bin/env bash
# Captures the fixture TUI under NO_COLOR and fails if the byte stream
# contains a color SGR. VHS cannot emit this stream (gif/webm/mp4 only).
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
visual_dir="$repo_root/tests/visual"
artifact_root="$repo_root/target/tui-visual"
workspace="$artifact_root/workspace"
typescript="$artifact_root/nocolor.typescript"
binary="${CARGO_TARGET_DIR:-$repo_root/target}/debug/yach"

cd "$repo_root"
cargo build -p yach

mkdir -p "$artifact_root/home" "$artifact_root/sessions" "$workspace"
cp "$visual_dir/session.jsonl" "$artifact_root/sessions/session-visual.jsonl"

export HOME="$artifact_root/home"
export YACH_SESSION_DIR="$artifact_root/sessions"

# `script` allocates the tty and logs the child's stdout (`-O` is the
# default). `/quit` is SlashAction::Quit and sets should_quit
# (`app.rs:3436-3439`). The sleep lets the resumed fixture paint before
# the quit; 15s total is the hang bound.
script -q -e -f -c "stty rows 40 cols 120; cd \"$workspace\" && env -i HOME=\"$HOME\" PATH=\"$PATH\" TERM=xterm-256color COLORTERM=truecolor NO_COLOR=1 YACH_SESSION_DIR=\"$YACH_SESSION_DIR\" \"$binary\" tui --backend fixture --resume" "$typescript" <<'KEYS' &
sleep 3
/quit
KEYS
capture_pid=$!

slept=0
while (( slept < 15 )) && kill -0 "$capture_pid" 2>/dev/null; do
  sleep 1
  slept=$((slept + 1))
done
if kill -0 "$capture_pid" 2>/dev/null; then
  kill "$capture_pid" 2>/dev/null || true
  wait "$capture_pid" 2>/dev/null || true
  printf 'nocolor capture hung past 15s\n' >&2
  exit 1
fi
wait "$capture_pid" 2>/dev/null || true

if ! grep -a -q 'Updated the greeting' "$typescript"; then
  printf 'nocolor capture never painted the fixture session\n' >&2
  exit 1
fi

# Color SGR only. Bold (1), dim (2), and reverse (7) are required in
# monochrome and must not fail this check. A parameter matches only at a
# `;` or `m` boundary, so "38" does not match inside "138".
python3 - "$typescript" <<'PY'
import re
import sys

data = open(sys.argv[1], "rb").read()
color = re.compile(
    rb"\x1b\[(?:"
    rb"(?:38|48|3[0-9]|4[0-9]|9[0-9]|10[0-9])"
    rb"(?:;[0-9]+)*m"
    rb"|(?:[0-9]+;)*(?:38|48|3[0-9]|4[0-9]|9[0-9]|10[0-9])"
    rb"(?:;[0-9]+)*m)"
)
found = sorted(set(color.findall(data)))
if found:
    sys.stderr.write("color SGR in NO_COLOR capture:\n")
    for sequence in found:
        sys.stderr.write(sequence.decode("ascii", "backslashreplace") + "\n")
    sys.exit(1)
PY

printf 'nocolor bytes: %s\n' "$typescript"
```

The Python scan is the check. It fails when any SGR parameter is `38` or `48` (truecolor and 256-color), `30-39` or `90-97` (foreground), or `40-49` or `100-107` (background), including when the color parameter is not first (`1;38;2;...`). It does not fail on `1` (bold), `2` (dim), or `7` (reverse), which monochrome helpers emit.

Run:

```bash
just dev bash tests/visual/check-nocolor.sh
```

Expected: prints `nocolor bytes: .../target/tui-visual/nocolor.typescript` and exits 0. If it prints `color SGR in NO_COLOR capture`, the listed sequences are a Task 4 or Task 5 defect (a widget still painting color under monochrome). Do not loosen the pattern.

- [ ] **Step 4: Re-review `session` and `narrow`**

In `tests/visual/session.tape` and `tests/visual/narrow.tape`, replace the stale wait. Both files have this line at line 17:

```tape
Wait+Screen /no model/
```

Replace it, in both files, with:

```tape
Wait+Screen /no model \(run \/connect\)/
```

Leave the launch line, geometry, and screenshots otherwise unchanged. The new tool panel is the thing under review; the wait change only stops the tape matching a substring that an unrelated status could also contain.

Run:

```bash
just tui-visual session narrow
```

Expected: `target/tui-visual/session.png` and `target/tui-visual/narrow.png` are written. Open both and check against the reported bug (dark green success band, gray output text):

- The successful `read_text_file` row and the successful `edit_text_file` row share one neutral panel. It is not green.
- The failed `edit_text_file` row ("malformed hashline patch") is the only tool row with the error surface, and its text is readable on that surface.
- Every tool row has a `┃` rule in the left column, including the failed row. The rule is visible on the panel, not the same color as the panel.
- Output text (`pub fn greeting`, `Updated src/lib.rs`) is lighter than the old gray-on-green and readable on the neutral panel.
- `narrow.png` (760px, `narrow.tape:4`) keeps the rule and the tool name on screen; the rule is not clipped by the inset composer. Wrapped output stays inside the panel.

`status.png` is still produced by `session.tape:27`. Confirm it shows `tokens: 554`. That is the existing fixture assertion, not a new theme check.

- [ ] **Step 5: Rewrite the README theme section**

`README.md:167` currently says `YACH_THEME` is an "Explicit path to a TUI theme JSON file." Replace that table row with:

```markdown
| `YACH_THEME` | `auto` | Preset (`dark`, `light`, `system`, `auto`), bundled palette name, or path to a theme JSON file. A non-empty `NO_COLOR` overrides it. |
| `NO_COLOR` | unset | Non-empty selects the monochrome theme and overrides `YACH_THEME` and `~/.yach/theme.json`. |
| `COLORFGBG` | unset | Read only when the theme is `auto`. Last field below 8 selects `dark`; otherwise `light`. Absent or unparsable selects `dark`. |
```

Replace `README.md:237-271` (the paragraph beginning "TUI themes use" through the "Unknown fields" paragraph) with:

```markdown
TUI appearance is chosen once, at startup. `NO_COLOR` set to any non-empty
value wins over everything else and selects the monochrome theme. Otherwise
the first match wins:

1. `YACH_THEME`, when it is a preset name (`dark`, `light`, `system`,
   `auto`), a bundled palette name, or a path to a theme file. Those names
   are reserved, so a file that happens to have one is reached as `./<name>`.
2. `~/.yach/theme.json`.
3. `auto`.

`<project>/.yach/theme.json` is not read. When that file exists, the TUI
shows one notice and leaves the file untouched: "project themes are not
supported; move it to `~/.yach/theme.json` or set `YACH_THEME`". For a
per-project look, set `YACH_THEME` in that project's environment.

`auto` reads `COLORFGBG` only. The value is `fg;bg` or `fg;default;bg`; the
last field is the background index. An index below 8 selects `dark`, any
other index selects `light`, and an absent or unparsable value selects
`dark`. `COLORFGBG` is set by only some terminals, is inherited stale by
shells and multiplexers, and does not update when the terminal switches
appearance. When the guess is wrong, set `YACH_THEME=light`,
`YACH_THEME=dark`, or `YACH_THEME=system`. `system` paints no surface, so it
stays legible on any background.

Presets:

| Name | Surfaces |
| --- | --- |
| `dark` | Painted dark panels. The built-in default behind `auto` when detection says dark. |
| `light` | Painted light panels with dark text on every surface. |
| `system` | No panel backgrounds. Status uses the terminal's ANSI colors. |
| `auto` | Resolves to `dark` or `light` from `COLORFGBG`. |

Bundled palettes are named themes, selected the same way (`YACH_THEME=nord`
or `"base": "nord"`). They are 24-bit; on a limited-color terminal use
`system`. The names are `catppuccin-latte`, `catppuccin-frappe`,
`catppuccin-macchiato`, `catppuccin-mocha`, `rose-pine`, `rose-pine-moon`,
`rose-pine-dawn`, `gruvbox-dark`, `gruvbox-light`, `tokyo-night-dark`,
`tokyo-night-storm`, `tokyo-night-moon`, `tokyo-night-light`,
`solarized-dark`, `solarized-light`, `dracula`, `nord`, and `monokai`.
Attribution is in `crates/yach-ui/themes/NOTICE.md`.

A theme file is strict JSON. `base` names the theme those overrides apply
to and defaults to `dark`, so an existing file that does not set `base`
keeps its meaning. `base` accepts a preset, `auto`, a bundled palette name,
or `<extension-id>:<theme-id>` for a theme contributed by an installed
extension (see [docs/extensions.md](docs/extensions.md)). Colors in the file
override that base. No file is merged with another file.

```json
{
  "base": "auto",
  "vars": {
    "surface": "#282832"
  },
  "colors": {
    "accent": "#00d7ff",
    "userMessageBackground": "#343541",
    "toolPendingBackground": "surface",
    "diffAdded": 22
  },
  "spacing": {
    "userMessageHorizontalPadding": 1,
    "userMessageVerticalPadding": 1,
    "toolHorizontalPadding": 1,
    "toolVerticalPadding": 1,
    "toolGap": 1
  }
}
```

Colors accept `#rrggbb`, ANSI names (`black`, `red`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `gray`, `darkGray`, `white`, `default`),
256-color indices, or a name from `vars`. Color tokens: `accent`, `border`,
`success`, `error`, `warning`, `muted`, `dim`, `text`,
`selectedBackground`, `selectedText`, `userMessageBackground`,
`userMessageText`, `toolPendingBackground`, `toolSuccessBackground`,
`toolErrorBackground`, `toolTitle`, `toolOutput`, `diffAdded`, `diffRemoved`,
`diffContext`, `diffHunk`, and `harness`. Unknown fields, tokens, colors, or
variables fail before the TUI opens rather than being ignored.

Monochrome (`NO_COLOR`) draws no color and no panel background. Metadata is
dim, the focused title and tool name and errors are bold, and the selected
row is reversed. Glyphs (`✓`, `✗`, `⚙`, `›`) and the tool row's `┃` rule
carry the distinctions that color carried.
```

Do not leave the old "project settings take precedence" sentence. It is the behavior this spec removes (`main.rs:425-450` today; Task 4 deletes that lookup).

- [ ] **Step 6: Document extension themes**

Append this section to `docs/extensions.md`, after the "Registration follows the manifest" section (the file ends at line 183):

````markdown

## Themes

An extension may contribute theme files. A theme is data: contributing one
adds no tool, requests no capability, and starts no host. Installing or
enabling the extension does not change the active theme. The theme applies
only when the user names it.

Declare it next to the other contributions:

```json
"contributes": {
  "themes": [{
    "id": "night",
    "title": "Night",
    "source": { "type": "extension_file", "path": "themes/night.json" },
    "max_bytes": 16384
  }]
}
```

`id` is a theme name, not an extension id. `path` is relative to the package
root and must not escape it. `themes/night.json` is an ordinary yach theme
file (see the theme section of [../README.md](../README.md)): the same
`base`, `vars`, `colors`, and `spacing` keys. Its own `base` may name a
preset or a bundled palette, not another extension theme.

The qualified name is `<extension-id>:<theme-id>`, for example
`example.themes:night`. The user selects it as the `base` of
`~/.yach/theme.json`, or sets `YACH_THEME` to a file whose `base` is that
name. `YACH_THEME=example.themes:night` is not a theme name: `YACH_THEME` is
a preset, a bundled palette, or a file path.

Reserved ids `dark`, `light`, `system`, and `auto` are rejected. Only
user-scope extensions contribute themes; a project-scope extension's themes
are not offered. If the selected extension is later disabled or removed, yach
falls back to `auto` (the user file's tokens still apply over it) and shows
one notice naming the missing theme. Re-enabling the extension restores it.
The selection is not rewritten.
````

- [ ] **Step 7: Supersede the Wave 3 tool-row treatment**

Insert the following after the title block of `docs/project/specs/2026-08-19-wave3-tui-visual-design.md`, before `## Problem` (after line 5):

```markdown

> **Superseded in part, 2026-10-06.** Tool-row treatment in this document
> (outcome-tinted full-width surfaces, and "a compact successful row receives
> no background band") is superseded by
> [the theming engine spec](2026-10-02-theming-engine-design.md). Every tool
> row is a neutral panel with a state-colored left rule (`┃`); only the
> failure surface is tinted. Theme selection, `NO_COLOR`, and the theme file's
> `base` key are also defined there. Transcript hierarchy, spacing, and the
> docked composer in this document still hold.

```

Leave the rest of that spec unchanged, including the owner-correction paragraphs. The note is the pointer; rewriting those paragraphs would hide what was decided at the time.

- [ ] **Step 8: Verify and record the binary size**

Run, in order:

```bash
just lint
just test
just tui-visual session narrow light nocolor
just dev bash tests/visual/check-nocolor.sh
```

Expected: `just lint` exits 0. The workspace tests exit 0. All four tapes write their screenshots under `target/tui-visual/`. The byte check prints `nocolor bytes:` and exits 0.

Re-open the four screenshots and repeat the legibility checks from Steps 2 and 4. The last `just tui-visual` is the run whose pixels count; earlier runs were deleted by `render.sh:12`.

Then measure the release binary against the size gate. Embedded palettes are about 25 KiB (`docs/project/specs/2026-10-02-theming-engine-design.md:449-450`); the gate is `binary/size_bytes` at 5% (`crates/yach-bench/perf-thresholds.toml:42-45`, plane:YACH-13).

```bash
just perf --filter 'binary/size_bytes' --deterministic
```

Expected: the `binary/size_bytes` row is `unchanged` or inside the 5% budget. If it is `regressed`, record the measured before/after byte counts in the commit message and stop; do not raise the budget in this task.

Commit the docs and tapes only:

```bash
jj commit README.md docs/extensions.md docs/project/specs/2026-08-19-wave3-tui-visual-design.md tests/visual -m "Record theme tapes and document theme selection"
```
