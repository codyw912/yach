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
            tool_success_background: Color::Rgb(40, 40, 50),
            tool_error_background: Color::Rgb(60, 40, 40),
            tool_title: Color::White,
            tool_output: Color::Rgb(192, 192, 192),
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
