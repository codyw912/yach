use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::sync::mpsc;
use yach_adapter_pi_rpc::{
    AdapterCapabilities, ParseError, PiCommand, PiRpcReader, PiRpcSession, PiRpcWriter,
    SessionError, negotiate_with as negotiate_with_rpc, parse_server_line,
    serialize_client_message, stock_rpc_handshake,
};
use yach_backend::{
    BackendMetadata, NativeEntryId, NativeRole, NativeSessionEvent, NativeSessionId,
    NativeSessionLog, NativeTurnId, NativeTurnOutcome, ProviderError, ProviderMessage,
    ProviderMetadata, ProviderModel, ProviderRequest,
    rig_adapter::{
        RigAnthropicSmokeConfig, RigChatGptSubscriptionSmokeConfig, RigOpenAiCompatibleSmokeConfig,
        RigProviderAdapterConfig, RigProviderConfig, run_anthropic_smoke,
        run_chatgpt_subscription_smoke, run_openai_compatible_http_smoke,
        run_openai_compatible_smoke, run_provider_request,
    },
    start_backend_session,
};
use yach_proto::{
    BackendEvent, BackendState, Capability, ClientEvent, DialogKind, DialogRequest, DialogResponse,
    ForkPosition, Handshake, MessageBody, MessageMeta, ModelInfo, PromptOutcome, RecentSession,
    ServerEvent, SessionMessage, SessionStats, TransportMessage,
};
use yach_ui::{UiCapabilities, alpha_handshake, negotiate_with as negotiate_with_ui, run_tui};

fn main() {
    let cli = CliArgs::from_args(std::env::args().skip(1));
    let result = cli.command.run(cli.quiet);
    let _emitted = emit_lines(&result.render_lines());
}

const PROMPT_SMOKE_TEXT: &str = "Reply with exactly: yach-smoke-ok";
const FORK_SEED_SMOKE_TEXT: &str = "Reply with exactly: yach-fork-seed-ok";
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
            Some("smoke-pi-rpc-fork-seeded") => Command::SmokePiRpcForkSeeded,
            Some("smoke-pi-rpc-resume") => Command::SmokePiRpcResume,
            Some("smoke-pi-rpc-tool") => Command::SmokePiRpcTool,
            Some("smoke-rig-openai-compatible") => Command::SmokeRigOpenAiCompatible,
            Some("smoke-openai-compatible-http") => Command::SmokeOpenAiCompatibleHttp,
            Some("smoke-rig-anthropic") => Command::SmokeRigAnthropic,
            Some("smoke-rig-chatgpt-subscription") => Command::SmokeRigChatGptSubscription,
            Some("smoke-rig-provider-request") => Command::SmokeRigProviderRequest,
            Some("run") => Command::Run,
            Some("tui") => Command::Tui {
                backend: selected_tui_backend(&positional[1..]),
            },
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
    SmokePiRpcForkSeeded,
    SmokePiRpcResume,
    SmokePiRpcTool,
    SmokeRigOpenAiCompatible,
    SmokeOpenAiCompatibleHttp,
    SmokeRigAnthropic,
    SmokeRigChatGptSubscription,
    SmokeRigProviderRequest,
    Run,
    Tui { backend: TuiBackendSelection },
    TuiDialogSmoke,
    TuiBenchReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiBackendSelection {
    Pi,
    Native,
    NativeProvider,
}

fn selected_tui_backend(args: &[String]) -> TuiBackendSelection {
    args.windows(2)
        .find_map(
            |window| match (window.first().map(String::as_str), window.get(1)) {
                (Some("--backend"), Some(value)) if value == "native" => {
                    Some(TuiBackendSelection::Native)
                }
                (Some("--backend"), Some(value)) if value == "native-provider" => {
                    Some(TuiBackendSelection::NativeProvider)
                }
                (Some("--backend"), Some(value)) if value == "pi" => Some(TuiBackendSelection::Pi),
                _ => None,
            },
        )
        .unwrap_or(TuiBackendSelection::Pi)
}

