use std::path::PathBuf;

use ratatui::style::Color;

use super::contrast::contrast_ratio;
use super::{Theme, ToolState};

pub(crate) const COLOR_TOKENS: [&str; 22] = [
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

/// Light variants of the pinned Tinted Theming schemes. Every other bundled
/// name is a dark variant: latte, dawn, and the `*-light` files are light.
pub(crate) const LIGHT_PALETTES: [&str; 5] = [
    "catppuccin-latte",
    "rose-pine-dawn",
    "gruvbox-light",
    "tokyo-night-light",
    "solarized-light",
];

pub(crate) fn themes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("themes")
}

fn paint(style: ratatui::style::Style) -> Result<(Color, Color), String> {
    match (style.fg, style.bg) {
        (Some(fg), Some(bg)) if fg != Color::Reset && bg != Color::Reset => Ok((fg, bg)),
        (fg, bg) => Err(format!(
            "surface is not an explicit pair: fg={fg:?} bg={bg:?}"
        )),
    }
}

/// A light variant must paint like `Theme::light()` (dark text on a light
/// panel), and a dark variant like `Theme::dark()`. Comparing luminance
/// catches a file whose `base` says one thing while its slots are the other.
pub(crate) fn assert_appearance(theme: &Theme, name: &str, light: bool) -> Result<(), String> {
    let (text, panel) = paint(theme.tool_surface(ToolState::Pending))?;
    let text_luminance = contrast_ratio(text, Color::Black)
        .ok_or_else(|| format!("{name}: text is not measurable"))?;
    let panel_luminance = contrast_ratio(panel, Color::Black)
        .ok_or_else(|| format!("{name}: panel is not measurable"))?;
    let text_is_dark = text_luminance < panel_luminance;
    if text_is_dark != light {
        return Err(format!(
            "{name}: appearance is {}, pinned upstream variant is {}",
            if text_is_dark { "light" } else { "dark" },
            if light { "light" } else { "dark" }
        ));
    }
    let preset = if light { Theme::light() } else { Theme::dark() };
    let (preset_text, preset_panel) = paint(preset.tool_surface(ToolState::Pending))
        .map_err(|error| format!("{name}: preset {error}"))?;
    let preset_text_dark = contrast_ratio(preset_text, Color::Black)
        .ok_or_else(|| format!("{name}: preset text is not measurable"))?;
    let preset_panel_dark = contrast_ratio(preset_panel, Color::Black)
        .ok_or_else(|| format!("{name}: preset panel is not measurable"))?;
    if (preset_text_dark < preset_panel_dark) != light {
        return Err(format!("{name}: preset appearance disagrees with {light}"));
    }
    Ok(())
}

/// Upstream project URL and the license statement exactly as `NOTICE.md`
/// writes them. The license cell is what upstream states, including the
/// qualified "none on file" / "none stated" rows; it is not a license yach
/// invented. Keyed by palette name so a swapped URL or a wrong-but-non-empty
/// license fails.
const EXPECTED_ATTRIBUTION: &[(&str, &str, &str)] = &[
    (
        "catppuccin-latte",
        "https://github.com/catppuccin/palette",
        "MIT",
    ),
    (
        "catppuccin-frappe",
        "https://github.com/catppuccin/palette",
        "MIT",
    ),
    (
        "catppuccin-macchiato",
        "https://github.com/catppuccin/palette",
        "MIT",
    ),
    (
        "catppuccin-mocha",
        "https://github.com/catppuccin/palette",
        "MIT",
    ),
    (
        "rose-pine",
        "https://github.com/rose-pine/rose-pine-palette",
        "MIT",
    ),
    (
        "rose-pine-moon",
        "https://github.com/rose-pine/rose-pine-palette",
        "MIT",
    ),
    (
        "rose-pine-dawn",
        "https://github.com/rose-pine/rose-pine-palette",
        "MIT",
    ),
    (
        "gruvbox-dark",
        "https://github.com/morhetz/gruvbox",
        "none on file; README states MIT/X11, repository has no LICENSE file",
    ),
    (
        "gruvbox-light",
        "https://github.com/morhetz/gruvbox",
        "none on file; README states MIT/X11, repository has no LICENSE file",
    ),
    (
        "tokyo-night-dark",
        "https://github.com/folke/tokyonight.nvim",
        "Apache-2.0",
    ),
    (
        "tokyo-night-storm",
        "https://github.com/folke/tokyonight.nvim",
        "Apache-2.0",
    ),
    (
        "tokyo-night-moon",
        "https://github.com/folke/tokyonight.nvim",
        "Apache-2.0",
    ),
    (
        "tokyo-night-light",
        "https://github.com/folke/tokyonight.nvim",
        "Apache-2.0",
    ),
    (
        "solarized-dark",
        "https://github.com/altercation/solarized",
        "MIT",
    ),
    (
        "solarized-light",
        "https://github.com/altercation/solarized",
        "MIT",
    ),
    ("dracula", "https://github.com/dracula/dracula-theme", "MIT"),
    ("nord", "https://github.com/nordtheme/nord", "MIT"),
    ("monokai", "http://www.monokai.nl", "none stated"),
];

/// Each palette needs its own attribution row, and that row's upstream URL
/// and license must match [`EXPECTED_ATTRIBUTION`] exactly.
pub(crate) fn notice_attributes(notice: &str, name: &str) -> Result<(), String> {
    let needle = format!("`{name}`");
    let row = notice
        .lines()
        .find(|line| line.contains(&needle))
        .ok_or_else(|| format!("NOTICE.md has no entry for {name}"))?;
    let cells: Vec<&str> = row.split('|').map(str::trim).collect();
    // | palette | scheme | author | upstream | license |
    if cells.len() < 6 {
        return Err(format!(
            "NOTICE.md entry for {name} is not an attribution row"
        ));
    }
    if cells[1] != needle {
        return Err(format!(
            "NOTICE.md entry for {name} does not lead with the palette name"
        ));
    }
    let Some((_, upstream, license)) = EXPECTED_ATTRIBUTION
        .iter()
        .find(|(palette, _, _)| *palette == name)
    else {
        return Err(format!("no expected attribution for {name}"));
    };
    if cells[4] != *upstream {
        return Err(format!(
            "NOTICE.md upstream for {name} is {:?}, expected {upstream:?}",
            cells[4]
        ));
    }
    if cells[5] != *license {
        return Err(format!(
            "NOTICE.md license for {name} is {:?}, expected {license:?}",
            cells[5]
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::notice_attributes;

    const MONOKAI_ROW: &str = "| `monokai` | Monokai | Wimer Hazenberg (http://www.monokai.nl) | http://www.monokai.nl | none stated |";

    #[test]
    fn wrong_upstream_url_is_rejected() -> Result<(), String> {
        let notice = MONOKAI_ROW.replace("http://www.monokai.nl |", "https://example.test/mono |");
        match notice_attributes(&notice, "monokai") {
            Err(error) if error.contains("upstream") => Ok(()),
            Err(error) => Err(format!("expected an upstream mismatch, got {error}")),
            Ok(()) => Err(String::from("swapped URL must fail")),
        }
    }

    #[test]
    fn wrong_but_nonempty_license_is_rejected() -> Result<(), String> {
        let notice = MONOKAI_ROW.replace("none stated", "MIT");
        match notice_attributes(&notice, "monokai") {
            Err(error) if error.contains("license") => Ok(()),
            Err(error) => Err(format!("expected a license mismatch, got {error}")),
            Ok(()) => Err(String::from("invented MIT must fail")),
        }
    }
}
