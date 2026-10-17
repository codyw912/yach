/// One bundled named palette. `json` is the theme file embedded from
/// `crates/yach-ui/themes/<name>.json`; startup parses only the selected one.
pub(crate) struct BundledPalette {
    pub name: &'static str,
    pub json: &'static str,
}

/// Spec order (`docs/project/specs/2026-10-02-theming-engine-design.md:353-360`).
pub(crate) const BUNDLED_PALETTES: [BundledPalette; 18] = [
    palette(
        "catppuccin-latte",
        include_str!("../../themes/catppuccin-latte.json"),
    ),
    palette(
        "catppuccin-frappe",
        include_str!("../../themes/catppuccin-frappe.json"),
    ),
    palette(
        "catppuccin-macchiato",
        include_str!("../../themes/catppuccin-macchiato.json"),
    ),
    palette(
        "catppuccin-mocha",
        include_str!("../../themes/catppuccin-mocha.json"),
    ),
    palette("rose-pine", include_str!("../../themes/rose-pine.json")),
    palette(
        "rose-pine-moon",
        include_str!("../../themes/rose-pine-moon.json"),
    ),
    palette(
        "rose-pine-dawn",
        include_str!("../../themes/rose-pine-dawn.json"),
    ),
    palette(
        "gruvbox-dark",
        include_str!("../../themes/gruvbox-dark.json"),
    ),
    palette(
        "gruvbox-light",
        include_str!("../../themes/gruvbox-light.json"),
    ),
    palette(
        "tokyo-night-dark",
        include_str!("../../themes/tokyo-night-dark.json"),
    ),
    palette(
        "tokyo-night-storm",
        include_str!("../../themes/tokyo-night-storm.json"),
    ),
    palette(
        "tokyo-night-moon",
        include_str!("../../themes/tokyo-night-moon.json"),
    ),
    palette(
        "tokyo-night-light",
        include_str!("../../themes/tokyo-night-light.json"),
    ),
    palette(
        "solarized-dark",
        include_str!("../../themes/solarized-dark.json"),
    ),
    palette(
        "solarized-light",
        include_str!("../../themes/solarized-light.json"),
    ),
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
pub(crate) fn bundled_palette(name: &str) -> Option<&'static BundledPalette> {
    BUNDLED_PALETTES.iter().find(|palette| palette.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{BUNDLED_PALETTES, bundled_palette};
    use crate::theme::contrast::assert_preset_contrast;
    use crate::theme::palette_test::{
        COLOR_TOKENS, LIGHT_PALETTES, assert_appearance, notice_attributes, themes_dir,
    };
    use crate::theme::{ColorValue, Theme, ThemeDocument, resolve_color};

    #[test]
    fn bundled_palette_files_parse_and_cover_every_token() -> Result<(), String> {
        if BUNDLED_PALETTES.len() != 18 {
            return Err(format!(
                "expected 18 palettes, got {}",
                BUNDLED_PALETTES.len()
            ));
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
                return Err(format!(
                    "{}: base {base} is not dark or light",
                    palette.name
                ));
            }
            let light = LIGHT_PALETTES.contains(&palette.name);
            if light != (base == "light") {
                return Err(format!(
                    "{}: base {base} does not match the pinned upstream variant",
                    palette.name
                ));
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
            assert_appearance(&theme, palette.name, light)?;
            let page = resolve_color(
                &ColorValue::Name(String::from("base00")),
                &document.vars,
                &mut BTreeSet::new(),
            )
            .map_err(|error| format!("{}: page {error}", palette.name))?;
            assert_preset_contrast(&theme, Some(page));
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
        let table: BTreeSet<&str> = BUNDLED_PALETTES
            .iter()
            .map(|palette| palette.name)
            .collect();
        let on_disk_ref: BTreeSet<&str> = on_disk.iter().map(String::as_str).collect();
        if table != on_disk_ref {
            return Err(format!("table {table:?} != themes/*.json {on_disk_ref:?}"));
        }
        let notice = std::fs::read_to_string(dir.join("NOTICE.md"))
            .map_err(|error| format!("read NOTICE.md: {error}"))?;
        for palette in &BUNDLED_PALETTES {
            notice_attributes(&notice, palette.name)?;
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