impl Command {
    fn run(&self, _quiet: bool) -> CommandResult {
        match self {
            Self::Version => CommandResult::Version,
            Self::BootstrapStub => run_bootstrap_stub(),
            Self::PrintCapabilities => print_capabilities(),
            Self::SmokePiRpc => run_smoke_bootstrap(),
            Self::SmokePiRpcPrompt => run_prompt_smoke(),
            Self::SmokePiRpcForkSeeded => run_seeded_fork_smoke(),
            Self::SmokePiRpcResume => run_resume_smoke(),
            Self::SmokePiRpcTool => run_tool_smoke(),
            Self::SmokeRigOpenAiCompatible => run_rig_openai_compatible_smoke(),
            Self::SmokeOpenAiCompatibleHttp => run_openai_compatible_http_smoke_command(),
            Self::SmokeRigAnthropic => run_rig_anthropic_smoke(),
            Self::SmokeRigChatGptSubscription => run_rig_chatgpt_subscription_smoke(),
            Self::SmokeRigProviderRequest => run_rig_provider_request_smoke(),
            Self::Run => run_interactive_session(),
            Self::Tui { backend } => run_tui_command(*backend),
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
    RigOpenAiCompatibleSmoke {
        outcome: RigSmokeOutcome,
        event_count: usize,
        text_delta_count: usize,
        completed: bool,
        matched_expected_text: bool,
        response_chars: usize,
        provider_response_id: Option<String>,
        message: Option<String>,
    },
    OpenAiCompatibleHttpSmoke {
        outcome: RigSmokeOutcome,
        status: Option<u16>,
        content_type: Option<String>,
        matched_expected_text: bool,
        response_chars: usize,
        message: Option<String>,
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
            Self::RigOpenAiCompatibleSmoke {
                outcome,
                event_count,
                text_delta_count,
                completed,
                matched_expected_text,
                response_chars,
                provider_response_id,
                message,
            } => {
                let mut lines = vec![
                    format!("rig_smoke_outcome={outcome:?}"),
                    format!("event_count={event_count}"),
                    format!("text_delta_count={text_delta_count}"),
                    format!("completed={completed}"),
                    format!("matched_expected_text={matched_expected_text}"),
                    format!("response_chars={response_chars}"),
                ];
                if let Some(provider_response_id) = provider_response_id {
                    lines.push(format!("provider_response_id={provider_response_id}"));
                }
                if let Some(message) = message {
                    lines.push(format!("message={message}"));
                }
                lines
            }
            Self::OpenAiCompatibleHttpSmoke {
                outcome,
                status,
                content_type,
                matched_expected_text,
                response_chars,
                message,
            } => {
                let mut lines = vec![
                    format!("http_smoke_outcome={outcome:?}"),
                    format!("matched_expected_text={matched_expected_text}"),
                    format!("response_chars={response_chars}"),
                ];
                if let Some(status) = status {
                    lines.push(format!("status={status}"));
                }
                if let Some(content_type) = content_type {
                    lines.push(format!("content_type={content_type}"));
                }
                if let Some(message) = message {
                    lines.push(format!("message={message}"));
                }
                lines
            }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum RigSmokeOutcome {
    MissingConfig,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RigSmokeEnvConfig {
    base_url: String,
    api_key: String,
    model: String,
    provider_label: String,
    timeout_secs: u64,
    max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RigSmokeConfigError {
    Missing(&'static str),
    Empty(&'static str),
    InvalidNumber(&'static str),
    InvalidValue {
        name: &'static str,
        value: String,
        reason: &'static str,
    },
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
    GetForkMessages { success: bool, count: usize },
    ForkEntry { success: bool, attempted: bool },
    SeedPrompt { success: bool },
    DiscoverRecentSessions { success: bool, count: usize },
    SwitchSession { success: bool, attempted: bool },
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
            Self::GetForkMessages { success, count } => {
                format!("operation=get_fork_messages success={success} count={count}")
            }
            Self::ForkEntry { success, attempted } => {
                format!("operation=fork_entry success={success} attempted={attempted}")
            }
            Self::SeedPrompt { success } => format!("operation=seed_prompt success={success}"),
            Self::DiscoverRecentSessions { success, count } => {
                format!("operation=discover_recent_sessions success={success} count={count}")
            }
            Self::SwitchSession { success, attempted } => {
                format!("operation=switch_session success={success} attempted={attempted}")
            }
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

fn rig_provider_adapter_config_from_env() -> Result<RigProviderAdapterConfig, RigSmokeConfigError> {
    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let provider = match provider.as_str() {
        "anthropic" => RigProviderConfig::Anthropic {
            api_key: required_env("YACH_RIG_ANTHROPIC_API_KEY")?,
        },
        "chatgpt-subscription" => RigProviderConfig::ChatGptSubscription {
            token_dir: PathBuf::from(required_env("YACH_RIG_CHATGPT_TOKEN_DIR")?),
        },
        _ => {
            return Err(RigSmokeConfigError::InvalidValue {
                name: "YACH_RIG_PROVIDER",
                value: provider,
                reason: "must be anthropic or chatgpt-subscription",
            });
        }
    };
    Ok(RigProviderAdapterConfig {
        provider,
        timeout: Duration::from_secs(optional_bounded_env(
            "YACH_RIG_PROVIDER_TIMEOUT_SECS",
            120,
            5,
            600,
        )?),
        max_tokens: optional_bounded_env("YACH_RIG_PROVIDER_MAX_TOKENS", 128, 1, 256)?,
    })
}

fn run_rig_provider_request_smoke() -> CommandResult {
    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let model = match provider.as_str() {
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-haiku-4-5")),
        "chatgpt-subscription" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        _ => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(String::from(
                    "YACH_RIG_PROVIDER must be anthropic or chatgpt-subscription",
                )),
            };
        }
    };
    let provider_config = match provider.as_str() {
        "anthropic" => match required_env("YACH_RIG_ANTHROPIC_API_KEY") {
            Ok(api_key) => RigProviderConfig::Anthropic { api_key },
            Err(error) => return missing_rig_provider_request_config(&error),
        },
        "chatgpt-subscription" => match required_env("YACH_RIG_CHATGPT_TOKEN_DIR") {
            Ok(token_dir) => RigProviderConfig::ChatGptSubscription {
                token_dir: PathBuf::from(token_dir),
            },
            Err(error) => return missing_rig_provider_request_config(&error),
        },
        _ => unreachable!("provider already validated"),
    };
    let timeout_secs = match optional_bounded_env("YACH_RIG_PROVIDER_TIMEOUT_SECS", 120, 5, 600) {
        Ok(value) => value,
        Err(error) => return missing_rig_provider_request_config(&error),
    };
    let max_tokens = match optional_bounded_env("YACH_RIG_PROVIDER_MAX_TOKENS", 128, 1, 256) {
        Ok(value) => value,
        Err(error) => return missing_rig_provider_request_config(&error),
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(String::from("failed to create tokio runtime")),
        };
    };
    let request = ProviderRequest {
        turn_id: NativeTurnId(String::from("rig-provider-request-smoke-turn")),
        model: ProviderModel { provider, model },
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: String::from("Reply with exactly: yach-rig-smoke-ok"),
        }],
        extensions: vec![],
    };
    match runtime.block_on(run_provider_request(
        RigProviderAdapterConfig {
            provider: provider_config,
            timeout: Duration::from_secs(timeout_secs),
            max_tokens,
        },
        request,
    )) {
        Ok(events) => provider_request_smoke_result(&events),
        Err(error) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(redacted_provider_error_message(&error)),
        },
    }
}

fn missing_rig_provider_request_config(error: &RigSmokeConfigError) -> CommandResult {
    CommandResult::RigOpenAiCompatibleSmoke {
        outcome: RigSmokeOutcome::MissingConfig,
        event_count: 0,
        text_delta_count: 0,
        completed: false,
        matched_expected_text: false,
        response_chars: 0,
        provider_response_id: None,
        message: Some(rig_config_error_message(error)),
    }
}

fn provider_request_smoke_result(events: &[yach_backend::ProviderStreamEvent]) -> CommandResult {
    let mut text = String::new();
    let mut provider_response_id = None;
    let completed = events.iter().any(|event| {
        if let yach_backend::ProviderStreamEvent::Completed {
            provider_response_id: id,
            ..
        } = event
        {
            provider_response_id.clone_from(id);
            true
        } else {
            false
        }
    });
    let text_delta_count = events
        .iter()
        .filter(|event| {
            if let yach_backend::ProviderStreamEvent::TextDelta { delta, .. } = event {
                text.push_str(delta);
                true
            } else {
                false
            }
        })
        .count();
    CommandResult::RigOpenAiCompatibleSmoke {
        outcome: RigSmokeOutcome::Completed,
        event_count: events.len(),
        text_delta_count,
        completed,
        matched_expected_text: text.trim() == "yach-rig-smoke-ok"
            || text.contains("yach-rig-smoke-ok"),
        response_chars: text.chars().count(),
        provider_response_id,
        message: None,
    }
}

fn run_rig_chatgpt_subscription_smoke() -> CommandResult {
    let token_dir = match required_env("YACH_RIG_CHATGPT_TOKEN_DIR") {
        Ok(value) => PathBuf::from(value),
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(format!(
                    "{}; set this to an explicit local cache directory for Rig OAuth tokens",
                    rig_config_error_message(&error)
                )),
            };
        }
    };
    let model = optional_env("YACH_RIG_CHATGPT_MODEL")
        .unwrap_or_else(|| String::from("gpt-5.3-codex-spark"));
    let timeout_secs = match optional_bounded_env("YACH_RIG_CHATGPT_TIMEOUT_SECS", 120, 10, 600) {
        Ok(value) => value,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let max_tokens = match optional_bounded_env("YACH_RIG_CHATGPT_MAX_TOKENS", 128, 1, 256) {
        Ok(value) => value,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let runtime = tokio::runtime::Runtime::new();
    let Ok(runtime) = runtime else {
        return CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(String::from("failed to create tokio runtime")),
        };
    };
    match runtime.block_on(run_chatgpt_subscription_smoke(
        RigChatGptSubscriptionSmokeConfig {
            model,
            token_dir,
            timeout: Duration::from_secs(timeout_secs),
            max_tokens,
        },
    )) {
        Ok(report) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Completed,
            event_count: report.event_count,
            text_delta_count: report.text_delta_count,
            completed: report.completed,
            matched_expected_text: report.matched_expected_text,
            response_chars: report.response_chars,
            provider_response_id: report.provider_response_id,
            message: None,
        },
        Err(error) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(redacted_provider_error_message(&error)),
        },
    }
}

fn run_rig_anthropic_smoke() -> CommandResult {
    let api_key = match required_env("YACH_RIG_ANTHROPIC_API_KEY") {
        Ok(api_key) => api_key,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let model = optional_env("YACH_RIG_ANTHROPIC_MODEL")
        .unwrap_or_else(|| String::from("claude-haiku-4-5"));
    let timeout_secs = match optional_bounded_env("YACH_RIG_ANTHROPIC_TIMEOUT_SECS", 30, 5, 120) {
        Ok(value) => value,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let max_tokens = match optional_bounded_env("YACH_RIG_ANTHROPIC_MAX_TOKENS", 128, 1, 256) {
        Ok(value) => value,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let runtime = tokio::runtime::Runtime::new();
    let Ok(runtime) = runtime else {
        return CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(String::from("failed to create tokio runtime")),
        };
    };
    match runtime.block_on(run_anthropic_smoke(RigAnthropicSmokeConfig {
        api_key,
        model,
        timeout: Duration::from_secs(timeout_secs),
        max_tokens,
    })) {
        Ok(report) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Completed,
            event_count: report.event_count,
            text_delta_count: report.text_delta_count,
            completed: report.completed,
            matched_expected_text: report.matched_expected_text,
            response_chars: report.response_chars,
            provider_response_id: report.provider_response_id,
            message: None,
        },
        Err(error) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(redacted_provider_error_message(&error)),
        },
    }
}

fn run_rig_openai_compatible_smoke() -> CommandResult {
    let env_config = match rig_smoke_config_from_env() {
        Ok(config) => config,
        Err(error) => {
            return CommandResult::RigOpenAiCompatibleSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                event_count: 0,
                text_delta_count: 0,
                completed: false,
                matched_expected_text: false,
                response_chars: 0,
                provider_response_id: None,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let runtime = tokio::runtime::Runtime::new();
    let Ok(runtime) = runtime else {
        return CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(String::from("failed to create tokio runtime")),
        };
    };
    let config = RigOpenAiCompatibleSmokeConfig {
        base_url: env_config.base_url,
        api_key: env_config.api_key,
        model: env_config.model,
        provider_label: env_config.provider_label,
        timeout: Duration::from_secs(env_config.timeout_secs),
        max_tokens: env_config.max_tokens,
    };

    match runtime.block_on(run_openai_compatible_smoke(config)) {
        Ok(report) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Completed,
            event_count: report.event_count,
            text_delta_count: report.text_delta_count,
            completed: report.completed,
            matched_expected_text: report.matched_expected_text,
            response_chars: report.response_chars,
            provider_response_id: report.provider_response_id,
            message: None,
        },
        Err(error) => CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::Failed,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(redacted_provider_error_message(&error)),
        },
    }
}

