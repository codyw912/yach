use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use tokio::sync::mpsc;
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
    BackendEvent, Capability, ClientEvent, DialogKind, DialogRequest, Handshake, ServerEvent,
};
use yach_ui::{
    RunTuiOptions, StartupTrace, alpha_handshake, negotiate_with as negotiate_with_ui, run_tui,
    run_tui_with_startup_trace_and_options,
};

mod headless;

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
            Some("smoke-rig-openai-compatible") => Command::SmokeRigOpenAiCompatible,
            Some("smoke-openai-compatible-http") => Command::SmokeOpenAiCompatibleHttp,
            Some("smoke-rig-anthropic") => Command::SmokeRigAnthropic,
            Some("smoke-rig-chatgpt-subscription") => Command::SmokeRigChatGptSubscription,
            Some("smoke-rig-provider-request") => Command::SmokeRigProviderRequest,
            Some("smoke-compaction") => Command::SmokeCompaction {
                session_path: positional.get(1).cloned(),
            },
            Some("install") => extension_install_command_from_args(&positional[1..]),
            Some("extension") => extension_command_from_args(&positional[1..]),
            Some("run") => Command::Run {
                args: positional[1..].to_vec(),
            },
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
    SmokeRigOpenAiCompatible,
    SmokeOpenAiCompatibleHttp,
    SmokeRigAnthropic,
    SmokeRigChatGptSubscription,
    SmokeRigProviderRequest,
    SmokeCompaction {
        session_path: Option<String>,
    },
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
    Run {
        args: Vec<String>,
    },
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
                _ => None,
            },
        )
        .unwrap_or(TuiBackendSelection::NativeProvider)
}

fn selected_tui_resume(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--resume")
}

