mod palettes;
mod select;

pub use select::{
    Appearance, BaseError, ExtensionThemes, appearance_from_colorfgbg, builtin_theme_names,
    resolve_base,
};
mod contrast;
#[cfg(test)]
mod palette_test;
mod presets;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    colors: ThemeColors,
    pub spacing: ThemeSpacing,
    pub composer: ComposerStyle,
    mode: ThemeMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThemeColors {
    accent: Color,
    border: Color,
    success: Color,
    error: Color,
    warning: Color,
    muted: Color,
    dim: Color,
    text: Color,
    selected_background: Color,
    selected_text: Color,
    user_message_background: Color,
    user_message_text: Color,
    tool_pending_background: Color,
    tool_success_background: Color,
    tool_error_background: Color,
    tool_title: Color,
    tool_output: Color,
    diff_added: Color,
    diff_removed: Color,
    diff_context: Color,
    diff_hunk: Color,
    harness: Color,
    status_background: Color,
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
        Self::foreground(self.colors.text, Modifier::empty())
    }

    #[must_use]
    pub fn muted(&self) -> Style {
        Self::foreground(self.colors.muted, self.dim_modifier())
    }

    #[must_use]
    pub fn dim(&self) -> Style {
        Self::foreground(self.colors.dim, self.dim_modifier())
    }

    #[must_use]
    pub fn accent(&self) -> Style {
        Self::foreground(self.colors.accent, self.bold_unless_painted())
    }

    /// Focused frames use the accent color; unfocused frames use `border`.
    /// Monochrome drops both colors and bolds only the focused frame.
    #[must_use]
    pub fn border(&self, focused: bool) -> Style {
        let color = if focused {
            self.colors.accent
        } else {
            self.colors.border
        };
        let modifier = if focused {
            self.bold_unless_painted()
        } else {
            Modifier::empty()
        };
        Self::foreground(color, modifier)
    }

    #[must_use]
    pub fn success(&self) -> Style {
        Self::foreground(self.colors.success, Modifier::empty())
    }

    #[must_use]
    pub fn warning(&self) -> Style {
        Self::foreground(self.colors.warning, Modifier::empty())
    }

    #[must_use]
    pub fn error(&self) -> Style {
        Self::foreground(self.colors.error, self.bold_unless_painted())
    }

    #[must_use]
    pub fn harness(&self) -> Style {
        Self::foreground(self.colors.harness, Modifier::empty())
    }

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

    /// Selection follows the same mode handling as other surfaces: System
    /// paints no background, and Mono keeps `REVERSED` with both colors
    /// reset. A file that sets `selectedBackground` still cannot paint a
    /// system selection.
    #[must_use]
    pub fn selected(&self) -> Style {
        match self.mode {
            ThemeMode::Mono => Style::new()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .add_modifier(Modifier::REVERSED),
            ThemeMode::System => self
                .surface(self.colors.selected_text, self.colors.selected_background)
                .add_modifier(Modifier::BOLD),
            ThemeMode::Painted => {
                self.surface(self.colors.selected_text, self.colors.selected_background)
            }
        }
    }

    /// Selected confirm/apply/reject label. Painted mode retains the role
    /// background, choosing a legible foreground for that specific pair.
    /// System and Mono select with attributes, never a painted background.
    #[must_use]
    pub fn selected_on(&self, role: Style) -> Style {
        if self.mode == ThemeMode::Painted {
            let background = role.fg.unwrap_or(Color::Reset);
            let foreground = match contrast::contrast_ratio(self.colors.selected_text, background) {
                Some(ratio) if ratio >= 4.5 => self.colors.selected_text,
                // Only a terminal-default background is unknowable. A Reset
                // foreground on a measurable painted background still needs
                // an explicit black/white pairing.
                None if background == Color::Reset => self.colors.selected_text,
                Some(_) | None => {
                    let black = Color::Rgb(0, 0, 0);
                    let white = Color::Rgb(255, 255, 255);
                    if contrast::contrast_ratio(black, background)
                        >= contrast::contrast_ratio(white, background)
                    {
                        black
                    } else {
                        white
                    }
                }
            };
            return Style::new().fg(foreground).bg(background);
        }
        self.selected().add_modifier(Modifier::BOLD)
    }

    /// Selected list row that today sets `selected_text` and no background.
    /// Painted keeps that. System and Mono take `selected()` so Mono stays
    /// reversed and System stays unpainted.
    #[must_use]
    pub fn selected_label(&self) -> Style {
        if self.mode == ThemeMode::Painted {
            return Style::new()
                .fg(self.colors.selected_text)
                .add_modifier(Modifier::BOLD);
        }
        self.selected().add_modifier(Modifier::BOLD)
    }

    #[must_use]
    pub fn user_message(&self) -> Style {
        self.surface(
            self.colors.user_message_text,
            self.colors.user_message_background,
        )
    }

    /// Dark and light presets share a neutral pending/success background;
    /// custom themes may override either state independently. Error and
    /// harness outcomes share the error background.
    #[must_use]
    pub fn tool_surface(&self, state: ToolState) -> Style {
        self.surface(self.colors.text, self.tool_background(state))
    }

    #[must_use]
    pub fn tool_rule(&self, state: ToolState) -> Style {
        Self::foreground(self.tool_rule_color(state), Modifier::empty())
    }

    #[must_use]
    pub fn tool_title(&self) -> Style {
        Self::foreground(self.colors.tool_title, Modifier::empty())
    }

    #[must_use]
    pub fn tool_output(&self, state: ToolState) -> Style {
        Style::new()
            .fg(self.colors.tool_output)
            .bg(self.tool_surface(state).bg.unwrap_or(Color::Reset))
    }

    #[must_use]
    pub fn diff(&self, kind: DiffKind) -> Style {
        Self::foreground(self.diff_color(kind), Modifier::empty())
    }

    fn foreground(color: Color, modifier: Modifier) -> Style {
        Style::new().fg(color).add_modifier(modifier)
    }

    fn surface(&self, foreground: Color, background: Color) -> Style {
        if self.mode == ThemeMode::Mono || self.mode == ThemeMode::System {
            return Style::new().fg(foreground).bg(Color::Reset);
        }
        Style::new().fg(foreground).bg(background)
    }

    fn tool_background(&self, state: ToolState) -> Color {
        match state {
            ToolState::Pending | ToolState::Harness => self.colors.tool_pending_background,
            ToolState::Success => self.colors.tool_success_background,
            ToolState::Error => self.colors.tool_error_background,
        }
    }

    fn tool_rule_color(&self, state: ToolState) -> Color {
        match state {
            ToolState::Pending => self.colors.warning,
            ToolState::Success => self.colors.dim,
            ToolState::Error => self.colors.error,
            ToolState::Harness => self.colors.harness,
        }
    }

    fn diff_color(&self, kind: DiffKind) -> Color {
        match kind {
            DiffKind::Added => self.colors.diff_added,
            DiffKind::Removed => self.colors.diff_removed,
            DiffKind::Context => self.colors.diff_context,
            DiffKind::Hunk => self.colors.diff_hunk,
        }
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
            "statusBackground" => colors.status_background = color,
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
    composer: ComposerOverrides,
}

