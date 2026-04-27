use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use yach_adapter_pi_rpc::{
    AdapterCapabilities, ParseError, PiCommand, PiRpcReader, PiRpcSession, PiRpcWriter,
    SessionError, negotiate_with as negotiate_with_rpc, parse_server_line,
    serialize_client_message, stock_rpc_handshake,
};
use yach_proto::{
    BackendEvent, Capability, ClientEvent, DialogKind, DialogRequest, DialogResponse, ForkPosition,
    Handshake, MessageBody, MessageMeta, ServerEvent, TransportMessage,
};
use yach_ui::{UiCapabilities, alpha_handshake, negotiate_with as negotiate_with_ui, run_tui};

fn main() {
    let cli = CliArgs::from_args(std::env::args().skip(1));
    let result = cli.command.run(cli.quiet);
    let _emitted = emit_lines(&result.render_lines());
}

const PROMPT_SMOKE_TEXT: &str = "Reply with exactly: yach-smoke-ok";
const TOOL_SMOKE_TEXT: &str =
    "Use a read-only tool to inspect the current directory, then reply with exactly: tool-smoke-ok";
const PROMPT_SMOKE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    command: Command,
    quiet: bool,
}

impl CliArgs {
    fn from_args(args: impl Iterator<Item = String>) -> Self {
        let mut quiet = false;
        let mut positional = Vec::new();

        for arg in args {
            match arg.as_str() {
                "--version" | "-V" => {
                    return Self {
                        command: Command::Version,
                        quiet: false,
                    };
                }
                "--quiet" | "-q" => quiet = true,
                _ => positional.push(arg),
            }
        }

        let command = match positional.first().map(String::as_str) {
            Some("print-capabilities") => Command::PrintCapabilities,
            Some("smoke-pi-rpc") => Command::SmokePiRpc,
            Some("smoke-pi-rpc-prompt") => Command::SmokePiRpcPrompt,
            Some("smoke-pi-rpc-tool") => Command::SmokePiRpcTool,
            Some("run") => Command::Run,
            Some("tui") => Command::Tui,
            Some("tui-dialog-smoke") => Command::TuiDialogSmoke,
            Some("tui-bench-ready") => Command::TuiBenchReady,
            _ => Command::BootstrapStub,
        };

        Self { command, quiet }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Version,
    BootstrapStub,
    PrintCapabilities,
    SmokePiRpc,
    SmokePiRpcPrompt,
    SmokePiRpcTool,
    Run,
    Tui,
    TuiDialogSmoke,
    TuiBenchReady,
}

impl Command {
    fn run(&self, _quiet: bool) -> CommandResult {
        match self {
            Self::Version => CommandResult::Version,
            Self::BootstrapStub => run_bootstrap_stub(),
            Self::PrintCapabilities => print_capabilities(),
            Self::SmokePiRpc => run_smoke_bootstrap(),
            Self::SmokePiRpcPrompt => run_prompt_smoke(),
            Self::SmokePiRpcTool => run_tool_smoke(),
            Self::Run => run_interactive_session(),
            Self::Tui => run_tui_command(),
            Self::TuiDialogSmoke => run_tui_dialog_smoke_command(),
            Self::TuiBenchReady => run_tui_bench_ready_command(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandResult {
    Version,
    BootstrapStub {
        ready: bool,
    },
    Capabilities {
        capabilities: Vec<Capability>,
    },
    SmokePiRpc {
        outcome: SmokeOutcome,
        operations: Vec<SmokeOperation>,
    },
    PromptSmoke {
        outcome: PromptSmokeOutcome,
        saw_delta: bool,
        saw_tool_start: bool,
        saw_tool_finish: bool,
        completed: bool,
        response_chars: usize,
    },
    InteractiveSession {
        exited: bool,
        transcript_entries: usize,
    },
    Tui {
        exited: bool,
    },
}

impl CommandResult {
    fn render_lines(&self) -> Vec<String> {
        match self {
            Self::Version => vec![format!("yach {}", env!("CARGO_PKG_VERSION"))],
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
            Self::PromptSmoke {
                outcome,
                saw_delta,
                saw_tool_start,
                saw_tool_finish,
                completed,
                response_chars,
            } => vec![
                format!("prompt_smoke_outcome={outcome:?}"),
                format!("saw_delta={saw_delta}"),
                format!("saw_tool_start={saw_tool_start}"),
                format!("saw_tool_finish={saw_tool_finish}"),
                format!("completed={completed}"),
                format!("response_chars={response_chars}"),
            ],
            Self::InteractiveSession {
                exited,
                transcript_entries,
            } => vec![
                format!("interactive_session_exited={exited}"),
                format!("transcript_entries={transcript_entries}"),
            ],
            Self::Tui { exited } => vec![format!("tui_exited={exited}")],
        }
    }
}

fn emit_lines(lines: &[String]) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    write_lines(&mut handle, lines)
}

fn write_lines(writer: &mut impl Write, lines: &[String]) -> io::Result<()> {
    for line in lines {
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
    }

    writer.flush()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SmokeOutcome {
    SpawnFailed,
    Initialized,
    InitializationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptSmokeOutcome {
    SpawnFailed,
    InitializationFailed,
    SendFailed,
    ReadFailed,
    Timeout,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PromptSmokeStats {
    flags: u8,
    response_chars: usize,
}

impl PromptSmokeStats {
    const SAW_DELTA: u8 = 1;
    const SAW_TOOL_START: u8 = 1 << 1;
    const SAW_TOOL_FINISH: u8 = 1 << 2;
    const COMPLETED: u8 = 1 << 3;

    fn mark_saw_delta(&mut self) {
        self.flags |= Self::SAW_DELTA;
    }

    fn mark_saw_tool_start(&mut self) {
        self.flags |= Self::SAW_TOOL_START;
    }

    fn mark_saw_tool_finish(&mut self) {
        self.flags |= Self::SAW_TOOL_FINISH;
    }

    fn mark_completed(&mut self) {
        self.flags |= Self::COMPLETED;
    }

    fn saw_delta(self) -> bool {
        self.flags & Self::SAW_DELTA != 0
    }

    fn saw_tool_start(self) -> bool {
        self.flags & Self::SAW_TOOL_START != 0
    }

    fn saw_tool_finish(self) -> bool {
        self.flags & Self::SAW_TOOL_FINISH != 0
    }

    fn completed(self) -> bool {
        self.flags & Self::COMPLETED != 0
    }
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

fn run_prompt_smoke() -> CommandResult {
    run_turn_smoke(PROMPT_SMOKE_TEXT)
}

fn run_tool_smoke() -> CommandResult {
    run_turn_smoke(TOOL_SMOKE_TEXT)
}

fn run_turn_smoke(prompt: &str) -> CommandResult {
    let ui_handshake = alpha_handshake();

    let Ok(mut session) = PiRpcSession::spawn(PiCommand::stock_rpc()) else {
        return prompt_smoke_result(PromptSmokeOutcome::SpawnFailed, PromptSmokeStats::default());
    };

    if session.initialize(ui_handshake).is_err() {
        return prompt_smoke_result(
            PromptSmokeOutcome::InitializationFailed,
            PromptSmokeStats::default(),
        );
    }

    let (mut child, reader, mut writer) = session.into_split();
    let (tx, rx) = std::sync::mpsc::channel();
    let _reader_handle = std::thread::spawn(move || prompt_smoke_reader(reader, &tx));

    let sent = writer.send_event(ClientEvent::PromptSubmitted {
        session_id: String::from("active"),
        prompt: String::from(prompt),
    });

    if sent.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return prompt_smoke_result(PromptSmokeOutcome::SendFailed, PromptSmokeStats::default());
    }

    let result = read_prompt_smoke_events(&rx, &mut writer, PROMPT_SMOKE_TIMEOUT);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn prompt_smoke_reader(
    mut reader: PiRpcReader<std::process::ChildStdout>,
    tx: &std::sync::mpsc::Sender<Result<TransportMessage, SessionError>>,
) {
    loop {
        let message = reader.read_next();
        let should_continue = message.is_ok();
        if tx.send(message).is_err() || !should_continue {
            break;
        }
    }
}

fn read_prompt_smoke_events(
    rx: &std::sync::mpsc::Receiver<Result<TransportMessage, SessionError>>,
    writer: &mut PiRpcWriter<std::process::ChildStdin>,
    timeout: Duration,
) -> CommandResult {
    let deadline = Instant::now() + timeout;
    let mut stats = PromptSmokeStats::default();

    loop {
        let now = Instant::now();
        if now >= deadline {
            return prompt_smoke_result(PromptSmokeOutcome::Timeout, stats);
        }

        match rx.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok(Ok(message)) => {
                let MessageBody::ServerEvent(event) = message.body else {
                    continue;
                };
                match event {
                    ServerEvent::PromptDelta { delta, .. } => {
                        stats.mark_saw_delta();
                        stats.response_chars += delta.len();
                    }
                    ServerEvent::ToolCallStarted { .. } => {
                        stats.mark_saw_tool_start();
                    }
                    ServerEvent::ToolCallFinished(_) => {
                        stats.mark_saw_tool_finish();
                    }
                    ServerEvent::StatusUpdated { message } if message.starts_with("agent_end") => {
                        stats.mark_completed();
                        return prompt_smoke_result(PromptSmokeOutcome::Completed, stats);
                    }
                    ServerEvent::DialogRequested(request) => {
                        let _ = writer.send_event(ClientEvent::DialogResolved {
                            dialog_id: request.id.unwrap_or_default(),
                            response: DialogResponse::Cancelled,
                        });
                    }
                    _ => {}
                }
            }
            Ok(Err(_)) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return prompt_smoke_result(PromptSmokeOutcome::ReadFailed, stats);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return prompt_smoke_result(PromptSmokeOutcome::Timeout, stats);
            }
        }
    }
}

fn prompt_smoke_result(outcome: PromptSmokeOutcome, stats: PromptSmokeStats) -> CommandResult {
    CommandResult::PromptSmoke {
        outcome,
        saw_delta: stats.saw_delta(),
        saw_tool_start: stats.saw_tool_start(),
        saw_tool_finish: stats.saw_tool_finish(),
        completed: stats.completed(),
        response_chars: stats.response_chars,
    }
}

fn run_interactive_session() -> CommandResult {
    let handshake = alpha_handshake();

    let Ok(mut session) = PiRpcSession::spawn(PiCommand::stock_rpc()) else {
        let _ = writeln!(io::stderr(), "failed to spawn pi --mode rpc");
        return CommandResult::InteractiveSession {
            exited: true,
            transcript_entries: 0,
        };
    };

    if session.initialize(handshake).is_err() {
        let _ = writeln!(io::stderr(), "failed to initialize pi rpc session");
        return CommandResult::InteractiveSession {
            exited: true,
            transcript_entries: 0,
        };
    }

    let mut transcript_entries = 0;
    let stdin = io::stdin();

    let _ = writeln!(io::stdout(), "yach session started. type /quit to exit.");
    let _ = writeln!(io::stdout(), "---");
    let _ = io::stdout().flush();

    loop {
        let _ = write!(io::stdout(), "> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "/quit" || trimmed == "/exit" {
            break;
        }

        transcript_entries += 1;

        if session.submit_prompt("active", trimmed).is_err() {
            let _ = writeln!(io::stderr(), "failed to submit prompt");
            break;
        }

        loop {
            match session.read_next() {
                Ok(message) => {
                    let yach_proto::MessageBody::ServerEvent(event) = message.body else {
                        continue;
                    };

                    match event {
                        ServerEvent::PromptDelta { delta, .. } => {
                            let _ = write!(io::stdout(), "{delta}");
                            let _ = io::stdout().flush();
                        }
                        ServerEvent::DialogRequested(request) => {
                            let _ = writeln!(io::stdout());
                            let _ = writeln!(
                                io::stdout(),
                                "[dialog] {}",
                                request.prompt.as_deref().unwrap_or("")
                            );
                            if let DialogKind::Select { options } = &request.kind {
                                for (i, opt) in options.iter().enumerate() {
                                    let _ = writeln!(io::stdout(), "  {i}: {}", opt.label);
                                }
                            }
                            let _ = write!(io::stdout(), "> ");
                            let _ = io::stdout().flush();

                            let mut response_line = String::new();
                            let _ = stdin.lock().read_line(&mut response_line);
                            let response = resolve_dialog_response(&request, response_line.trim());

                            let dialog_message = TransportMessage::client(
                                MessageMeta::new("dialog-response-1"),
                                ClientEvent::DialogResolved {
                                    dialog_id: request.id.clone().unwrap_or_default(),
                                    response,
                                },
                            );
                            let _ = session.send(&dialog_message);
                        }
                        ServerEvent::StatusUpdated { message } => {
                            if message.starts_with("agent_end") || message.starts_with("turn_end") {
                                let _ = writeln!(io::stdout());
                                let _ = writeln!(io::stdout(), "---");
                                break;
                            }
                        }
                        ServerEvent::ToolCallStarted {
                            tool_name, preview, ..
                        } => {
                            let label = match preview {
                                Some(preview) if !preview.is_empty() => {
                                    format!("{tool_name} {preview}")
                                }
                                _ => tool_name,
                            };
                            let _ = writeln!(io::stdout(), "\n[tool: {label}]");
                        }
                        ServerEvent::ToolCallFinished(result) => {
                            let status = if result.is_error { "error" } else { "ok" };
                            let _ = writeln!(
                                io::stdout(),
                                "\n[tool result: {} {status}]",
                                result.tool_name
                            );
                        }
                        ServerEvent::SessionChanged { session_id } => {
                            let _ = writeln!(io::stdout(), "\n[session: {session_id}]");
                        }
                        ServerEvent::AvailableModelsUpdated { models } => {
                            let _ =
                                writeln!(io::stdout(), "\n[models: {} available]", models.len());
                        }
                        ServerEvent::ForkMessagesUpdated { messages } => {
                            let _ = writeln!(io::stdout(), "\n[fork points: {}]", messages.len());
                        }
                        ServerEvent::SessionMessagesUpdated { messages } => {
                            let _ = writeln!(io::stdout(), "\n[messages: {}]", messages.len());
                        }
                        ServerEvent::SessionStatsUpdated(stats) => {
                            if let Some(count) = stats.message_count {
                                let _ = writeln!(io::stdout(), "\n[session messages: {count}]");
                            }
                        }
                        ServerEvent::ModelChanged { model } => {
                            let _ = writeln!(io::stdout(), "\n[model: {model}]");
                        }
                        ServerEvent::StateUpdated(state) => {
                            let model = match (&state.model_provider, &state.model_id) {
                                (Some(provider), Some(id)) => Some(format!("{provider}/{id}")),
                                _ => state.model_name.clone().or_else(|| state.model_id.clone()),
                            };
                            if let Some(model) = model {
                                let _ = writeln!(io::stdout(), "\n[model: {model}]");
                            }
                            if let Some(session_id) = state.session_id {
                                let _ = writeln!(io::stdout(), "\n[session: {session_id}]");
                            }
                        }
                        ServerEvent::TitleChanged { title } => {
                            let _ = writeln!(io::stdout(), "\n[title: {title}]");
                        }
                        ServerEvent::NotificationRaised(notification) => {
                            let _ = writeln!(
                                io::stdout(),
                                "\n[{}] {}",
                                notification.level,
                                notification.message
                            );
                        }
                        ServerEvent::WidgetUpdated(widget) => {
                            let _ = writeln!(io::stdout(), "\n[widget: {}]", widget.title);
                        }
                        ServerEvent::Ready { .. } => {}
                    }
                }
                Err(SessionError::EndOfStream) => break,
                Err(_) => {}
            }
        }
    }

    CommandResult::InteractiveSession {
        exited: true,
        transcript_entries,
    }
}

fn resolve_dialog_response(request: &DialogRequest, input: &str) -> DialogResponse {
    match &request.kind {
        DialogKind::Confirm => DialogResponse::Confirmed {
            accepted: matches!(input.to_lowercase().as_str(), "y" | "yes" | "true"),
        },
        DialogKind::Input { .. } | DialogKind::Editor { .. } => DialogResponse::Text {
            value: input.to_owned(),
        },
        DialogKind::Select { options } => {
            if options.is_empty() {
                return DialogResponse::Cancelled;
            }

            let trimmed = input.trim();
            if let Ok(index) = trimmed.parse::<usize>()
                && let Some(option) = options.get(index)
            {
                return DialogResponse::Selection {
                    value: option.value.clone(),
                };
            }

            let value = options
                .iter()
                .find(|option| option.label.eq_ignore_ascii_case(trimmed))
                .unwrap_or(&options[0])
                .value
                .clone();
            DialogResponse::Selection { value }
        }
    }
}

fn run_tui_dialog_smoke_command() -> CommandResult {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {error}");
            return CommandResult::Tui { exited: true };
        }
    };

    match runtime.block_on(async move {
        let (client_tx, mut client_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let (backend_tx, backend_rx) = mpsc::unbounded_channel::<BackendEvent>();

        tokio::spawn(async move {
            let negotiated = negotiate_with_ui(&stock_rpc_handshake());
            let _ = backend_tx.send(BackendEvent::Connected { negotiated });
            let _ = backend_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: String::from("dialog smoke: confirm/input/select/editor"),
            }));

            for request in dialog_smoke_requests() {
                let title = request
                    .title
                    .clone()
                    .unwrap_or_else(|| String::from("dialog"));
                let _ =
                    backend_tx.send(BackendEvent::Server(ServerEvent::DialogRequested(request)));
                while let Some(event) = client_rx.recv().await {
                    if matches!(event, ClientEvent::DialogResolved { .. }) {
                        let _ = backend_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: format!("resolved: {title}"),
                        }));
                        break;
                    }
                }
            }

            let _ = backend_tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: String::from("dialog smoke complete; Ctrl+C to quit"),
            }));
        });

        run_tui(client_tx, backend_rx).await
    }) {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(error) => {
            let _ = writeln!(io::stderr(), "tui dialog smoke error: {error}");
            CommandResult::Tui { exited: true }
        }
    }
}