impl Command {
    fn run(&self, quiet: bool, startup_trace: Option<&StartupTrace>) -> CommandResult {
        if let Some(trace) = startup_trace {
            trace.mark("command_run_start");
        }
        match self {
            Self::Version => CommandResult::Version,
            Self::Run { args } => run_headless_cli_command(args, quiet),
            Self::Help => CommandResult::Usage,
            Self::Unknown { name } => CommandResult::UsageError {
                message: format!("unknown command '{name}'"),
            },
            Self::PrintCapabilities => print_capabilities(),
            Self::SmokeRigOpenAiCompatible => run_rig_openai_compatible_smoke(),
            Self::SmokeOpenAiCompatibleHttp => run_openai_compatible_http_smoke_command(),
            Self::SmokeRigAnthropic => run_rig_anthropic_smoke(),
            Self::SmokeRigChatGptSubscription => run_rig_chatgpt_subscription_smoke(),
            Self::SmokeRigProviderRequest => run_rig_provider_request_smoke(),
            Self::SmokeCompaction { session_path } => run_compaction_smoke(session_path.as_deref()),
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
    Tui {
        exited: bool,
    },
    CompactionSmoke {
        outcome: RigSmokeOutcome,
        lines: Vec<String>,
    },
    /// `yach run` writes its outcome document itself (stdout or file);
    /// only the exit code flows back through the command result.
    HeadlessRun {
        exit_code: u8,
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
            Self::HeadlessRun { exit_code } => *exit_code,
            Self::Version
            | Self::Usage
            | Self::Capabilities { .. }
            | Self::RigOpenAiCompatibleSmoke { .. }
            | Self::OpenAiCompatibleHttpSmoke { .. }
            | Self::ExtensionDiagnostics { .. }
            | Self::ExtensionManagement { .. }
            | Self::Tui { .. }
            | Self::CompactionSmoke { .. } => 0,
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
            Self::Tui { exited } => vec![format!("tui_exited={exited}")],
            Self::CompactionSmoke { outcome, lines } => {
                let mut rendered = vec![format!("compaction_smoke_outcome={outcome:?}")];
                rendered.extend(lines.clone());
                rendered
            }
            Self::HeadlessRun { .. } => Vec::new(),
        }
    }
}

/// `yach run`: parse flags, load the provider from env, and hand off to
/// the headless driver. Setup failures exit 2 without emitting an
/// outcome document; from the driver onward one is always emitted.
fn run_headless_cli_command(args: &[String], global_quiet: bool) -> CommandResult {
    // Setup errors go to stderr: `yach run` reserves stdout for the
    // outcome document, and a `> outcome.json` redirect must never eat
    // the explanation (rotation dogfood finding 2026-07-26).
    let setup_error = |message: String| {
        let mut stderr = io::stderr();
        let _ = writeln!(stderr, "error={message}");
        for line in usage_lines() {
            let _ = writeln!(stderr, "{line}");
        }
        CommandResult::HeadlessRun {
            exit_code: headless::EXIT_SETUP_ERROR,
        }
    };
    let mut options = match headless::parse_run_args(args) {
        Ok(options) => options,
        Err(message) => return setup_error(message),
    };
    options.quiet |= global_quiet;
    let adapter =
        match rig_provider_adapter_config_from_env_with_model_override(options.model.is_some()) {
            Ok(config) => config,
            Err(error) => return setup_error(rig_config_error_message(&error)),
        };
    let provider_label = native_provider_label_from_config(&adapter);
    let provider = NativeProviderDogfoodConfig {
        // --model overrides the env-derived model (yacht substitutes its
        // vessel model via this flag).
        model: options
            .model
            .clone()
            .unwrap_or_else(|| native_provider_model_from_env(provider_label)),
        test_delay_ms: native_provider_test_delay_ms(),
        adapter,
    };
    let exit_code = headless::run_headless_command(
        &options,
        provider,
        extension_package_roots_from_env(),
        Some(native_extension_package_root_loader()),
    );
    CommandResult::HeadlessRun { exit_code }
}

fn usage_lines() -> Vec<String> {
    vec![
        String::from("usage: yach [options]            start an interactive session"),
        String::from("       yach <command> [options]"),
        String::from("commands: run, extension, install, print-capabilities"),
        String::from("options: --resume, --backend <native|native-fixture>, --version, --help"),
        String::from(
            "run: headless session — --prompt <text> | --script <jsonl>, --project-root <dir>,",
        ),
        String::from(
            "     --session-path <file>, --full-auto, --turn-timeout-secs <n>, --outcome <file|->, --quiet",
        ),
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

fn print_capabilities() -> CommandResult {
    let handshake = native_backend_handshake();
    CommandResult::Capabilities {
        capabilities: handshake.capabilities,
    }
}

/// Capabilities of the native backend, used for capability printing and
/// backend-free TUI smoke paths.
fn native_backend_handshake() -> Handshake {
    Handshake::new(
        "yach-native-dogfood",
        vec![
            Capability::PromptStreaming,
            Capability::PromptCancellation,
            Capability::StatusEntries,
            Capability::Notifications,
            Capability::LocalEdit,
            Capability::ExtensionLifecycle,
            Capability::FirstRenderEvents,
        ],
    )
}

fn rig_provider_adapter_config_from_env() -> Result<RigProviderAdapterConfig, RigSmokeConfigError> {
    rig_provider_adapter_config_from_env_with_model_override(false)
}

/// `model_overridden` is true when the caller supplies the model itself
/// (`yach run --model`, yacht's `{model}` substitution) — the
/// openai-compatible fail-fast model check is skipped then.
fn rig_provider_adapter_config_from_env_with_model_override(
    model_overridden: bool,
) -> Result<RigProviderAdapterConfig, RigSmokeConfigError> {
    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let provider = match provider.as_str() {
        "anthropic" => RigProviderConfig::Anthropic {
            api_key: required_env("YACH_RIG_ANTHROPIC_API_KEY")?,
            base_url: optional_env("YACH_RIG_ANTHROPIC_BASE_URL"),
        },
        "chatgpt-subscription" => RigProviderConfig::ChatGptSubscription {
            token_dir: PathBuf::from(required_env("YACH_RIG_CHATGPT_TOKEN_DIR")?),
        },
        // Stopgap env wiring for rotation; the friendlier provider/model
        // product surface is a slated design item (docs/project/board.md).
        "openai-compatible" => {
            // The model has no sane universal default on compat endpoints;
            // require it up front so misconfiguration fails at setup —
            // unless the caller overrides the model directly.
            if !model_overridden {
                let _ = required_env("YACH_RIG_OPENAI_COMPAT_MODEL")?;
            }
            RigProviderConfig::OpenAiCompatible {
                base_url: required_env("YACH_RIG_OPENAI_COMPAT_BASE_URL")?,
                api_key: required_env("YACH_RIG_OPENAI_COMPAT_API_KEY")?,
            }
        }
        _ => {
            return Err(RigSmokeConfigError::InvalidValue {
                name: "YACH_RIG_PROVIDER",
                value: provider,
                reason: "must be anthropic, chatgpt-subscription, or openai-compatible",
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
        // 32k is the cohort's modal per-turn output budget (Claude Code and
        // opencode default to it) and is within every current Claude model's
        // ceiling. Thinking tokens count inside this budget, so small values
        // truncate responses mid-tool-call (stop_reason=max_tokens). Revisit
        // when a model catalog can supply per-model ceilings; see
        // docs/project/records/2026-07-16-max-output-tokens-research.md.
        max_tokens: optional_bounded_env("YACH_RIG_PROVIDER_MAX_TOKENS", 32_000, 1024, 128_000)?,
        // 200k is every current Claude model's standard window; the value
        // only feeds compaction accounting, which carries threshold slack.
        context_window: optional_bounded_env(
            "YACH_RIG_PROVIDER_CONTEXT_WINDOW",
            200_000,
            10_000,
            2_000_000,
        )?,
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
                    "YACH_RIG_PROVIDER must be anthropic, chatgpt-subscription, or openai-compatible",
                )),
            };
        }
    };
    let provider_config = match provider.as_str() {
        "anthropic" => match required_env("YACH_RIG_ANTHROPIC_API_KEY") {
            Ok(api_key) => RigProviderConfig::Anthropic {
                api_key,
                base_url: optional_env("YACH_RIG_ANTHROPIC_BASE_URL"),
            },
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
            context_window: 200_000,
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

/// Run the real summarization pass over an existing session log and print
/// the summary for human inspection — the tool for judging continuation
/// quality and iterating on the summary prompt (the Pi gated-live-test
/// pattern, in yach's smoke-command idiom). Requires the same `YACH_RIG_*`
/// provider environment as the other rig smokes.
fn run_compaction_smoke(session_path: Option<&str>) -> CommandResult {
    let failed = |lines: Vec<String>| CommandResult::CompactionSmoke {
        outcome: RigSmokeOutcome::Failed,
        lines,
    };
    let Some(session_path) = session_path else {
        return CommandResult::CompactionSmoke {
            outcome: RigSmokeOutcome::MissingConfig,
            lines: vec![String::from("usage: yach smoke-compaction <session.jsonl>")],
        };
    };
    let log = match yach_backend::NativeJsonlSessionStore::new(PathBuf::from(session_path)).load() {
        Ok(log) => log,
        Err(error) => {
            return failed(vec![format!("failed to load session log: {error}")]);
        }
    };
    let config = yach_backend::NativeCompactionConfig::default();
    let current_estimate = yach_backend::estimate_current_context_tokens(&log);
    let mut lines = vec![
        format!("session_path={session_path}"),
        format!("event_count={}", log.events.len()),
        format!("estimated_context_tokens={current_estimate}"),
        format!("keep_recent_tokens={}", config.keep_recent_tokens),
    ];
    let Some(cut) = yach_backend::select_compaction_cut(&log, config.keep_recent_tokens) else {
        lines.push(String::from(
            "nothing to compact: the whole session fits the kept budget",
        ));
        return failed(lines);
    };
    let previous = yach_backend::newest_compaction_checkpoint(&log);
    let preparation = yach_backend::CompactionPreparation {
        serialized_conversation: yach_backend::serialize_events_for_summary(
            &log.events[cut.fold_range.clone()],
        ),
        previous_summary: previous.as_ref().map(|view| view.summary.to_owned()),
        previous_details: previous.as_ref().map(|view| view.details.clone()),
        first_kept_entry_id: cut.first_kept_entry_id.clone(),
        tokens_before: current_estimate,
        reason: yach_backend::NativeCompactionReason::Manual,
        focus_instructions: None,
    };
    lines.push(format!(
        "folded_events={} kept_from_entry={}",
        cut.fold_range.len(),
        cut.first_kept_entry_id.0
    ));
    lines.push(format!(
        "serialized_conversation_chars={}",
        preparation.serialized_conversation.chars().count()
    ));
    if preparation.previous_summary.is_some() {
        lines.push(String::from("anchored=true (previous checkpoint found)"));
    }

    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let model = match provider.as_str() {
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-haiku-4-5")),
        "chatgpt-subscription" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        _ => {
            lines.push(String::from(
                "YACH_RIG_PROVIDER must be anthropic, chatgpt-subscription, or openai-compatible",
            ));
            return failed(lines);
        }
    };
    let adapter_config = match rig_provider_adapter_config_from_env() {
        Ok(config) => config,
        Err(error) => {
            lines.push(rig_config_error_message(&error));
            return CommandResult::CompactionSmoke {
                outcome: RigSmokeOutcome::MissingConfig,
                lines,
            };
        }
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        lines.push(String::from("failed to create tokio runtime"));
        return failed(lines);
    };
    let request = ProviderRequest {
        turn_id: NativeTurnId(String::from("compaction-smoke-turn")),
        model: ProviderModel { provider, model },
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: yach_backend::build_summary_prompt(&preparation),
        }],
        extensions: vec![],
    };
    let started = std::time::Instant::now();
    match runtime.block_on(run_provider_request(adapter_config, request)) {
        Ok(events) => {
            let summary: String = events
                .iter()
                .filter_map(|event| match event {
                    yach_backend::ProviderStreamEvent::TextDelta { delta, .. } => {
                        Some(delta.as_str())
                    }
                    _ => None,
                })
                .collect();
            if summary.trim().is_empty() {
                lines.push(String::from("summarizer returned no text"));
                return failed(lines);
            }
            let kept_tail_tokens: u64 = log.events[cut.kept_start_index..]
                .iter()
                .map(yach_backend::estimate_event_tokens)
                .sum();
            lines.push(format!("duration_ms={}", started.elapsed().as_millis()));
            lines.push(format!(
                "estimated_tokens_after={}",
                yach_backend::estimate_text_tokens(&summary).saturating_add(kept_tail_tokens)
            ));
            lines.push(String::from("--- summary ---"));
            lines.extend(summary.lines().map(str::to_owned));
            CommandResult::CompactionSmoke {
                outcome: RigSmokeOutcome::Completed,
                lines,
            }
        }
        Err(error) => {
            lines.push(redacted_provider_error_message(&error));
            failed(lines)
        }
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
    let timeout_secs = match optional_bounded_env("YACH_RIG_ANTHROPIC_TIMEOUT_SECS", 120, 5, 600) {
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
            let negotiated = negotiate_with_ui(&native_backend_handshake());
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
    let adapter_handshake = native_backend_handshake();
    let negotiated = negotiate_with_ui(&adapter_handshake);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };

    match runtime.block_on(async move {
        let backend_session = start_backend_session(BackendMetadata::native_dogfood(), negotiated);
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
        // Sonnet is the interactive default: coding sessions need more
        // capability than the haiku-tier smoke-test default. Overridable
        // per launch and switchable live via /model.
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-sonnet-5")),
        "chatgpt-subscription" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        // No sane universal default on compat endpoints; config parsing
        // requires this env when the provider is selected.
        "openai-compatible" => optional_env("YACH_RIG_OPENAI_COMPAT_MODEL").unwrap_or_default(),
        _ => String::from("unknown"),
    }
}

fn native_provider_label_from_config(config: &RigProviderAdapterConfig) -> &'static str {
    match &config.provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "chatgpt-subscription",
        RigProviderConfig::OpenAiCompatible { .. } => "openai-compatible",
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
                            base_url: None,
                        },
                        timeout: std::time::Duration::from_millis(1),
                        max_tokens: 1,
                        context_window: 200_000,
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
                        context_window: 200_000,
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
        RigSmokeConfigError, RigSmokeOutcome, TuiBackendSelection, dialog_smoke_requests,
        extension_store_path, native_dogfood_runner_config, native_provider_setup_error_message,
        native_tui_session_path_from_latest, print_capabilities, run_extension_install_command,
        run_extension_list_command, run_extension_remove_command,
        run_extension_set_enabled_command,
    };
    use std::path::{Path, PathBuf};
    use tokio::sync::mpsc;
    use yach_backend::{
        ExtensionActivationState, ExtensionInstallScope, NativeDogfoodRunnerConfig,
        native_session_log_path, run_native_dogfood_loop,
    };
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

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
        let backend = CliArgs::from_args(
            [String::from("--backend"), String::from("native-fixture")].into_iter(),
        );

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
                backend: TuiBackendSelection::NativeFixture,
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
        let dialog_smoke = CliArgs::from_args([String::from("tui-dialog-smoke")].into_iter());
        let tui = CliArgs::from_args([String::from("tui")].into_iter());
        let resume_tui =
            CliArgs::from_args([String::from("tui"), String::from("--resume")].into_iter());
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
        assert_eq!(dialog_smoke.command, Command::TuiDialogSmoke);
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
    fn rendered_capabilities_are_stable() {
        let lines = print_capabilities().render_lines();

        assert!(!lines.is_empty());
        assert!(lines[0].starts_with("capability="));
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
}