fn run_openai_compatible_http_smoke_command() -> CommandResult {
    let env_config = match rig_smoke_config_from_env() {
        Ok(config) => config,
        Err(error) => {
            return CommandResult::OpenAiCompatibleHttpSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                status: None,
                content_type: None,
                matched_expected_text: false,
                response_chars: 0,
                message: Some(rig_config_error_message(&error)),
            };
        }
    };
    let runtime = tokio::runtime::Runtime::new();
    let Ok(runtime) = runtime else {
        return CommandResult::OpenAiCompatibleHttpSmoke {
            outcome: RigSmokeOutcome::Failed,
            status: None,
            content_type: None,
            matched_expected_text: false,
            response_chars: 0,
            message: Some(String::from("failed to create tokio runtime")),
        };
    };
    let config = RigOpenAiCompatibleSmokeConfig {
        base_url: env_config.base_url,
        api_key: env_config.api_key,
        model: env_config.model,
        provider_label: env_config.provider_label,
        timeout: Duration::from_secs(env_config.timeout_secs),
        max_tokens: env_config.max_tokens,
    };
    match runtime.block_on(run_openai_compatible_http_smoke(config)) {
        Ok(report) => CommandResult::OpenAiCompatibleHttpSmoke {
            outcome: RigSmokeOutcome::Completed,
            status: Some(report.status),
            content_type: report.content_type,
            matched_expected_text: report.matched_expected_text,
            response_chars: report.response_chars,
            message: None,
        },
        Err(error) => CommandResult::OpenAiCompatibleHttpSmoke {
            outcome: RigSmokeOutcome::Failed,
            status: None,
            content_type: None,
            matched_expected_text: false,
            response_chars: 0,
            message: Some(redacted_provider_error_message(&error)),
        },
    }
}

fn rig_smoke_config_from_env() -> Result<RigSmokeEnvConfig, RigSmokeConfigError> {
    Ok(RigSmokeEnvConfig {
        base_url: required_env("YACH_RIG_OPENAI_COMPAT_BASE_URL")?,
        api_key: required_env("YACH_RIG_OPENAI_COMPAT_API_KEY")?,
        model: required_env("YACH_RIG_OPENAI_COMPAT_MODEL")?,
        provider_label: optional_env("YACH_RIG_OPENAI_COMPAT_PROVIDER_LABEL")
            .unwrap_or_else(|| String::from("openai-compatible")),
        timeout_secs: optional_bounded_env("YACH_RIG_OPENAI_COMPAT_TIMEOUT_SECS", 30, 5, 120)?,
        max_tokens: optional_bounded_env("YACH_RIG_OPENAI_COMPAT_MAX_TOKENS", 32, 1, 128)?,
    })
}

fn required_env(name: &'static str) -> Result<String, RigSmokeConfigError> {
    let value = std::env::var(name).map_err(|_| RigSmokeConfigError::Missing(name))?;
    if value.trim().is_empty() {
        return Err(RigSmokeConfigError::Empty(name));
    }
    Ok(value)
}

