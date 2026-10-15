//! WCAG 2.x contrast over the colors a theme can name.
//!
//! Test-only: presets and palettes assert contrast here, widgets do not.

use ratatui::style::Color;

use super::{Theme, ThemeMode, ToolState};

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

/// Body text on every painted surface is at least 4.5:1. `muted`,
/// `success`, `warning`, `error`, and `harness` are at least 3:1 against
/// the tool panel. Task 2's palettes call this too.
///
/// `toolOutput` on `dark` is still `Rgb(128, 128, 128)` and does not clear
/// 4.5:1 on the pending panel. Task 5 changes that default; until then this
/// helper skips `tool_output` for `Theme::dark()` only.
pub(crate) fn assert_preset_contrast(theme: &Theme) {
    assert_eq!(theme.mode(), ThemeMode::Painted);
    let panel = theme.tool_surface(ToolState::Pending).bg.unwrap_or(Color::Reset);
    for (name, surface) in [
        ("selected", theme.selected()),
        ("user_message", theme.user_message()),
        ("tool_pending", theme.tool_surface(ToolState::Pending)),
        ("tool_success", theme.tool_surface(ToolState::Success)),
        ("tool_error", theme.tool_surface(ToolState::Error)),
    ] {
        let foreground = surface.fg.unwrap_or(Color::Reset);
        let background = surface.bg.unwrap_or(Color::Reset);
        assert_contrast_at_least(name, foreground, background, 4.5);
    }
    let text = theme.text().fg.unwrap_or(Color::Reset);
    assert_contrast_at_least("text on panel", text, panel, 4.5);
    // Dark tool output stays below the body-text bar until Task 5.
    if theme != &Theme::dark() {
        let output = theme.tool_output(ToolState::Pending).fg.unwrap_or(Color::Reset);
        assert_contrast_at_least("tool_output on panel", output, panel, 4.5);
    }
    for (name, style) in [
        ("muted", theme.muted()),
        ("success", theme.success()),
        ("warning", theme.warning()),
        ("error", theme.error()),
        ("harness", theme.harness()),
    ] {
        let foreground = style.fg.unwrap_or(Color::Reset);
        assert_contrast_at_least(name, foreground, panel, 3.0);
    }
}

fn assert_contrast_at_least(name: &str, foreground: Color, background: Color, minimum: f64) {
    let ratio = contrast_ratio(foreground, background);
    assert!(
        ratio.is_some_and(|ratio| ratio >= minimum),
        "{name} contrast {ratio:?} is below {minimum}:1 ({foreground:?} on {background:?})"
    );
}

fn relative_luminance(color: Color) -> Option<f64> {
    let (red, green, blue) = to_rgb(color)?;
    Some(0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue))
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
        // R8: ratatui's crossterm backend maps named White to Indexed(15),
        // whose xterm RGB is (255, 255, 255), not the dim gray at index 7.
        Color::White => Some((255, 255, 255)),
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
