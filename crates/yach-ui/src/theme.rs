use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use ratatui::style::Color;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub colors: ThemeColors,
    pub spacing: ThemeSpacing,
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
    pub const fn pi_dark() -> Self {
        Self {
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
        }
    }

    pub fn from_json(json: &str) -> Result<Self, ThemeLoadError> {
        let source: ThemeFile = serde_json::from_str(json).map_err(ThemeLoadError::Json)?;
        let mut theme = Self::default();
        for (token, value) in &source.colors {
            let color = resolve_color(value, &source.vars, &mut BTreeSet::new())?;
            theme.set_color(token, color)?;
        }
        theme.spacing.apply(&source.spacing);
        Ok(theme)
    }

    pub fn load(path: &Path) -> Result<Self, ThemeLoadError> {
        let json = fs::read_to_string(path).map_err(ThemeLoadError::Io)?;
        Self::from_json(&json)
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
        Self::pi_dark()
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
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidColor(String),
    UnknownColorToken(String),
    UnknownVariable(String),
    VariableCycle(String),
}

impl fmt::Display for ThemeLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "theme file read failed: {error}"),
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    #[serde(default)]
    vars: BTreeMap<String, ColorValue>,
    #[serde(default)]
    colors: BTreeMap<String, ColorValue>,
    #[serde(default)]
    spacing: ThemeSpacingOverrides,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ColorValue {
    Name(String),
    Indexed(u8),
}

#[derive(Debug, Default, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeLoadError};
    use ratatui::style::Color;

    #[test]
    fn default_theme_matches_pi_message_and_tool_surfaces() {
        let theme = Theme::default();

        assert_eq!(theme.colors.user_message_background, Color::Rgb(52, 53, 65));
        assert_eq!(theme.colors.tool_success_background, Color::Rgb(40, 50, 40));
        assert_eq!(theme.spacing.user_message_vertical_padding, 1);
        assert_eq!(theme.spacing.tool_vertical_padding, 1);
        assert_eq!(theme.spacing.tool_gap, 1);
    }

    #[test]
    fn json_theme_overrides_colors_variables_indices_and_spacing() {
        let parsed = Theme::from_json(
            r##"{
                "vars": { "surface": "#101820" },
                "colors": {
                    "userMessageBackground": "surface",
                    "toolSuccessBackground": 22,
                    "accent": "darkGray"
                },
                "spacing": {
                    "userMessageVerticalPadding": 2,
                    "toolGap": 3
                }
            }"##,
        );
        assert!(parsed.is_ok());
        let Ok(theme) = parsed else {
            return;
        };

        assert_eq!(theme.colors.user_message_background, Color::Rgb(16, 24, 32));
        assert_eq!(theme.colors.tool_success_background, Color::Indexed(22));
        assert_eq!(theme.colors.accent, Color::DarkGray);
        assert_eq!(theme.spacing.user_message_vertical_padding, 2);
        assert_eq!(theme.spacing.tool_gap, 3);
    }

    #[test]
    fn json_theme_rejects_unknown_tokens() {
        let error = Theme::from_json(r#"{"colors":{"unknown":"red"}}"#);

        assert!(
            matches!(error, Err(ThemeLoadError::UnknownColorToken(token)) if token == "unknown")
        );
    }
}
