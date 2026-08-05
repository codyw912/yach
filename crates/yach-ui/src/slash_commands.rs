#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashAction {
    Quit,
    Clear,
    Model,
    Connect,
    Session,
    Resume,
    Fork,
    Thinking,
    Compact,
    Perf,
    Edit,
    ExtensionStop,
    ExtensionReload,
    ExtensionStatus,
    Help,
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/quit",
        description: "Exit the session",
        action: SlashAction::Quit,
    },
    SlashCommand {
        name: "/exit",
        description: "Exit the session",
        action: SlashAction::Quit,
    },
    SlashCommand {
        name: "/clear",
        description: "Clear the transcript",
        action: SlashAction::Clear,
    },
    SlashCommand {
        name: "/model",
        description: "Change the model",
        action: SlashAction::Model,
    },
    SlashCommand {
        name: "/connect",
        description: "Manage provider connections",
        action: SlashAction::Connect,
    },
    SlashCommand {
        name: "/session",
        description: "Switch session",
        action: SlashAction::Session,
    },
    SlashCommand {
        name: "/resume",
        description: "Resume a recent session",
        action: SlashAction::Resume,
    },
    SlashCommand {
        name: "/fork",
        description: "Fork current session",
        action: SlashAction::Fork,
    },
    SlashCommand {
        name: "/thinking",
        description: "Change thinking level",
        action: SlashAction::Thinking,
    },
    SlashCommand {
        name: "/compact",
        description: "Compact context now, optional focus instructions",
        action: SlashAction::Compact,
    },
    SlashCommand {
        name: "/perf",
        description: "Show performance metrics",
        action: SlashAction::Perf,
    },
    SlashCommand {
        name: "/debug-edit",
        description: "Debug local edit flow",
        action: SlashAction::Edit,
    },
    SlashCommand {
        name: "/extension-stop",
        description: "Stop an active extension",
        action: SlashAction::ExtensionStop,
    },
    SlashCommand {
        name: "/extension-reload",
        description: "Reload a discovered extension",
        action: SlashAction::ExtensionReload,
    },
    SlashCommand {
        name: "/extension-status",
        description: "Show live extension status",
        action: SlashAction::ExtensionStatus,
    },
    SlashCommand {
        name: "/help",
        description: "Show available commands",
        action: SlashAction::Help,
    },
];

#[derive(Debug, Clone, Copy)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub action: SlashAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashParseResult {
    Command(SlashAction),
    CommandWithArgs { action: SlashAction, args: String },
    ArgumentsUnsupported,
    Unknown,
    NotSlash,
}

pub fn match_slash_commands(prefix: &str) -> Vec<&'static SlashCommand> {
    SLASH_COMMANDS
        .iter()
        .filter(|cmd| cmd.name.starts_with(prefix))
        .collect()
}

pub fn parse_slash_command(input: &str) -> SlashParseResult {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return SlashParseResult::NotSlash;
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return SlashParseResult::NotSlash;
    };
    let has_args = parts.next().is_some();

    let Some(command) = SLASH_COMMANDS.iter().find(|cmd| cmd.name == command) else {
        return SlashParseResult::Unknown;
    };

    if has_args {
        if matches!(
            command.action,
            SlashAction::ExtensionStop
                | SlashAction::ExtensionReload
                | SlashAction::ExtensionStatus
                | SlashAction::Compact
        ) {
            return SlashParseResult::CommandWithArgs {
                action: command.action,
                args: trimmed[command.name.len()..].trim().to_string(),
            };
        }
        SlashParseResult::ArgumentsUnsupported
    } else {
        SlashParseResult::Command(command.action)
    }
}

#[cfg(test)]
mod tests {
    use super::{SlashAction, SlashParseResult, match_slash_commands, parse_slash_command};

    #[test]
    fn completion_includes_executable_commands() {
        let matches = match_slash_commands("/");
        let names = matches.iter().map(|cmd| cmd.name).collect::<Vec<_>>();

        for expected in [
            "/quit",
            "/exit",
            "/clear",
            "/model",
            "/connect",
            "/session",
            "/resume",
            "/fork",
            "/thinking",
            "/compact",
            "/perf",
            "/debug-edit",
            "/extension-stop",
            "/extension-reload",
            "/extension-status",
            "/help",
        ] {
            assert!(names.contains(&expected));
        }
    }
    #[test]
    fn parser_accepts_connect_without_arguments_and_rejects_arguments() {
        assert!(matches!(
            parse_slash_command("/connect"),
            SlashParseResult::Command(_)
        ));
        assert_eq!(
            parse_slash_command("/connect unsupported"),
            SlashParseResult::ArgumentsUnsupported
        );
    }

    #[test]
    fn parser_accepts_debug_edit_command() {
        assert_eq!(
            parse_slash_command("/debug-edit"),
            SlashParseResult::Command(SlashAction::Edit)
        );
    }

    #[test]
    fn parser_accepts_resume_command() {
        assert_eq!(
            parse_slash_command("/resume"),
            SlashParseResult::Command(SlashAction::Resume)
        );
    }

    #[test]
    fn parser_requires_exact_commands() {
        assert_eq!(
            parse_slash_command("/clear"),
            SlashParseResult::Command(SlashAction::Clear)
        );
        assert_eq!(parse_slash_command("/clearance"), SlashParseResult::Unknown);
        assert_eq!(parse_slash_command("/quit-now"), SlashParseResult::Unknown);
    }

    #[test]
    fn parser_rejects_arguments_for_alpha_commands() {
        assert_eq!(
            parse_slash_command("/clear now"),
            SlashParseResult::ArgumentsUnsupported
        );
        assert_eq!(
            parse_slash_command("/model gpt-5"),
            SlashParseResult::ArgumentsUnsupported
        );
    }

    #[test]
    fn parser_accepts_compact_with_optional_focus_instructions() {
        assert_eq!(
            parse_slash_command("/compact"),
            SlashParseResult::Command(SlashAction::Compact)
        );
        assert_eq!(
            parse_slash_command("/compact keep the migration plan"),
            SlashParseResult::CommandWithArgs {
                action: SlashAction::Compact,
                args: String::from("keep the migration plan"),
            }
        );
    }

    #[test]
    fn parser_accepts_extension_stop_selector_argument() {
        assert_eq!(
            parse_slash_command("/extension-stop example.toy-tools"),
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionStop,
                args: String::from("example.toy-tools"),
            }
        );
    }

    #[test]
    fn parser_accepts_extension_reload_selector_argument() {
        assert_eq!(
            parse_slash_command("/extension-reload example.toy-tools"),
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionReload,
                args: String::from("example.toy-tools"),
            }
        );
    }

    #[test]
    fn parser_accepts_extension_status_with_optional_selector_argument() {
        assert_eq!(
            parse_slash_command("/extension-status"),
            SlashParseResult::Command(SlashAction::ExtensionStatus)
        );
        assert_eq!(
            parse_slash_command("/extension-status example.toy-tools"),
            SlashParseResult::CommandWithArgs {
                action: SlashAction::ExtensionStatus,
                args: String::from("example.toy-tools"),
            }
        );
    }
}
