#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashAction {
    Quit,
    Clear,
    Model,
    Session,
    Fork,
    Thinking,
    Perf,
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
        name: "/session",
        description: "Switch session",
        action: SlashAction::Session,
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
        name: "/perf",
        description: "Show performance metrics",
        action: SlashAction::Perf,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashParseResult {
    Command(SlashAction),
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
        SlashParseResult::ArgumentsUnsupported
    } else {
        SlashParseResult::Command(command.action)
    }
}

pub fn slash_help_text() -> String {
    let commands = SLASH_COMMANDS
        .iter()
        .map(|command| command.name)
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "Commands: {commands} | Alt+M/F2: models | Ctrl+S: sessions | Ctrl+F: fork | Ctrl+T: thinking | Ctrl+P: perf"
    )
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
            "/session",
            "/fork",
            "/thinking",
            "/perf",
            "/help",
        ] {
            assert!(names.contains(&expected));
        }
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
}
