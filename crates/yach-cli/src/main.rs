use std::io::{self, Write};

use yach_adapter_pi_rpc::{
    AdapterCapabilities, ParseError, PiCommand, PiRpcSession, SessionError,
    negotiate_with as negotiate_with_rpc, parse_server_line, serialize_client_message,
    stock_rpc_handshake,
};
use yach_proto::{
    Capability, ClientEvent, DialogResponse, Handshake, MessageMeta, TransportMessage,
};
use yach_ui::{UiCapabilities, alpha_handshake, negotiate_with as negotiate_with_ui};

fn main() {
    let command = Command::from_args(std::env::args().skip(1));
    let result = command.run();
    let _emitted = emit_lines(&result.render_lines());
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    BootstrapStub,
    PrintCapabilities,
    SmokePiRpc,
}

impl Command {
    fn from_args(mut args: impl Iterator<Item = String>) -> Self {
        match args.next().as_deref() {
            Some("print-capabilities") => Self::PrintCapabilities,
            Some("smoke-pi-rpc") => Self::SmokePiRpc,
            _ => Self::BootstrapStub,
        }
    }

    fn run(&self) -> CommandResult {
        match self {
            Self::BootstrapStub => run_bootstrap_stub(),
            Self::PrintCapabilities => print_capabilities(),
            Self::SmokePiRpc => run_smoke_bootstrap(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandResult {
    BootstrapStub { ready: bool },
    Capabilities { capabilities: Vec<Capability> },
    SmokePiRpc {
        outcome: SmokeOutcome,
        operations: Vec<SmokeOperation>,
    },
}

impl CommandResult {
    fn render_lines(&self) -> Vec<String> {
        match self {
            Self::BootstrapStub { ready } => vec![format!("bootstrap_stub_ready={ready}")],
            Self::Capabilities { capabilities } => capabilities
                .iter()
                .map(|capability| format!("capability={capability:?}"))
                .collect(),
            Self::SmokePiRpc {
                outcome,
                operations,
            } => {
                let mut lines = vec![format!("smoke_outcome={outcome:?}")];
                lines.extend(operations.iter().map(SmokeOperation::render_line));
                lines
            }
        }
    }
}

fn emit_lines(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    for line in lines {
        handle.write_all(line.as_bytes())?;
        handle.write_all(b"\n")?;
    }

    handle.flush()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SmokeOutcome {
    SpawnFailed,
    Initialized,
    InitializationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SmokeOperation {
    Initialize { success: bool },
    GetState { success: bool },
    SelectModel { success: bool },
    ForkSession { success: bool },
    GetSessionStats { success: bool },
    GetMessages { success: bool },
    ResolveDialog { success: bool },
}

impl SmokeOperation {
    fn render_line(&self) -> String {
        match self {
            Self::Initialize { success } => format!("operation=initialize success={success}"),
            Self::GetState { success } => format!("operation=get_state success={success}"),
            Self::SelectModel { success } => format!("operation=select_model success={success}"),
            Self::ForkSession { success } => format!("operation=fork_session success={success}"),
            Self::GetSessionStats { success } => {
                format!("operation=get_session_stats success={success}")
            }
            Self::GetMessages { success } => format!("operation=get_messages success={success}"),
            Self::ResolveDialog { success } => {
                format!("operation=resolve_dialog success={success}")
            }
        }
    }
}

fn run_bootstrap_stub() -> CommandResult {
    let ui_capabilities = UiCapabilities::alpha();
    let adapter_capabilities = AdapterCapabilities::stock_rpc();
    let ui_handshake = alpha_handshake();
    let adapter_handshake = stock_rpc_handshake();
    let ui_negotiation = negotiate_with_ui(&adapter_handshake);
    let adapter_negotiation = negotiate_with_rpc(&ui_handshake);
    let bootstrap_message = TransportMessage::client(
        MessageMeta::new("bootstrap-1").with_correlation_id("session-bootstrap"),
        ClientEvent::Initialize(ui_handshake.clone()),
    );
    let bootstrap_line = serialize_client_message(&bootstrap_message);
    let parsed_ready = parse_server_line(r#"{"method":"ready","params":{}}"#, "server-1");

    let ready = ui_capabilities.supports(Capability::PromptStreaming)
        && adapter_capabilities.supports(Capability::PromptStreaming)
        && ui_handshake.supports(Capability::Dialogs)
        && adapter_handshake.supports(Capability::Dialogs)
        && ui_negotiation.supports(Capability::PromptStreaming)
        && adapter_negotiation.supports(Capability::Dialogs)
        && bootstrap_line.is_ok()
        && parsed_ready.is_ok();

    CommandResult::BootstrapStub { ready }
}

fn print_capabilities() -> CommandResult {
    let handshake = stock_rpc_handshake();
    CommandResult::Capabilities {
        capabilities: handshake.capabilities,
    }
}

fn run_smoke_bootstrap() -> CommandResult {
    let handshake = alpha_handshake();

    match PiRpcSession::spawn(PiCommand::stock_rpc()) {
        Ok(mut session) => {
            let (outcome, operations) = smoke_session(&mut session, &handshake);
            CommandResult::SmokePiRpc {
                outcome,
                operations,
            }
        }
        Err(_) => CommandResult::SmokePiRpc {
            outcome: SmokeOutcome::SpawnFailed,
            operations: vec![SmokeOperation::Initialize { success: false }],
        },
    }
}

fn smoke_session(
    session: &mut PiRpcSession,
    handshake: &Handshake,
) -> (SmokeOutcome, Vec<SmokeOperation>) {
    match session.initialize(handshake.clone()) {
        Ok(_) => {
            let model_message = TransportMessage::client(
                    MessageMeta::new("smoke-model-1"),
                    ClientEvent::ModelSelected {
                        model: String::from("gpt-5"),
                    },
                );
            let model_success = send_smoke_message(session, &model_message);

            let get_state_success = send_raw_smoke_line(
                session,
                "get_state",
                &[],
            );

            let fork_message = TransportMessage::client(
                    MessageMeta::new("smoke-fork-1"),
                    ClientEvent::SessionForkRequested {
                        session_id: String::from("current"),
                    },
                );
            let fork_success = send_smoke_message(session, &fork_message);

            let get_stats_success = send_raw_smoke_line(
                session,
                "get_session_stats",
                &[],
            );

            let get_messages_success = send_raw_smoke_line(
                session,
                "get_messages",
                &[],
            );

            let dialog_message = TransportMessage::client(
                    MessageMeta::new("smoke-dialog-1"),
                    ClientEvent::DialogResolved {
                        dialog_id: String::from("smoke-dialog"),
                        response: DialogResponse::Confirmed { accepted: true },
                    },
                );
            let dialog_success = send_smoke_message(session, &dialog_message);

            (
                SmokeOutcome::Initialized,
                vec![
                    SmokeOperation::Initialize { success: true },
                    SmokeOperation::GetState {
                        success: get_state_success,
                    },
                    SmokeOperation::SelectModel {
                        success: model_success,
                    },
                    SmokeOperation::ForkSession {
                        success: fork_success,
                    },
                    SmokeOperation::GetSessionStats {
                        success: get_stats_success,
                    },
                    SmokeOperation::GetMessages {
                        success: get_messages_success,
                    },
                    SmokeOperation::ResolveDialog {
                        success: dialog_success,
                    },
                ],
            )
        }
        Err(SessionError::Spawn(_) | SessionError::MissingStdin | SessionError::MissingStdout) => (
            SmokeOutcome::SpawnFailed,
            vec![SmokeOperation::Initialize { success: false }],
        ),
        Err(_) => (
            SmokeOutcome::InitializationFailed,
            vec![SmokeOperation::Initialize { success: false }],
        ),
    }
}

fn send_smoke_message(session: &mut PiRpcSession, message: &TransportMessage) -> bool {
    session.send(message).is_ok()
}

fn send_raw_smoke_line(
    session: &mut PiRpcSession,
    command_type: &str,
    fields: &[(&str, &str)],
) -> bool {
    let Ok(request_id) = session.send_rpc_command(command_type, fields) else {
        return false;
    };

    read_until_response(session, &request_id).unwrap_or(false)
}

fn read_until_response(session: &mut PiRpcSession, request_id: &str) -> Result<bool, ParseError> {
    loop {
        let message = session.read_next().map_err(map_session_parse_error)?;
        if message.meta.correlation_id.as_deref() == Some(request_id) {
            return Ok(true);
        }
    }
}

fn map_session_parse_error(error: SessionError) -> ParseError {
    match error {
        SessionError::Parse(parse_error) => parse_error,
        SessionError::EndOfStream => ParseError::EmptyLine,
        other => ParseError::InvalidJson(format!("session_error:{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Command, CommandResult, SmokeOperation, SmokeOutcome, emit_lines, print_capabilities,
        run_bootstrap_stub,
    };

    #[test]
    fn command_parsing_defaults_to_bootstrap_stub() {
        let command = Command::from_args(std::iter::empty());

        assert_eq!(command, Command::BootstrapStub);
    }

    #[test]
    fn command_parsing_recognizes_supported_commands() {
        let print = Command::from_args([String::from("print-capabilities")].into_iter());
        let smoke = Command::from_args([String::from("smoke-pi-rpc")].into_iter());

        assert_eq!(print, Command::PrintCapabilities);
        assert_eq!(smoke, Command::SmokePiRpc);
    }

    #[test]
    fn bootstrap_stub_reports_ready_state() {
        let result = run_bootstrap_stub();

        assert_eq!(result, CommandResult::BootstrapStub { ready: true });
    }

    #[test]
    fn print_capabilities_returns_adapter_capabilities() {
        let result = print_capabilities();

        let CommandResult::Capabilities { capabilities } = result else {
            unreachable!();
        };
        assert!(!capabilities.is_empty());
    }

    #[test]
    fn smoke_outcome_has_stable_variants() {
        assert_eq!(SmokeOutcome::SpawnFailed, SmokeOutcome::SpawnFailed);
        assert_ne!(SmokeOutcome::SpawnFailed, SmokeOutcome::Initialized);
    }

    #[test]
    fn rendered_capabilities_are_stable() {
        let lines = print_capabilities().render_lines();

        assert!(!lines.is_empty());
        assert!(lines[0].starts_with("capability="));
    }

    #[test]
    fn rendered_smoke_results_include_operations() {
        let result = CommandResult::SmokePiRpc {
            outcome: SmokeOutcome::Initialized,
            operations: vec![
                SmokeOperation::Initialize { success: true },
                SmokeOperation::SelectModel { success: true },
            ],
        };

        let lines = result.render_lines();

        assert_eq!(lines[0], "smoke_outcome=Initialized");
        assert!(lines[1].contains("operation=initialize"));
        assert!(lines[2].contains("operation=select_model"));
    }

    #[test]
    fn emit_lines_accepts_rendered_output() {
        let lines = vec![String::from("alpha"), String::from("beta")];

        assert!(emit_lines(&lines).is_ok());
    }
}