fn optional_env(name: &'static str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn optional_bounded_env(
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, RigSmokeConfigError> {
    let Some(value) = optional_env(name) else {
        return Ok(default);
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|_| RigSmokeConfigError::InvalidNumber(name))?;
    Ok(parsed.clamp(min, max))
}

fn rig_config_error_message(error: &RigSmokeConfigError) -> String {
    match error {
        RigSmokeConfigError::Missing(name) => format!("missing required env var {name}"),
        RigSmokeConfigError::Empty(name) => format!("empty required env var {name}"),
        RigSmokeConfigError::InvalidNumber(name) => format!("invalid numeric env var {name}"),
        RigSmokeConfigError::InvalidValue {
            name,
            value,
            reason,
        } => format!("invalid env var {name}={value}: {reason}"),
    }
}

fn redacted_provider_error_message(error: &ProviderError) -> String {
    let prefix = format!("provider_error_kind={:?}; {}", error.kind, error.message);
    match error.redacted_debug.as_deref() {
        Some(debug) if !debug.is_empty() => format!("{prefix}: {debug}"),
        _ => prefix,
    }
}

fn run_seeded_fork_smoke() -> CommandResult {
    let handshake = alpha_handshake();

    match PiRpcSession::spawn(PiCommand::stock_rpc()) {
        Ok(mut session) => {
            let (outcome, operations) = smoke_seeded_fork_session(&mut session, &handshake);
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

fn run_resume_smoke() -> CommandResult {
    let handshake = alpha_handshake();

    match PiRpcSession::spawn(PiCommand::stock_rpc()) {
        Ok(mut session) => {
            let (outcome, operations) = smoke_resume_session(&mut session, &handshake);
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
                    ServerEvent::PromptFinished {
                        outcome: PromptOutcome::Completed,
                        ..
                    } => {
                        stats.mark_completed();
                        return prompt_smoke_result(PromptSmokeOutcome::Completed, stats);
                    }
                    ServerEvent::PromptFinished { .. } => {
                        return prompt_smoke_result(PromptSmokeOutcome::ReadFailed, stats);
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
                        ServerEvent::PromptFinished { message, .. } => {
                            if let Some(message) = message {
                                let _ = writeln!(io::stdout(), "\n[{message}]");
                            }
                            let _ = writeln!(io::stdout(), "---");
                            break;
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
                        ServerEvent::RecentSessionsUpdated { sessions } => {
                            let _ =
                                writeln!(io::stdout(), "\n[recent sessions: {}]", sessions.len());
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
        let backend_session = start_backend_session(BackendMetadata::pi_rpc(), negotiated);
        let _ = backend_session
            .endpoints
            .backend_tx
            .send(BackendEvent::Server(ServerEvent::StateUpdated(
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
        let _ = backend_session
            .channels
            .client_tx
            .send(ClientEvent::Initialize(ui_handshake));
        run_tui(
            backend_session.channels.client_tx,
            backend_session.channels.backend_rx,
        )
        .await
    }) {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(e) => {
            let _ = writeln!(io::stderr(), "tui error: {e}");
            CommandResult::Tui { exited: true }
        }
    }
}

fn run_tui_command(backend: TuiBackendSelection) -> CommandResult {
    let ui_handshake = alpha_handshake();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };

    let result = match backend {
        TuiBackendSelection::Pi => {
            let pi_backend = match start_pi_tui_backend(PiCommand::stock_rpc(), ui_handshake) {
                Ok(backend) => backend,
                Err(error) => {
                    let _ = writeln!(
                        io::stderr(),
                        "failed to start pi rpc backend: {}",
                        error.message()
                    );
                    return CommandResult::Tui { exited: true };
                }
            };
            runtime.block_on(run_tui_with_pi_backend(pi_backend))
        }
        TuiBackendSelection::Native => runtime.block_on(run_tui_with_native_backend(ui_handshake)),
        TuiBackendSelection::NativeProvider => match rig_provider_adapter_config_from_env() {
            Ok(config) => {
                runtime.block_on(run_tui_with_native_provider_backend(ui_handshake, config))
            }
            Err(error) => {
                let _ = writeln!(
                    io::stderr(),
                    "failed to configure native provider backend: {}",
                    rig_config_error_message(&error)
                );
                return CommandResult::Tui { exited: true };
            }
        },
    };

    match result {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(e) => {
            let _ = writeln!(io::stderr(), "tui error: {e}");
            CommandResult::Tui { exited: true }
        }
    }
}

async fn run_tui_with_native_provider_backend(
    ui_handshake: Handshake,
    provider_config: RigProviderAdapterConfig,
) -> io::Result<()> {
    run_tui_with_native_backend_config(ui_handshake, Some(provider_config)).await
}

async fn run_tui_with_native_backend(ui_handshake: Handshake) -> io::Result<()> {
    run_tui_with_native_backend_config(ui_handshake, None).await
}

async fn run_tui_with_native_backend_config(
    ui_handshake: Handshake,
    provider_config: Option<RigProviderAdapterConfig>,
) -> io::Result<()> {
    let native_handshake = Handshake::new(
        if provider_config.is_some() {
            "yach-native-provider-dogfood"
        } else {
            "yach-native-dogfood"
        },
        vec![
            Capability::PromptStreaming,
            Capability::PromptCancellation,
            Capability::StatusEntries,
            Capability::Notifications,
        ],
    );
    let negotiated = negotiate_with_ui(&native_handshake);
    let backend_session = start_backend_session(BackendMetadata::native_dogfood(), negotiated);
    let session_path = native_session_log_path("default");
    let _ = backend_session
        .channels
        .client_tx
        .send(ClientEvent::Initialize(ui_handshake));

    let native_tx = backend_session.endpoints.backend_tx.clone();
    let native_handle = tokio::spawn(native_dogfood_loop(
        backend_session.endpoints.client_rx,
        native_tx,
        session_path,
        provider_config,
    ));

    let ui_result = run_tui(
        backend_session.channels.client_tx,
        backend_session.channels.backend_rx,
    )
    .await;

    native_handle.abort();
    ui_result
}

async fn native_dogfood_loop(
    mut rx: mpsc::UnboundedReceiver<ClientEvent>,
    tx: mpsc::UnboundedSender<BackendEvent>,
    session_path: PathBuf,
    provider_config: Option<RigProviderAdapterConfig>,
) {
    send_native_initial_state(&tx, &session_path, provider_config.as_ref());
    let mut turn_index = 0_u64;
    let mut active_provider_turn: Option<(tokio::task::JoinHandle<()>, NativeTurnId)> = None;

    while let Some(event) = rx.recv().await {
        match event {
            ClientEvent::Initialize(_) => {
                send_native_initial_state(&tx, &session_path, provider_config.as_ref());
            }
            ClientEvent::AvailableModelsRequested => {
                send_native_models(&tx, provider_config.as_ref());
            }
            ClientEvent::PromptCancelled { session_id } => {
                if let Some((handle, turn_id)) = active_provider_turn.take() {
                    handle.abort();
                    persist_native_cancelled_turn(
                        &tx,
                        &session_path,
                        turn_id,
                        "native provider prompt cancelled",
                    );
                }
                let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
                    session_id,
                    outcome: PromptOutcome::Cancelled,
                    message: Some(String::from("native dogfood prompt cancelled")),
                }));
            }
            ClientEvent::RecentSessionsRequested => send_native_recent_sessions(&tx, &session_path),
            ClientEvent::SessionMessagesRequested => {
                send_native_session_messages(&tx, &session_path);
            }
            ClientEvent::SessionStatsRequested => send_native_session_stats(&tx, &session_path),
            ClientEvent::PromptSubmitted { session_id, prompt } => {
                if prompt.trim().is_empty() {
                    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                        message: String::from("native dogfood: empty prompt ignored"),
                    }));
                    continue;
                }
                turn_index = turn_index.saturating_add(1);
                if provider_config.is_some() {
                    if active_provider_turn
                        .as_ref()
                        .is_some_and(|(handle, _)| handle.is_finished())
                    {
                        active_provider_turn = None;
                    }
                    if active_provider_turn.is_some() {
                        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                            message: String::from("native provider: prompt already in progress"),
                        }));
                        continue;
                    }
                    let turn_id = NativeTurnId(format!("turn-{turn_index}"));
                    let handle = tokio::spawn(handle_native_prompt(
                        tx.clone(),
                        session_path.clone(),
                        session_id,
                        prompt,
                        turn_index,
                        provider_config.clone(),
                    ));
                    active_provider_turn = Some((handle, turn_id));
                } else {
                    handle_native_prompt(
                        tx.clone(),
                        session_path.clone(),
                        session_id,
                        prompt,
                        turn_index,
                        provider_config.clone(),
                    )
                    .await;
                }
            }
            ClientEvent::ModelSelected { model } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
            }
            ClientEvent::ModelSelectedDetailed { provider, model_id } => {
                let model = format!("{provider}/{model_id}");
                let _ = tx.send(BackendEvent::Server(ServerEvent::ModelChanged { model }));
            }
            ClientEvent::SessionSelected { session_id } if session_id == "default" => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::SessionChanged {
                    session_id,
                }));
            }
            ClientEvent::SessionSelected { session_id } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!("native dogfood: unknown session {session_id}"),
                }));
            }
            ClientEvent::ForkMessagesRequested | ClientEvent::SessionForkRequested { .. } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: String::from(
                        "native dogfood: fork/session tree UI is not available yet",
                    ),
                }));
            }
            ClientEvent::ThinkingLevelSelected { level } => {
                let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                    message: format!(
                        "native dogfood: thinking level {level} noted but not used yet"
                    ),
                }));
            }
            ClientEvent::SessionPathSelected { .. }
            | ClientEvent::DialogResolved { .. }
            | ClientEvent::WidgetCleared { .. } => {}
        }
    }
}

fn send_native_initial_state(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    provider_config: Option<&RigProviderAdapterConfig>,
) {
    let session_file = Some(session_path.to_string_lossy().into_owned());
    let _ = tx.send(BackendEvent::Server(ServerEvent::Ready {
        handshake: Handshake::new(
            "yach-native-dogfood",
            vec![Capability::PromptStreaming, Capability::PromptCancellation],
        ),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StateUpdated(
        BackendState {
            model_id: Some(native_active_model(provider_config).id),
            model_name: Some(native_active_model(provider_config).name),
            model_provider: Some(native_active_model(provider_config).provider),
            session_id: Some(String::from("default")),
            session_file,
            thinking_level: Some(String::from("low")),
            is_streaming: false,
            is_compacting: false,
            message_count: native_session_message_count(session_path),
            pending_message_count: Some(0),
        },
    )));
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: native_status_message(provider_config),
    }));
    send_native_models(tx, provider_config);
}

fn send_native_models(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    provider_config: Option<&RigProviderAdapterConfig>,
) {
    let _ = tx.send(BackendEvent::Server(ServerEvent::AvailableModelsUpdated {
        models: vec![native_active_model(provider_config)],
    }));
}

fn native_active_model(provider_config: Option<&RigProviderAdapterConfig>) -> ModelInfo {
    let Some(provider_config) = provider_config else {
        return ModelInfo {
            id: String::from("fixture-echo"),
            name: String::from("Fixture Echo"),
            provider: String::from("native"),
        };
    };
    let provider = match provider_config.provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
    };
    let id = native_provider_model_from_env(provider);
    ModelInfo {
        name: id.clone(),
        id,
        provider: provider.to_owned(),
    }
}

fn native_status_message(provider_config: Option<&RigProviderAdapterConfig>) -> String {
    if let Some(provider_config) = provider_config {
        let model = native_active_model(Some(provider_config));
        format!(
            "backend: native provider dogfood via {}/{}; tools/resources unavailable",
            model.provider, model.id
        )
    } else {
        String::from("backend: native dogfood; tools/resources/provider APIs are unavailable")
    }
}

