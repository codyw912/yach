use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, UNIX_EPOCH};

use tokio::sync::mpsc;
use yach_adapter_pi_rpc::{
    ParseError, PiCommand, PiRpcReader, PiRpcSession, PiRpcWriter, SessionError,
    stock_rpc_handshake,
};
use yach_backend::{
    BackendMetadata, ExtensionActivationDiagnostic, ExtensionActivationErrorKind,
    ExtensionActivationState, ExtensionInstallError, ExtensionInstallRecord,
    ExtensionInstallRefKind, ExtensionInstallScope, ExtensionInstallStore, ExtensionManifestIndex,
    ExtensionPackageRoot, NativeDogfoodRunnerConfig, NativeExtensionPackageRootLoader,
    NativeProviderDogfoodConfig, NativeRole, NativeStartupTraceMarker, NativeTurnId, ProviderError,
    ProviderErrorKind, ProviderMessage, ProviderModel, ProviderRequest,
    latest_native_session_log_path, native_fresh_session_id, native_session_log_path,
    rig_adapter::{RigProviderAdapterConfig, RigProviderConfig, run_provider_request},
    rig_diagnostics::{
        RigAnthropicSmokeConfig, RigChatGptSubscriptionSmokeConfig, RigOpenAiCompatibleSmokeConfig,
        run_anthropic_smoke, run_chatgpt_subscription_smoke, run_openai_compatible_http_smoke,
        run_openai_compatible_smoke,
    },
    run_native_dogfood_loop, start_backend_session,
};
use yach_proto::{
    BackendEvent, Capability, ClientEvent, DialogKind, DialogRequest, DialogResponse, ForkPosition,
    Handshake, MessageBody, MessageMeta, PromptOutcome, RecentSession, ServerEvent,
    TransportMessage,
};
use yach_ui::{
    RunTuiOptions, StartupTrace, alpha_handshake, negotiate_with as negotiate_with_ui, run_tui,
    run_tui_with_startup_trace_and_options,
};

fn main() -> ExitCode {
    let startup_trace = StartupTrace::from_env("YACH_STARTUP_TRACE");
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("process_main_start");
    }
    let cli = CliArgs::from_args(std::env::args().skip(1));
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("cli_args_parsed");
    }
    let result = cli.command.run(cli.quiet, startup_trace.as_ref());
    if emit_lines(&result.render_lines()).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(result.exit_code())
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
                "--help" | "-h" => {
                    return Self {
                        command: Command::Help,
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
            Some("install") => extension_install_command_from_args(&positional[1..]),
            Some("extension") => extension_command_from_args(&positional[1..]),
            Some("run") => Command::Run,
            Some("tui") => Command::Tui {
                backend: selected_tui_backend(&positional[1..]),
                resume: selected_tui_resume(&positional[1..]),
            },
            Some("tui-dialog-smoke") => Command::TuiDialogSmoke,
            Some("tui-bench-ready") => Command::TuiBenchReady,
            // Bare flags without a command belong to the default interactive
            // session, e.g. `yach --resume` or `yach --backend pi`.
            Some(flag) if flag.starts_with('-') => Command::Tui {
                backend: selected_tui_backend(&positional),
                resume: selected_tui_resume(&positional),
            },
            Some(name) => Command::Unknown {
                name: String::from(name),
            },
            // Plain `yach` starts an interactive TUI session.
            None => Command::Tui {
                backend: selected_tui_backend(&[]),
                resume: false,
            },
        };

        Self { command, quiet }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Version,
    Help,
    Unknown {
        name: String,
    },
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
    ExtensionInstall {
        source: String,
        scope: ExtensionInstallScope,
        enabled: bool,
    },
    ExtensionRemove {
        selector: String,
        scope: ExtensionInstallScope,
    },
    ExtensionSetEnabled {
        selector: String,
        scope: ExtensionInstallScope,
        enabled: bool,
    },
    ExtensionList,
    ExtensionDoctor {
        extension_id: Option<String>,
    },
    Run,
    Tui {
        backend: TuiBackendSelection,
        resume: bool,
    },
    TuiDialogSmoke,
    TuiBenchReady,
}

fn extension_command_from_args(args: &[String]) -> Command {
    match args.first().map(String::as_str) {
        Some("install") => extension_install_command_from_args(&args[1..]),
        Some("remove") => {
            extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Remove)
        }
        Some("enable") => {
            extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Enable)
        }
        Some("disable") => {
            extension_selector_command_from_args(&args[1..], ExtensionSelectorAction::Disable)
        }
        Some("doctor") => Command::ExtensionDoctor {
            extension_id: args.get(1).cloned(),
        },
        _ => Command::ExtensionList,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionSelectorAction {
    Remove,
    Enable,
    Disable,
}

fn extension_install_command_from_args(args: &[String]) -> Command {
    let scope = extension_scope_from_args(args);
    let enabled = !args.iter().any(|arg| arg == "--disabled");
    let source = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    Command::ExtensionInstall {
        source,
        scope,
        enabled,
    }
}

fn extension_selector_command_from_args(
    args: &[String],
    action: ExtensionSelectorAction,
) -> Command {
    let scope = extension_scope_from_args(args);
    let selector = args
        .iter()
        .find(|arg| !arg.starts_with("--"))
        .cloned()
        .unwrap_or_default();
    match action {
        ExtensionSelectorAction::Remove => Command::ExtensionRemove { selector, scope },
        ExtensionSelectorAction::Enable => Command::ExtensionSetEnabled {
            selector,
            scope,
            enabled: true,
        },
        ExtensionSelectorAction::Disable => Command::ExtensionSetEnabled {
            selector,
            scope,
            enabled: false,
        },
    }
}