impl ThemeDocument {
    /// Parses the file. Color tokens and variables are checked in [`Self::apply`],
    /// so a document with an unknown token or a bad color still parses.
    pub fn parse(json: &str) -> Result<Self, ThemeLoadError> {
        let file: ThemeFile = serde_json::from_str(json).map_err(ThemeLoadError::Json)?;
        Ok(Self {
            base: file.base,
            vars: file.vars,
            colors: file.colors,
            spacing: file.spacing,
            composer: file.composer,
        })
    }

    #[must_use]
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    #[cfg(test)]
    pub fn color_tokens(&self) -> impl Iterator<Item = &str> {
        self.colors.keys().map(String::as_str)
    }

    #[cfg(test)]
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
        if let Some(style) = self.composer.style {
            base.composer = style;
        }
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
    #[serde(default)]
    composer: ComposerOverrides,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
struct ComposerOverrides {
    style: Option<ComposerStyle>,
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
        if hex.len() != 6 || !hex.is_ascii() {
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

#[cfg(test)]
mod tests {
    use super::{
        ComposerStyle, DiffKind, Theme, ThemeDocument, ThemeLoadError, ThemeMode, ToolState,
    };
    use crate::theme::contrast::{assert_preset_contrast, contrast_ratio};
    use ratatui::style::{Color, Modifier, Style};

    #[test]
    fn composer_style_parses_names_and_defaults_to_box() {
        for (json, expected) in [
            (r"{}", ComposerStyle::Box),
            (r#"{"composer":{}}"#, ComposerStyle::Box),
            (r#"{"composer":{"style":"box"}}"#, ComposerStyle::Box),
            (r#"{"composer":{"style":"rule"}}"#, ComposerStyle::Rule),
            (r#"{"composer":{"style":"band"}}"#, ComposerStyle::Band),
        ] {
            let theme =
                ThemeDocument::parse(json).and_then(|document| document.apply(Theme::dark()));
            assert_eq!(
                theme.ok().map(|theme| theme.composer),
                Some(expected),
                "{json}"
            );
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
        assert_eq!(
            Theme::light().status_band(),
            Some(Color::Rgb(236, 236, 242))
        );
        let mut band = Theme::dark();
        band.composer = ComposerStyle::Band;
        assert_eq!(band.composer_style(), ComposerStyle::Band);
        for base in [Theme::system(), Theme::monochrome()] {
            let mut theme = base;
            theme.composer = ComposerStyle::Band;
            assert_eq!(theme.status_band(), None, "{:?}", base.mode());
            assert_eq!(
                theme.composer_style(),
                ComposerStyle::Rule,
                "{:?}",
                base.mode()
            );
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
            let theme =
                ThemeDocument::parse(json).and_then(|document| document.apply(Theme::dark()));
            assert!(theme.is_ok(), "{json}");
            let Ok(theme) = theme else { return };
            assert_eq!(theme.status_band(), expected, "{json}");
            let style = if expected.is_some() {
                ComposerStyle::Band
            } else {
                ComposerStyle::Rule
            };
            assert_eq!(theme.composer_style(), style, "{json}");
        }
    }

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
    fn selected_action_colors_clear_body_contrast() {
        for name in super::builtin_theme_names() {
            let theme = super::resolve_base(name, super::Appearance::Dark, &());
            assert!(theme.is_ok(), "{name}: {theme:?}");
            let Ok(theme) = theme else { return };
            for (role_name, role) in [
                ("success", theme.success()),
                ("error", theme.error()),
                ("warning", theme.warning()),
            ] {
                let selected = theme.selected_on(role);
                if bg(selected) == Color::Reset {
                    continue;
                }
                let ratio = contrast_ratio(fg(selected), bg(selected));
                assert!(
                    ratio.is_some_and(|ratio| ratio >= 4.5),
                    "{name} {role_name}: {selected:?}, contrast {ratio:?}"
                );
            }
        }
    }

    #[test]
    fn selected_action_keeps_readable_requested_text_and_unmeasurable_pairs() {
        let document =
            ThemeDocument::parse(r##"{"colors":{"selectedText":"#ffffff","success":"#000000"}}"##);
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else { return };
        assert_eq!(
            fg(theme.selected_on(theme.success())),
            Color::Rgb(255, 255, 255)
        );
        assert_eq!(
            fg(theme.selected_on(Style::new())),
            Color::Rgb(255, 255, 255)
        );
    }

    #[test]
    fn selected_action_resolves_reset_text_on_measurable_background() {
        let document = ThemeDocument::parse(r#"{"colors":{"selectedText":"default"}}"#);
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        let theme = document.apply(Theme::dark());
        assert!(theme.is_ok());
        let Ok(theme) = theme else { return };
        let selected = theme.selected_on(theme.success());
        assert!(contrast_ratio(fg(selected), bg(selected)).is_some_and(|ratio| ratio >= 4.5));
    }

    #[test]
    fn non_ascii_hex_is_rejected_without_panicking() {
        assert_eq!(super::parse_color("#aéaaa"), None);
        let document = ThemeDocument::parse(r##"{"colors":{"accent":"#aéaaa"}}"##);
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        assert!(document.apply(Theme::dark()).is_err());
    }

    #[test]
    fn dark_preset_keeps_explicit_rgb_palette_values() {
        let theme = Theme::dark();
        assert_eq!(theme.mode(), ThemeMode::Painted);
        assert_eq!(Theme::default(), theme);

        assert_eq!(theme.colors.accent, Color::Rgb(0, 215, 255));
        assert_eq!(theme.colors.border, Color::Rgb(95, 135, 255));
        assert_eq!(theme.colors.success, Color::Rgb(181, 189, 104));
        assert_eq!(theme.colors.error, Color::Rgb(204, 102, 102));
        assert_eq!(theme.colors.warning, Color::Rgb(240, 198, 116));
        assert_eq!(theme.colors.muted, Color::Rgb(128, 128, 128));
        assert_eq!(theme.colors.dim, Color::Rgb(110, 110, 110));
        assert_eq!(theme.colors.text, Color::Rgb(234, 234, 234));
        assert_eq!(theme.colors.selected_background, Color::Rgb(58, 58, 74));
        assert_eq!(theme.colors.selected_text, Color::Rgb(234, 234, 234));
        assert_eq!(theme.colors.user_message_background, Color::Rgb(52, 53, 65));
        assert_eq!(theme.colors.user_message_text, Color::Rgb(234, 234, 234));
        assert_eq!(theme.colors.tool_pending_background, Color::Rgb(40, 40, 50));
        assert_eq!(theme.colors.tool_success_background, Color::Rgb(40, 40, 50));
        assert_eq!(theme.colors.tool_error_background, Color::Rgb(60, 40, 40));
        assert_eq!(theme.colors.tool_title, Color::Rgb(234, 234, 234));
        assert_eq!(theme.colors.tool_output, Color::Rgb(192, 192, 192));
        assert_eq!(theme.colors.diff_added, Color::Rgb(181, 189, 104));
        assert_eq!(theme.colors.diff_removed, Color::Rgb(204, 102, 102));
        assert_eq!(theme.colors.diff_context, Color::Rgb(128, 128, 128));
        assert_eq!(theme.colors.diff_hunk, Color::Rgb(0, 215, 255));
        assert_eq!(theme.colors.harness, Color::Rgb(178, 148, 187));
        assert_eq!(theme.spacing.user_message_horizontal_padding, 0);
        assert_eq!(theme.spacing.user_message_vertical_padding, 1);
        assert_eq!(theme.spacing.tool_horizontal_padding, 1);
        assert_eq!(theme.spacing.tool_vertical_padding, 0);
        assert_eq!(theme.spacing.tool_gap, 1);

        assert_eq!(
            theme.colors.tool_success_background,
            theme.colors.tool_pending_background
        );
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
        assert_eq!(
            theme.colors.status_background,
            Theme::dark().colors.status_background
        );
        assert_eq!(theme.composer, ComposerStyle::Box);
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
        let document =
            ThemeDocument::parse(r##"{ "base": "light", "colors": { "accent": "#abcdef" } }"##);
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
        // F1: validation happens in apply, not parse. `#xyz` is not a named
        // color, so the preserved resolver reports it as an unknown variable.
        let unknown = ThemeDocument::parse(r#"{"colors":{"unknown":"red"}}"#);
        assert!(unknown.is_ok());
        let Ok(unknown) = unknown else { return };
        let unknown = unknown.apply(Theme::dark());
        assert!(matches!(
            unknown,
            Err(ThemeLoadError::UnknownColorToken(token)) if token == "unknown"
        ));

        let missing = ThemeDocument::parse(r#"{"colors":{"accent":"nope"}}"#);
        assert!(missing.is_ok());
        let Ok(missing) = missing else { return };
        let missing = missing.apply(Theme::dark());
        assert!(matches!(
            missing,
            Err(ThemeLoadError::UnknownVariable(name)) if name == "nope"
        ));

        let cycle = ThemeDocument::parse(
            r#"{ "vars": { "a": "b", "b": "a" }, "colors": { "accent": "a" } }"#,
        );
        assert!(cycle.is_ok());
        let Ok(cycle) = cycle else { return };
        let cycle = cycle.apply(Theme::dark());
        assert!(matches!(cycle, Err(ThemeLoadError::VariableCycle(_))));

        // deny_unknown_fields survives the new `base` key: a stray top-level
        // field is still a parse error.
        let extra = ThemeDocument::parse(r#"{ "name": "nope" }"#);
        assert!(matches!(extra, Err(ThemeLoadError::Json(_))));

        let bad_color = ThemeDocument::parse(r##"{"colors":{"accent":"#xyz"}}"##);
        assert!(bad_color.is_ok());
        let Ok(bad_color) = bad_color else { return };
        let bad_color = bad_color.apply(Theme::dark());
        assert!(matches!(
            bad_color,
            Err(ThemeLoadError::UnknownVariable(value)) if value == "#xyz"
        ));
    }

    #[test]
    fn dark_and_light_paint_explicit_pairs_at_spec_contrast() {
        // Presets paint no page of their own; measure muted and dim against a
        // typical terminal background for each appearance.
        for (theme, page) in [
            (Theme::dark(), Color::Rgb(28, 28, 28)),
            (Theme::light(), Color::Rgb(250, 250, 250)),
        ] {
            assert_preset_contrast(&theme, Some(page));
            for (name, surface) in painted_surfaces(&theme) {
                assert!(!is_reset(fg(surface)), "{name} fg is Reset");
                assert!(!is_reset(bg(surface)), "{name} bg is Reset");
            }
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
        // F2: a focused frame uses the accent color. System accent is Cyan,
        // not Reset; the unfocused border stays Reset.
        assert_eq!(fg(theme.border(true)), Color::Cyan);
        assert!(is_reset(fg(theme.border(false))));
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
    fn system_selected_ignores_a_painted_background_override() {
        // F17: selected() uses the same mode handling as other surfaces.
        // A file that sets selectedBackground over system still paints none.
        let document = ThemeDocument::parse(
            r##"{ "base": "system", "colors": { "selectedBackground": "#112233", "selectedText": "#445566" } }"##,
        );
        assert!(document.is_ok());
        let Ok(document) = document else { return };
        let theme = document.apply(Theme::system());
        assert!(theme.is_ok());
        let Ok(theme) = theme else { return };
        assert_eq!(theme.mode(), ThemeMode::System);
        assert_eq!(
            theme.colors.selected_background,
            Color::Rgb(0x11, 0x22, 0x33)
        );
        assert!(is_reset(bg(theme.selected())));
        assert!(theme.selected().add_modifier.contains(Modifier::BOLD));
        assert!(!theme.selected().add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn monochrome_is_reset_plus_the_modifier_table() {
        let theme = Theme::monochrome();
        assert_eq!(theme.mode(), ThemeMode::Mono);
        let colors = theme.colors;
        for color in [
            colors.accent,
            colors.border,
            colors.success,
            colors.error,
            colors.warning,
            colors.muted,
            colors.dim,
            colors.text,
            colors.selected_background,
            colors.selected_text,
            colors.user_message_background,
            colors.user_message_text,
            colors.tool_pending_background,
            colors.tool_success_background,
            colors.tool_error_background,
            colors.tool_title,
            colors.tool_output,
            colors.diff_added,
            colors.diff_removed,
            colors.diff_context,
            colors.diff_hunk,
            colors.harness,
            colors.status_background,
        ] {
            assert_eq!(color, Color::Reset);
        }

        assert!(theme.muted().add_modifier.contains(Modifier::DIM));
        assert!(theme.dim().add_modifier.contains(Modifier::DIM));
        assert!(theme.accent().add_modifier.contains(Modifier::BOLD));
        assert!(!theme.tool_title().add_modifier.contains(Modifier::BOLD));
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
        assert_eq!(
            theme.diff(DiffKind::Removed).add_modifier,
            Modifier::empty()
        );
    }

    #[test]
    fn painted_and_system_share_pending_and_success_surfaces() {
        for theme in [Theme::light(), Theme::dark()] {
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
        assert_eq!(
            bg(theme.user_message()),
            theme.colors.user_message_background
        );

        assert_eq!(
            bg(theme.tool_surface(ToolState::Pending)),
            theme.colors.tool_pending_background
        );
        assert_eq!(
            bg(theme.tool_surface(ToolState::Success)),
            theme.colors.tool_success_background
        );
        assert_eq!(
            bg(theme.tool_surface(ToolState::Error)),
            theme.colors.tool_error_background
        );
        assert_eq!(
            bg(theme.tool_surface(ToolState::Harness)),
            theme.colors.tool_pending_background
        );
        assert_eq!(
            fg(theme.tool_surface(ToolState::Pending)),
            theme.colors.text
        );

        assert_eq!(
            fg(theme.tool_rule(ToolState::Pending)),
            theme.colors.warning
        );
        assert_eq!(fg(theme.tool_rule(ToolState::Success)), theme.colors.dim);
        assert_eq!(fg(theme.tool_rule(ToolState::Error)), theme.colors.error);
        assert_eq!(
            fg(theme.tool_rule(ToolState::Harness)),
            theme.colors.harness
        );

        assert!(!theme.tool_title().add_modifier.contains(Modifier::BOLD));
        assert_eq!(fg(theme.tool_title()), theme.colors.tool_title);
        assert_eq!(
            fg(theme.tool_output(ToolState::Success)),
            theme.colors.tool_output
        );
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

        // R8: named White is the xterm bright white, (255, 255, 255), the
        // same RGB as Indexed(15). Contrast against black is 21:1, not the
        // plan's pinned 17.3 (that figure was the dim gray at index 7).
        assert_eq!(
            contrast_ratio(Color::White, Color::Black),
            contrast_ratio(Color::Rgb(255, 255, 255), Color::Rgb(0, 0, 0)),
        );
        let named = contrast_ratio(Color::White, Color::Black);
        assert!(named.is_some_and(|ratio| (ratio - 21.0).abs() < 0.01));
        assert_eq!(
            contrast_ratio(Color::White, Color::Black),
            contrast_ratio(Color::Indexed(15), Color::Black),
        );

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