async fn handle_native_prompt(
    tx: mpsc::UnboundedSender<BackendEvent>,
    session_path: PathBuf,
    session_id: String,
    prompt: String,
    turn_index: u64,
    provider_config: Option<RigProviderAdapterConfig>,
) {
    let session_id = if session_id.is_empty() {
        String::from("default")
    } else {
        session_id
    };
    if session_id != "default" {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native dogfood: unknown session {session_id}"),
        }));
        return;
    }

    let turn_id = NativeTurnId(format!("turn-{turn_index}"));
    let user_entry_id = NativeEntryId(format!("entry-{turn_index}-user"));
    let assistant_entry_id = NativeEntryId(format!("entry-{turn_index}-assistant"));
    let response = format!("native dogfood fixture response: {prompt}");
    let fixture_outcome = native_fixture_outcome(&prompt);
    let mut log = load_native_log_or_default(&session_path);
    log.push(NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("default")),
        entry_id: user_entry_id.clone(),
        parent_entry_id: None,
        turn_id: turn_id.clone(),
        role: NativeRole::User,
        text: prompt.clone(),
        provider: None,
    });

    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: String::from("turn_start native dogfood"),
    }));

    if let Some(provider_config) = provider_config {
        if let Err(error) = log.write_to_file(&session_path) {
            let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
                message: format!("native dogfood: failed to persist session log: {error}"),
            }));
        }
        handle_native_provider_prompt(
            &tx,
            &session_path,
            &prompt,
            provider_config,
            &mut log,
            NativeProviderTurnRefs {
                turn: turn_id,
                user_entry: user_entry_id,
                assistant_entry: assistant_entry_id,
            },
        )
        .await;
        return;
    }

    match fixture_outcome {
        NativeFixtureOutcome::Completed => {
            for delta in native_response_chunks(&response) {
                if tx
                    .send(BackendEvent::Server(ServerEvent::PromptDelta {
                        session_id: String::from("default"),
                        delta,
                    }))
                    .is_err()
                {
                    log.push(NativeSessionEvent::TurnFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id,
                        outcome: NativeTurnOutcome::Cancelled,
                        reason: Some(String::from("ui receiver dropped")),
                    });
                    let _ = log.write_to_file(&session_path);
                    return;
                }
            }
            log.push(NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("default")),
                entry_id: assistant_entry_id,
                parent_entry_id: Some(user_entry_id),
                turn_id: turn_id.clone(),
                role: NativeRole::Assistant,
                text: response,
                provider: None,
            });
            log.push(NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("default")),
                turn_id,
                outcome: NativeTurnOutcome::Completed,
                reason: None,
            });
        }
        NativeFixtureOutcome::Failed => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::fixture_failure(),
            );
        }
        NativeFixtureOutcome::Malformed => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Failed,
                &ProviderError::malformed_stream("native dogfood fixture malformed stream"),
            );
        }
        NativeFixtureOutcome::Cancelled => {
            persist_native_fixture_error(
                &tx,
                &mut log,
                turn_id,
                NativeTurnOutcome::Cancelled,
                &ProviderError::cancelled("native dogfood fixture cancellation"),
            );
        }
    }

    let status = match log.write_to_file(&session_path) {
        Ok(()) => fixture_outcome.status_message().to_owned(),
        Err(error) => format!("native dogfood: failed to persist session log: {error}"),
    };
    let outcome = fixture_outcome.prompt_outcome();
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: status.clone(),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
        session_id: String::from("default"),
        outcome,
        message: Some(status),
    }));
    send_native_session_stats(&tx, &session_path);
}

#[derive(Debug, Clone)]
struct NativeProviderTurnRefs {
    turn: NativeTurnId,
    user_entry: NativeEntryId,
    assistant_entry: NativeEntryId,
}

async fn handle_native_provider_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    prompt: &str,
    provider_config: RigProviderAdapterConfig,
    log: &mut NativeSessionLog,
    ids: NativeProviderTurnRefs,
) {
    let provider_name = match &provider_config.provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
    };
    let model_id = native_provider_model_from_env(provider_name);
    if let Some(delay_ms) = native_provider_test_delay_ms() {
        let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
            message: format!("native provider test delay: {delay_ms}ms"),
        }));
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let request = ProviderRequest {
        turn_id: ids.turn.clone(),
        model: ProviderModel {
            provider: provider_name.to_owned(),
            model: model_id.clone(),
        },
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: prompt.to_owned(),
        }],
        extensions: vec![],
    };
    let events = run_provider_request(provider_config, request).await;
    let mut assistant_text = String::new();
    match events {
        Ok(events) => {
            let mut completed = false;
            for event in events {
                match event {
                    yach_backend::ProviderStreamEvent::TextDelta { delta, .. } => {
                        assistant_text.push_str(&delta);
                        if tx
                            .send(BackendEvent::Server(ServerEvent::PromptDelta {
                                session_id: String::from("default"),
                                delta,
                            }))
                            .is_err()
                        {
                            log.push(NativeSessionEvent::TurnFinished {
                                session_id: NativeSessionId(String::from("default")),
                                turn_id: ids.turn,
                                outcome: NativeTurnOutcome::Cancelled,
                                reason: Some(String::from("ui receiver dropped")),
                            });
                            let _ = log.write_to_file(session_path);
                            return;
                        }
                    }
                    yach_backend::ProviderStreamEvent::Completed { .. } => completed = true,
                    yach_backend::ProviderStreamEvent::Failed { error, .. } => {
                        persist_native_fixture_error(
                            tx,
                            log,
                            ids.turn,
                            NativeTurnOutcome::Failed,
                            &error,
                        );
                        finish_native_prompt(
                            tx,
                            session_path,
                            log,
                            "turn_end native provider failed",
                            PromptOutcome::Failed,
                        );
                        return;
                    }
                    yach_backend::ProviderStreamEvent::Cancelled { reason, .. } => {
                        persist_native_fixture_error(
                            tx,
                            log,
                            ids.turn,
                            NativeTurnOutcome::Cancelled,
                            &ProviderError::cancelled(
                                reason.unwrap_or_else(|| String::from("native provider cancelled")),
                            ),
                        );
                        finish_native_prompt(
                            tx,
                            session_path,
                            log,
                            "turn_end native provider cancelled",
                            PromptOutcome::Cancelled,
                        );
                        return;
                    }
                    _ => {}
                }
            }
            log.push(NativeSessionEvent::EntryAppended {
                session_id: NativeSessionId(String::from("default")),
                entry_id: ids.assistant_entry,
                parent_entry_id: Some(ids.user_entry),
                turn_id: ids.turn.clone(),
                role: NativeRole::Assistant,
                text: assistant_text,
                provider: Some(ProviderMetadata {
                    provider: provider_name.to_owned(),
                    model: model_id,
                    response_id: None,
                }),
            });
            log.push(NativeSessionEvent::TurnFinished {
                session_id: NativeSessionId(String::from("default")),
                turn_id: ids.turn,
                outcome: if completed {
                    NativeTurnOutcome::Completed
                } else {
                    NativeTurnOutcome::Failed
                },
                reason: if completed {
                    None
                } else {
                    Some(String::from("provider stream ended without completion"))
                },
            });
            let outcome = if completed {
                PromptOutcome::Completed
            } else {
                PromptOutcome::Failed
            };
            finish_native_prompt(tx, session_path, log, "turn_end native provider", outcome);
        }
        Err(error) => {
            persist_native_fixture_error(tx, log, ids.turn, NativeTurnOutcome::Failed, &error);
            finish_native_prompt(
                tx,
                session_path,
                log,
                "turn_end native provider failed",
                PromptOutcome::Failed,
            );
        }
    }
}

fn finish_native_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    log: &NativeSessionLog,
    status: &str,
    outcome: PromptOutcome,
) {
    let status = match log.write_to_file(session_path) {
        Ok(()) => status.to_owned(),
        Err(error) => format!("native dogfood: failed to persist session log: {error}"),
    };
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: status.clone(),
    }));
    let _ = tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
        session_id: String::from("default"),
        outcome,
        message: Some(status),
    }));
    send_native_session_stats(tx, session_path);
}

fn native_provider_test_delay_ms() -> Option<u64> {
    optional_bounded_env("YACH_NATIVE_PROVIDER_TEST_DELAY_MS", 0, 0, 30_000)
        .ok()
        .filter(|delay| *delay > 0)
}

fn native_provider_model_from_env(provider: &str) -> String {
    match provider {
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-haiku-4-5")),
        "chatgpt-subscription" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        _ => String::from("unknown"),
    }
}

fn persist_native_cancelled_turn(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    turn_id: NativeTurnId,
    reason: &str,
) {
    let mut log = load_native_log_or_default(session_path);
    log.push(NativeSessionEvent::TurnFinished {
        session_id: NativeSessionId(String::from("default")),
        turn_id,
        outcome: NativeTurnOutcome::Cancelled,
        reason: Some(reason.to_owned()),
    });
    finish_native_prompt(
        tx,
        session_path,
        &log,
        "turn_end native provider cancelled",
        PromptOutcome::Cancelled,
    );
}