fn extension_scope_from_args(args: &[String]) -> ExtensionInstallScope {
    if args.iter().any(|arg| arg == "--project") {
        ExtensionInstallScope::Project
    } else {
        ExtensionInstallScope::User
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiBackendSelection {
    Pi,
    NativeFixture,
    NativeProvider,
}

fn selected_tui_backend(args: &[String]) -> TuiBackendSelection {
    args.windows(2)
        .find_map(
            |window| match (window.first().map(String::as_str), window.get(1)) {
                (Some("--backend"), Some(value)) if value == "native-fixture" => {
                    Some(TuiBackendSelection::NativeFixture)
                }
                (Some("--backend"), Some(value)) if value == "native" => {
                    Some(TuiBackendSelection::NativeProvider)
                }
                (Some("--backend"), Some(value)) if value == "native-provider" => {
                    Some(TuiBackendSelection::NativeProvider)
                }
                (Some("--backend"), Some(value)) if value == "pi" => Some(TuiBackendSelection::Pi),
                _ => None,
            },
        )
        .unwrap_or(TuiBackendSelection::NativeProvider)
}

fn selected_tui_resume(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--resume")
}

impl Command {
    fn run(&self, _quiet: bool, startup_trace: Option<&StartupTrace>) -> CommandResult {
        if let Some(trace) = startup_trace {
            trace.mark("command_run_start");
        }
        match self {
            Self::Version => CommandResult::Version,
            Self::Help => CommandResult::Usage,
            Self::Unknown { name } => CommandResult::UsageError {
                message: format!("unknown command '{name}'"),
            },
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
            Self::ExtensionInstall {
                source,
                scope,
                enabled,
            } => run_extension_install_command(source, *scope, *enabled),
            Self::ExtensionRemove { selector, scope } => {
                run_extension_remove_command(selector, *scope)
            }
            Self::ExtensionSetEnabled {
                selector,
                scope,
                enabled,
            } => run_extension_set_enabled_command(selector, *scope, *enabled),
            Self::ExtensionList => run_extension_list_command(),
            Self::ExtensionDoctor { extension_id } => {
                run_extension_doctor_command(extension_id.as_deref())
            }
            Self::Run => run_interactive_session(),
            Self::Tui { backend, resume } => run_tui_command(*backend, *resume, startup_trace),
            Self::TuiDialogSmoke => run_tui_dialog_smoke_command(),
            Self::TuiBenchReady => run_tui_bench_ready_command(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommandResult {
    Version,
    Usage,
    UsageError {
        message: String,
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
    ExtensionDiagnostics {
        command: ExtensionDiagnosticsCommand,
        outcome: ExtensionDiagnosticsOutcome,
        records: Vec<ExtensionDiagnosticRecord>,
        message: Option<String>,
        host_start_count: usize,
    },
    ExtensionManagement {
        action: ExtensionManagementAction,
        outcome: ExtensionManagementOutcome,
        scope: ExtensionInstallScope,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionDiagnosticsCommand {
    List,
    Doctor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionDiagnosticsOutcome {
    Completed,
    Failed,
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionManagementAction {
    Install,
    Remove,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionManagementOutcome {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtensionDiagnosticRecord {
    id: Option<String>,
    version: Option<String>,
    scope: ExtensionInstallScope,
    package_root: PathBuf,
    manifest_path: Option<PathBuf>,
    source_ref: Option<String>,
    install_source: Option<String>,
    install_enabled: bool,
    discovered: bool,
    activation_state: ExtensionActivationState,
    generation: u64,
    last_error_kind: Option<ExtensionActivationErrorKind>,
    last_error_summary: Option<String>,
    registered_tools: Vec<String>,
    provider_visible_tools: Vec<String>,
}

impl CommandResult {
    const fn exit_code(&self) -> u8 {
        match self {
            Self::UsageError { .. } => 2,
            Self::Version
            | Self::Usage
            | Self::Capabilities { .. }
            | Self::SmokePiRpc { .. }
            | Self::PromptSmoke { .. }
            | Self::RigOpenAiCompatibleSmoke { .. }
            | Self::OpenAiCompatibleHttpSmoke { .. }
            | Self::ExtensionDiagnostics { .. }
            | Self::ExtensionManagement { .. }
            | Self::InteractiveSession { .. }
            | Self::Tui { .. } => 0,
        }
    }

    fn render_lines(&self) -> Vec<String> {
        match self {
            Self::Version => vec![format!("yach {}", env!("CARGO_PKG_VERSION"))],
            Self::Usage => usage_lines(),
            Self::UsageError { message } => {
                let mut lines = vec![format!("error={message}")];
                lines.extend(usage_lines());
                lines
            }
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
            Self::ExtensionDiagnostics {
                command,
                outcome,
                records,
                message,
                host_start_count,
            } => {
                let mut lines = vec![
                    format!(
                        "extension_command={}",
                        extension_diagnostics_command_label(*command)
                    ),
                    format!("extension_outcome={outcome:?}"),
                    format!("extension_count={}", records.len()),
                    format!("host_start_count={host_start_count}"),
                ];
                lines.extend(records.iter().map(ExtensionDiagnosticRecord::render_line));
                if let Some(message) = message {
                    lines.push(format!("message={message}"));
                }
                lines
            }
            Self::ExtensionManagement {
                action,
                outcome,
                scope,
                message,
            } => {
                let mut lines = vec![
                    format!(
                        "extension_action={}",
                        extension_management_action_label(*action)
                    ),
                    format!("extension_outcome={outcome:?}"),
                    format!("extension_scope={}", extension_install_scope_label(*scope)),
                ];
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

fn usage_lines() -> Vec<String> {
    vec![
        String::from("usage: yach [options]            start an interactive session"),
        String::from("       yach <command> [options]"),
        String::from("commands: extension, install, print-capabilities"),
        String::from("options: --resume, --backend <native|native-fixture>, --version, --help"),
    ]
}

impl ExtensionDiagnosticRecord {
    fn from_activation_diagnostic(
        diagnostic: ExtensionActivationDiagnostic,
        install_enabled: bool,
        discovered: bool,
    ) -> Self {
        Self {
            id: diagnostic.extension_id,
            version: diagnostic.version,
            scope: diagnostic.scope,
            package_root: diagnostic.package_root,
            manifest_path: diagnostic.manifest_path,
            source_ref: diagnostic.source_ref,
            install_source: diagnostic.install_source,
            install_enabled,
            discovered,
            activation_state: diagnostic.activation_state,
            generation: diagnostic.generation,
            last_error_kind: diagnostic.last_error_kind,
            last_error_summary: diagnostic.last_error_summary,
            registered_tools: diagnostic.registered_tools,
            provider_visible_tools: diagnostic.provider_visible_tools,
        }
    }

    fn render_line(&self) -> String {
        let id = self.id.as_deref().unwrap_or("none");
        let version = self.version.as_deref().unwrap_or("none");
        let manifest_path = self
            .manifest_path
            .as_ref()
            .map_or_else(|| String::from("none"), |path| path.display().to_string());
        let source_ref = self.source_ref.as_deref().unwrap_or("none");
        let install_source = self.install_source.as_deref().unwrap_or("none");
        let last_error_kind = self
            .last_error_kind
            .map_or("none", ExtensionActivationErrorKind::as_str);
        let last_error_summary = self.last_error_summary.as_deref().unwrap_or("none");
        format!(
            "extension id={} version={} scope={} package_root={} manifest_path={} source_ref={} install_source={} install_enabled={} discovered={} activation_state={} generation={} last_error_kind={} last_error_summary={} registered_tool_count={} registered_tools={} provider_visible_tools={}",
            id,
            version,
            extension_install_scope_label(self.scope),
            self.package_root.display(),
            manifest_path,
            source_ref,
            install_source,
            self.install_enabled,
            self.discovered,
            self.activation_state.as_str(),
            self.generation,
            last_error_kind,
            last_error_summary,
            self.registered_tools.len(),
            extension_tool_names_label(&self.registered_tools),
            extension_tool_names_label(&self.provider_visible_tools)
        )
    }
}

fn extension_tool_names_label(names: &[String]) -> String {
    if names.is_empty() {
        String::from("none")
    } else {
        names.join(",")
    }
}

const fn extension_diagnostics_command_label(command: ExtensionDiagnosticsCommand) -> &'static str {
    match command {
        ExtensionDiagnosticsCommand::List => "list",
        ExtensionDiagnosticsCommand::Doctor => "doctor",
    }
}

const fn extension_install_scope_label(scope: ExtensionInstallScope) -> &'static str {
    match scope {
        ExtensionInstallScope::User => "user",
        ExtensionInstallScope::Project => "project",
        ExtensionInstallScope::Ephemeral => "ephemeral",
    }
}

const fn extension_management_action_label(action: ExtensionManagementAction) -> &'static str {
    match action {
        ExtensionManagementAction::Install => "install",
        ExtensionManagementAction::Remove => "remove",
        ExtensionManagementAction::Enable => "enable",
        ExtensionManagementAction::Disable => "disable",
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
    let prefix = format!(
        "provider_error_kind={}; {}",
        provider_error_kind_label(error.kind),
        error.message
    );
    match error.redacted_debug.as_deref() {
        Some(debug) if !debug.is_empty() => format!("{prefix}: {debug}"),
        _ => prefix,
    }
}

fn native_provider_setup_error_message(error: &RigSmokeConfigError) -> String {
    format!(
        "native provider setup failed: {}",
        rig_config_error_message(error)
    )
}

const fn provider_error_kind_label(kind: ProviderErrorKind) -> &'static str {
    match kind {
        ProviderErrorKind::Authentication => "authentication",
        ProviderErrorKind::RateLimited => "rate_limited",
        ProviderErrorKind::InvalidRequest => "invalid_request",
        ProviderErrorKind::ContextLength => "context_length",
        ProviderErrorKind::UnavailableModel => "unavailable_model",
        ProviderErrorKind::Timeout => "timeout",
        ProviderErrorKind::Network => "network",
        ProviderErrorKind::ProviderInternal => "provider_internal",
        ProviderErrorKind::SafetyRefusal => "safety_refusal",
        ProviderErrorKind::MalformedStream => "malformed_stream",
        ProviderErrorKind::Backpressure => "backpressure",
        ProviderErrorKind::Cancelled => "cancelled",
        ProviderErrorKind::Unknown => "unknown",
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

    let Ok((mut child, reader, mut writer)) = session.into_split() else {
        return prompt_smoke_result(
            PromptSmokeOutcome::InitializationFailed,
            PromptSmokeStats::default(),
        );
    };
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
                        ServerEvent::LocalEditPreviewReady { preview, .. } => {
                            let _ =
                                writeln!(io::stdout(), "\n[local edit preview: {}]", preview.path);
                        }
                        ServerEvent::ToolReviewRequested {
                            tool_name, payload, ..
                        } => {
                            let yach_proto::ToolReviewPayload::LocalEdit { preview } = payload;
                            let _ = writeln!(
                                io::stdout(),
                                "\n[tool review: {tool_name} {}]",
                                preview.path
                            );
                        }
                        ServerEvent::LocalEditFinished {
                            outcome, message, ..
                        } => {
                            let _ = writeln!(io::stdout(), "\n[local edit {outcome:?}: {message}]");
                        }
                        ServerEvent::ExtensionLifecycleFinished {
                            outcome, message, ..
                        } => {
                            let _ = writeln!(
                                io::stdout(),
                                "\n[extension lifecycle {outcome:?}: {message}]"
                            );
                        }
                        ServerEvent::ExtensionDiagnosticSnapshotUpdated {
                            outcome,
                            records,
                            message,
                            ..
                        } => {
                            let message = message
                                .unwrap_or_else(|| format!("extension_count={}", records.len()));
                            let _ = writeln!(
                                io::stdout(),
                                "\n[extension status {outcome:?}: {message}]"
                            );
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

fn run_tui_command(
    backend: TuiBackendSelection,
    resume: bool,
    startup_trace: Option<&StartupTrace>,
) -> CommandResult {
    let ui_handshake = alpha_handshake();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("tokio_runtime_created");
    }

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
        TuiBackendSelection::NativeFixture => runtime.block_on(run_tui_with_native_backend(
            ui_handshake,
            resume,
            startup_trace.cloned(),
        )),
        TuiBackendSelection::NativeProvider => match rig_provider_adapter_config_from_env() {
            Ok(config) => runtime.block_on(run_tui_with_native_provider_backend(
                ui_handshake,
                config,
                resume,
                startup_trace.cloned(),
            )),
            Err(error) => {
                let message = native_provider_setup_error_message(&error);
                let _ = writeln!(io::stderr(), "{message}");
                runtime.block_on(run_tui_with_unconfigured_native_provider_backend(
                    ui_handshake,
                    message,
                    resume,
                    startup_trace.cloned(),
                ))
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
    resume: bool,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    run_tui_with_native_backend_config(
        ui_handshake,
        Some(provider_config),
        None,
        resume,
        startup_trace,
    )
    .await
}

async fn run_tui_with_native_backend(
    ui_handshake: Handshake,
    resume: bool,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    run_tui_with_native_backend_config(ui_handshake, None, None, resume, startup_trace).await
}

/// Launch the native TUI without a provider after provider setup failed, so
/// the user still gets a session that surfaces the setup error recoverably
/// instead of an exit before first render.
async fn run_tui_with_unconfigured_native_provider_backend(
    ui_handshake: Handshake,
    provider_setup_error: String,
    resume: bool,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    run_tui_with_native_backend_config(
        ui_handshake,
        None,
        Some(provider_setup_error),
        resume,
        startup_trace,
    )
    .await
}

async fn run_tui_with_native_backend_config(
    ui_handshake: Handshake,
    provider_config: Option<RigProviderAdapterConfig>,
    provider_setup_error: Option<String>,
    resume: bool,
    startup_trace: Option<StartupTrace>,
) -> io::Result<()> {
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("native_backend_setup_start");
    }
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
            Capability::LocalEdit,
            Capability::ExtensionLifecycle,
            Capability::FirstRenderEvents,
        ],
    );
    let negotiated = negotiate_with_ui(&native_handshake);
    let backend_session = start_backend_session(BackendMetadata::native_dogfood(), negotiated);
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("native_backend_session_started");
    }
    let fresh_session_id = native_fresh_session_id();
    let latest_session_path = latest_native_session_log_path();
    let resume_existing_session = resume && latest_session_path.is_some();
    let session_path =
        native_tui_session_path_from_latest(resume, latest_session_path, &fresh_session_id);
    let provider = provider_config.map(|adapter| {
        let provider_label = native_provider_label_from_config(&adapter);
        NativeProviderDogfoodConfig {
            model: native_provider_model_from_env(provider_label),
            test_delay_ms: native_provider_test_delay_ms(),
            adapter,
        }
    });
    let _ = backend_session
        .channels
        .client_tx
        .send(ClientEvent::Initialize(ui_handshake));
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("native_client_initialize_sent");
    }
    if resume_existing_session {
        let _ = backend_session
            .channels
            .client_tx
            .send(ClientEvent::SessionPathSelected {
                session_path: session_path.to_string_lossy().into_owned(),
            });
    }

    let native_tx = backend_session.endpoints.backend_tx.clone();
    let native_config = native_dogfood_runner_config(
        session_path,
        provider,
        provider_setup_error,
        startup_trace.as_ref(),
    );
    let native_handle = tokio::spawn(run_native_dogfood_loop(
        backend_session.endpoints.client_rx,
        native_tx,
        native_config,
    ));
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("native_backend_task_spawned");
    }

    let ui_options = RunTuiOptions {
        resume_session: resume,
    };
    let ui_result = run_tui_with_startup_trace_and_options(
        backend_session.channels.client_tx,
        backend_session.channels.backend_rx,
        startup_trace,
        ui_options,
    )
    .await;

    native_handle.abort();
    ui_result
}

fn native_tui_session_path_from_latest(
    resume: bool,
    latest_session_path: Option<PathBuf>,
    fresh_session_id: &str,
) -> PathBuf {
    if resume && let Some(path) = latest_session_path {
        return path;
    }
    native_session_log_path(fresh_session_id)
}

fn native_dogfood_runner_config(
    session_path: PathBuf,
    provider: Option<NativeProviderDogfoodConfig>,
    provider_setup_error: Option<String>,
    startup_trace: Option<&StartupTrace>,
) -> NativeDogfoodRunnerConfig {
    NativeDogfoodRunnerConfig {
        session_path,
        project_root: std::env::current_dir().ok(),
        provider,
        provider_setup_error,
        extension_package_roots: extension_package_roots_from_env(),
        extension_package_root_loader: Some(native_extension_package_root_loader()),
        startup_trace: startup_trace.cloned().map(native_startup_trace_marker),
    }
}

fn native_startup_trace_marker(startup_trace: StartupTrace) -> NativeStartupTraceMarker {
    NativeStartupTraceMarker::new(move |label| {
        startup_trace.mark(label);
        startup_trace.flush();
    })
}

fn extension_package_roots_from_env() -> Vec<ExtensionPackageRoot> {
    std::env::var_os("YACH_EXTENSION_PACKAGE_ROOTS")
        .map(|value| {
            std::env::split_paths(&value)
                .map(|root| ExtensionPackageRoot {
                    root,
                    scope: ExtensionInstallScope::User,
                    source_ref: Some(String::from("env:YACH_EXTENSION_PACKAGE_ROOTS")),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extension_package_roots_from_env_and_install_records(
    records: &[ExtensionInstallRecord],
) -> Vec<ExtensionPackageRoot> {
    let mut roots = extension_package_roots_from_install_records(records);
    roots.extend(extension_package_roots_from_env());
    roots
}

fn native_extension_package_root_loader() -> NativeExtensionPackageRootLoader {
    NativeExtensionPackageRootLoader::new(installed_extension_package_roots)
}

fn installed_extension_package_roots() -> Vec<ExtensionPackageRoot> {
    extension_package_roots_from_install_records(&installed_extension_records())
}

fn extension_package_roots_from_install_records(
    records: &[ExtensionInstallRecord],
) -> Vec<ExtensionPackageRoot> {
    records
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| record.kind == ExtensionInstallRefKind::LocalPath)
        .map(|record| ExtensionPackageRoot {
            root: record.package_root.clone(),
            scope: record.scope,
            source_ref: Some(record.source.clone()),
        })
        .collect()
}

fn extension_store_path(scope: ExtensionInstallScope) -> io::Result<PathBuf> {
    match scope {
        ExtensionInstallScope::User => {
            if let Some(path) = std::env::var_os("YACH_EXTENSION_USER_STORE").map(PathBuf::from) {
                return Ok(path);
            }
            let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "HOME is not set"));
            };
            Ok(home.join(".yach/extensions.json"))
        }
        ExtensionInstallScope::Project => {
            if let Some(path) = std::env::var_os("YACH_EXTENSION_PROJECT_STORE").map(PathBuf::from)
            {
                return Ok(path);
            }
            Ok(std::env::current_dir()?.join(".yach/extensions.json"))
        }
        ExtensionInstallScope::Ephemeral => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ephemeral extension store is runtime-only",
        )),
    }
}

fn run_extension_install_command(
    source: &str,
    scope: ExtensionInstallScope,
    enabled: bool,
) -> CommandResult {
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        store
            .install_ref(source, scope, enabled)
            .map_err(|error| extension_install_io_error(&error))?;
        store
            .save_to_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        Ok(format!("installed {source}"))
    })();
    extension_management_result(ExtensionManagementAction::Install, scope, result)
}

fn run_extension_remove_command(selector: &str, scope: ExtensionInstallScope) -> CommandResult {
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        let resolved_selector = resolve_extension_install_selector(&store, selector);
        store
            .remove(&resolved_selector)
            .map_err(|error| extension_install_io_error(&error))?;
        store
            .save_to_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        Ok(format!("removed {selector}"))
    })();
    extension_management_result(ExtensionManagementAction::Remove, scope, result)
}

fn run_extension_set_enabled_command(
    selector: &str,
    scope: ExtensionInstallScope,
    enabled: bool,
) -> CommandResult {
    let action = if enabled {
        ExtensionManagementAction::Enable
    } else {
        ExtensionManagementAction::Disable
    };
    let result = (|| {
        let path = extension_store_path(scope)?;
        let mut store = ExtensionInstallStore::load_from_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        let resolved_selector = resolve_extension_install_selector(&store, selector);
        store
            .set_enabled(&resolved_selector, enabled)
            .map_err(|error| extension_install_io_error(&error))?;
        store
            .save_to_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
        Ok(format!(
            "{} {selector}",
            if enabled { "enabled" } else { "disabled" }
        ))
    })();
    extension_management_result(action, scope, result)
}

fn resolve_extension_install_selector(store: &ExtensionInstallStore, selector: &str) -> String {
    let selector_path = PathBuf::from(selector);
    if store
        .records
        .iter()
        .any(|record| record.source == selector || record.package_root == selector_path)
    {
        return selector.to_owned();
    }

    store
        .records
        .iter()
        .find_map(|record| {
            let root = ExtensionPackageRoot {
                root: record.package_root.clone(),
                scope: record.scope,
                source_ref: Some(record.source.clone()),
            };
            let index = ExtensionManifestIndex::from_package_roots([root]).ok()?;
            index
                .records()
                .iter()
                .any(|package| package.manifest.id.0 == selector)
                .then(|| record.source.clone())
        })
        .unwrap_or_else(|| selector.to_owned())
}

fn extension_management_result(
    action: ExtensionManagementAction,
    scope: ExtensionInstallScope,
    result: io::Result<String>,
) -> CommandResult {
    match result {
        Ok(message) => CommandResult::ExtensionManagement {
            action,
            outcome: ExtensionManagementOutcome::Completed,
            scope,
            message: Some(message),
        },
        Err(error) => CommandResult::ExtensionManagement {
            action,
            outcome: ExtensionManagementOutcome::Failed,
            scope,
            message: Some(error.to_string()),
        },
    }
}

fn extension_install_io_error(error: &ExtensionInstallError) -> io::Error {
    io::Error::other(format!(
        "extension install failed: {}",
        extension_install_error_label(error)
    ))
}

fn extension_install_error_label(error: &ExtensionInstallError) -> &'static str {
    match error {
        ExtensionInstallError::EmptyRef => "empty_ref",
        ExtensionInstallError::UnsupportedRef { .. } => "unsupported_ref",
        ExtensionInstallError::AdapterUnavailable { .. } => "adapter_unavailable",
        ExtensionInstallError::MissingLocalPath { .. } => "missing_local_path",
        ExtensionInstallError::StoreIo => "store_io",
        ExtensionInstallError::StoreMalformed => "store_malformed",
        ExtensionInstallError::RecordNotFound { .. } => "record_not_found",
    }
}

fn run_extension_list_command() -> CommandResult {
    extension_diagnostics_result(ExtensionDiagnosticsCommand::List, None)
}

fn run_extension_doctor_command(extension_id: Option<&str>) -> CommandResult {
    extension_diagnostics_result(ExtensionDiagnosticsCommand::Doctor, extension_id)
}

fn extension_diagnostics_result(
    command: ExtensionDiagnosticsCommand,
    extension_id: Option<&str>,
) -> CommandResult {
    let install_records = match loaded_extension_install_records() {
        Ok(records) => records,
        Err(message) => {
            return CommandResult::ExtensionDiagnostics {
                command,
                outcome: ExtensionDiagnosticsOutcome::Failed,
                records: Vec::new(),
                message: Some(format!("extension diagnostics failed: {message}")),
                host_start_count: 0,
            };
        }
    };
    match ExtensionManifestIndex::from_package_roots(
        extension_package_roots_from_env_and_install_records(&install_records),
    ) {
        Ok(index) => {
            let records = extension_diagnostic_records_from_index(
                index.records(),
                &install_records,
                extension_id,
            );
            let outcome = if extension_id.is_some() && records.is_empty() {
                ExtensionDiagnosticsOutcome::NotFound
            } else {
                ExtensionDiagnosticsOutcome::Completed
            };
            let message = if matches!(outcome, ExtensionDiagnosticsOutcome::NotFound) {
                extension_id.map(|id| format!("extension {id} not found"))
            } else {
                None
            };
            CommandResult::ExtensionDiagnostics {
                command,
                outcome,
                records,
                message,
                host_start_count: index.host_start_count(),
            }
        }
        Err(error) => CommandResult::ExtensionDiagnostics {
            command,
            outcome: ExtensionDiagnosticsOutcome::Failed,
            records: extension_diagnostic_records_from_installs(&install_records, extension_id),
            message: Some(format!(
                "extension diagnostics failed: {}",
                extension_package_index_error_label(&error)
            )),
            host_start_count: 0,
        },
    }
}

fn installed_extension_records() -> Vec<ExtensionInstallRecord> {
    loaded_extension_install_records().unwrap_or_default()
}

fn loaded_extension_install_records() -> Result<Vec<ExtensionInstallRecord>, String> {
    let mut records = load_extension_install_store_for_scope(ExtensionInstallScope::User)?.records;
    records.extend(load_extension_install_store_for_scope(ExtensionInstallScope::Project)?.records);
    Ok(records)
}

fn load_extension_install_store_for_scope(
    scope: ExtensionInstallScope,
) -> Result<ExtensionInstallStore, String> {
    let path = extension_store_path(scope).map_err(|_| String::from("store_path"))?;
    ExtensionInstallStore::load_from_path(&path)
        .map_err(|error| extension_install_error_label(&error).to_owned())
}

fn extension_diagnostic_records_from_index(
    package_records: &[yach_backend::ExtensionPackageRecord],
    install_records: &[ExtensionInstallRecord],
    extension_id: Option<&str>,
) -> Vec<ExtensionDiagnosticRecord> {
    let mut records = package_records
        .iter()
        .filter_map(|record| {
            let install = install_records
                .iter()
                .find(|install| install.package_root == record.package_root);
            let diagnostic = ExtensionDiagnosticRecord::from_activation_diagnostic(
                ExtensionActivationDiagnostic::from_package_record(record, install),
                install.is_none_or(|record| record.enabled),
                true,
            );
            extension_diagnostic_record_matches(&diagnostic, extension_id).then_some(diagnostic)
        })
        .collect::<Vec<_>>();

    records.extend(
        install_records
            .iter()
            .filter(|install| {
                !package_records
                    .iter()
                    .any(|record| record.package_root == install.package_root)
            })
            .filter_map(|install| {
                let diagnostic = extension_diagnostic_record_from_install(install);
                extension_diagnostic_record_matches(&diagnostic, extension_id).then_some(diagnostic)
            }),
    );
    records.sort_by(extension_diagnostic_record_order);
    records
}

fn extension_diagnostic_records_from_installs(
    install_records: &[ExtensionInstallRecord],
    extension_id: Option<&str>,
) -> Vec<ExtensionDiagnosticRecord> {
    let mut records = install_records
        .iter()
        .filter_map(|install| {
            let diagnostic = extension_diagnostic_record_from_install(install);
            extension_diagnostic_record_matches(&diagnostic, extension_id).then_some(diagnostic)
        })
        .collect::<Vec<_>>();
    records.sort_by(extension_diagnostic_record_order);
    records
}

fn extension_diagnostic_record_from_install(
    install: &ExtensionInstallRecord,
) -> ExtensionDiagnosticRecord {
    ExtensionDiagnosticRecord::from_activation_diagnostic(
        ExtensionActivationDiagnostic::from_install_record(install),
        install.enabled,
        false,
    )
}

fn extension_diagnostic_record_matches(
    record: &ExtensionDiagnosticRecord,
    extension_id: Option<&str>,
) -> bool {
    extension_id.is_none_or(|selector| {
        record.id.as_deref() == Some(selector)
            || record.install_source.as_deref() == Some(selector)
            || record.package_root.to_string_lossy() == selector
    })
}

fn extension_diagnostic_record_order(
    left: &ExtensionDiagnosticRecord,
    right: &ExtensionDiagnosticRecord,
) -> std::cmp::Ordering {
    left.id
        .as_deref()
        .unwrap_or("none")
        .cmp(right.id.as_deref().unwrap_or("none"))
        .then_with(|| left.package_root.cmp(&right.package_root))
}

fn extension_package_index_error_label(
    error: &yach_backend::ExtensionPackageIndexError,
) -> &'static str {
    match error {
        yach_backend::ExtensionPackageIndexError::MissingPackageRoot { .. } => {
            "missing_package_root"
        }
        yach_backend::ExtensionPackageIndexError::MissingManifest { .. } => "missing_manifest",
        yach_backend::ExtensionPackageIndexError::MissingManifestFile { .. } => {
            "missing_manifest_file"
        }
        yach_backend::ExtensionPackageIndexError::MalformedPackageJson { .. } => {
            "malformed_package_json"
        }
        yach_backend::ExtensionPackageIndexError::InvalidManifestPointer { .. } => {
            "invalid_manifest_pointer"
        }
        yach_backend::ExtensionPackageIndexError::ManifestPathEscapedPackageRoot { .. } => {
            "manifest_path_escaped_package_root"
        }
        yach_backend::ExtensionPackageIndexError::Manifest { .. } => "invalid_manifest",
        yach_backend::ExtensionPackageIndexError::Catalog(_) => "catalog_error",
    }
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

fn native_provider_label_from_config(config: &RigProviderAdapterConfig) -> &'static str {
    match &config.provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
    }
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
    Split(SessionError),
}

impl PiTuiBackendStartupError {
    fn message(&self) -> String {
        match self {
            Self::Spawn(error) => format!("spawn failed: {error:?}"),
            Self::Initialize(error) => format!("initialize failed: {error:?}"),
            Self::Split(error) => format!("split failed: {error:?}"),
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
    let (child, reader, writer) = session
        .into_split()
        .map_err(PiTuiBackendStartupError::Split)?;

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

    sessions.sort_by_key(|session| std::cmp::Reverse(session.modified_unix_ms));
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
#[test]
fn native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids() {
    use tokio::sync::mpsc;
    use yach_backend::{
        NativeDogfoodRunnerConfig, NativeEntryId, NativeJsonlSessionStore, NativeRole,
        NativeSessionEvent, NativeSessionEventSink, NativeSessionId, NativeTurnId,
        NativeTurnOutcome, run_native_dogfood_loop,
    };
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

    let runtime = tokio::runtime::Runtime::new();
    assert!(runtime.is_ok());
    let runtime = runtime.ok();
    let Some(runtime) = runtime else {
        return;
    };

    runtime.block_on(async {
        let path = tests::temp_native_log_path();
        let store = NativeJsonlSessionStore::new(path.clone());
        assert!(
            store
                .append_events(&[
                    NativeSessionEvent::EntryAppended {
                        session_id: NativeSessionId(String::from("default")),
                        entry_id: NativeEntryId(String::from("entry-0-user")),
                        parent_entry_id: None,
                        turn_id: NativeTurnId(String::from("turn-0")),
                        role: NativeRole::User,
                        text: String::from("seed prompt"),
                        provider: None,
                    },
                    NativeSessionEvent::TurnFinished {
                        session_id: NativeSessionId(String::from("default")),
                        turn_id: NativeTurnId(String::from("turn-0")),
                        outcome: NativeTurnOutcome::Completed,
                        reason: None,
                    },
                ])
                .is_ok()
        );

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_native_dogfood_loop(
            client_rx,
            backend_tx,
            NativeDogfoodRunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
            },
        ));

        assert!(
            client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("resumed prompt"),
                })
                .is_ok()
        );

        for _ in 0..64 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv()).await;
            let Ok(Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))) = event
            else {
                continue;
            };
            if message.starts_with("turn_end") {
                break;
            }
        }

        handle.abort();
        let loaded = store.load();
        let _ = std::fs::remove_file(path);
        assert!(loaded.is_ok());
        let user_turn_ids = loaded
            .unwrap_or_default()
            .events
            .into_iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EntryAppended {
                    turn_id,
                    role: NativeRole::User,
                    ..
                } => Some(turn_id.0),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(user_turn_ids, vec!["turn-0", "turn-1"]);
    });
}

