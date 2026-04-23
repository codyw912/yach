pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/quit",
        description: "Exit the session",
    },
    SlashCommand {
        name: "/clear",
        description: "Clear the transcript",
    },
    SlashCommand {
        name: "/model",
        description: "Change the model",
    },
    SlashCommand {
        name: "/session",
        description: "Switch session",
    },
    SlashCommand {
        name: "/help",
        description: "Show available commands",
    },
];

pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
}

pub fn match_slash_commands(prefix: &str) -> Vec<&SlashCommand> {
    SLASH_COMMANDS
        .iter()
        .filter(|cmd| cmd.name.starts_with(prefix))
        .collect()
}