fn persist_native_fixture_error(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    log: &mut NativeSessionLog,
    turn_id: NativeTurnId,
    outcome: NativeTurnOutcome,
    error: &ProviderError,
) {
    let reason = native_provider_error_reason(error);
    let _ = tx.send(BackendEvent::Server(ServerEvent::StatusUpdated {
        message: error.message.clone(),
    }));
    log.push(NativeSessionEvent::TurnFinished {
        session_id: NativeSessionId(String::from("default")),
        turn_id,
        outcome,
        reason: Some(reason),
    });
}

fn native_provider_error_reason(error: &ProviderError) -> String {
    match error.redacted_debug.as_deref() {
        Some(debug) if !debug.is_empty() => {
            format!(
                "provider_error kind={:?} message={} debug={debug}",
                error.kind, error.message
            )
        }
        _ => format!(
            "provider_error kind={:?} message={}",
            error.kind, error.message
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeFixtureOutcome {
    Completed,
    Failed,
    Malformed,
    Cancelled,
}

impl NativeFixtureOutcome {
    const fn status_message(self) -> &'static str {
        match self {
            Self::Completed => "turn_end native dogfood",
            Self::Failed => "turn_end native dogfood failed",
            Self::Malformed => "turn_end native dogfood malformed",
            Self::Cancelled => "turn_end native dogfood cancelled",
        }
    }

    const fn prompt_outcome(self) -> PromptOutcome {
        match self {
            Self::Completed => PromptOutcome::Completed,
            Self::Failed | Self::Malformed => PromptOutcome::Failed,
            Self::Cancelled => PromptOutcome::Cancelled,
        }
    }
}

fn native_fixture_outcome(prompt: &str) -> NativeFixtureOutcome {
    if prompt.contains("/native-fixture-fail") {
        NativeFixtureOutcome::Failed
    } else if prompt.contains("/native-fixture-malformed") {
        NativeFixtureOutcome::Malformed
    } else if prompt.contains("/native-fixture-cancel") {
        NativeFixtureOutcome::Cancelled
    } else {
        NativeFixtureOutcome::Completed
    }
}

fn native_response_chunks(response: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in response.chars() {
        current.push(character);
        if current.len() >= 16 {
            chunks.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn send_native_session_messages(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let messages = load_native_log_or_default(session_path)
        .events
        .into_iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended {
                entry_id,
                role,
                text,
                ..
            } => Some(SessionMessage {
                role: native_role_label(role),
                text,
                entry_id: Some(entry_id.0),
            }),
            NativeSessionEvent::TurnFinished { .. } => None,
        })
        .collect();
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionMessagesUpdated {
        messages,
    }));
}

fn send_native_session_stats(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let messages = load_native_log_or_default(session_path)
        .events
        .into_iter()
        .filter_map(|event| match event {
            NativeSessionEvent::EntryAppended { role, .. } => Some(role),
            NativeSessionEvent::TurnFinished { .. } => None,
        })
        .collect::<Vec<_>>();
    let message_count = u64::try_from(messages.len()).ok();
    let user_message_count = count_native_role(&messages, NativeRole::User);
    let assistant_message_count = count_native_role(&messages, NativeRole::Assistant);
    let tool_message_count = count_native_role(&messages, NativeRole::Tool);
    let _ = tx.send(BackendEvent::Server(ServerEvent::SessionStatsUpdated(
        SessionStats {
            message_count,
            user_message_count,
            assistant_message_count,
            tool_message_count,
            total_tokens: None,
        },
    )));
}

fn send_native_recent_sessions(tx: &mpsc::UnboundedSender<BackendEvent>, session_path: &Path) {
    let session = RecentSession {
        path: session_path.to_string_lossy().into_owned(),
        id: Some(String::from("default")),
        name: Some(String::from("native dogfood default")),
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        modified_unix_ms: fs::metadata(session_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
        message_count: native_session_message_count(session_path),
        first_message: native_session_first_message(session_path),
    };
    let _ = tx.send(BackendEvent::Server(ServerEvent::RecentSessionsUpdated {
        sessions: vec![session],
    }));
}

fn native_session_log_path(session_id: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".yach")
        .join("native-sessions")
        .join(format!("{session_id}.jsonl"))
}

fn load_native_log_or_default(path: &Path) -> NativeSessionLog {
    NativeSessionLog::load_from_file(path).unwrap_or_default()
}

fn native_session_message_count(path: &Path) -> Option<u64> {
    u64::try_from(
        load_native_log_or_default(path)
            .events
            .iter()
            .filter(|event| matches!(event, NativeSessionEvent::EntryAppended { .. }))
            .count(),
    )
    .ok()
}

fn native_session_first_message(path: &Path) -> Option<String> {
    load_native_log_or_default(path)
        .events
        .into_iter()
        .find_map(|event| match event {
            NativeSessionEvent::EntryAppended { text, .. } => Some(text),
            NativeSessionEvent::TurnFinished { .. } => None,
        })
}

fn native_role_label(role: NativeRole) -> String {
    match role {
        NativeRole::User => String::from("user"),
        NativeRole::Assistant => String::from("assistant"),
        NativeRole::Tool => String::from("tool"),
        NativeRole::System => String::from("system"),
    }
}

fn count_native_role(messages: &[NativeRole], role: NativeRole) -> Option<u64> {
    u64::try_from(
        messages
            .iter()
            .filter(|message_role| **message_role == role)
            .count(),
    )
    .ok()
}

struct PiTuiBackend {
    ui_handshake: Handshake,
    negotiated: yach_proto::NegotiatedCapabilities,
    child: std::process::Child,
    reader: PiRpcReader<std::process::ChildStdout>,
    writer: PiRpcWriter<std::process::ChildStdin>,
}

#[derive(Debug)]
enum PiTuiBackendStartupError {
    Spawn(SessionError),
    Initialize(SessionError),
}

impl PiTuiBackendStartupError {
    fn message(&self) -> String {
        match self {
            Self::Spawn(error) => format!("spawn failed: {error:?}"),
            Self::Initialize(error) => format!("initialize failed: {error:?}"),
        }
    }
}

fn start_pi_tui_backend(
    command: PiCommand,
    ui_handshake: Handshake,
) -> Result<PiTuiBackend, PiTuiBackendStartupError> {
    let mut session = PiRpcSession::spawn(command).map_err(PiTuiBackendStartupError::Spawn)?;
    let adapter_handshake = session
        .initialize(ui_handshake.clone())
        .map_err(PiTuiBackendStartupError::Initialize)?;
    let negotiated = negotiate_with_ui(&adapter_handshake);
    let (child, reader, writer) = session.into_split();

    Ok(PiTuiBackend {
        ui_handshake,
        negotiated,
        child,
        reader,
        writer,
    })
}

async fn run_tui_with_pi_backend(pi_backend: PiTuiBackend) -> io::Result<()> {
    let PiTuiBackend {
        ui_handshake,
        negotiated,
        mut child,
        reader,
        writer,
    } = pi_backend;

    let backend_session = start_backend_session(BackendMetadata::pi_rpc(), negotiated);
    let _ = backend_session
        .channels
        .client_tx
        .send(ClientEvent::Initialize(ui_handshake));

    let reader_tx = backend_session.endpoints.backend_tx.clone();
    let writer_tx = backend_session.endpoints.backend_tx.clone();
    let reader_handle = tokio::task::spawn_blocking(move || bridge_reader_loop(reader, &reader_tx));
    let writer_handle = tokio::task::spawn_blocking(move || {
        bridge_writer_loop(writer, backend_session.endpoints.client_rx, &writer_tx);
    });

    let ui_result = run_tui(
        backend_session.channels.client_tx,
        backend_session.channels.backend_rx,
    )
    .await;

    let _ = child.kill();
    let _ = child.wait();
    let _ = reader_handle.await;
    let _ = writer_handle.await;

    ui_result
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
        if event == ClientEvent::RecentSessionsRequested {
            let _ = tx.send(BackendEvent::Server(ServerEvent::RecentSessionsUpdated {
                sessions: discover_recent_sessions(),
            }));
            continue;
        }

        if let Err(error) = writer.send_event(event) {
            let _ = tx.send(BackendEvent::Disconnected {
                reason: format!("backend write failed: {error:?}"),
            });
            break;
        }
    }
}