#[cfg(test)]
#[test]
fn native_dogfood_loop_emits_existing_session_messages_after_explicit_path_selection() {
    use tokio::sync::mpsc;
    use yach_backend::{
        NativeDogfoodRunnerConfig, NativeEntryId, NativeJsonlSessionStore, NativeRole,
        NativeSessionEvent, NativeSessionEventSink, NativeSessionId, NativeTurnId,
        run_native_dogfood_loop,
    };
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

    let runtime = tokio::runtime::Runtime::new();
    assert!(runtime.is_ok());
    let runtime = runtime.ok();
    let Some(runtime) = runtime else {
        return;
    };

    runtime.block_on(async {
        let path = tests::temp_native_log_path();
        let store = NativeJsonlSessionStore::new(path.clone());
        assert!(
            store
                .append_events(&[
                    NativeSessionEvent::EntryAppended {
                        session_id: NativeSessionId(String::from("default")),
                        entry_id: NativeEntryId(String::from("entry-0-user")),
                        parent_entry_id: None,
                        turn_id: NativeTurnId(String::from("turn-0")),
                        role: NativeRole::User,
                        text: String::from("seed prompt"),
                        provider: None,
                    },
                    NativeSessionEvent::EntryAppended {
                        session_id: NativeSessionId(String::from("default")),
                        entry_id: NativeEntryId(String::from("entry-0-assistant")),
                        parent_entry_id: Some(NativeEntryId(String::from("entry-0-user"))),
                        turn_id: NativeTurnId(String::from("turn-0")),
                        role: NativeRole::Assistant,
                        text: String::from("seed answer"),
                        provider: None,
                    },
                ])
                .is_ok()
        );

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_native_dogfood_loop(
            client_rx,
            backend_tx,
            NativeDogfoodRunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
            },
        ));
        assert!(
            client_tx
                .send(ClientEvent::SessionPathSelected {
                    session_path: path.to_string_lossy().into_owned(),
                })
                .is_ok()
        );

        let mut saw_messages = false;
        for _ in 0..8 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv()).await;
            let Ok(Some(BackendEvent::Server(ServerEvent::SessionMessagesUpdated { messages }))) =
                event
            else {
                continue;
            };
            saw_messages = true;
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[0].role, "user");
            assert_eq!(messages[0].text, "seed prompt");
            assert_eq!(messages[1].role, "assistant");
            assert_eq!(messages[1].text, "seed answer");
            break;
        }

        handle.abort();
        let _ = std::fs::remove_file(path);
        assert!(saw_messages);
    });
}