fn dialog_smoke_requests() -> Vec<DialogRequest> {
    vec![
        DialogRequest {
            id: Some(String::from("smoke-confirm")),
            title: Some(String::from("Confirm smoke")),
            prompt: Some(String::from("Confirm dialog works?")),
            kind: DialogKind::Confirm,
        },
        DialogRequest {
            id: Some(String::from("smoke-input")),
            title: Some(String::from("Input smoke")),
            prompt: Some(String::from("Type Unicode, move cursor, then submit.")),
            kind: DialogKind::Input {
                default: Some(String::from("🙂é")),
            },
        },
        DialogRequest {
            id: Some(String::from("smoke-select")),
            title: Some(String::from("Select smoke")),
            prompt: Some(String::from("Pick an option.")),
            kind: DialogKind::Select {
                options: vec![
                    yach_proto::DialogOption {
                        label: String::from("Alpha"),
                        value: String::from("alpha"),
                    },
                    yach_proto::DialogOption {
                        label: String::from("Beta"),
                        value: String::from("beta"),
                    },
                ],
            },
        },
        DialogRequest {
            id: Some(String::from("smoke-editor")),
            title: Some(String::from("Editor smoke")),
            prompt: Some(String::from(
                "Edit text, use Ctrl+J for newline, submit with Enter.",
            )),
            kind: DialogKind::Editor {
                initial_text: Some(String::from("line one")),
            },
        },
    ]
}