fn discover_recent_sessions() -> Vec<RecentSession> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let session_dir = default_pi_session_dir(&cwd);
    let Ok(entries) = fs::read_dir(session_dir) else {
        return Vec::new();
    };

    let mut sessions = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .filter_map(|path| recent_session_from_file(&path))
        .collect::<Vec<_>>();

    sessions.sort_by(|left, right| right.modified_unix_ms.cmp(&left.modified_unix_ms));
    sessions.truncate(50);
    sessions
}

fn default_pi_session_dir(cwd: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let cwd = cwd.to_string_lossy();
    let trimmed = cwd.trim_start_matches(['/', '\\']);
    let safe_path = trimmed.replace(['/', '\\', ':'], "-");
    home.join(".pi")
        .join("agent")
        .join("sessions")
        .join(format!("--{safe_path}--"))
}

fn recent_session_from_file(path: &Path) -> Option<RecentSession> {
    let content = fs::read_to_string(path).ok()?;
    let metadata = fs::metadata(path).ok();
    let modified_unix_ms = metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    let mut id = None;
    let mut cwd = None;
    let mut name = None;
    let mut message_count = 0_u64;
    let mut first_message = None;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("session") => {
                id = value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                cwd = value
                    .get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
            }
            Some("message") => {
                message_count += 1;
                if first_message.is_none() {
                    first_message = value
                        .get("message")
                        .and_then(|message| message.get("content"))
                        .map(message_preview)
                        .filter(|preview| !preview.is_empty());
                }
            }
            Some("session_info") => {
                name = value
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
            }
            _ => {}
        }
    }

    Some(RecentSession {
        path: path.to_string_lossy().to_string(),
        id,
        name,
        cwd,
        modified_unix_ms,
        message_count: Some(message_count),
        first_message,
    })
}

fn message_preview(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return text.chars().take(96).collect();
    }

    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join("")
                .chars()
                .take(96)
                .collect()
        })
        .unwrap_or_default()
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

            let fork_messages = send_raw_smoke_event(session, "get_fork_messages", &[]);
            let (get_fork_messages_success, fork_message_count, entry_fork_success) =
                match fork_messages {
                    Ok(ServerEvent::ForkMessagesUpdated { messages }) => {
                        let entry_fork_success = messages.first().is_some_and(|message| {
                            let entry_fork_message = TransportMessage::client(
                                MessageMeta::new("smoke-entry-fork-1"),
                                ClientEvent::SessionForkRequested {
                                    session_id: String::from("current"),
                                    entry_id: Some(message.entry_id.clone()),
                                    position: ForkPosition::Before,
                                },
                            );
                            send_smoke_message(session, &entry_fork_message)
                        });
                        (true, messages.len(), entry_fork_success)
                    }
                    Ok(_) | Err(_) => (false, 0, false),
                };

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
                    SmokeOperation::GetForkMessages {
                        success: get_fork_messages_success,
                        count: fork_message_count,
                    },
                    SmokeOperation::ForkEntry {
                        success: entry_fork_success,
                        attempted: fork_message_count > 0,
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

fn smoke_resume_session(
    session: &mut PiRpcSession,
    handshake: &Handshake,
) -> (SmokeOutcome, Vec<SmokeOperation>) {
    if session.initialize(handshake.clone()).is_err() {
        return (
            SmokeOutcome::InitializationFailed,
            vec![SmokeOperation::Initialize { success: false }],
        );
    }

    let recent_sessions = discover_recent_sessions();
    let target_session = recent_sessions.first().map(|session| session.path.clone());
    let switch_success = target_session.as_ref().is_some_and(|session_path| {
        send_raw_smoke_line(session, "switch_session", &[("sessionPath", session_path)])
    });

    (
        SmokeOutcome::Initialized,
        vec![
            SmokeOperation::Initialize { success: true },
            SmokeOperation::DiscoverRecentSessions {
                success: !recent_sessions.is_empty(),
                count: recent_sessions.len(),
            },
            SmokeOperation::SwitchSession {
                success: switch_success,
                attempted: target_session.is_some(),
            },
        ],
    )
}

fn smoke_seeded_fork_session(
    session: &mut PiRpcSession,
    handshake: &Handshake,
) -> (SmokeOutcome, Vec<SmokeOperation>) {
    if session.initialize(handshake.clone()).is_err() {
        return (
            SmokeOutcome::InitializationFailed,
            vec![SmokeOperation::Initialize { success: false }],
        );
    }

    let seed_sent = session
        .submit_prompt("active", FORK_SEED_SMOKE_TEXT)
        .is_ok();
    let seed_completed =
        seed_sent && read_until_agent_end(session, PROMPT_SMOKE_TIMEOUT).unwrap_or(false);

    let fork_messages = if seed_completed {
        send_raw_smoke_event(session, "get_fork_messages", &[])
    } else {
        Err(ParseError::InvalidJson(String::from("seed_prompt_failed")))
    };

    let (get_fork_messages_success, fork_message_count, entry_fork_success) = match fork_messages {
        Ok(ServerEvent::ForkMessagesUpdated { messages }) => {
            let entry_fork_success = messages.first().is_some_and(|message| {
                let entry_fork_message = TransportMessage::client(
                    MessageMeta::new("smoke-seeded-entry-fork-1"),
                    ClientEvent::SessionForkRequested {
                        session_id: String::from("current"),
                        entry_id: Some(message.entry_id.clone()),
                        position: ForkPosition::Before,
                    },
                );
                send_smoke_message(session, &entry_fork_message)
            });
            (true, messages.len(), entry_fork_success)
        }
        Ok(_) | Err(_) => (false, 0, false),
    };

    (
        SmokeOutcome::Initialized,
        vec![
            SmokeOperation::Initialize { success: true },
            SmokeOperation::SeedPrompt {
                success: seed_completed,
            },
            SmokeOperation::GetForkMessages {
                success: get_fork_messages_success,
                count: fork_message_count,
            },
            SmokeOperation::ForkEntry {
                success: entry_fork_success,
                attempted: fork_message_count > 0,
            },
        ],
    )
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

fn send_raw_smoke_event(
    session: &mut PiRpcSession,
    command_type: &str,
    fields: &[(&str, &str)],
) -> Result<ServerEvent, ParseError> {
    let request_id = session
        .send_rpc_command(command_type, fields)
        .map_err(map_session_parse_error)?;

    read_until_response_event(session, &request_id)
}

fn read_until_agent_end(session: &mut PiRpcSession, timeout: Duration) -> Result<bool, ParseError> {
    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            return Ok(false);
        }

        let message = session.read_next().map_err(map_session_parse_error)?;
        let MessageBody::ServerEvent(event) = message.body else {
            continue;
        };
        match event {
            ServerEvent::StatusUpdated { message } if message.starts_with("agent_end") => {
                return Ok(true);
            }
            ServerEvent::DialogRequested(request) => {
                let _ = session.send(&TransportMessage::client(
                    MessageMeta::new("seed-dialog-response-1"),
                    ClientEvent::DialogResolved {
                        dialog_id: request.id.unwrap_or_default(),
                        response: DialogResponse::Cancelled,
                    },
                ));
            }
            _ => {}
        }
    }
}

fn read_until_response(session: &mut PiRpcSession, request_id: &str) -> Result<bool, ParseError> {
    read_until_response_event(session, request_id).map(|_| true)
}