#[cfg(test)]
#[test]
fn native_dogfood_loop_persists_prompt_runtime_metrics() {
    let persisted = tests::run_native_fixture_prompt("hello metrics");

    assert!(persisted.contains("metric_recorded"));
    assert!(persisted.contains("native_prompt_total"));
    assert!(!persisted.contains("session_log_load"));
}

#[cfg(test)]
#[test]
fn native_dogfood_loop_provider_cancel_persists_user_entry() {
    use tokio::sync::mpsc;
    use yach_backend::{
        NativeDogfoodRunnerConfig, NativeJsonlSessionStore, NativeProviderDogfoodConfig,
        NativeRole, NativeSessionEvent,
        rig_adapter::{RigProviderAdapterConfig, RigProviderConfig},
        run_native_dogfood_loop,
    };
    use yach_proto::{ClientEvent, PromptOutcome};

    let runtime = tokio::runtime::Runtime::new();
    assert!(runtime.is_ok());
    let runtime = runtime.ok();
    let Some(runtime) = runtime else {
        return;
    };

    runtime.block_on(async {
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let path = tests::temp_native_log_path();
        let store = NativeJsonlSessionStore::new(path.clone());
        let handle = tokio::spawn(run_native_dogfood_loop(
            client_rx,
            backend_tx,
            NativeDogfoodRunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: Some(NativeProviderDogfoodConfig {
                    adapter: RigProviderAdapterConfig {
                        provider: RigProviderConfig::Anthropic {
                            api_key: String::from("fake-test-key"),
                        },
                        timeout: std::time::Duration::from_millis(1),
                        max_tokens: 1,
                    },
                    model: String::from("fake-test-model"),
                    test_delay_ms: Some(500),
                }),
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
            },
        ));

        assert!(
            client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("cancel before provider start"),
                })
                .is_ok()
        );
        assert!(
            client_tx
                .send(ClientEvent::PromptCancelled {
                    session_id: String::from("default"),
                })
                .is_ok()
        );

        let prompt_finished = tests::collect_prompt_finished_for(
            &mut backend_rx,
            std::time::Duration::from_millis(100),
        )
        .await;

        handle.abort();
        let loaded = store.load();
        let _ = std::fs::remove_file(path);
        assert_eq!(prompt_finished, vec![PromptOutcome::Cancelled]);
        assert!(loaded.is_ok());
        let events = loaded.unwrap_or_default().events;
        assert!(events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::EntryAppended {
                role: NativeRole::User,
                text,
                ..
            } if text == "cancel before provider start"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            NativeSessionEvent::TurnFinished { turn_id, .. } if turn_id.0 == "turn-0"
        )));
    });
}