fn run_tui_bench_ready_command() -> CommandResult {
    let ui_handshake = alpha_handshake();
    let adapter_handshake = stock_rpc_handshake();
    let negotiated = negotiate_with_ui(&adapter_handshake);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };

    match runtime.block_on(async move {
        let (client_tx, _client_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let (backend_tx, backend_rx) = mpsc::unbounded_channel::<BackendEvent>();
        let _ = backend_tx.send(BackendEvent::Connected { negotiated });
        let _ = backend_tx.send(BackendEvent::Server(ServerEvent::StateUpdated(
            yach_proto::BackendState {
                model_id: Some(String::from("bench-model")),
                model_name: Some(String::from("Bench Model")),
                model_provider: Some(String::from("bench")),
                session_id: Some(String::from("bench-session")),
                session_file: None,
                thinking_level: Some(String::from("low")),
                is_streaming: false,
                is_compacting: false,
                message_count: Some(0),
                pending_message_count: Some(0),
            },
        )));
        let _ = client_tx.send(ClientEvent::Initialize(ui_handshake));
        run_tui(client_tx, backend_rx).await
    }) {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(e) => {
            let _ = writeln!(io::stderr(), "tui error: {e}");
            CommandResult::Tui { exited: true }
        }
    }
}

fn run_tui_command() -> CommandResult {
    let ui_handshake = alpha_handshake();

    let Ok(mut session) = PiRpcSession::spawn(PiCommand::stock_rpc()) else {
        let _ = writeln!(io::stderr(), "failed to spawn pi --mode rpc");
        return CommandResult::Tui { exited: true };
    };

    let adapter_handshake = match session.initialize(ui_handshake.clone()) {
        Ok(handshake) => handshake,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "failed to initialize pi rpc session: {error:?}"
            );
            return CommandResult::Tui { exited: true };
        }
    };
    let negotiated = negotiate_with_ui(&adapter_handshake);
    let (mut child, reader, writer) = session.into_split();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };

    match runtime.block_on(async move {
        let (client_tx, client_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let (backend_tx, backend_rx) = mpsc::unbounded_channel::<BackendEvent>();
        let _ = backend_tx.send(BackendEvent::Connected { negotiated });
        let _ = client_tx.send(ClientEvent::Initialize(ui_handshake));

        let reader_tx = backend_tx.clone();
        let writer_tx = backend_tx.clone();
        let reader_handle =
            tokio::task::spawn_blocking(move || bridge_reader_loop(reader, &reader_tx));
        let writer_handle =
            tokio::task::spawn_blocking(move || bridge_writer_loop(writer, client_rx, &writer_tx));

        let ui_result = run_tui(client_tx, backend_rx).await;

        let _ = child.kill();
        let _ = child.wait();
        let _ = reader_handle.await;
        let _ = writer_handle.await;

        ui_result
    }) {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(e) => {
            let _ = writeln!(io::stderr(), "tui error: {e}");
            CommandResult::Tui { exited: true }
        }
    }
}