fn read_until_response_event(
    session: &mut PiRpcSession,
    request_id: &str,
) -> Result<ServerEvent, ParseError> {
    loop {
        let message = session.read_next().map_err(map_session_parse_error)?;
        if message.meta.correlation_id.as_deref() == Some(request_id) {
            let MessageBody::ServerEvent(event) = message.body else {
                return Err(ParseError::InvalidJson(String::from("non_server_response")));
            };
            return Ok(event);
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
        CliArgs, Command, CommandResult, NativeFixtureOutcome, PiTuiBackendStartupError,
        PromptSmokeOutcome, RigSmokeOutcome, SmokeOperation, SmokeOutcome, TuiBackendSelection,
        dialog_smoke_requests, native_dogfood_loop, native_fixture_outcome,
        native_provider_error_reason, native_response_chunks, print_capabilities,
        run_bootstrap_stub, start_pi_tui_backend,
    };
    use tokio::sync::mpsc;
    use yach_adapter_pi_rpc::PiCommand;
    use yach_backend::{ProviderError, ProviderErrorKind};
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};
    use yach_ui::alpha_handshake;

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
        let rig_smoke =
            CliArgs::from_args([String::from("smoke-rig-openai-compatible")].into_iter());
        let http_smoke =
            CliArgs::from_args([String::from("smoke-openai-compatible-http")].into_iter());
        let anthropic_smoke = CliArgs::from_args([String::from("smoke-rig-anthropic")].into_iter());
        let chatgpt_smoke =
            CliArgs::from_args([String::from("smoke-rig-chatgpt-subscription")].into_iter());
        let provider_request_smoke =
            CliArgs::from_args([String::from("smoke-rig-provider-request")].into_iter());
        let fork_seeded =
            CliArgs::from_args([String::from("smoke-pi-rpc-fork-seeded")].into_iter());
        let resume_smoke = CliArgs::from_args([String::from("smoke-pi-rpc-resume")].into_iter());
        let dialog_smoke = CliArgs::from_args([String::from("tui-dialog-smoke")].into_iter());
        let run = CliArgs::from_args([String::from("run")].into_iter());
        let tui = CliArgs::from_args([String::from("tui")].into_iter());
        let native_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("native"),
            ]
            .into_iter(),
        );
        let native_provider_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("native-provider"),
            ]
            .into_iter(),
        );

        assert_eq!(print.command, Command::PrintCapabilities);
        assert_eq!(smoke.command, Command::SmokePiRpc);
        assert_eq!(prompt_smoke.command, Command::SmokePiRpcPrompt);
        assert_eq!(rig_smoke.command, Command::SmokeRigOpenAiCompatible);
        assert_eq!(http_smoke.command, Command::SmokeOpenAiCompatibleHttp);
        assert_eq!(anthropic_smoke.command, Command::SmokeRigAnthropic);
        assert_eq!(chatgpt_smoke.command, Command::SmokeRigChatGptSubscription);
        assert_eq!(
            provider_request_smoke.command,
            Command::SmokeRigProviderRequest
        );
        assert_eq!(fork_seeded.command, Command::SmokePiRpcForkSeeded);
        assert_eq!(resume_smoke.command, Command::SmokePiRpcResume);
        assert_eq!(dialog_smoke.command, Command::TuiDialogSmoke);
        assert_eq!(run.command, Command::Run);
        assert_eq!(
            tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Pi,
            }
        );
        assert_eq!(
            native_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Native,
            }
        );
        assert_eq!(
            native_provider_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
            }
        );
    }

    #[test]
    fn native_response_chunks_preserve_unicode() {
        let chunks = native_response_chunks("hello 🙂 native dogfood");

        assert_eq!(chunks.concat(), "hello 🙂 native dogfood");
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn native_fixture_outcome_uses_explicit_markers() {
        assert_eq!(
            native_fixture_outcome("hello"),
            NativeFixtureOutcome::Completed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-fail"),
            NativeFixtureOutcome::Failed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-malformed"),
            NativeFixtureOutcome::Malformed
        );
        assert_eq!(
            native_fixture_outcome("/native-fixture-cancel"),
            NativeFixtureOutcome::Cancelled
        );
    }

    #[test]
    fn native_dogfood_loop_streams_and_persists_prompt() {
        let runtime = tokio::runtime::Runtime::new();
        assert!(runtime.is_ok());
        let runtime = runtime.ok();
        let Some(runtime) = runtime else {
            return;
        };

        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let path = temp_native_log_path();
            let handle = tokio::spawn(native_dogfood_loop(
                client_rx,
                backend_tx,
                path.clone(),
                None,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("hello"),
                    })
                    .is_ok()
            );

            let mut saw_delta = false;
            let mut saw_turn_end = false;
            for _ in 0..16 {
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv())
                        .await;
                let Ok(Some(event)) = event else {
                    break;
                };
                match event {
                    BackendEvent::Server(ServerEvent::PromptDelta { .. }) => {
                        saw_delta = true;
                    }
                    BackendEvent::Server(ServerEvent::StatusUpdated { message }) => {
                        saw_turn_end |= message.starts_with("turn_end");
                    }
                    BackendEvent::Connected { .. }
                    | BackendEvent::Disconnected { .. }
                    | BackendEvent::Server(_) => {}
                }
                if saw_delta && saw_turn_end {
                    break;
                }
            }

            handle.abort();
            let persisted = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(path);

            assert!(saw_delta);
            assert!(saw_turn_end);
            assert!(persisted.contains("hello"));
            assert!(persisted.contains("turn_finished"));
        });
    }

    #[test]
    fn native_provider_error_reason_persists_kind_with_redacted_debug() {
        let reason = native_provider_error_reason(&ProviderError {
            kind: ProviderErrorKind::Authentication,
            message: String::from("Provider auth failed"),
            redacted_debug: Some(String::from("authorization=<redacted>")),
        });

        assert!(reason.contains("kind=Authentication"));
        assert!(reason.contains("Provider auth failed"));
        assert!(reason.contains("authorization=<redacted>"));
        assert!(!reason.contains("sk-"));
    }

    #[test]
    fn native_dogfood_loop_persists_failed_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-fail");

        assert!(persisted.contains("failed"));
        assert!(persisted.contains("ProviderInternal"));
        assert!(persisted.contains("native dogfood fixture provider failure"));
    }

    #[test]
    fn native_dogfood_loop_persists_malformed_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-malformed");

        assert!(persisted.contains("failed"));
        assert!(persisted.contains("native dogfood fixture malformed stream"));
    }

    #[test]
    fn native_dogfood_loop_persists_cancelled_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-cancel");

        assert!(persisted.contains("cancelled"));
        assert!(persisted.contains("native dogfood fixture cancellation"));
    }

    fn run_native_fixture_prompt(prompt: &str) -> String {
        let runtime = tokio::runtime::Runtime::new();
        assert!(runtime.is_ok());
        let runtime = runtime.ok();
        let Some(runtime) = runtime else {
            return String::new();
        };

        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let path = temp_native_log_path();
            let handle = tokio::spawn(native_dogfood_loop(
                client_rx,
                backend_tx,
                path.clone(),
                None,
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: prompt.to_owned(),
                    })
                    .is_ok()
            );

            for _ in 0..16 {
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv())
                        .await;
                let Ok(Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))) = event
                else {
                    continue;
                };
                if message.starts_with("turn_end") {
                    break;
                }
            }

            handle.abort();
            let persisted = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(path);
            persisted
        })
    }

    fn temp_native_log_path() -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        std::env::temp_dir().join(format!("yach-native-dogfood-test-{unique}.jsonl"))
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
    fn rendered_rig_smoke_result_redacts_to_summary_fields() {
        let result = CommandResult::RigOpenAiCompatibleSmoke {
            outcome: RigSmokeOutcome::MissingConfig,
            event_count: 0,
            text_delta_count: 0,
            completed: false,
            matched_expected_text: false,
            response_chars: 0,
            provider_response_id: None,
            message: Some(String::from(
                "missing required env var YACH_RIG_OPENAI_COMPAT_API_KEY",
            )),
        };

        let lines = result.render_lines();

        assert!(lines.contains(&String::from("rig_smoke_outcome=MissingConfig")));
        assert!(lines.contains(&String::from("completed=false")));
        assert!(lines.iter().all(|line| !line.contains("sk-")));
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

    #[test]
    fn pi_tui_backend_startup_reports_spawn_failure() {
        let result = start_pi_tui_backend(
            PiCommand::new("definitely-not-a-yach-test-binary"),
            alpha_handshake(),
        );

        assert!(matches!(result, Err(PiTuiBackendStartupError::Spawn(_))));
    }

    #[test]
    fn pi_tui_backend_startup_reports_initialize_failure() {
        let result = start_pi_tui_backend(
            PiCommand::new("sh")
                .with_arg("-c")
                .with_arg("printf 'not-json\\n'"),
            alpha_handshake(),
        );

        assert!(matches!(
            result,
            Err(PiTuiBackendStartupError::Initialize(_))
        ));
    }
}
