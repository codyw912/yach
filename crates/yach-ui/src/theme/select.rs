use super::palettes::{BUNDLED_PALETTES, BundledPalette, bundled_palette};
use super::{Theme, ThemeDocument, ThemeLoadError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

pub const PRESET_NAMES: [&str; 4] = ["dark", "light", "system", "auto"];

/// The last COLORFGBG field is the background; absent or invalid means dark.
#[must_use]
pub fn appearance_from_colorfgbg(value: Option<&str>) -> Appearance {
    let Some(last) = value.and_then(|value| value.rsplit(';').next()) else {
        return Appearance::Dark;
    };
    match last.trim().parse::<i64>() {
        Ok(index) if index >= 8 => Appearance::Light,
        _ => Appearance::Dark,
    }
}

#[cfg(test)]
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

fn apply_palette(palette: &BundledPalette) -> Result<Theme, BaseError> {
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

/// Resolves presets, palettes over their own base, or an extension over a builtin.
/// Extension bases cannot chain to another extension; absent bases mean dark.
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
    let base_name = document.base().map_or("dark", |name| name);
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

#[cfg(test)]
mod tests {
    use super::{
        Appearance, BaseError, ExtensionThemes, appearance_from_colorfgbg, builtin_theme_names,
        is_builtin_theme_name, resolve_base,
    };
    use crate::theme::{ComposerStyle, Theme, ThemeDocument, ThemeMode};
    use ratatui::style::Color;

    #[test]
    fn colorfgbg_last_field_picks_appearance() {
        for (value, expected) in [
            (None, Appearance::Dark),
            (Some(""), Appearance::Dark),
            (Some("nope"), Appearance::Dark),
            (Some("15;0"), Appearance::Dark),
            (Some("0;default;15"), Appearance::Light),
            (Some("default;7"), Appearance::Dark),
            (Some("15;8"), Appearance::Light),
            (Some("0;15;"), Appearance::Dark),
            (Some("0;default;default"), Appearance::Dark),
            (Some(" 0; 15 "), Appearance::Light),
            (Some("0;-1"), Appearance::Dark),
        ] {
            assert_eq!(
                appearance_from_colorfgbg(value),
                expected,
                "COLORFGBG={value:?}"
            );
        }
    }

    struct OneTheme(&'static str);
    impl ExtensionThemes for OneTheme {
        fn theme_json(&self, name: &str) -> Option<String> {
            (name == "nightowl:dim").then(|| self.0.to_owned())
        }
    }

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

    #[test]
    fn resolve_base_presets_follow_appearance_and_keep_mode() -> Result<(), String> {
        for (name, appearance, expected) in [
            ("dark", Appearance::Light, Theme::dark()),
            ("light", Appearance::Dark, Theme::light()),
            ("auto", Appearance::Dark, Theme::dark()),
            ("auto", Appearance::Light, Theme::light()),
            ("system", Appearance::Dark, Theme::system()),
        ] {
            let theme = resolve_base(name, appearance, &()).map_err(|e| format!("{e:?}"))?;
            if theme != expected || theme.mode() != expected.mode() {
                return Err(format!(
                    "{name} resolved to {theme:?}, expected {expected:?}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn resolve_base_palette_applies_over_its_own_base() -> Result<(), String> {
        let theme = resolve_base("catppuccin-mocha", Appearance::Light, &())
            .map_err(|e| format!("{e:?}"))?;
        if theme.mode() != ThemeMode::Painted
            || theme.text().fg != Some(Color::Rgb(205, 214, 244))
            || theme == Theme::dark()
            || theme == Theme::light()
        {
            return Err(format!("mocha lost its dark palette: {theme:?}"));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_unknown_name_is_unknown() {
        let error = resolve_base("not-a-theme", Appearance::Dark, &());
        assert!(
            matches!(&error, Err(BaseError::Unknown(name)) if name == "not-a-theme"),
            "{error:?}"
        );
    }

    #[test]
    fn resolve_base_extension_theme_applies_over_builtin_base() -> Result<(), String> {
        let theme = resolve_base(
            "nightowl:dim",
            Appearance::Dark,
            &OneTheme(r##"{"base":"catppuccin-mocha","colors":{"accent":"#112233"}}"##),
        )
        .map_err(|e| format!("{e:?}"))?;
        if theme.text().fg != Some(Color::Rgb(205, 214, 244))
            || theme.accent().fg != Some(Color::Rgb(17, 34, 51))
        {
            return Err(format!("extension lost palette or override: {theme:?}"));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_rejects_extension_theme_whose_base_is_not_builtin() {
        for (json, expected) in [
            (r#"{"base":"other:theme"}"#, "other:theme"),
            (r#"{"base":"not-a-palette"}"#, "not-a-palette"),
        ] {
            let error = resolve_base("nightowl:dim", Appearance::Dark, &OneTheme(json));
            assert!(
                matches!(&error, Err(BaseError::ExtensionBaseNotBuiltin { theme, base }) if theme == "nightowl:dim" && base == expected),
                "{error:?}"
            );
        }
    }

    #[test]
    fn resolve_base_extension_theme_defaults_base_to_dark() -> Result<(), String> {
        let theme = resolve_base(
            "nightowl:dim",
            Appearance::Light,
            &OneTheme(r##"{"colors":{"accent":"#112233"}}"##),
        )
        .map_err(|e| format!("{e:?}"))?;
        if theme.text().fg != Theme::dark().text().fg
            || theme.accent().fg != Some(Color::Rgb(17, 34, 51))
        {
            return Err(format!(
                "extension lost default dark base or override: {theme:?}"
            ));
        }
        Ok(())
    }

    #[test]
    fn resolve_base_invalid_extension_is_reported() {
        for json in [
            "not json",
            r#"{"colors":{"accent":"invalid"}}"#,
            r##"{"colors":{"accent":"#aéaaa"}}"##,
        ] {
            let error = resolve_base("nightowl:dim", Appearance::Dark, &OneTheme(json));
            assert!(
                matches!(&error, Err(BaseError::Invalid { name, .. }) if name == "nightowl:dim"),
                "{error:?}"
            );
        }
    }

    #[test]
    fn builtin_names_cover_presets_and_palettes_only() {
        assert!(is_builtin_theme_name("auto"));
        assert!(is_builtin_theme_name("catppuccin-latte"));
        assert!(!is_builtin_theme_name("nightowl:dim"));
        assert!(!is_builtin_theme_name("./dark"));
        let names = builtin_theme_names();
        assert_eq!(names.len(), 22);
        assert!(names.contains(&"dark"));
        assert!(names.contains(&"monokai"));
        assert!(!names.contains(&"nightowl:dim"));
    }
}