fn bridge_reader_loop(
    mut reader: PiRpcReader<std::process::ChildStdout>,
    tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    loop {
        match reader.read_next() {
            Ok(message) => {
                let MessageBody::ServerEvent(event) = message.body else {
                    continue;
                };

                if tx.send(BackendEvent::Server(event)).is_err() {
                    break;
                }
            }
            Err(SessionError::EndOfStream) => {
                let _ = tx.send(BackendEvent::Disconnected {
                    reason: String::from("backend exited"),
                });
                break;
            }
            Err(error) => {
                let _ = tx.send(BackendEvent::Disconnected {
                    reason: format!("backend error: {error:?}"),
                });
                break;
            }
        }
    }
}

fn bridge_writer_loop(
    mut writer: PiRpcWriter<std::process::ChildStdin>,
    mut rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: &mpsc::UnboundedSender<BackendEvent>,
) {
    while let Some(event) = rx.blocking_recv() {
        if let Err(error) = writer.send_event(event) {
            let _ = tx.send(BackendEvent::Disconnected {
                reason: format!("backend write failed: {error:?}"),
            });
            break;
        }
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
                ClientEvent::ModelSelectedDetailed {
                    provider: String::from("openai"),
                    model_id: String::from("gpt-5"),
                },
            );
            let model_success = send_smoke_message(session, &model_message);

            let get_state_success = send_raw_smoke_line(session, "get_state", &[]);

            let fork_message = TransportMessage::client(
                MessageMeta::new("smoke-fork-1"),
                ClientEvent::SessionForkRequested {
                    session_id: String::from("current"),
                    entry_id: None,
                    position: ForkPosition::Before,
                },
            );
            let fork_success = send_smoke_message(session, &fork_message);

            let get_stats_success = send_raw_smoke_line(session, "get_session_stats", &[]);

            let get_messages_success = send_raw_smoke_line(session, "get_messages", &[]);

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
        CliArgs, Command, CommandResult, PromptSmokeOutcome, SmokeOperation, SmokeOutcome,
        dialog_smoke_requests, print_capabilities, run_bootstrap_stub,
    };

    #[test]
    fn cli_defaults_to_bootstrap_stub() {
        let cli = CliArgs::from_args(std::iter::empty());

        assert_eq!(cli.command, Command::BootstrapStub);
        assert!(!cli.quiet);
    }

    #[test]
    fn cli_parses_supported_commands() {
        let print = CliArgs::from_args([String::from("print-capabilities")].into_iter());
        let smoke = CliArgs::from_args([String::from("smoke-pi-rpc")].into_iter());
        let prompt_smoke = CliArgs::from_args([String::from("smoke-pi-rpc-prompt")].into_iter());
        let dialog_smoke = CliArgs::from_args([String::from("tui-dialog-smoke")].into_iter());
        let run = CliArgs::from_args([String::from("run")].into_iter());

        assert_eq!(print.command, Command::PrintCapabilities);
        assert_eq!(smoke.command, Command::SmokePiRpc);
        assert_eq!(prompt_smoke.command, Command::SmokePiRpcPrompt);
        assert_eq!(dialog_smoke.command, Command::TuiDialogSmoke);
        assert_eq!(run.command, Command::Run);
    }

    #[test]
    fn dialog_smoke_requests_cover_all_dialog_kinds() {
        let requests = dialog_smoke_requests();
        let kinds = requests
            .iter()
            .map(|request| match &request.kind {
                yach_proto::DialogKind::Confirm => "confirm",
                yach_proto::DialogKind::Input { .. } => "input",
                yach_proto::DialogKind::Select { .. } => "select",
                yach_proto::DialogKind::Editor { .. } => "editor",
            })
            .collect::<Vec<_>>();

        assert_eq!(kinds, vec!["confirm", "input", "select", "editor"]);
        assert!(requests.iter().all(|request| request.id.is_some()));
    }

    #[test]
    fn cli_parses_version_flag() {
        let long = CliArgs::from_args([String::from("--version")].into_iter());
        let short = CliArgs::from_args([String::from("-V")].into_iter());

        assert_eq!(long.command, Command::Version);
        assert_eq!(short.command, Command::Version);
    }

    #[test]
    fn cli_parses_quiet_flag() {
        let long = CliArgs::from_args(
            [String::from("--quiet"), String::from("print-capabilities")].into_iter(),
        );
        let short = CliArgs::from_args(
            [String::from("-q"), String::from("print-capabilities")].into_iter(),
        );

        assert!(long.quiet);
        assert_eq!(long.command, Command::PrintCapabilities);
        assert!(short.quiet);
        assert_eq!(short.command, Command::PrintCapabilities);
    }

    #[test]
    fn version_flag_takes_precedence_over_command() {
        let cli = CliArgs::from_args(
            [
                String::from("print-capabilities"),
                String::from("--version"),
            ]
            .into_iter(),
        );

        assert_eq!(cli.command, Command::Version);
    }

    #[test]
    fn version_renders_package_version() {
        let lines = CommandResult::Version.render_lines();

        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("yach "));
        assert_eq!(lines[0], format!("yach {}", env!("CARGO_PKG_VERSION")));
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
    fn rendered_prompt_smoke_results_are_stable() {
        let result = CommandResult::PromptSmoke {
            outcome: PromptSmokeOutcome::Completed,
            saw_delta: true,
            saw_tool_start: true,
            saw_tool_finish: true,
            completed: true,
            response_chars: 13,
        };

        let lines = result.render_lines();

        assert_eq!(lines[0], "prompt_smoke_outcome=Completed");
        assert_eq!(lines[1], "saw_delta=true");
        assert_eq!(lines[2], "saw_tool_start=true");
        assert_eq!(lines[3], "saw_tool_finish=true");
        assert_eq!(lines[4], "completed=true");
        assert_eq!(lines[5], "response_chars=13");
    }

    #[test]
    fn emit_lines_accepts_rendered_output() {
        let lines = vec![String::from("alpha"), String::from("beta")];
        let mut output = Vec::new();

        assert!(super::write_lines(&mut output, &lines).is_ok());
        assert_eq!(output, b"alpha\nbeta\n");
    }
}