#[cfg(test)]
#[test]
fn native_dogfood_loop_provider_cancel_after_finish_does_not_duplicate_terminal_turn() {
    use tokio::sync::mpsc;
    use yach_backend::{
        NativeDogfoodRunnerConfig, NativeJsonlSessionStore, NativeProviderDogfoodConfig,
        NativeSessionEvent,
        rig_adapter::{RigProviderAdapterConfig, RigProviderConfig},
        run_native_dogfood_loop,
    };
    use yach_proto::{ClientEvent, PromptOutcome};

    let runtime = tokio::runtime::Runtime::new();
    assert!(runtime.is_ok());
    let runtime = runtime.ok();
    let Some(runtime) = runtime else {
        return;
    };

    runtime.block_on(async {
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let path = tests::temp_native_log_path();
        let store = NativeJsonlSessionStore::new(path.clone());
        let handle = tokio::spawn(run_native_dogfood_loop(
            client_rx,
            backend_tx,
            NativeDogfoodRunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: Some(NativeProviderDogfoodConfig {
                    adapter: RigProviderAdapterConfig {
                        provider: RigProviderConfig::ChatGptSubscription {
                            token_dir: path.with_extension("missing-token-dir"),
                        },
                        timeout: std::time::Duration::from_millis(1),
                        max_tokens: 1,
                    },
                    model: String::from("fake-test-model"),
                    test_delay_ms: None,
                }),
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
            },
        ));

        assert!(
            client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("finish before cancel"),
                })
                .is_ok()
        );

        assert!(tests::wait_for_prompt_finished(&mut backend_rx, PromptOutcome::Failed).await);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(
            client_tx
                .send(ClientEvent::PromptCancelled {
                    session_id: String::from("default"),
                })
                .is_ok()
        );
        let stale_cancel_prompt_finished = tests::collect_prompt_finished_for(
            &mut backend_rx,
            std::time::Duration::from_millis(100),
        )
        .await;

        handle.abort();
        let loaded = store.load();
        let _ = std::fs::remove_file(path);
        assert!(stale_cancel_prompt_finished.is_empty());
        assert!(loaded.is_ok());
        let terminal_turn_count = loaded
            .unwrap_or_default()
            .events
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    NativeSessionEvent::TurnFinished { turn_id, .. } if turn_id.0 == "turn-0"
                )
            })
            .count();

        assert_eq!(terminal_turn_count, 1);
    });
}

#[cfg(test)]
mod tests {
    use super::{
        CliArgs, Command, CommandResult, ExtensionDiagnosticRecord, ExtensionDiagnosticsCommand,
        ExtensionDiagnosticsOutcome, ExtensionManagementAction, ExtensionManagementOutcome,
        PiTuiBackendStartupError, PromptSmokeOutcome, RigSmokeConfigError, RigSmokeOutcome,
        SmokeOperation, SmokeOutcome, TuiBackendSelection, dialog_smoke_requests,
        extension_store_path, native_dogfood_runner_config, native_provider_setup_error_message,
        native_tui_session_path_from_latest, print_capabilities, run_extension_install_command,
        run_extension_list_command, run_extension_remove_command,
        run_extension_set_enabled_command, start_pi_tui_backend,
    };
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;
    use yach_adapter_pi_rpc::PiCommand;
    use yach_backend::{
        ExtensionActivationState, ExtensionInstallScope, NativeDogfoodRunnerConfig,
        native_session_log_path, run_native_dogfood_loop,
    };
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};
    use yach_ui::alpha_handshake;

    #[test]
    fn cli_defaults_to_interactive_tui_session() {
        let cli = CliArgs::from_args(std::iter::empty());

        assert_eq!(
            cli.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: false,
            }
        );
        assert!(!cli.quiet);
    }

    #[test]
    fn cli_bare_flags_configure_the_default_tui_session() {
        let resume = CliArgs::from_args([String::from("--resume")].into_iter());
        let backend =
            CliArgs::from_args([String::from("--backend"), String::from("pi")].into_iter());

        assert_eq!(
            resume.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: true,
            }
        );
        assert_eq!(
            backend.command,
            Command::Tui {
                backend: TuiBackendSelection::Pi,
                resume: false,
            }
        );
    }

    #[test]
    fn cli_help_flag_prints_usage_and_succeeds() {
        let cli = CliArgs::from_args([String::from("--help")].into_iter());

        assert_eq!(cli.command, Command::Help);
        let result = Command::Help.run(false, None);
        assert_eq!(result.exit_code(), 0);
        assert!(
            result
                .render_lines()
                .iter()
                .any(|line| line.contains("usage: yach"))
        );
    }

    #[test]
    fn cli_unknown_command_does_not_bootstrap() {
        let cli = CliArgs::from_args([String::from("tiu")].into_iter());

        assert_eq!(
            cli.command,
            Command::Unknown {
                name: String::from("tiu"),
            }
        );
    }

    #[test]
    fn unknown_command_renders_usage_and_exits_with_misuse() {
        let result = Command::Unknown {
            name: String::from("tiu"),
        }
        .run(false, None);

        let lines = result.render_lines();

        assert_eq!(result.exit_code(), 2);
        assert!(lines.contains(&String::from("error=unknown command 'tiu'")));
        assert!(lines.iter().any(|line| line.contains("usage: yach")));
        assert!(lines.iter().any(|line| line.contains("print-capabilities")));
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
        let extension_list =
            CliArgs::from_args([String::from("extension"), String::from("list")].into_iter());
        let extension_doctor = CliArgs::from_args(
            [
                String::from("extension"),
                String::from("doctor"),
                String::from("example.scan-toy-tools"),
            ]
            .into_iter(),
        );
        let fork_seeded =
            CliArgs::from_args([String::from("smoke-pi-rpc-fork-seeded")].into_iter());
        let resume_smoke = CliArgs::from_args([String::from("smoke-pi-rpc-resume")].into_iter());
        let dialog_smoke = CliArgs::from_args([String::from("tui-dialog-smoke")].into_iter());
        let run = CliArgs::from_args([String::from("run")].into_iter());
        let tui = CliArgs::from_args([String::from("tui")].into_iter());
        let resume_tui =
            CliArgs::from_args([String::from("tui"), String::from("--resume")].into_iter());
        let pi_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("pi"),
            ]
            .into_iter(),
        );
        let native_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("native"),
            ]
            .into_iter(),
        );
        let native_fixture_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("native-fixture"),
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
        assert_eq!(extension_list.command, Command::ExtensionList);
        assert_eq!(
            extension_doctor.command,
            Command::ExtensionDoctor {
                extension_id: Some(String::from("example.scan-toy-tools")),
            }
        );
        assert_eq!(fork_seeded.command, Command::SmokePiRpcForkSeeded);
        assert_eq!(resume_smoke.command, Command::SmokePiRpcResume);
        assert_eq!(dialog_smoke.command, Command::TuiDialogSmoke);
        assert_eq!(run.command, Command::Run);
        assert_eq!(
            tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: false,
            }
        );
        assert_eq!(
            resume_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: true,
            }
        );
        assert_eq!(
            pi_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Pi,
                resume: false,
            }
        );
        assert_eq!(
            native_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: false,
            }
        );
        assert_eq!(
            native_fixture_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeFixture,
                resume: false,
            }
        );
        assert_eq!(
            native_provider_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::NativeProvider,
                resume: false,
            }
        );
    }

    #[test]
    fn extension_cli_parses_diagnostics_commands() {
        let extension_list =
            CliArgs::from_args([String::from("extension"), String::from("list")].into_iter());
        let extension_doctor = CliArgs::from_args(
            [
                String::from("extension"),
                String::from("doctor"),
                String::from("example.scan-toy-tools"),
            ]
            .into_iter(),
        );

        assert_eq!(extension_list.command, Command::ExtensionList);
        assert_eq!(
            extension_doctor.command,
            Command::ExtensionDoctor {
                extension_id: Some(String::from("example.scan-toy-tools")),
            }
        );
    }

    #[test]
    fn cli_parses_extension_install_management_commands() {
        assert_eq!(
            CliArgs::from_args(["install", "./ext"].into_iter().map(String::from)).command,
            Command::ExtensionInstall {
                source: String::from("./ext"),
                scope: ExtensionInstallScope::User,
                enabled: true,
            }
        );
        assert_eq!(
            CliArgs::from_args(
                ["extension", "install", "./ext", "--project", "--disabled"]
                    .into_iter()
                    .map(String::from),
            )
            .command,
            Command::ExtensionInstall {
                source: String::from("./ext"),
                scope: ExtensionInstallScope::Project,
                enabled: false,
            }
        );
        assert_eq!(
            CliArgs::from_args(
                ["extension", "disable", "./ext"]
                    .into_iter()
                    .map(String::from)
            )
            .command,
            Command::ExtensionSetEnabled {
                selector: String::from("./ext"),
                scope: ExtensionInstallScope::User,
                enabled: false,
            }
        );
    }

    #[test]
    fn extension_install_management_renders_stable_lines() {
        let result = CommandResult::ExtensionManagement {
            action: ExtensionManagementAction::Install,
            outcome: ExtensionManagementOutcome::Completed,
            scope: ExtensionInstallScope::User,
            message: Some(String::from("installed ./ext")),
        };

        assert_eq!(
            result.render_lines(),
            vec![
                "extension_action=install",
                "extension_outcome=Completed",
                "extension_scope=user",
                "message=installed ./ext",
            ]
        );
    }

    #[test]
    fn extension_store_path_uses_environment_overrides() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("store-path")?;
        let user = root.path().join("user.json");
        let project = root.path().join("project.json");
        with_extension_store_env(&user, &project, || {
            expect_equal(
                &expect_ok(extension_store_path(ExtensionInstallScope::User))?,
                &user,
            )?;
            expect_equal(
                &expect_ok(extension_store_path(ExtensionInstallScope::Project))?,
                &project,
            )
        })
    }

    #[test]
    fn extension_list_includes_enabled_and_disabled_install_records() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("diagnostics")?;
        let enabled = root.path().join("enabled");
        let disabled = root.path().join("disabled");
        expect_ok(std::fs::create_dir_all(&enabled))?;
        expect_ok(std::fs::create_dir_all(&disabled))?;
        write_test_extension_manifest(&enabled, "example.enabled-install")?;

        let user_store = root.path().join("extensions.json");
        let project_store = root.path().join("project-extensions.json");
        with_extension_store_env(&user_store, &project_store, || {
            let enabled = path_string(&enabled)?;
            let disabled = path_string(&disabled)?;
            expect_true(
                matches!(
                    run_extension_install_command(&enabled, ExtensionInstallScope::User, true),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "enabled install should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_install_command(&disabled, ExtensionInstallScope::User, false),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "disabled install should complete",
            )?;

            let lines = run_extension_list_command().render_lines();

            expect_true(
                lines
                    .iter()
                    .any(|line| line.contains("install_enabled=true")),
                "missing enabled install line",
            )?;
            expect_true(
                lines
                    .iter()
                    .any(|line| line.contains("install_enabled=false")),
                "missing disabled install line",
            )?;
            expect_true(
                lines.iter().any(|line| line.contains("discovered=true")),
                "missing discovered line",
            )?;
            expect_true(
                lines.iter().any(|line| line.contains("discovered=false")),
                "missing undiscovered line",
            )?;
            expect_true(
                lines
                    .iter()
                    .any(|line| line.contains("activation_state=discovered")),
                "missing discovered activation state",
            )?;
            expect_true(
                lines.iter().any(|line| {
                    line.contains("activation_state=blocked")
                        && line.contains("last_error_kind=disabled")
                }),
                "missing blocked disabled activation state",
            )
        })
    }

    #[test]
    fn extension_set_enabled_accepts_manifest_id_selector() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("selector-id")?;
        let package = root.path().join("package");
        expect_ok(std::fs::create_dir_all(&package))?;
        write_test_extension_manifest(&package, "example.selector-id")?;

        let user_store = root.path().join("extensions.json");
        let project_store = root.path().join("project-extensions.json");
        with_extension_store_env(&user_store, &project_store, || {
            let package_string = path_string(&package)?;
            expect_true(
                matches!(
                    run_extension_install_command(
                        &package_string,
                        ExtensionInstallScope::User,
                        true
                    ),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "install should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_set_enabled_command(
                        "example.selector-id",
                        ExtensionInstallScope::User,
                        false
                    ),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "disable by manifest id should complete",
            )?;

            let lines = run_extension_list_command().render_lines();

            expect_true(
                lines.iter().any(|line| {
                    line.contains(&package.display().to_string())
                        && line.contains("install_enabled=false")
                        && line.contains("discovered=false")
                }),
                "missing disabled undiscovered package line",
            )
        })
    }

    #[test]
    fn extension_remove_accepts_path_and_manifest_id_selectors() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("remove")?;
        let first = root.path().join("first");
        let second = root.path().join("second");
        expect_ok(std::fs::create_dir_all(&first))?;
        expect_ok(std::fs::create_dir_all(&second))?;
        write_test_extension_manifest(&first, "example.remove-first")?;
        write_test_extension_manifest(&second, "example.remove-second")?;

        let user_store = root.path().join("extensions.json");
        let project_store = root.path().join("project-extensions.json");
        with_extension_store_env(&user_store, &project_store, || {
            let first_string = path_string(&first)?;
            let second_string = path_string(&second)?;
            expect_true(
                matches!(
                    run_extension_install_command(&first_string, ExtensionInstallScope::User, true),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "first install should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_install_command(
                        &second_string,
                        ExtensionInstallScope::User,
                        true
                    ),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "second install should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_remove_command(&first_string, ExtensionInstallScope::User),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "remove by path should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_remove_command(
                        "example.remove-second",
                        ExtensionInstallScope::User
                    ),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "remove by manifest id should complete",
            )?;

            let lines = run_extension_list_command().render_lines();
            expect_true(
                lines.contains(&String::from("extension_count=0")),
                "extension list should be empty",
            )
        })
    }

    #[test]
    fn extension_diagnostics_report_malformed_install_store() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("malformed-store")?;
        let user_store = root.path().join("extensions.json");
        let project_store = root.path().join("project-extensions.json");
        expect_ok(std::fs::write(&user_store, "{not json"))?;

        with_extension_store_env(&user_store, &project_store, || {
            let lines = run_extension_list_command().render_lines();

            expect_true(
                lines.contains(&String::from("extension_outcome=Failed")),
                "diagnostics should fail",
            )?;
            expect_true(
                lines.contains(&String::from(
                    "message=extension diagnostics failed: store_malformed",
                )),
                "diagnostics should report malformed store",
            )
        })
    }

    #[test]
    fn native_config_defers_installed_roots_to_first_render_loader() -> Result<(), String> {
        let _guard = env_lock()?;
        let root = TestTempDir::new("native-roots")?;
        let enabled = root.path().join("enabled");
        let disabled = root.path().join("disabled");
        let env_root = root.path().join("env");
        expect_ok(std::fs::create_dir_all(&enabled))?;
        expect_ok(std::fs::create_dir_all(&disabled))?;
        expect_ok(std::fs::create_dir_all(&env_root))?;
        let expected_enabled = expect_ok(std::fs::canonicalize(&enabled))?;
        let expected_disabled = expect_ok(std::fs::canonicalize(&disabled))?;

        let user_store = root.path().join("extensions.json");
        let project_store = root.path().join("project-extensions.json");
        with_extension_store_env(&user_store, &project_store, || {
            let enabled = path_string(&enabled)?;
            let disabled = path_string(&disabled)?;
            unsafe {
                std::env::set_var("YACH_EXTENSION_PACKAGE_ROOTS", &env_root);
            }
            expect_true(
                matches!(
                    run_extension_install_command(&enabled, ExtensionInstallScope::User, true),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "enabled install should complete",
            )?;
            expect_true(
                matches!(
                    run_extension_install_command(&disabled, ExtensionInstallScope::User, false),
                    CommandResult::ExtensionManagement {
                        outcome: ExtensionManagementOutcome::Completed,
                        ..
                    }
                ),
                "disabled install should complete",
            )?;

            let config = native_dogfood_runner_config(temp_native_log_path(), None, None, None);

            expect_true(
                !config
                    .extension_package_roots
                    .iter()
                    .any(|root| root.root == enabled),
                "installed roots should not be in startup config roots",
            )?;
            expect_true(
                config.extension_package_roots.iter().any(|root| {
                    root.root == env_root
                        && root.source_ref.as_deref() == Some("env:YACH_EXTENSION_PACKAGE_ROOTS")
                }),
                "env roots should remain in startup config roots",
            )?;

            let Some(loader) = config.extension_package_root_loader.as_ref() else {
                return Err(String::from("missing extension package root loader"));
            };
            let installed_roots = loader.load();

            expect_true(
                installed_roots
                    .iter()
                    .any(|root| root.root == expected_enabled),
                "loader should include enabled install",
            )?;
            expect_true(
                !installed_roots
                    .iter()
                    .any(|root| root.root == expected_disabled),
                "loader should exclude disabled install",
            )?;
            unsafe {
                std::env::remove_var("YACH_EXTENSION_PACKAGE_ROOTS");
            }
            Ok(())
        })
    }

    #[test]
    fn native_tui_fresh_launch_ignores_existing_latest_session() {
        let latest = std::env::temp_dir().join("latest-native-session.jsonl");
        assert_eq!(
            native_tui_session_path_from_latest(true, Some(latest.clone()), "fresh-session"),
            latest
        );
        assert_eq!(
            native_tui_session_path_from_latest(false, Some(latest), "fresh-session"),
            native_session_log_path("fresh-session")
        );
    }

    #[test]
    fn native_tui_resume_without_existing_session_uses_fresh_session() {
        assert_eq!(
            native_tui_session_path_from_latest(true, None, "fresh-session"),
            native_session_log_path("fresh-session")
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
            let handle = tokio::spawn(run_native_dogfood_loop(
                client_rx,
                backend_tx,
                NativeDogfoodRunnerConfig {
                    session_path: path.clone(),
                    project_root: None,
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
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
            for _ in 0..64 {
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
    fn native_backend_config_uses_launch_cwd_as_project_root() {
        let expected = std::env::current_dir().ok();
        let config = native_dogfood_runner_config(temp_native_log_path(), None, None, None);

        assert!(expected.is_some());
        assert_eq!(config.project_root, expected);
    }

    #[test]
    fn native_provider_setup_error_copy_is_actionable() {
        let message = native_provider_setup_error_message(&RigSmokeConfigError::Missing(
            "YACH_RIG_ANTHROPIC_API_KEY",
        ));

        assert_eq!(
            message,
            "native provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn native_dogfood_loop_persists_failed_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-fail");

        assert!(persisted.contains("failed"));
        assert!(persisted.contains("provider_internal"));
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

    pub(super) fn run_native_fixture_prompt(prompt: &str) -> String {
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
            let handle = tokio::spawn(run_native_dogfood_loop(
                client_rx,
                backend_tx,
                NativeDogfoodRunnerConfig {
                    session_path: path.clone(),
                    project_root: None,
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                },
            ));

            assert!(
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: prompt.to_owned(),
                    })
                    .is_ok()
            );

            for _ in 0..64 {
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

    pub(super) fn temp_native_log_path() -> std::path::PathBuf {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "yach-native-dogfood-test-{}-{unique}-{id}.jsonl",
            std::process::id()
        ))
    }

    fn env_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        expect_ok(ENV_LOCK.lock())
    }

    fn with_extension_store_env(
        user: &Path,
        project: &Path,
        f: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        unsafe {
            std::env::set_var("YACH_EXTENSION_USER_STORE", user);
            std::env::set_var("YACH_EXTENSION_PROJECT_STORE", project);
        }
        let result = f();
        unsafe {
            std::env::remove_var("YACH_EXTENSION_USER_STORE");
            std::env::remove_var("YACH_EXTENSION_PROJECT_STORE");
        }
        result
    }

    fn write_test_extension_manifest(package_root: &Path, id: &str) -> Result<(), String> {
        let manifest = format!(
            r#"{{
  "schema": "yach.extension.v1",
  "id": "{id}",
  "version": "0.1.0",
  "main": {{
    "command": "node",
    "args": ["./extension.js"]
  }},
  "activation": {{
    "events": ["onCommand:{id}"]
  }},
  "contributes": {{
    "tools": []
  }}
}}"#
        );
        expect_ok(std::fs::write(
            package_root.join("yach.extension.json"),
            manifest,
        ))
    }

    fn path_string(path: &Path) -> Result<String, String> {
        path.to_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
    }

    fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> Result<T, String> {
        result.map_err(|error| format!("{error:?}"))
    }

    fn expect_equal<T>(actual: &T, expected: &T) -> Result<(), String>
    where
        T: std::fmt::Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(format!("expected {expected:?}, got {actual:?}"))
        }
    }

    fn expect_true(actual: bool, message: &str) -> Result<(), String> {
        if actual {
            Ok(())
        } else {
            Err(message.to_owned())
        }
    }

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(name: &str) -> Result<Self, String> {
            static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yach-cli-{name}-{}-{unique}-{id}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            expect_ok(std::fs::create_dir_all(&path))?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    pub(super) async fn wait_for_prompt_finished(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
        expected_outcome: yach_proto::PromptOutcome,
    ) -> bool {
        for _ in 0..64 {
            let event =
                tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv()).await;
            let Ok(Some(BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. }))) = event
            else {
                continue;
            };
            if outcome == expected_outcome {
                return true;
            }
        }

        false
    }

    pub(super) async fn collect_prompt_finished_for(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
        duration: std::time::Duration,
    ) -> Vec<yach_proto::PromptOutcome> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut outcomes = Vec::new();
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            let event = tokio::time::timeout_at(deadline, backend_rx.recv()).await;
            let Ok(Some(BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. }))) = event
            else {
                continue;
            };
            outcomes.push(outcome);
        }
        outcomes
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
    fn rendered_extension_diagnostics_are_stable_and_read_only() {
        let result = CommandResult::ExtensionDiagnostics {
            command: ExtensionDiagnosticsCommand::List,
            outcome: ExtensionDiagnosticsOutcome::Completed,
            records: vec![ExtensionDiagnosticRecord {
                id: Some(String::from("example.scan-toy-tools")),
                version: Some(String::from("0.1.0")),
                scope: ExtensionInstallScope::Project,
                package_root: PathBuf::from("/tmp/yach-extension"),
                manifest_path: Some(PathBuf::from("/tmp/yach-extension/yach.extension.json")),
                source_ref: Some(String::from("test-package-root")),
                install_source: Some(String::from("./ext")),
                install_enabled: true,
                discovered: true,
                activation_state: ExtensionActivationState::Discovered,
                generation: 0,
                last_error_kind: None,
                last_error_summary: None,
                registered_tools: Vec::new(),
                provider_visible_tools: Vec::new(),
            }],
            message: None,
            host_start_count: 0,
        };

        let lines = result.render_lines();

        assert_eq!(lines[0], "extension_command=list");
        assert_eq!(lines[1], "extension_outcome=Completed");
        assert_eq!(lines[2], "extension_count=1");
        assert_eq!(lines[3], "host_start_count=0");
        assert_eq!(
            lines[4],
            "extension id=example.scan-toy-tools version=0.1.0 scope=project package_root=/tmp/yach-extension manifest_path=/tmp/yach-extension/yach.extension.json source_ref=test-package-root install_source=./ext install_enabled=true discovered=true activation_state=discovered generation=0 last_error_kind=none last_error_summary=none registered_tool_count=0 registered_tools=none provider_visible_tools=none"
        );
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
