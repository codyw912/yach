use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use tokio::sync::mpsc;
use yach_connections::{ConnectionId, CredentialError, CredentialStore, ProviderSecret};

use yach_backend::{
    BackendMetadata, CatalogModelEntry, ExtensionActivationDiagnostic,
    ExtensionActivationErrorKind, ExtensionActivationState, ExtensionInstallError,
    ExtensionInstallRecord, ExtensionInstallRefKind, ExtensionInstallScope, ExtensionInstallStore,
    ExtensionManifestIndex, ExtensionPackageRoot, ExtensionPackageRootLoader, ModelDiscoveryFuture,
    ModelDiscoveryOutcome, ProviderConfig, ProviderError, ProviderErrorKind, ProviderMessage,
    ProviderModel, ProviderRequest, Role, RunnerConfig, StartupTraceMarker, TurnId,
    fresh_session_id, latest_native_session_log_path,
    model_discovery::DiscoveredProviderModel,
    rig_adapter::{
        MaxTokensParam, RigProviderAdapterConfig, RigProviderConfig, run_provider_request,
    },
    rig_diagnostics::{
        RigAnthropicSmokeConfig, RigChatGptSubscriptionSmokeConfig, RigOpenAiCompatibleSmokeConfig,
        RigOpenAiSmokeConfig, run_anthropic_smoke, run_chatgpt_subscription_smoke,
        run_openai_compatible_http_smoke, run_openai_compatible_smoke, run_openai_smoke,
    },
    run_native_loop, run_native_loop_with_negotiated_capabilities, session_log_path,
    start_backend_session,
};
use yach_proto::{
    BackendEvent, Capability, ClientEvent, DialogKind, DialogRequest, Handshake, ModelInfo,
    NegotiatedCapabilities, PromptOutcome, ServerEvent,
};
use yach_ui::{
    RunTuiOptions, StartupTrace, Theme, alpha_handshake, negotiate_with as negotiate_with_ui,
    run_tui, run_tui_with_startup_trace_and_options,
};
mod model_discovery_cache;
mod provider_connections;

mod catalog_refresh;
mod headless;
mod rpc;

fn main() -> ExitCode {
    let startup_trace = StartupTrace::from_env("YACH_STARTUP_TRACE");
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("process_main_start");
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("__extension-host")
        && args.get(1).map(String::as_str) == Some("hashline")
    {
        return match yach_hashline_extension::run_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(1),
        };
    }
    let cli = CliArgs::from_args(args.into_iter());
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
            Some("smoke-rig-openai") => Command::SmokeRigOpenAi,
            Some("smoke-rig-chatgpt-subscription") => Command::SmokeRigChatGptSubscription,
            Some("smoke-rig-provider-request") => Command::SmokeRigProviderRequest,
            Some("smoke-compaction") => Command::SmokeCompaction {
                session_path: positional.get(1).cloned(),
            },
            Some("smoke-responses-compaction") => Command::SmokeResponsesCompaction,
            Some("install") => extension_install_command_from_args(&positional[1..]),
            Some("extension") => extension_command_from_args(&positional[1..]),
            Some("rpc") => Command::Rpc {
                args: positional[1..].to_vec(),
            },
            Some("run") => Command::Run {
                args: positional[1..].to_vec(),
            },
            Some("tui") => Command::Tui {
                backend: selected_tui_backend(&positional[1..]),
                resume: selected_tui_resume(&positional[1..]),
            },
            Some("tui-dialog-smoke") => Command::TuiDialogSmoke,
            Some("tui-provider-connection-smoke") => Command::TuiProviderConnectionSmoke,
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
    SmokeRigOpenAi,
    SmokeRigChatGptSubscription,
    SmokeRigProviderRequest,
    SmokeResponsesCompaction,
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
    Rpc {
        args: Vec<String>,
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
    TuiProviderConnectionSmoke,
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
    Fixture,
    Provider,
}

fn selected_tui_backend(args: &[String]) -> TuiBackendSelection {
    args.windows(2)
        .find_map(
            |window| match (window.first().map(String::as_str), window.get(1)) {
                // `fixture` selects the scripted echo runner; the real
                // provider runner is the default and needs no value.
                (Some("--backend"), Some(value)) if value == "fixture" => {
                    Some(TuiBackendSelection::Fixture)
                }
                _ => None,
            },
        )
        .unwrap_or(TuiBackendSelection::Provider)
}

fn selected_tui_resume(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--resume")
}

fn load_tui_theme(project_root: Option<&Path>) -> Result<Theme, String> {
    let explicit = std::env::var_os("YACH_THEME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(path) = tui_theme_path(project_root, explicit.as_deref(), home.as_deref()) else {
        return Ok(Theme::default());
    };
    Theme::load(&path).map_err(|error| format!("failed to load theme {}: {error}", path.display()))
}

fn tui_theme_path(
    project_root: Option<&Path>,
    explicit: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }

    let project_theme = project_root.map(|root| root.join(".yach/theme.json"));
    if project_theme.as_ref().is_some_and(|path| path.is_file()) {
        return project_theme;
    }

    let user_theme = home.map(|root| root.join(".yach/theme.json"));
    user_theme.filter(|path| path.is_file())
}

impl Command {
    fn run(&self, quiet: bool, startup_trace: Option<&StartupTrace>) -> CommandResult {
        if let Some(trace) = startup_trace {
            trace.mark("command_run_start");
        }
        match self {
            Self::Version => CommandResult::Version,
            Self::Run { args } => run_headless_cli_command(args, quiet),
            Self::Rpc { args } => run_rpc_cli_command(args),
            Self::Help => CommandResult::Usage,
            Self::Unknown { name } => CommandResult::UsageError {
                message: format!("unknown command '{name}'"),
            },
            Self::PrintCapabilities => print_capabilities(),
            Self::SmokeRigOpenAiCompatible => run_rig_openai_compatible_smoke(),
            Self::SmokeOpenAiCompatibleHttp => run_openai_compatible_http_smoke_command(),
            Self::SmokeRigAnthropic => run_rig_anthropic_smoke(),
            Self::SmokeRigOpenAi => run_rig_openai_smoke(),
            Self::SmokeResponsesCompaction => run_responses_compaction_smoke(),
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
            Self::TuiProviderConnectionSmoke => run_tui_provider_connection_smoke_command(),
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
    TuiProviderConnectionSmoke {
        passed: bool,
        fixture_models: bool,
        fixture_prompt: bool,
        prompt_finished: bool,
        exact_activation_count: u64,
        active_removal_rejected: bool,
    },
    CompactionSmoke {
        outcome: RigSmokeOutcome,
        lines: Vec<String>,
    },
    ResponsesCompactionSmoke {
        outcome: RigSmokeOutcome,
        model: Option<String>,
        stages: Vec<ResponsesCompactionSmokeStage>,
        artifact_item_count: usize,
        token_count: u64,
    },
    /// `yach run` writes its outcome document itself (stdout or file);
    /// only the exit code flows back through the command result.
    HeadlessRun {
        exit_code: u8,
    },
    /// `yach rpc` owns its JSONL stdout; only the process exit code returns
    /// through the command result.
    Rpc {
        exit_code: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesCompactionSmokeStage {
    ResponsesTurn,
    NativeCompact,
    PortableSummary,
    ReplayedContinuation,
    ModelSwitchReplay,
}

impl ResponsesCompactionSmokeStage {
    const fn label(self) -> &'static str {
        match self {
            Self::ResponsesTurn => "responses_turn",
            Self::NativeCompact => "native_compact",
            Self::PortableSummary => "portable_summary",
            Self::ReplayedContinuation => "replayed_continuation",
            Self::ModelSwitchReplay => "model_switch_replay",
        }
    }
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
            Self::TuiProviderConnectionSmoke { passed, .. } => {
                if *passed {
                    0
                } else {
                    1
                }
            }
            Self::HeadlessRun { exit_code } | Self::Rpc { exit_code } => *exit_code,
            Self::Version
            | Self::Usage
            | Self::Capabilities { .. }
            | Self::RigOpenAiCompatibleSmoke { .. }
            | Self::OpenAiCompatibleHttpSmoke { .. }
            | Self::ExtensionDiagnostics { .. }
            | Self::ExtensionManagement { .. }
            | Self::Tui { .. }
            | Self::CompactionSmoke { .. }
            | Self::ResponsesCompactionSmoke { .. } => 0,
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
            Self::TuiProviderConnectionSmoke {
                passed,
                fixture_models,
                fixture_prompt,
                prompt_finished,
                exact_activation_count,
                active_removal_rejected,
            } => vec![
                format!(
                    "provider_connection_smoke={}",
                    if *passed { "passed" } else { "failed" }
                ),
                format!("fixture_models={fixture_models}"),
                format!("fixture_prompt={fixture_prompt}"),
                format!("prompt_finished={prompt_finished}"),
                format!("exact_activation_count={exact_activation_count}"),
                format!("active_removal_rejected={active_removal_rejected}"),
            ],
            Self::CompactionSmoke { outcome, lines } => {
                let mut rendered = vec![format!("compaction_smoke_outcome={outcome:?}")];
                rendered.extend(lines.clone());
                rendered
            }
            Self::ResponsesCompactionSmoke {
                outcome,
                model,
                stages,
                artifact_item_count,
                token_count,
            } => {
                let mut rendered = vec![format!(
                    "responses_compaction_smoke={}",
                    match outcome {
                        RigSmokeOutcome::Completed => "passed",
                        RigSmokeOutcome::Failed => "failed",
                        RigSmokeOutcome::MissingConfig => "missing_config",
                    }
                )];
                if let Some(model) = model {
                    rendered.push(format!("model={model}"));
                }
                rendered.extend(
                    stages
                        .iter()
                        .map(|stage| format!("stage={}", stage.label())),
                );
                rendered.push(format!("artifact_item_count={artifact_item_count}"));
                rendered.push(format!("token_count={token_count}"));
                if matches!(outcome, RigSmokeOutcome::MissingConfig) {
                    rendered.push(String::from("prerequisite=YACH_RIG_OPENAI_API_KEY"));
                }
                rendered
            }
            Self::HeadlessRun { .. } | Self::Rpc { .. } => Vec::new(),
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
    // Load the invocation's one snapshot before spawning its background
    // refresh. Resolution uses this clone; a completed refresh only feeds a
    // later invocation and never blocks the current one.
    let layers = ModelOverrideLayers::load_for_project(options.project_root.as_deref());
    let catalog_refresh = catalog_refresh::spawn_refresh_status(layers.fetched.clone());
    // One resolution shared by the request path and the outcome document
    // (`resolved.profile` / `resolved.output_budget`) — the document
    // reports exactly what the config that ran this session used, never
    // a second, independently-resolved copy of it.
    let resolved = match rig_provider_adapter_config_from_env_with_model_override(
        options.model.as_deref(),
        &layers,
    ) {
        Ok(resolved) => resolved,
        Err(error) => return setup_error(rig_config_error_message(&error)),
    };
    let provider = ProviderConfig {
        model: resolved.model,
        connection_id: None,
        connection_display: None,
        test_delay_ms: provider_test_delay_ms(),
        adapter: Arc::new(resolved.adapter),
        catalog_models: Vec::new().into(),
        responses_compact: resolved
            .profile
            .responses_compact
            .as_ref()
            .map(|capability| capability.value),
    };
    let exit_code = headless::run_headless_command(
        &options,
        provider,
        &resolved.profile,
        &resolved.output_budget,
        extension_package_roots_from_env(),
        Some(extension_package_root_loader()),
        catalog_refresh,
    );
    CommandResult::HeadlessRun { exit_code }
}
fn run_rpc_cli_command(args: &[String]) -> CommandResult {
    let options = match rpc::parse_rpc_args(args) {
        Ok(options) => options,
        Err(message) => {
            let mut stderr = io::stderr();
            let _ = writeln!(stderr, "error={message}");
            for line in usage_lines() {
                let _ = writeln!(stderr, "{line}");
            }
            return CommandResult::Rpc { exit_code: 2 };
        }
    };
    CommandResult::Rpc {
        exit_code: rpc::run_rpc_command(options),
    }
}

fn usage_lines() -> Vec<String> {
    vec![
        String::from("usage: yach [options]            start an interactive session"),
        String::from("       yach <command> [options]"),
        String::from("commands: run, rpc, extension, install, print-capabilities"),
        String::from("options: --resume, --backend fixture, --version, --help"),
        String::from(
            "rpc: protocol server — ClientEvent JSONL on stdin, ServerEvent JSONL on stdout,",
        ),
        String::from("     --project-root <dir>, --session <id> | --session-path <file>,"),
        String::from("     --backend fixture"),
        String::from(
            "run: headless session — --prompt <text> | --script <jsonl>, --project-root <dir>,",
        ),
        String::from("     --session <id> | --session-path <file>, --model <id>, --full-auto,"),
        String::from("     --turn-timeout-secs <n>, --outcome <file|->, --quiet"),
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
    let handshake = backend_handshake();
    CommandResult::Capabilities {
        capabilities: handshake.capabilities,
    }
}

/// Capabilities of the native backend, used for capability printing and
/// backend-free TUI smoke paths.
fn backend_handshake() -> Handshake {
    Handshake::new(
        "yach-native",
        vec![
            Capability::PromptStreaming,
            Capability::PromptCancellation,
            Capability::StatusEntries,
            Capability::Notifications,
            Capability::LocalEdit,
            Capability::ExtensionLifecycle,
            Capability::FirstRenderEvents,
            Capability::StructuredReviewRows,
        ],
    )
}

/// Convenience wrapper for a single-resolution call site (the compaction
/// smoke path and the TUI's config check): it needs no discovery snapshot,
/// so it loads its own `ModelOverrideLayers`.
fn rig_provider_adapter_config_from_env() -> Result<RigProviderAdapterConfig, RigSmokeConfigError> {
    let layers = ModelOverrideLayers::load_for_project(None);
    rig_provider_adapter_config_from_env_with_model_override(None, &layers)
        .map(|resolved| resolved.adapter)
}

/// The config layer's one catalog resolution, shared by the request path
/// (`adapter`) and, for `yach run`, the outcome document (`profile` /
/// `output_budget`). Threading this through rather than re-resolving in
/// the headless driver is what makes the document and the request that
/// produced it structurally unable to disagree.
struct ResolvedProviderConfig {
    adapter: RigProviderAdapterConfig,
    model: String,
    profile: yach_catalog::ModelProfile,
    output_budget: yach_catalog::Sourced<u64>,
}

/// `model_override` is `Some` when the caller supplies the model itself
/// (`yach run --model`, yacht's `{model}` substitution): the
/// openai-compatible/openai fail-fast model-env check is skipped, and the
/// supplied model — not the env-derived one `provider_model_from_env`
/// would produce — is what catalog resolution (`resolve_model_profile`)
/// uses, so `max_tokens` / `context_window` / `max_tokens_param` describe
/// the model that will actually run rather than the env default.
/// `layers` is the caller's one `ModelOverrideLayers::load_for_project()` for
/// the invocation. The interactive path clones this same snapshot into its
/// inert discovery future, so selection metadata cannot observe a different
/// catalog generation.
fn rig_provider_adapter_config_from_env_with_model_override(
    model_override: Option<&str>,
    layers: &ModelOverrideLayers,
) -> Result<ResolvedProviderConfig, RigSmokeConfigError> {
    let provider_label =
        optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let provider = match provider_label.as_str() {
        "anthropic" => RigProviderConfig::Anthropic {
            api_key: ProviderSecret::new(required_env("YACH_RIG_ANTHROPIC_API_KEY")?),
            base_url: optional_env("YACH_RIG_ANTHROPIC_BASE_URL"),
        },
        "openai-codex" => RigProviderConfig::ChatGptSubscription {
            auth_file: PathBuf::from(required_env("YACH_RIG_CHATGPT_TOKEN_DIR")?).join("auth.json"),
        },
        // Stopgap env wiring for rotation; the friendlier provider/model
        // product surface is a slated design item (docs/project/board.md).
        "openai-compatible" => {
            // The model has no sane universal default on compat endpoints;
            // require it up front so misconfiguration fails at setup —
            // unless the caller overrides the model directly.
            if model_override.is_none() {
                let _ = required_env("YACH_RIG_OPENAI_COMPAT_MODEL")?;
            }
            RigProviderConfig::OpenAiCompatible {
                base_url: required_env("YACH_RIG_OPENAI_COMPAT_BASE_URL")?,
                api_key: ProviderSecret::new(required_env("YACH_RIG_OPENAI_COMPAT_API_KEY")?),
            }
        }
        // OpenAI proper over the Responses API (canonical endpoint).
        // Aggregators wearing the chat-completions shape use
        // openai-compatible. Like compat, no default model: require it
        // up front so misconfiguration fails at setup, unless the
        // caller overrides the model directly.
        "openai" => {
            if model_override.is_none() {
                let _ = required_env("YACH_RIG_OPENAI_MODEL")?;
            }
            RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(required_env("YACH_RIG_OPENAI_API_KEY")?),
                base_url: optional_env("YACH_RIG_OPENAI_BASE_URL"),
            }
        }
        _ => {
            return Err(RigSmokeConfigError::InvalidValue {
                name: "YACH_RIG_PROVIDER",
                value: provider_label,
                reason: "must be anthropic, openai-codex, openai, or openai-compatible",
            });
        }
    };

    // Fail fast on a malformed numeric override, same as every other
    // numeric env var here (e.g. TIMEOUT_SECS below). A bad *override
    // file* degrades to a warning instead (see `resolve_model_profile`),
    // but an operator-typed env var still errors out at setup — env vars
    // are typed by whoever launches the session, not shipped alongside it.
    let env_max_tokens =
        optional_bounded_env_value("YACH_RIG_PROVIDER_MAX_TOKENS", 1_024, 128_000)?;
    optional_bounded_env_value("YACH_RIG_PROVIDER_CONTEXT_WINDOW", 10_000, 2_000_000)?;

    let model = resolved_model_for_config(&provider_label, model_override);
    let profile = resolve_model_profile(layers, &provider_label, &model);
    let max_tokens_param = max_tokens_param_from_catalog(profile.output_tokens_param.value);
    // Resolved through yach-catalog: baked snapshot -> user
    // (~/.yach/models.toml) -> project (.yach/models.toml) -> env, per
    // field, with env winning outright and a 32k/200k/MaxTokens floor when
    // nothing else supplies a value. The cohort-default and floor
    // semantics (previously inlined here) now live in the crate; see
    // yach_catalog::resolve and yach_catalog::effective_output_budget, and
    // docs/project/records/2026-07-16-max-output-tokens-research.md. This
    // is the process's only call to `effective_output_budget` for the
    // model that will run — the outcome document (for `yach run`) reports
    // this same `Sourced<u64>`, never a second, independent resolution.
    let output_budget = yach_catalog::effective_output_budget(&profile, env_max_tokens);

    Ok(ResolvedProviderConfig {
        adapter: RigProviderAdapterConfig {
            provider,
            timeout: Duration::from_secs(optional_bounded_env(
                "YACH_RIG_PROVIDER_TIMEOUT_SECS",
                120,
                5,
                600,
            )?),
            max_tokens: output_budget.value,
            context_window: profile.context_window.value,
            max_tokens_param,
        },
        model,
        profile,
        output_budget,
    })
}

/// Parses the `YACH_RIG_PROVIDER_MAX_TOKENS_PARAM` spelling into the
/// catalog's enum, falling back to the same default the catalog itself
/// falls back to when nothing else supplies a value. Pure, so the parsing
/// is testable with a plain `&str` rather than a real env var.
fn output_tokens_param_from_env_value(value: &str) -> yach_catalog::OutputTokensParam {
    match value {
        "max_completion_tokens" => yach_catalog::OutputTokensParam::MaxCompletionTokens,
        _ => yach_catalog::OutputTokensParam::MaxTokens,
    }
}

/// Maps the catalog's provider-agnostic output-tokens-param data to the
/// rig-adapter's own spelling enum. Pure glue between `yach-catalog` (data
/// layer, no rig dependency) and `yach-backend`'s `RigProviderAdapterConfig`
/// (rig-facing, no catalog dependency).
fn max_tokens_param_from_catalog(value: yach_catalog::OutputTokensParam) -> MaxTokensParam {
    match value {
        yach_catalog::OutputTokensParam::MaxTokens => MaxTokensParam::MaxTokens,
        yach_catalog::OutputTokensParam::MaxCompletionTokens => MaxTokensParam::MaxCompletionTokens,
    }
}

/// Read and parse a models.toml-shaped override file. Missing files and
/// I/O errors degrade to absent silently (a file that isn't there is not a
/// misconfiguration); a file that exists but fails to parse degrades to
/// absent with a stderr warning — a bad correction file must never block a
/// session. A well-formed-but-wrong-shaped file (e.g. a stray top-level
/// table that isn't itself a table of per-model tables, such as a future
/// `[settings]` block or a provider-name typo carrying scalar fields) is
/// rejected by `Overrides::from_toml_str` as the same kind of parse error
/// — `#[serde(flatten)]` requires every top-level table's values to
/// themselves deserialize as `CatalogEntry` tables, so a scalar-valued
/// stray table fails the whole-file parse and lands on this same
/// warn-and-ignore path rather than silently becoming a phantom provider
/// or aborting the session.
fn load_model_overrides(path: &std::path::Path) -> Option<yach_catalog::Overrides> {
    let text = std::fs::read_to_string(path).ok()?;
    match yach_catalog::Overrides::from_toml_str(&text) {
        Ok(overrides) => Some(overrides),
        Err(error) => {
            let mut stderr = io::stderr();
            let _ = writeln!(
                stderr,
                "warning: ignoring malformed {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// User/project/env override layers for `yach_catalog::resolve`. Loading
/// is I/O (two file reads) and, on a malformed override file, emits a
/// stderr warning (see `load_model_overrides`); a caller resolving
/// several models in one invocation (the catalog `/model` list) loads
/// this once and reuses it, instead of re-reading the files — and
/// re-warning — per model.
#[derive(Clone)]
struct ModelOverrideLayers {
    user: Option<yach_catalog::Overrides>,
    project: Option<yach_catalog::Overrides>,
    fetched: Option<yach_catalog::CachedCatalog>,
    fetched_codex: Option<yach_catalog::CachedCatalog>,
    env: yach_catalog::EnvOverrides,
}

impl ModelOverrideLayers {
    fn load_for_project(project_root: Option<&Path>) -> Self {
        // Same HOME lookup `extension_store_path`'s user scope uses for
        // `~/.yach/extensions.json` — one home-dir mechanism for the whole
        // `~/.yach` tree.
        let user = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".yach/models.toml"))
            .and_then(|path| load_model_overrides(&path));
        let project_path = project_root
            .unwrap_or_else(|| Path::new("."))
            .join(".yach/models.toml");
        let project = load_model_overrides(&project_path);
        // The cache is loaded once with the two override layers. A later
        // background refresh receives this clone, so it never re-reads the
        // cache or changes this invocation's resolved catalog generation.
        let fetched = catalog_refresh::load_cache();
        let fetched_codex = catalog_refresh::load_codex_cache();
        let env = yach_catalog::EnvOverrides {
            // Tolerant here (invalid text -> absent, not an error): there
            // is no `Result` to propagate through here, and any real
            // misconfiguration is already caught by the strict
            // `optional_bounded_env_value(...)?` checks in
            // `rig_provider_adapter_config_from_env_with_model_override`
            // before this runs.
            context_window: optional_bounded_env_value(
                "YACH_RIG_PROVIDER_CONTEXT_WINDOW",
                10_000,
                2_000_000,
            )
            .unwrap_or_default(),
            max_tokens: optional_bounded_env_value("YACH_RIG_PROVIDER_MAX_TOKENS", 1_024, 128_000)
                .unwrap_or_default(),
            output_tokens_param: optional_env("YACH_RIG_PROVIDER_MAX_TOKENS_PARAM")
                .as_deref()
                .map(output_tokens_param_from_env_value),
        };
        Self {
            user,
            project,
            fetched,
            fetched_codex,
            env,
        }
    }

    fn resolve(&self, provider_label: &str, model: &str) -> yach_catalog::ModelProfile {
        self.resolve_with_catalog(
            provider_label,
            model,
            yach_catalog::baked_catalog(),
            &self.env,
        )
    }

    fn resolve_with_catalog(
        &self,
        provider_label: &str,
        model: &str,
        baked: &yach_catalog::Catalog,
        env: &yach_catalog::EnvOverrides,
    ) -> yach_catalog::ModelProfile {
        let fetched = if provider_label == "openai-codex" {
            self.fetched_codex
                .as_ref()
                .map(|cached| (&cached.catalog, cached.retrieved.as_str()))
        } else {
            self.fetched
                .as_ref()
                .map(|cached| (&cached.catalog, cached.retrieved.as_str()))
        };
        yach_catalog::resolve(
            provider_label,
            model,
            baked,
            fetched,
            self.user.as_ref(),
            self.project.as_ref(),
            env,
        )
    }
}

/// Resolve the active model through the invocation's already-loaded layers.
/// Missing or malformed override files degrade to absent (see
/// `load_model_overrides`) and therefore never block a session.
fn resolve_model_profile(
    layers: &ModelOverrideLayers,
    provider_label: &str,
    model: &str,
) -> yach_catalog::ModelProfile {
    layers.resolve(provider_label, model)
}

fn catalog_tool_call(
    catalog: &yach_catalog::Catalog,
    provider_label: &str,
    model: &str,
) -> Option<bool> {
    catalog
        .entry(provider_label, model)
        .or_else(|| {
            (provider_label == "openai-compatible")
                .then(|| catalog.entry_by_model_id(model))
                .flatten()
        })
        .and_then(|entry| entry.tool_call)
}

fn layers_tool_call(
    layers: &ModelOverrideLayers,
    baked: &yach_catalog::Catalog,
    provider_label: &str,
    model: &str,
) -> Option<bool> {
    layers
        .project
        .as_ref()
        .and_then(|overrides| overrides.entry(provider_label, model))
        .and_then(|entry| entry.tool_call)
        .or_else(|| {
            layers
                .user
                .as_ref()
                .and_then(|overrides| overrides.entry(provider_label, model))
                .and_then(|entry| entry.tool_call)
        })
        .or_else(|| {
            let fetched = if provider_label == "openai-codex" {
                layers.fetched_codex.as_ref()
            } else {
                layers.fetched.as_ref()
            };
            fetched.and_then(|cached| catalog_tool_call(&cached.catalog, provider_label, model))
        })
        .or_else(|| catalog_tool_call(baked, provider_label, model))
}

fn catalog_entries_from_discovery(
    provider_label: &str,
    discovered: Vec<DiscoveredProviderModel>,
    layers: &ModelOverrideLayers,
    baked: &yach_catalog::Catalog,
) -> Vec<CatalogModelEntry> {
    discovered
        .into_iter()
        .rev()
        .filter_map(|DiscoveredProviderModel { id, display_name }| {
            let tool_call = layers_tool_call(layers, baked, provider_label, &id);
            if tool_call == Some(false) {
                return None;
            }
            let curated = tool_call == Some(true);

            let profile = layers.resolve_with_catalog(provider_label, &id, baked, &layers.env);
            let output_budget =
                yach_catalog::effective_output_budget(&profile, layers.env.max_tokens);
            let name = if matches!(
                &profile.display_name.source,
                yach_catalog::CatalogSource::Default
            ) {
                display_name.unwrap_or_else(|| id.clone())
            } else {
                profile.display_name.value.clone()
            };
            Some(CatalogModelEntry {
                info: ModelInfo {
                    id,
                    name,
                    provider: provider_label.to_owned(),
                    connection_id: None,
                    connection_display: None,
                },
                curated,
                context_window: profile.context_window.value,
                output_budget: output_budget.value,
                max_tokens_param: max_tokens_param_from_catalog(profile.output_tokens_param.value),
                responses_compact: profile.responses_compact.map(|capability| capability.value),
            })
        })
        .collect()
}
#[cfg(test)]
fn discovered(
    id: &str,
    display_name: Option<&str>,
) -> yach_backend::model_discovery::DiscoveredProviderModel {
    yach_backend::model_discovery::DiscoveredProviderModel {
        id: id.to_owned(),
        display_name: display_name.map(str::to_owned),
    }
}

#[cfg(test)]
fn entry_ids(entries: &[CatalogModelEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.info.id.as_str()).collect()
}

#[cfg(test)]
fn model_layers_fixture() -> ModelOverrideLayers {
    ModelOverrideLayers {
        user: None,
        project: None,
        fetched: None,
        fetched_codex: None,
        env: yach_catalog::EnvOverrides::default(),
    }
}

#[cfg(test)]
#[test]
fn discovery_keeps_unknown_ids_but_filters_known_non_generation_entries() {
    let mut baked = yach_catalog::Catalog::empty("test");
    baked.insert(
        "openai",
        "known-chat",
        yach_catalog::CatalogEntry {
            context_window: Some(128_000),
            output_ceiling: Some(16_000),
            tool_call: Some(true),
            responses_compact: Some(true),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    baked.insert(
        "openai",
        "known-embedding",
        yach_catalog::CatalogEntry {
            context_window: Some(0),
            output_ceiling: Some(0),
            tool_call: Some(false),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let layers = model_layers_fixture();

    let entries = catalog_entries_from_discovery(
        "openai",
        vec![
            discovered("known-chat", None),
            discovered("known-embedding", None),
            discovered("brand-new", Some("Brand New")),
        ],
        &layers,
        &baked,
    );

    assert_eq!(entry_ids(&entries), vec!["brand-new", "known-chat"]);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.curated)
            .map(|entry| entry.info.id.as_str())
            .collect::<Vec<_>>(),
        vec!["known-chat"],
        "known generation rows are curated while unknown rows remain complete-only"
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.info.id == "known-chat")
            .and_then(|entry| entry.responses_compact),
        Some(true)
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.info.id == "brand-new")
            .and_then(|entry| entry.responses_compact),
        None
    );
}

#[cfg(test)]
#[test]
fn legacy_fetched_capability_gap_falls_back_to_baked_tool_call() {
    let mut baked = yach_catalog::Catalog::empty("test");
    baked.insert(
        "openai",
        "gpt-5",
        yach_catalog::CatalogEntry {
            tool_call: Some(true),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let legacy = yach_catalog::CachedCatalog::from_json_str(
        r#"{
            "etag": "\"legacy\"",
            "last_modified": null,
            "retrieved": "2026-08-05",
            "catalog": {
                "snapshot_date": "2026-08-05",
                "providers": {
                    "openai": {
                        "models": {
                            "gpt-5": {
                                "context_window": 272000,
                                "output_ceiling": 128000
                            }
                        }
                    }
                }
            }
        }"#,
    );
    assert!(legacy.is_ok());
    let Ok(legacy) = legacy else {
        return;
    };
    let layers = ModelOverrideLayers {
        user: None,
        project: None,
        fetched: Some(legacy),
        fetched_codex: None,
        env: yach_catalog::EnvOverrides::default(),
    };

    let entries =
        catalog_entries_from_discovery("openai", vec![discovered("gpt-5", None)], &layers, &baked);

    assert_eq!(entry_ids(&entries), vec!["gpt-5"]);
    assert!(entries[0].curated);
}

#[cfg(test)]
#[test]
fn override_only_model_without_tool_call_stays_complete_only() {
    let user = yach_catalog::Overrides::from_toml_str(
        "[openai.override-only]\ncontext_window = 128000\noutput_ceiling = 16000\n",
    );
    assert!(user.is_ok());
    let Ok(user) = user else {
        return;
    };
    let layers = ModelOverrideLayers {
        user: Some(user),
        project: None,
        fetched: None,
        fetched_codex: None,
        env: yach_catalog::EnvOverrides::default(),
    };

    let entries = catalog_entries_from_discovery(
        "openai",
        vec![discovered("override-only", None)],
        &layers,
        &yach_catalog::Catalog::empty("test"),
    );

    assert_eq!(entry_ids(&entries), vec!["override-only"]);
    assert!(!entries[0].curated);
}

#[cfg(test)]
#[test]
fn discovery_uses_catalog_name_then_provider_name_then_id() {
    let mut baked = yach_catalog::Catalog::empty("test");
    baked.insert(
        "anthropic",
        "known",
        yach_catalog::CatalogEntry {
            context_window: Some(128_000),
            output_ceiling: Some(16_000),
            display_name: Some(String::from("Catalog Name")),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let layers = model_layers_fixture();
    let entries = catalog_entries_from_discovery(
        "anthropic",
        vec![
            discovered("known", Some("Provider Name")),
            discovered("provider-named", Some("Provider Name")),
            discovered("id-only", None),
        ],
        &layers,
        &baked,
    );

    let names: Vec<&str> = entries
        .iter()
        .map(|entry| entry.info.name.as_str())
        .collect();
    assert_eq!(names, vec!["id-only", "Provider Name", "Catalog Name"]);
}

#[cfg(test)]
#[test]
fn discovery_preserves_hyphenated_and_compact_dated_ids_returned_by_provider() {
    let baked = yach_catalog::Catalog::empty("test");
    let layers = model_layers_fixture();
    let entries = catalog_entries_from_discovery(
        "openai",
        vec![
            discovered("gpt-4o-2024-05-13", None),
            discovered("claude-x-20260101", None),
        ],
        &layers,
        &baked,
    );

    assert_eq!(
        entry_ids(&entries),
        vec!["claude-x-20260101", "gpt-4o-2024-05-13"]
    );
}

#[cfg(test)]
#[test]
fn config_glue_preserves_the_default_floor_for_an_unknown_model() {
    // With no catalog entry and no env override, the values that would
    // feed `RigProviderAdapterConfig` must equal today's defaults exactly:
    // 200k context window, 32k output budget, `max_tokens` spelling. Uses
    // `yach_catalog::resolve` directly (the same call `resolve_model_profile`
    // makes) rather than `resolve_model_profile` itself, so the test stays
    // hermetic — it doesn't depend on the real HOME or the process cwd.
    let profile = yach_catalog::resolve(
        "openai-compatible",
        "mystery-model",
        yach_catalog::baked_catalog(),
        None,
        None,
        None,
        &yach_catalog::EnvOverrides::default(),
    );

    assert_eq!(profile.context_window.value, 200_000);
    assert_eq!(
        yach_catalog::effective_output_budget(&profile, None).value,
        32_000
    );
    assert_eq!(
        max_tokens_param_from_catalog(profile.output_tokens_param.value),
        MaxTokensParam::MaxTokens
    );
}

#[cfg(test)]
#[test]
fn model_override_layers_resolve_prefers_fetched_over_baked_but_loses_to_project_override() {
    // `ModelOverrideLayers` constructed directly (no filesystem, no HOME
    // dependency) with a fetched-cache fixture set on the `fetched` field —
    // this is the glue-level mirror of yach_catalog's own
    // `fetched_layer_beats_baked_and_loses_to_overrides` test, proving the
    // CLI's resolve() call actually threads the cache through rather than
    // silently stubbing `None` (the slice-1 placeholder this test replaces).
    let mut fetched_catalog = yach_catalog::Catalog::empty("unused");
    fetched_catalog.insert(
        "anthropic",
        "m",
        yach_catalog::CatalogEntry {
            context_window: Some(150_000),
            output_ceiling: Some(40_000),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let cached = yach_catalog::CachedCatalog {
        etag: None,
        last_modified: None,
        checked_at_unix_ms: None,
        retrieved: String::from("2026-08-03"),
        catalog: fetched_catalog,
    };
    let Ok(project) =
        yach_catalog::Overrides::from_toml_str("[anthropic.m]\ncontext_window = 160000\n")
    else {
        unreachable!("fixture toml must parse");
    };
    let layers = ModelOverrideLayers {
        user: None,
        project: Some(project),
        fetched: Some(cached),
        fetched_codex: None,
        env: yach_catalog::EnvOverrides::default(),
    };

    let profile = layers.resolve("anthropic", "m");

    // project override wins where it speaks…
    assert_eq!(profile.context_window.value, 160_000);
    // …fetched beats baked where the override is silent.
    assert_eq!(profile.output_ceiling.value, 40_000);
    assert!(matches!(
        &profile.output_ceiling.source,
        yach_catalog::CatalogSource::Fetched { retrieved } if retrieved == "2026-08-03"
    ));
}

#[cfg(test)]
#[test]
fn model_override_layers_resolve_without_a_fetched_cache_matches_slice1_behavior() {
    // The offline floor at the `ModelOverrideLayers` glue level: `fetched:
    // None` (what a missing/malformed cache file degrades to — see
    // `catalog_refresh::load_cache`) must resolve identically to slice 1,
    // before the fetched layer existed at all.
    let layers = ModelOverrideLayers {
        user: None,
        project: None,
        fetched: None,
        fetched_codex: None,
        env: yach_catalog::EnvOverrides::default(),
    };

    let profile = layers.resolve("openai-compatible", "mystery-model");

    assert_eq!(
        profile.context_window.value,
        yach_catalog::DEFAULT_CONTEXT_WINDOW
    );
    assert!(matches!(
        profile.context_window.source,
        yach_catalog::CatalogSource::Default
    ));
}

#[cfg(test)]
#[test]
fn model_override_layers_loads_from_explicit_project_root() -> Result<(), String> {
    // A process launched elsewhere must resolve the project's explicit
    // `--project-root`, not whatever happens to be under the process cwd.
    // This test deliberately never changes cwd.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let base = std::env::temp_dir().join(format!(
        "yach-cli-explicit-project-root-{}-{unique}",
        std::process::id()
    ));
    let first_root = base.join("first");
    let second_root = base.join("second");
    let result = (|| -> Result<(), String> {
        for (root, display_name) in [
            (&first_root, "first project"),
            (&second_root, "second project"),
        ] {
            let models_dir = root.join(".yach");
            std::fs::create_dir_all(&models_dir).map_err(|error| error.to_string())?;
            std::fs::write(
                models_dir.join("models.toml"),
                format!("[anthropic.project-root-test]\ndisplay_name = \"{display_name}\"\n"),
            )
            .map_err(|error| error.to_string())?;
        }

        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        if cwd == second_root {
            return Err(String::from("test root must differ from the process cwd"));
        }
        let layers = ModelOverrideLayers::load_for_project(Some(&second_root));
        let profile = layers.resolve("anthropic", "project-root-test");
        if profile.display_name.value == "second project" {
            Ok(())
        } else {
            Err(format!(
                "expected explicit project root's display name, got {}",
                profile.display_name.value
            ))
        }
    })();
    let _ = std::fs::remove_dir_all(&base);
    result
}

#[cfg(test)]
#[test]
fn headless_project_root_option_selects_its_model_override() -> Result<(), String> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root = std::env::temp_dir().join(format!(
        "yach-cli-headless-project-root-{}-{unique}",
        std::process::id()
    ));
    let result = (|| -> Result<(), String> {
        let models_dir = root.join(".yach");
        std::fs::create_dir_all(&models_dir).map_err(|error| error.to_string())?;
        std::fs::write(
            models_dir.join("models.toml"),
            "[anthropic.headless-root-test]\ndisplay_name = \"headless root\"\n",
        )
        .map_err(|error| error.to_string())?;
        let args = vec![
            String::from("--prompt"),
            String::from("test"),
            String::from("--project-root"),
            root.to_string_lossy().into_owned(),
        ];
        let options = headless::parse_run_args(&args)?;
        let layers = ModelOverrideLayers::load_for_project(options.project_root.as_deref());

        if layers
            .resolve("anthropic", "headless-root-test")
            .display_name
            .value
            == "headless root"
        {
            Ok(())
        } else {
            Err(String::from(
                "headless --project-root must select its models.toml override",
            ))
        }
    })();
    let _ = std::fs::remove_dir_all(&root);
    result
}

#[cfg(test)]
#[test]
fn config_glue_budgets_the_ceiling_when_it_undercuts_the_cohort_default() {
    // A baked model with a ceiling below the 32k cohort default should
    // budget the ceiling, not the default — constructed through a `Catalog`
    // fixture via the pure functions, not the real baked/global catalog.
    let mut catalog = yach_catalog::Catalog::empty("test-snapshot");
    catalog.insert(
        "anthropic",
        "tiny-ceiling-model",
        yach_catalog::CatalogEntry {
            output_ceiling: Some(8_192),
            ..yach_catalog::CatalogEntry::default()
        },
    );
    let profile = yach_catalog::resolve(
        "anthropic",
        "tiny-ceiling-model",
        &catalog,
        None,
        None,
        None,
        &yach_catalog::EnvOverrides::default(),
    );

    assert_eq!(
        yach_catalog::effective_output_budget(&profile, None).value,
        8_192
    );
}

#[cfg(test)]
#[test]
fn max_tokens_param_mapping_covers_both_catalog_spellings() {
    assert_eq!(
        max_tokens_param_from_catalog(yach_catalog::OutputTokensParam::MaxTokens),
        MaxTokensParam::MaxTokens
    );
    assert_eq!(
        max_tokens_param_from_catalog(yach_catalog::OutputTokensParam::MaxCompletionTokens),
        MaxTokensParam::MaxCompletionTokens
    );
}

#[cfg(test)]
#[test]
fn output_tokens_param_from_env_value_recognizes_the_completion_spelling_and_defaults_otherwise() {
    assert_eq!(
        output_tokens_param_from_env_value("max_completion_tokens"),
        yach_catalog::OutputTokensParam::MaxCompletionTokens
    );
    assert_eq!(
        output_tokens_param_from_env_value("max_tokens"),
        yach_catalog::OutputTokensParam::MaxTokens
    );
    assert_eq!(
        output_tokens_param_from_env_value("nonsense"),
        yach_catalog::OutputTokensParam::MaxTokens
    );
}

#[cfg(test)]
#[test]
fn clamped_optional_numeric_value_is_absent_none_in_range_verbatim_and_clamped_out_of_range() {
    assert_eq!(
        clamped_optional_numeric_value("YACH_TEST_VAR", None, 1_024, 128_000),
        Ok(None)
    );
    assert_eq!(
        clamped_optional_numeric_value(
            "YACH_TEST_VAR",
            Some(String::from("50000")),
            1_024,
            128_000
        ),
        Ok(Some(50_000))
    );
    // Out-of-range values clamp rather than error — the same bounds
    // behavior `optional_bounded_env` has always had.
    assert_eq!(
        clamped_optional_numeric_value(
            "YACH_TEST_VAR",
            Some(String::from("999999999")),
            1_024,
            128_000
        ),
        Ok(Some(128_000))
    );
    assert_eq!(
        clamped_optional_numeric_value(
            "YACH_TEST_VAR",
            Some(String::from("not-a-number")),
            1_024,
            128_000
        ),
        Err(RigSmokeConfigError::InvalidNumber("YACH_TEST_VAR"))
    );
}

#[cfg(test)]
#[test]
fn load_model_overrides_degrades_a_stray_top_level_table_to_a_warning() -> Result<(), String> {
    // A realistic future/typo shape: `[settings]` is not a provider table,
    // and its field is a scalar, not a per-model table — `#[serde(flatten)]`
    // can't deserialize that as `BTreeMap<String, CatalogEntry>`, so the
    // whole file fails to parse. `load_model_overrides` must degrade that
    // to `None` (a stderr warning, never a crash) rather than aborting the
    // session — and a sibling override file that *does* parse must still
    // be honored, i.e. the bad file's blast radius is itself, not the
    // session.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "yach-cli-stray-settings-table-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    let malformed_path = dir.join("malformed-models.toml");
    std::fs::write(&malformed_path, "[settings]\nenabled = true\n")
        .map_err(|error| error.to_string())?;
    let valid_path = dir.join("valid-models.toml");
    std::fs::write(
        &valid_path,
        "[anthropic.claude-test]\ncontext_window = 111000\n",
    )
    .map_err(|error| error.to_string())?;

    let malformed_result = load_model_overrides(&malformed_path);
    let valid_result = load_model_overrides(&valid_path);
    let _ = std::fs::remove_dir_all(&dir);

    if malformed_result.is_some() {
        return Err(String::from(
            "a stray top-level table with scalar fields should degrade to absent, not parse",
        ));
    }
    let Some(valid_overrides) = valid_result else {
        return Err(String::from(
            "a well-formed override file should still load despite a sibling malformed one",
        ));
    };
    let profile = yach_catalog::resolve(
        "anthropic",
        "claude-test",
        &yach_catalog::Catalog::empty("test-snapshot"),
        None,
        None,
        Some(&valid_overrides),
        &yach_catalog::EnvOverrides::default(),
    );
    if profile.context_window.value == 111_000 {
        Ok(())
    } else {
        Err(format!(
            "expected the valid override's context_window (111000), got {}",
            profile.context_window.value
        ))
    }
}

fn run_rig_provider_request_smoke() -> CommandResult {
    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let model = match provider.as_str() {
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-haiku-4-5")),
        "openai-codex" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        "openai" => match required_env("YACH_RIG_OPENAI_MODEL") {
            Ok(model) => model,
            Err(error) => return missing_rig_provider_request_config(&error),
        },
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
                    "YACH_RIG_PROVIDER must be anthropic, openai-codex, or openai for this smoke; openai-compatible has its own smoke command",
                )),
            };
        }
    };
    let provider_config = match provider.as_str() {
        "anthropic" => match required_env("YACH_RIG_ANTHROPIC_API_KEY") {
            Ok(api_key) => RigProviderConfig::Anthropic {
                api_key: ProviderSecret::new(api_key),
                base_url: optional_env("YACH_RIG_ANTHROPIC_BASE_URL"),
            },
            Err(error) => return missing_rig_provider_request_config(&error),
        },
        "openai-codex" => match required_env("YACH_RIG_CHATGPT_TOKEN_DIR") {
            Ok(token_dir) => RigProviderConfig::ChatGptSubscription {
                auth_file: PathBuf::from(token_dir).join("auth.json"),
            },
            Err(error) => return missing_rig_provider_request_config(&error),
        },
        "openai" => match required_env("YACH_RIG_OPENAI_API_KEY") {
            Ok(api_key) => RigProviderConfig::OpenAi {
                api_key: ProviderSecret::new(api_key),
                base_url: optional_env("YACH_RIG_OPENAI_BASE_URL"),
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
        turn_id: TurnId(String::from("rig-provider-request-smoke-turn")),
        model: ProviderModel { provider, model },
        messages: vec![ProviderMessage::text(
            Role::User,
            String::from("Reply with exactly: yach-rig-smoke-ok"),
        )],
        extensions: vec![],
        native_request: None,
        approved_tool_advertising: None,
    };
    let adapter = RigProviderAdapterConfig {
        provider: provider_config,
        timeout: Duration::from_secs(timeout_secs),
        max_tokens,
        context_window: 200_000,
        max_tokens_param: MaxTokensParam::default(),
    };
    match runtime.block_on(run_provider_request(&adapter, request)) {
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
    let log = match yach_backend::JsonlSessionStore::new(PathBuf::from(session_path)).load() {
        Ok(log) => log,
        Err(error) => {
            return failed(vec![format!("failed to load session log: {error}")]);
        }
    };
    let config = yach_backend::CompactionConfig::default();
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
    lines.push(format!(
        "folded_events={} kept_from_entry={}",
        cut.fold_range.len(),
        cut.first_kept_entry_id.0
    ));
    lines.push(format!(
        "serialized_conversation_chars={}",
        yach_backend::serialize_events_for_summary(&log.events[cut.fold_range.clone()])
            .chars()
            .count()
    ));
    if previous.is_some() {
        lines.push(String::from("anchored=true (previous checkpoint found)"));
    }

    let provider = optional_env("YACH_RIG_PROVIDER").unwrap_or_else(|| String::from("anthropic"));
    let model = match provider.as_str() {
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-haiku-4-5")),
        "openai-codex" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        "openai" => optional_env("YACH_RIG_OPENAI_MODEL").unwrap_or_default(),
        _ => {
            lines.push(String::from(
                "YACH_RIG_PROVIDER must be anthropic, openai-codex, openai, or openai-compatible",
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
    let preparation = yach_backend::CompactionPreparation {
        serialized_conversation: yach_backend::serialize_events_for_summary(
            &log.events[cut.fold_range.clone()],
        ),
        previous_summary: previous.as_ref().map(|view| view.summary.to_owned()),
        previous_details: previous.as_ref().map(|view| view.details.clone()),
        first_kept_entry_id: cut.first_kept_entry_id.clone(),
        tokens_before: current_estimate,
        reason: yach_backend::CompactionReason::Manual,
        focus_instructions: None,
        provider: std::sync::Arc::new(yach_backend::CompactionProviderContext {
            provider: provider.clone(),
            wire: if provider == "openai" {
                String::from("openai-responses")
            } else {
                String::from("unsupported")
            },
            model: model.clone(),
            connection: String::from("manual-compaction-smoke"),
            responses_compact: None,
            adapter: std::sync::Arc::new(adapter_config.clone()),
        }),
        native_request: None,
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        lines.push(String::from("failed to create tokio runtime"));
        return failed(lines);
    };
    let request = ProviderRequest {
        turn_id: TurnId(String::from("compaction-smoke-turn")),
        model: ProviderModel { provider, model },
        messages: vec![ProviderMessage::text(
            Role::User,
            yach_backend::build_summary_prompt(&preparation),
        )],
        extensions: vec![],
        native_request: None,
        approved_tool_advertising: None,
    };
    let started = std::time::Instant::now();
    match runtime.block_on(run_provider_request(&adapter_config, request)) {
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

struct ResponsesCompactionSmokeWorkspace {
    root: PathBuf,
    session_path: PathBuf,
}

impl Drop for ResponsesCompactionSmokeWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn responses_compaction_smoke_workspace() -> io::Result<ResponsesCompactionSmokeWorkspace> {
    let root = std::env::temp_dir().join(format!(
        "yach-responses-compaction-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos())
    ));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;
    let yach_dir = root.join(".yach");
    if let Err(error) = std::fs::create_dir(&yach_dir).and_then(|()| {
        std::fs::write(
            yach_dir.join("config.json"),
            r#"{"compaction":{"reserve_tokens":0,"keep_recent_tokens":1,"auto_threshold_percent":10}}"#,
        )
    }) {
        let _ = std::fs::remove_dir_all(&root);
        return Err(error);
    }
    let session_path = root.join("session.jsonl");
    let seed_session_id = yach_backend::SessionId(String::from("session"));
    let seed_turn_id = TurnId(String::from("seed-turn"));
    let seed_events = [
        yach_backend::SessionEvent::EntryAppended {
            session_id: seed_session_id.clone(),
            entry_id: yach_backend::EntryId(String::from("seed-user")),
            parent_entry_id: None,
            turn_id: seed_turn_id.clone(),
            role: Role::User,
            text: String::from("Preserve the prior diagnostic context."),
            provider: None,
        },
        yach_backend::SessionEvent::EntryAppended {
            session_id: seed_session_id.clone(),
            entry_id: yach_backend::EntryId(String::from("seed-assistant")),
            parent_entry_id: Some(yach_backend::EntryId(String::from("seed-user"))),
            turn_id: seed_turn_id.clone(),
            role: Role::Assistant,
            text: "prior diagnostic context ".repeat(500),
            provider: None,
        },
        yach_backend::SessionEvent::TurnFinished {
            session_id: seed_session_id,
            turn_id: seed_turn_id,
            outcome: yach_backend::TurnOutcome::Completed,
            reason: None,
        },
    ];
    let store = yach_backend::JsonlSessionStore::new(session_path.clone());
    if let Err(error) = yach_backend::SessionEventSink::append_events(&store, &seed_events) {
        let _ = std::fs::remove_dir_all(&root);
        return Err(error);
    }
    Ok(ResponsesCompactionSmokeWorkspace { root, session_path })
}

fn run_responses_compaction_smoke() -> CommandResult {
    let Ok(api_key) = required_env("YACH_RIG_OPENAI_API_KEY") else {
        return CommandResult::ResponsesCompactionSmoke {
            outcome: RigSmokeOutcome::MissingConfig,
            model: None,
            stages: Vec::new(),
            artifact_item_count: 0,
            token_count: 0,
        };
    };
    let Ok(model) = required_env("YACH_RIG_OPENAI_MODEL") else {
        return responses_compaction_smoke_failed(None, Vec::new(), 0, 0);
    };
    let Ok(timeout_secs) = optional_bounded_env("YACH_RIG_OPENAI_TIMEOUT_SECS", 120, 5, 600) else {
        return responses_compaction_smoke_failed(Some(model), Vec::new(), 0, 0);
    };
    let adapter = Arc::new(RigProviderAdapterConfig {
        provider: RigProviderConfig::OpenAi {
            api_key: ProviderSecret::new(api_key),
            base_url: optional_env("YACH_RIG_OPENAI_BASE_URL"),
        },
        timeout: Duration::from_secs(timeout_secs),
        max_tokens: 1_024,
        context_window: 20_000,
        max_tokens_param: MaxTokensParam::default(),
    });
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return responses_compaction_smoke_failed(Some(model), Vec::new(), 0, 0);
    };
    runtime.block_on(run_responses_compaction_runner_smoke(model, adapter))
}

async fn run_responses_compaction_runner_smoke(
    model: String,
    adapter: Arc<RigProviderAdapterConfig>,
) -> CommandResult {
    let Ok(workspace) = responses_compaction_smoke_workspace() else {
        return responses_compaction_smoke_failed(Some(model), Vec::new(), 0, 0);
    };
    let session_path = workspace.session_path.clone();
    let store = yach_backend::JsonlSessionStore::new(session_path.clone());
    let (client_tx, client_rx) = mpsc::unbounded_channel();
    let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
    let backend = tokio::spawn(run_native_loop(
        client_rx,
        backend_tx,
        RunnerConfig {
            session_path: session_path.clone(),
            project_root: Some(workspace.root.clone()),
            provider: Some(ProviderConfig {
                adapter,
                model: model.clone(),
                connection_id: None,
                connection_display: None,
                test_delay_ms: None,
                catalog_models: Vec::new().into(),
                responses_compact: Some(true),
            }),
            provider_setup_error: None,
            extension_package_roots: Vec::new(),
            extension_package_root_loader: None,
            startup_trace: None,
            catalog_refresh: None,
            model_discovery: None,
            provider_connections: None,
        },
    ));
    let mut stages = Vec::new();
    let session_id = String::from("default");
    // The private persisted session contains one completed, foldable prior
    // turn. This live prompt therefore crosses the configured threshold
    // through the same runner path a resumed session uses.
    let first = ClientEvent::PromptSubmitted {
        session_id: session_id.clone(),
        prompt: String::from("Reply with exactly: yach-responses-compaction-smoke-ok"),
    };
    if client_tx.send(first).is_err() || !smoke_wait_for_completion(&mut backend_rx).await {
        backend.abort();
        let _ = std::fs::remove_file(session_path);
        return responses_compaction_smoke_failed(Some(model), stages, 0, 0);
    }
    stages.push(ResponsesCompactionSmokeStage::ResponsesTurn);

    let log = store.load().ok();
    let checkpoint = log.as_ref().and_then(|log| {
        log.events.iter().rev().find_map(|event| match event {
            yach_backend::SessionEvent::CompactionCheckpoint {
                summary, details, ..
            } if !summary.trim().is_empty() => details
                .get("native")
                .and_then(|native| native.get("window"))
                .and_then(serde_json::Value::as_array)
                .filter(|window| !window.is_empty())
                .map(Vec::len),
            _ => None,
        })
    });
    let Some(artifact_item_count) = checkpoint else {
        backend.abort();
        let _ = std::fs::remove_file(session_path);
        return responses_compaction_smoke_failed(Some(model), stages, 0, 0);
    };
    stages.push(ResponsesCompactionSmokeStage::NativeCompact);
    stages.push(ResponsesCompactionSmokeStage::PortableSummary);

    if client_tx
        .send(ClientEvent::PromptSubmitted {
            session_id,
            prompt: String::from("Continue the smoke using the compacted context."),
        })
        .is_err()
        || !smoke_wait_for_completion(&mut backend_rx).await
    {
        backend.abort();
        let _ = std::fs::remove_file(session_path);
        return responses_compaction_smoke_failed(Some(model), stages, artifact_item_count, 0);
    }
    stages.push(ResponsesCompactionSmokeStage::ReplayedContinuation);
    if let Some(alt_model) = optional_env("YACH_RIG_OPENAI_SMOKE_ALT_MODEL") {
        if client_tx
            .send(ClientEvent::ModelSelected { model: alt_model })
            .is_err()
            || !smoke_wait_for_model_change(&mut backend_rx).await
            || client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("Exercise the alternate smoke model."),
                })
                .is_err()
            || !smoke_wait_for_completion(&mut backend_rx).await
            || client_tx
                .send(ClientEvent::ModelSelected {
                    model: model.clone(),
                })
                .is_err()
            || !smoke_wait_for_model_change(&mut backend_rx).await
            || client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("Resume the original compacted smoke context."),
                })
                .is_err()
            || !smoke_wait_for_completion(&mut backend_rx).await
        {
            backend.abort();
            let _ = std::fs::remove_file(session_path);
            return responses_compaction_smoke_failed(Some(model), stages, artifact_item_count, 0);
        }
        stages.push(ResponsesCompactionSmokeStage::ModelSwitchReplay);
    }
    let token_count = store
        .load()
        .ok()
        .and_then(|log| {
            log.events.iter().rev().find_map(|event| match event {
                yach_backend::SessionEvent::EntryAppended {
                    provider: Some(provider),
                    ..
                } => provider.usage.and_then(|usage| usage.total_tokens),
                _ => None,
            })
        })
        .unwrap_or(0);
    backend.abort();
    let _ = std::fs::remove_file(session_path);
    CommandResult::ResponsesCompactionSmoke {
        outcome: RigSmokeOutcome::Completed,
        model: Some(model),
        stages,
        artifact_item_count,
        token_count,
    }
}

async fn smoke_wait_for_completion(backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>) -> bool {
    tokio::time::timeout(Duration::from_mins(3), async {
        while let Some(event) = backend_rx.recv().await {
            if let BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. }) = event {
                return outcome == PromptOutcome::Completed;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

async fn smoke_wait_for_model_change(
    backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
) -> bool {
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = backend_rx.recv().await {
            if matches!(event, BackendEvent::Server(ServerEvent::ModelChanged(_))) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

fn responses_compaction_smoke_failed(
    model: Option<String>,
    stages: Vec<ResponsesCompactionSmokeStage>,
    artifact_item_count: usize,
    token_count: u64,
) -> CommandResult {
    CommandResult::ResponsesCompactionSmoke {
        outcome: RigSmokeOutcome::Failed,
        model,
        stages,
        artifact_item_count,
        token_count,
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

fn run_rig_openai_smoke() -> CommandResult {
    let api_key = match required_env("YACH_RIG_OPENAI_API_KEY") {
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
    let model = match required_env("YACH_RIG_OPENAI_MODEL") {
        Ok(model) => model,
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
    let timeout_secs = match optional_bounded_env("YACH_RIG_OPENAI_TIMEOUT_SECS", 120, 5, 600) {
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
    let max_tokens = match optional_bounded_env("YACH_RIG_OPENAI_MAX_TOKENS", 128, 1, 256) {
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
    match runtime.block_on(run_openai_smoke(RigOpenAiSmokeConfig {
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

/// Same numeric-format validation and range clamp as `optional_bounded_env`,
/// but returns `None` when the var is absent instead of substituting a
/// default — the default now lives in the model catalog
/// (`resolve_model_profile`), not in this parser. Kept as one function so
/// the bounds behavior (invalid text errors, in-range values pass through,
/// out-of-range values clamp) stays identical everywhere it's used: with
/// `?` where the caller can fail the session, or `.unwrap_or_default()`
/// where the caller only wants a best-effort value and never a hard error
/// (a bad env var degrades the same way a bad override file does there).
fn optional_bounded_env_value(
    name: &'static str,
    min: u64,
    max: u64,
) -> Result<Option<u64>, RigSmokeConfigError> {
    clamped_optional_numeric_value(name, optional_env(name), min, max)
}

/// The pure half of `optional_bounded_env_value`: given the raw string
/// already read from `name` (or `None` if it was absent/blank), parses and
/// clamps it. Split out so the clamp-and-error behavior is testable with a
/// constructed `Option<String>` instead of a real env var.
fn clamped_optional_numeric_value(
    name: &'static str,
    value: Option<String>,
    min: u64,
    max: u64,
) -> Result<Option<u64>, RigSmokeConfigError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|_| RigSmokeConfigError::InvalidNumber(name))?;
    Ok(Some(parsed.clamp(min, max)))
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
    format!(
        "provider_error_kind={}",
        provider_error_kind_label(error.kind)
    )
}

fn provider_setup_error_message(error: &RigSmokeConfigError) -> String {
    format!("provider setup failed: {}", rig_config_error_message(error))
}

/// Missing legacy env config is a setup error only when no stored `/connect`
/// registry can supply auth instead. Genuine failures surface as in-session
/// status from the unconfigured backend; launch never prints them because a
/// pre-launch stderr line is hidden by the alternate screen and reappears
/// after exit.
fn unconfigured_launch_setup_error(
    error: &RigSmokeConfigError,
    stored_connections_available: bool,
) -> Option<String> {
    if stored_connections_available {
        None
    } else {
        Some(provider_setup_error_message(error))
    }
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
            let negotiated = negotiate_with_ui(&backend_handshake());
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

const TUI_PROVIDER_CONNECTION_SMOKE_SECRET: &str = "task-seven-密-key";

fn run_tui_provider_connection_smoke_command() -> CommandResult {
    let Ok(fixture) =
        TuiProviderConnectionSmokeFixture::start(TUI_PROVIDER_CONNECTION_SMOKE_SECRET)
    else {
        return CommandResult::TuiProviderConnectionSmoke {
            passed: false,
            fixture_models: false,
            fixture_prompt: false,
            prompt_finished: false,
            exact_activation_count: 0,
            active_removal_rejected: false,
        };
    };
    let _ = writeln!(
        io::stderr(),
        "provider_connection_smoke_base_url={}",
        fixture.base_url
    );
    let scratch = TuiProviderConnectionSmokeScratch::new();
    let credentials = Arc::new(InMemorySmokeCredentialStore::default());
    let layers = ModelOverrideLayers::load_for_project(Some(scratch.path()));
    let runtime = Arc::new(
        provider_connections::CliProviderConnectionRuntime::with_stores(
            Arc::new(yach_connections::JsonConnectionMetadataStore::new(
                scratch.path().join("connections.json"),
            )),
            credentials,
            layers.clone(),
            None,
        ),
    );
    let observation = Arc::new(TuiProviderConnectionSmokeObservation::default());
    let observed = observation.clone();
    let observer: BackendEventObserver = Arc::new(move |event| {
        let BackendEvent::Server(event) = event else {
            return;
        };
        match event {
            ServerEvent::PromptFinished {
                outcome: yach_proto::PromptOutcome::Completed,
                ..
            } => observed.prompt_finished.store(true, Ordering::SeqCst),
            ServerEvent::ModelChanged(target)
                if target.model == "task-7-model" && target.connection_id.is_some() =>
            {
                observed
                    .exact_activation_count
                    .fetch_add(1, Ordering::SeqCst);
            }
            ServerEvent::StatusUpdated { message }
                if message == "select another connection before removing the active connection" =>
            {
                observed
                    .active_removal_rejected
                    .store(true, Ordering::SeqCst);
            }
            _ => {}
        }
    });
    let runtime_result = tokio::runtime::Runtime::new().and_then(|tokio_runtime| {
        tokio_runtime.block_on(run_tui_with_native_backend_config_observed(
            alpha_handshake(),
            NativeTuiBackendSetup::Fixture,
            NativeTuiRunConfig {
                ui_options: RunTuiOptions {
                    resume_session: false,
                    theme: Theme::default(),
                },
                startup_trace: None,
                catalog_refresh: None,
                project_root: Some(scratch.path().to_owned()),
                layers: &layers,
                provider_connections_override: Some(
                    runtime as Arc<dyn yach_backend::ProviderConnectionRuntime>,
                ),
                event_observer: Some(observer),
                session_path_override: Some(scratch.path().join("session.jsonl")),
            },
        ))
    });
    let fixture_models = fixture.models.load(Ordering::SeqCst);
    let fixture_prompt = fixture.prompt.load(Ordering::SeqCst)
        && fixture.expected_auth_and_model.load(Ordering::SeqCst);
    let prompt_finished = observation.prompt_finished.load(Ordering::SeqCst);
    let exact_activation_count = observation.exact_activation_count.load(Ordering::SeqCst);
    let active_removal_rejected = observation.active_removal_rejected.load(Ordering::SeqCst);
    let passed = runtime_result.is_ok()
        && fixture_models
        && fixture_prompt
        && prompt_finished
        && exact_activation_count == 1
        && active_removal_rejected;
    CommandResult::TuiProviderConnectionSmoke {
        passed,
        fixture_models,
        fixture_prompt,
        prompt_finished,
        exact_activation_count,
        active_removal_rejected,
    }
}

#[derive(Default)]
struct TuiProviderConnectionSmokeObservation {
    prompt_finished: AtomicBool,
    exact_activation_count: AtomicU64,
    active_removal_rejected: AtomicBool,
}

struct TuiProviderConnectionSmokeScratch {
    path: PathBuf,
}

impl TuiProviderConnectionSmokeScratch {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "yach-provider-connection-smoke-{}-{unique}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
        ));
        let _ = std::fs::create_dir_all(&path);
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TuiProviderConnectionSmokeScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Default)]
struct InMemorySmokeCredentialStore {
    values: Mutex<BTreeMap<String, ProviderSecret>>,
}

impl CredentialStore for InMemorySmokeCredentialStore {
    fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError> {
        self.values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .insert(id.as_str().to_owned(), secret.clone());
        Ok(())
    }

    fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .get(id.as_str())
            .cloned())
    }

    fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
        self.values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .remove(id.as_str());
        Ok(())
    }
}

struct TuiProviderConnectionSmokeFixture {
    base_url: String,
    models: Arc<AtomicBool>,
    prompt: Arc<AtomicBool>,
    expected_auth_and_model: Arc<AtomicBool>,
}

impl TuiProviderConnectionSmokeFixture {
    fn start(expected_secret: &'static str) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let models = Arc::new(AtomicBool::new(false));
        let prompt = Arc::new(AtomicBool::new(false));
        let expected_auth_and_model = Arc::new(AtomicBool::new(false));
        let fixture_models = models.clone();
        let fixture_prompt = prompt.clone();
        let fixture_request = expected_auth_and_model.clone();
        std::thread::spawn(move || {
            // Creation validates once, then the runner refreshes. The picker
            // refreshes again before the provider turn uses the same fixture.
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let Ok(request) = read_tui_provider_connection_smoke_request(&mut stream) else {
                    return;
                };
                if request.starts_with("GET /v1/models ") {
                    fixture_models.store(true, Ordering::SeqCst);
                    let body = "{\"object\":\"list\",\"data\":[{\"id\":\"task-7-model\",\"object\":\"model\",\"created\":0,\"owned_by\":\"fixture\"}]}";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                } else if request.starts_with("POST /v1/chat/completions ") {
                    fixture_prompt.store(true, Ordering::SeqCst);
                    let has_secret = request
                        .to_ascii_lowercase()
                        .contains(&format!("authorization: bearer {expected_secret}"));
                    let has_model = request.contains("\"model\":\"task-7-model\"");
                    fixture_request.store(has_secret && has_model, Ordering::SeqCst);
                    let body = concat!(
                        "data: {\"id\":\"chatcmpl-task-7\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"task-7-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"task seven completion\"},\"finish_reason\":null}]}\n\n",
                        "data: {\"id\":\"chatcmpl-task-7\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"task-7-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                        "data: [DONE]\n\n"
                    );
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
            }
        });
        Ok(Self {
            base_url: format!("http://{address}/v1"),
            models,
            prompt,
            expected_auth_and_model,
        })
    }
}
fn read_tui_provider_connection_smoke_request(
    stream: &mut std::net::TcpStream,
) -> Result<String, std::io::Error> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected_len = None;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
        if expected_len.is_none()
            && let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            expected_len = Some(header_end + 4 + content_length);
        }
        if expected_len.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&request).into_owned())
}

fn run_tui_bench_ready_command() -> CommandResult {
    let ui_handshake = alpha_handshake();
    let adapter_handshake = backend_handshake();
    let negotiated = negotiate_with_ui(&adapter_handshake);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(io::stderr(), "failed to create tokio runtime: {e}");
            return CommandResult::Tui { exited: true };
        }
    };

    match runtime.block_on(async move {
        let backend_session = start_backend_session(BackendMetadata::native(), negotiated);
        let _ = backend_session
            .endpoints
            .backend_tx
            .send(BackendEvent::Server(ServerEvent::StateUpdated(
                yach_proto::BackendState {
                    model_id: Some(String::from("bench-model")),
                    model_name: Some(String::from("Bench Model")),
                    model_provider: Some(String::from("bench")),
                    model_connection_id: None,
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
    let project_root = std::env::current_dir().ok();
    let theme = match load_tui_theme(project_root.as_deref()) {
        Ok(theme) => theme,
        Err(error) => {
            return CommandResult::UsageError { message: error };
        }
    };
    let ui_options = RunTuiOptions {
        resume_session: resume,
        theme,
    };
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

    // Resolve cwd once so override and theme loading use the same project
    // root as the backend runner.
    let layers = ModelOverrideLayers::load_for_project(project_root.as_deref());

    let result = match backend {
        // `--backend fixture` has no provider and so no active model for a
        // refresh to ever feed — the spawn (real network I/O) moves into
        // the provider arm below instead of running unconditionally before
        // the branch, so a fixture-backed TUI session never fetches.
        TuiBackendSelection::Fixture => runtime.block_on(run_tui_with_native_backend(
            ui_handshake,
            ui_options,
            startup_trace.cloned(),
            None,
            project_root.clone(),
            &layers,
        )),
        TuiBackendSelection::Provider => {
            // The refresh receives this already-loaded cache clone. The
            // current session keeps resolving from `layers`; only a later
            // invocation observes a successful refresh.
            let catalog_refresh = Some(catalog_refresh::spawn_refresh_status(
                layers.fetched.clone(),
            ));
            match rig_provider_adapter_config_from_env_with_model_override(None, &layers) {
                Ok(resolved) => runtime.block_on(run_tui_with_native_provider_backend(
                    ui_handshake,
                    resolved,
                    ui_options,
                    startup_trace.cloned(),
                    catalog_refresh,
                    project_root.clone(),
                    &layers,
                )),
                Err(error) => {
                    let setup_error = unconfigured_launch_setup_error(
                        &error,
                        provider_connections::has_stored_connections(),
                    );
                    runtime.block_on(run_tui_with_unconfigured_native_provider_backend(
                        ui_handshake,
                        setup_error,
                        ui_options,
                        startup_trace.cloned(),
                        catalog_refresh,
                        project_root.clone(),
                        &layers,
                    ))
                }
            }
        }
    };

    match result {
        Ok(()) => CommandResult::Tui { exited: true },
        Err(e) => {
            let _ = writeln!(io::stderr(), "tui error: {e}");
            CommandResult::Tui { exited: true }
        }
    }
}

enum NativeTuiBackendSetup {
    Fixture,
    Configured {
        adapter: Arc<RigProviderAdapterConfig>,
        model: String,
        responses_compact: Option<bool>,
    },
    Unconfigured(Option<String>),
}

async fn run_tui_with_native_provider_backend(
    ui_handshake: Handshake,
    provider_config: ResolvedProviderConfig,
    ui_options: RunTuiOptions,
    startup_trace: Option<StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    project_root: Option<PathBuf>,
    layers: &ModelOverrideLayers,
) -> io::Result<()> {
    run_tui_with_native_backend_config(
        ui_handshake,
        NativeTuiBackendSetup::Configured {
            adapter: Arc::new(provider_config.adapter),
            model: provider_config.model,
            responses_compact: provider_config
                .profile
                .responses_compact
                .map(|capability| capability.value),
        },
        ui_options,
        startup_trace,
        catalog_refresh,
        project_root,
        layers,
    )
    .await
}

async fn run_tui_with_native_backend(
    ui_handshake: Handshake,
    ui_options: RunTuiOptions,
    startup_trace: Option<StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    project_root: Option<PathBuf>,
    layers: &ModelOverrideLayers,
) -> io::Result<()> {
    run_tui_with_native_backend_config(
        ui_handshake,
        NativeTuiBackendSetup::Fixture,
        ui_options,
        startup_trace,
        catalog_refresh,
        project_root,
        layers,
    )
    .await
}

/// Launch the native TUI without a provider after provider setup failed, so
/// the user still gets a session that surfaces the setup error recoverably
/// instead of an exit before first render.
async fn run_tui_with_unconfigured_native_provider_backend(
    ui_handshake: Handshake,
    provider_setup_error: Option<String>,
    ui_options: RunTuiOptions,
    startup_trace: Option<StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    project_root: Option<PathBuf>,
    layers: &ModelOverrideLayers,
) -> io::Result<()> {
    run_tui_with_native_backend_config(
        ui_handshake,
        NativeTuiBackendSetup::Unconfigured(provider_setup_error),
        ui_options,
        startup_trace,
        catalog_refresh,
        project_root,
        layers,
    )
    .await
}

fn native_backend_handshake(
    setup: &NativeTuiBackendSetup,
    provider_connections_available: bool,
) -> Handshake {
    let mut capabilities = vec![
        Capability::PromptStreaming,
        Capability::PromptCancellation,
        Capability::StatusEntries,
        Capability::Notifications,
        Capability::LocalEdit,
        Capability::ExtensionLifecycle,
        Capability::FirstRenderEvents,
        Capability::StructuredReviewRows,
    ];
    if provider_connections_available {
        capabilities.push(Capability::ProviderConnections);
    }
    Handshake::new(
        match setup {
            NativeTuiBackendSetup::Configured { .. } => "yach-native-provider",
            NativeTuiBackendSetup::Fixture | NativeTuiBackendSetup::Unconfigured(_) => {
                "yach-native"
            }
        },
        capabilities,
    )
}

type BackendEventObserver = Arc<dyn Fn(&BackendEvent) + Send + Sync>;

async fn run_tui_with_native_backend_config(
    ui_handshake: Handshake,
    setup: NativeTuiBackendSetup,
    ui_options: RunTuiOptions,
    startup_trace: Option<StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    project_root: Option<PathBuf>,
    layers: &ModelOverrideLayers,
) -> io::Result<()> {
    run_tui_with_native_backend_config_observed(
        ui_handshake,
        setup,
        NativeTuiRunConfig {
            ui_options,
            startup_trace,
            catalog_refresh,
            project_root,
            layers,
            provider_connections_override: None,
            event_observer: None,
            session_path_override: None,
        },
    )
    .await
}

struct NativeTuiRunConfig<'a> {
    ui_options: RunTuiOptions,
    startup_trace: Option<StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    project_root: Option<PathBuf>,
    layers: &'a ModelOverrideLayers,
    provider_connections_override: Option<Arc<dyn yach_backend::ProviderConnectionRuntime>>,
    event_observer: Option<BackendEventObserver>,
    session_path_override: Option<PathBuf>,
}

async fn run_tui_with_native_backend_config_observed(
    ui_handshake: Handshake,
    setup: NativeTuiBackendSetup,
    options: NativeTuiRunConfig<'_>,
) -> io::Result<()> {
    let NativeTuiRunConfig {
        ui_options,
        startup_trace,
        catalog_refresh,
        project_root,
        layers,
        provider_connections_override,
        event_observer,
        session_path_override,
    } = options;
    let resume = ui_options.resume_session;
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("backend_setup_start");
    }
    let environment = match &setup {
        NativeTuiBackendSetup::Configured { adapter, .. } => {
            Some(provider_connections::EnvironmentConnection::from_runtime_adapter(adapter))
        }
        NativeTuiBackendSetup::Fixture | NativeTuiBackendSetup::Unconfigured(_) => None,
    };
    let runtime_timeout = match &setup {
        NativeTuiBackendSetup::Configured { adapter, .. } => adapter.timeout,
        NativeTuiBackendSetup::Fixture | NativeTuiBackendSetup::Unconfigured(_) => {
            provider_connection_timeout()
        }
    };
    let runtime_test_delay_ms = provider_test_delay_ms();
    let provider_connections = provider_connections_override.or_else(|| {
        provider_connections::CliProviderConnectionRuntime::system(
            layers.clone(),
            environment,
            runtime_timeout,
            runtime_test_delay_ms,
        )
        .map(|runtime| Arc::new(runtime) as Arc<dyn yach_backend::ProviderConnectionRuntime>)
    });
    let backend_handshake = native_backend_handshake(&setup, provider_connections.is_some());
    let negotiated = NegotiatedCapabilities::from_handshakes(&ui_handshake, &backend_handshake);
    let backend_session = start_backend_session(BackendMetadata::native(), negotiated.clone());
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("backend_session_started");
    }
    let fresh_session_id = fresh_session_id();
    let latest_session_path = latest_native_session_log_path();
    let resume_existing_session = resume && latest_session_path.is_some();
    let session_path = session_path_override.unwrap_or_else(|| {
        tui_session_path_from_latest(resume, latest_session_path, &fresh_session_id)
    });
    let (provider, provider_setup_error, model_discovery) = match setup {
        NativeTuiBackendSetup::Fixture => (None, None, None),
        NativeTuiBackendSetup::Configured {
            adapter,
            model,
            responses_compact,
        } => {
            let legacy_discovery = provider_connections.is_none().then(|| {
                let adapter = adapter.clone();
                let layers = layers.clone();
                Box::pin(async move {
                    let version = yach_catalog::baked_codex_protocol_version();
                    match yach_backend::model_discovery::discover_provider_models(
                        &adapter.provider,
                        adapter.timeout,
                        Some(version.as_str()),
                    )
                    .await
                    {
                        Ok(discovered) => {
                            ModelDiscoveryOutcome::Available(catalog_entries_from_discovery(
                                provider_label_from_config(&adapter),
                                discovered,
                                &layers,
                                yach_catalog::baked_catalog(),
                            ))
                        }
                        Err(_) => ModelDiscoveryOutcome::Failed {
                            message: String::from("provider model discovery failed"),
                        },
                    }
                }) as ModelDiscoveryFuture
            });
            (
                Some(ProviderConfig {
                    model,
                    connection_id: provider_connections
                        .as_ref()
                        .map(|_| yach_connections::ConnectionId::environment()),
                    connection_display: provider_connections
                        .as_ref()
                        .map(|_| String::from("Environment")),
                    test_delay_ms: runtime_test_delay_ms,
                    adapter,
                    responses_compact,
                    catalog_models: Vec::new().into(),
                }),
                None,
                legacy_discovery,
            )
        }
        NativeTuiBackendSetup::Unconfigured(error) => (None, error, None),
    };
    let client_tx = backend_session.channels.client_tx;
    let _ = client_tx.send(ClientEvent::Initialize(ui_handshake));
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("client_initialize_sent");
    }
    if resume_existing_session {
        let _ = client_tx.send(ClientEvent::SessionPathSelected {
            session_path: session_path.to_string_lossy().into_owned(),
        });
    }

    let event_tx = backend_session.endpoints.backend_tx.clone();
    let backend_config = runner_config(RunnerConfigInput {
        session_path,
        project_root,
        provider,
        provider_setup_error,
        startup_trace: startup_trace.as_ref(),
        catalog_refresh,
        model_discovery,
        provider_connections,
    });
    let backend_handle = tokio::spawn(run_native_loop_with_negotiated_capabilities(
        backend_session.endpoints.client_rx,
        event_tx,
        backend_config,
        negotiated,
    ));
    if let Some(trace) = startup_trace.as_ref() {
        trace.mark("backend_task_spawned");
    }

    let backend_rx = if let Some(observer) = event_observer {
        let mut source_rx = backend_session.channels.backend_rx;
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = source_rx.recv().await {
                observer(&event);
                if ui_tx.send(event).is_err() {
                    break;
                }
            }
        });
        ui_rx
    } else {
        backend_session.channels.backend_rx
    };
    let ui_result =
        run_tui_with_startup_trace_and_options(client_tx, backend_rx, startup_trace, ui_options)
            .await;

    backend_handle.abort();
    ui_result
}

fn tui_session_path_from_latest(
    resume: bool,
    latest_session_path: Option<PathBuf>,
    fresh_session_id: &str,
) -> PathBuf {
    if resume && let Some(path) = latest_session_path {
        return path;
    }
    session_log_path(fresh_session_id)
}

struct RunnerConfigInput<'a> {
    session_path: PathBuf,
    project_root: Option<PathBuf>,
    provider: Option<ProviderConfig>,
    provider_setup_error: Option<String>,
    startup_trace: Option<&'a StartupTrace>,
    catalog_refresh: Option<std::sync::mpsc::Receiver<String>>,
    model_discovery: Option<ModelDiscoveryFuture>,
    provider_connections: Option<Arc<dyn yach_backend::ProviderConnectionRuntime>>,
}

fn runner_config(input: RunnerConfigInput<'_>) -> RunnerConfig {
    let RunnerConfigInput {
        session_path,
        project_root,
        provider,
        provider_setup_error,
        startup_trace,
        catalog_refresh,
        model_discovery,
        provider_connections,
    } = input;
    RunnerConfig {
        session_path,
        project_root,
        provider,
        provider_setup_error,
        extension_package_roots: extension_package_roots_from_env(),
        extension_package_root_loader: Some(extension_package_root_loader()),
        startup_trace: startup_trace.cloned().map(startup_trace_marker),
        catalog_refresh,
        model_discovery,
        provider_connections,
    }
}

fn startup_trace_marker(startup_trace: StartupTrace) -> StartupTraceMarker {
    StartupTraceMarker::new(move |label| {
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
    roots.extend(
        records
            .iter()
            .filter(|record| !record.enabled && record.kind == ExtensionInstallRefKind::Bundled)
            .map(|record| ExtensionPackageRoot {
                root: record.package_root.clone(),
                scope: record.scope,
                source_ref: Some(record.source.clone()),
            }),
    );
    roots.extend(extension_package_roots_from_env());
    roots
}

fn extension_package_root_loader() -> ExtensionPackageRootLoader {
    ExtensionPackageRootLoader::new(installed_extension_package_roots)
}

fn installed_extension_package_roots() -> Vec<ExtensionPackageRoot> {
    extension_package_roots_from_install_records(&installed_extension_records())
}

#[cfg(not(test))]
fn bundled_hashline_package_root() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let root = home
        .join(".yach/bundled/yach-hashline")
        .join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    std::fs::set_permissions(&root, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;

    let executable = std::env::current_exe()?;
    let executable = executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "yach executable path is not UTF-8",
        )
    })?;
    let mut manifest =
        serde_json::from_str::<serde_json::Value>(yach_hashline_extension::MANIFEST_JSON)
            .map_err(io::Error::other)?;
    manifest["main"]["command"] = serde_json::Value::String(executable.to_owned());
    manifest["main"]["args"] = serde_json::json!(["__extension-host", "hashline"]);
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    bytes.push(b'\n');

    let manifest_path = root.join("yach.extension.json");
    if std::fs::read(&manifest_path).ok().as_deref() != Some(bytes.as_slice()) {
        let temp_path = root.join(format!(".yach.extension.json.{}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp_path, &manifest_path)?;
    }
    #[cfg(unix)]
    std::fs::set_permissions(
        &manifest_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )?;

    Ok(root)
}

#[cfg(not(test))]
fn ensure_bundled_hashline_install_record() -> io::Result<()> {
    let package_root = bundled_hashline_package_root()?;
    let path = extension_store_path(ExtensionInstallScope::User)?;
    let mut store = ExtensionInstallStore::load_from_path(&path)
        .map_err(|error| extension_install_io_error(&error))?;
    let before = store.clone();
    store
        .install_bundled("yach.hashline", &package_root, ExtensionInstallScope::User)
        .map_err(|error| extension_install_io_error(&error))?;
    if store != before {
        store
            .save_to_path(&path)
            .map_err(|error| extension_install_io_error(&error))?;
    }
    Ok(())
}

fn extension_package_roots_from_install_records(
    records: &[ExtensionInstallRecord],
) -> Vec<ExtensionPackageRoot> {
    records
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| {
            matches!(
                record.kind,
                ExtensionInstallRefKind::LocalPath | ExtensionInstallRefKind::Bundled
            )
        })
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
        #[cfg(not(test))]
        ensure_bundled_hashline_install_record()?;
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
        #[cfg(not(test))]
        ensure_bundled_hashline_install_record()?;
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
        ExtensionInstallError::BundledCannotRemove { .. } => "bundled_cannot_remove",
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
    #[cfg(not(test))]
    if let Err(error) = ensure_bundled_hashline_install_record() {
        return CommandResult::ExtensionDiagnostics {
            command,
            outcome: ExtensionDiagnosticsOutcome::Failed,
            records: Vec::new(),
            message: Some(format!("extension diagnostics failed: {error}")),
            host_start_count: 0,
        };
    }
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
    #[cfg(not(test))]
    if let Err(error) = ensure_bundled_hashline_install_record() {
        let _ = writeln!(
            io::stderr(),
            "warning: failed to prepare bundled hashline extension: {error}"
        );
    }
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

fn provider_test_delay_ms() -> Option<u64> {
    optional_bounded_env("YACH_NATIVE_PROVIDER_TEST_DELAY_MS", 0, 0, 30_000)
        .ok()
        .filter(|delay| *delay > 0)
}

fn provider_connection_timeout() -> Duration {
    Duration::from_secs(
        optional_bounded_env("YACH_RIG_PROVIDER_TIMEOUT_SECS", 120, 5, 600).unwrap_or(120),
    )
}

fn provider_model_from_env(provider: &str) -> String {
    match provider {
        // Sonnet is the interactive default: coding sessions need more
        // capability than the haiku-tier smoke-test default. Overridable
        // per launch and switchable live via /model.
        "anthropic" => optional_env("YACH_RIG_ANTHROPIC_MODEL")
            .unwrap_or_else(|| String::from("claude-sonnet-5")),
        "openai-codex" => optional_env("YACH_RIG_CHATGPT_MODEL")
            .unwrap_or_else(|| String::from("gpt-5.3-codex-spark")),
        // No default model on OpenAI proper either; config parsing
        // requires this env when the provider is selected.
        "openai" => optional_env("YACH_RIG_OPENAI_MODEL").unwrap_or_default(),
        // No sane universal default on compat endpoints; config parsing
        // requires this env when the provider is selected.
        "openai-compatible" => optional_env("YACH_RIG_OPENAI_COMPAT_MODEL").unwrap_or_default(),
        _ => String::from("unknown"),
    }
}

/// The model catalog resolution (and, previously, nothing else) should
/// use: an explicit override wins outright and verbatim, never falling
/// back to — or being silently shadowed by — the env-derived model. Pure
/// glue over `provider_model_from_env`, split out so the override-wins
/// contract is testable without needing to mutate real env vars.
fn resolved_model_for_config(provider_label: &str, model_override: Option<&str>) -> String {
    model_override.map_or_else(|| provider_model_from_env(provider_label), String::from)
}

#[cfg(test)]
#[test]
fn resolved_model_for_config_prefers_the_explicit_override_over_the_env_default() {
    // The whole point of the fix: an explicit --model override must win
    // outright, never falling back to (or being shadowed by) whatever
    // provider_model_from_env would derive from the environment — this is
    // what used to be wrong: the profile was always resolved against the
    // env-default model, even when the caller supplied one directly.
    assert_eq!(
        resolved_model_for_config("anthropic", Some("claude-haiku-4-5")),
        "claude-haiku-4-5"
    );
    assert_eq!(
        resolved_model_for_config("openai", Some("gpt-5.4-mini")),
        "gpt-5.4-mini"
    );
}

#[cfg(test)]
#[test]
fn resolved_model_for_config_falls_back_to_the_env_default_when_absent() {
    // No override supplied: must equal whatever `provider_model_from_env`
    // would produce on its own. Asserted relatively, against a live call
    // to `provider_model_from_env`, rather than against a hardcoded
    // literal — so this doesn't depend on (or need to mutate) real env
    // state; it holds whether or not YACH_RIG_*_MODEL happens to be set.
    for provider_label in ["anthropic", "openai-codex", "openai", "openai-compatible"] {
        assert_eq!(
            resolved_model_for_config(provider_label, None),
            provider_model_from_env(provider_label)
        );
    }
}

fn provider_label_from_config(config: &RigProviderAdapterConfig) -> &'static str {
    match &config.provider {
        RigProviderConfig::Anthropic { .. } => "anthropic",
        RigProviderConfig::ChatGptSubscription { .. } => "openai-codex",
        RigProviderConfig::OpenAi { .. } => "openai",
        RigProviderConfig::OpenAiCompatible { .. } => "openai-compatible",
    }
}
#[cfg(test)]
#[test]
fn provider_label_covers_openai_responses_variant() {
    let config = RigProviderAdapterConfig {
        provider: RigProviderConfig::OpenAi {
            api_key: ProviderSecret::new(String::from("test-key")),
            base_url: None,
        },
        timeout: Duration::from_secs(5),
        max_tokens: 1024,
        context_window: 10_000,
        max_tokens_param: MaxTokensParam::default(),
    };
    assert_eq!(provider_label_from_config(&config), "openai");
}

#[cfg(test)]
#[test]
fn loop_resumes_existing_session_without_duplicate_turn_ids() {
    use tokio::sync::mpsc;
    use yach_backend::{
        EntryId, JsonlSessionStore, Role, RunnerConfig, SessionEvent, SessionEventSink, SessionId,
        TurnId, TurnOutcome, run_native_loop,
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
        let store = JsonlSessionStore::new(path.clone());
        assert!(
            store
                .append_events(&[
                    SessionEvent::EntryAppended {
                        session_id: SessionId(String::from("default")),
                        entry_id: EntryId(String::from("entry-0-user")),
                        parent_entry_id: None,
                        turn_id: TurnId(String::from("turn-0")),
                        role: Role::User,
                        text: String::from("seed prompt"),
                        provider: None,
                    },
                    SessionEvent::TurnFinished {
                        session_id: SessionId(String::from("default")),
                        turn_id: TurnId(String::from("turn-0")),
                        outcome: TurnOutcome::Completed,
                        reason: None,
                    },
                ])
                .is_ok()
        );

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_native_loop(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
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
                SessionEvent::EntryAppended {
                    turn_id,
                    role: Role::User,
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
fn loop_emits_existing_session_messages_after_explicit_path_selection() {
    use tokio::sync::mpsc;
    use yach_backend::{
        EntryId, JsonlSessionStore, Role, RunnerConfig, SessionEvent, SessionEventSink, SessionId,
        TurnId, run_native_loop,
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
        let store = JsonlSessionStore::new(path.clone());
        assert!(
            store
                .append_events(&[
                    SessionEvent::EntryAppended {
                        session_id: SessionId(String::from("default")),
                        entry_id: EntryId(String::from("entry-0-user")),
                        parent_entry_id: None,
                        turn_id: TurnId(String::from("turn-0")),
                        role: Role::User,
                        text: String::from("seed prompt"),
                        provider: None,
                    },
                    SessionEvent::EntryAppended {
                        session_id: SessionId(String::from("default")),
                        entry_id: EntryId(String::from("entry-0-assistant")),
                        parent_entry_id: Some(EntryId(String::from("entry-0-user"))),
                        turn_id: TurnId(String::from("turn-0")),
                        role: Role::Assistant,
                        text: String::from("seed answer"),
                        provider: None,
                    },
                ])
                .is_ok()
        );

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_native_loop(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
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
fn loop_persists_prompt_runtime_metrics() {
    let persisted = tests::run_native_fixture_prompt("hello metrics");

    assert!(persisted.contains("metric_recorded"));
    assert!(persisted.contains("prompt_total"));
    assert!(!persisted.contains("session_log_load"));
}

#[cfg(test)]
#[test]
fn loop_provider_cancel_persists_user_entry() {
    use tokio::sync::mpsc;
    use yach_backend::{
        JsonlSessionStore, ProviderConfig, Role, RunnerConfig, SessionEvent,
        rig_adapter::{RigProviderAdapterConfig, RigProviderConfig},
        run_native_loop,
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
        let store = JsonlSessionStore::new(path.clone());
        let handle = tokio::spawn(run_native_loop(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: Some(ProviderConfig {
                    adapter: Arc::new(RigProviderAdapterConfig {
                        provider: RigProviderConfig::Anthropic {
                            api_key: ProviderSecret::new(String::from("fake-test-key")),
                            base_url: None,
                        },
                        timeout: std::time::Duration::from_millis(1),
                        max_tokens: 1,
                        context_window: 200_000,
                        max_tokens_param: MaxTokensParam::default(),
                    }),
                    model: String::from("fake-test-model"),
                    connection_id: None,
                    connection_display: None,
                    test_delay_ms: Some(500),
                    catalog_models: Vec::new().into(),
                    responses_compact: None,
                }),
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
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

        // Wait for the first terminal event with slack for a loaded CI
        // runner, then sample briefly for duplicates; a fixed sampling
        // window alone flaked when the loop took >100ms to get scheduled.
        let first_finished =
            tests::first_prompt_finished(&mut backend_rx, std::time::Duration::from_secs(10)).await;
        let extra_finished = tests::collect_prompt_finished_for(
            &mut backend_rx,
            std::time::Duration::from_millis(100),
        )
        .await;

        handle.abort();
        let loaded = store.load();
        let _ = std::fs::remove_file(path);
        assert_eq!(first_finished, Some(PromptOutcome::Cancelled));
        assert!(extra_finished.is_empty());
        assert!(loaded.is_ok());
        let events = loaded.unwrap_or_default().events;
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::EntryAppended {
                role: Role::User,
                text,
                ..
            } if text == "cancel before provider start"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::TurnFinished { turn_id, .. } if turn_id.0 == "turn-0"
        )));
    });
}

#[cfg(test)]
#[test]
fn loop_provider_cancel_after_finish_does_not_duplicate_terminal_turn() {
    use tokio::sync::mpsc;
    use yach_backend::{
        JsonlSessionStore, ProviderConfig, RunnerConfig, SessionEvent,
        rig_adapter::{RigProviderAdapterConfig, RigProviderConfig},
        run_native_loop,
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
        let store = JsonlSessionStore::new(path.clone());
        let handle = tokio::spawn(run_native_loop(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: path.clone(),
                project_root: None,
                provider: Some(ProviderConfig {
                    adapter: Arc::new(RigProviderAdapterConfig {
                        provider: RigProviderConfig::ChatGptSubscription {
                            auth_file: path.with_extension("missing-token-dir").join("auth.json"),
                        },
                        timeout: std::time::Duration::from_millis(1),
                        max_tokens: 1,
                        context_window: 200_000,
                        max_tokens_param: MaxTokensParam::default(),
                    }),
                    model: String::from("fake-test-model"),
                    connection_id: None,
                    connection_display: None,
                    test_delay_ms: None,
                    catalog_models: Vec::new().into(),
                    responses_compact: None,
                }),
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                startup_trace: None,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
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
        assert!(stale_cancel_prompt_finished.is_empty());

        handle.abort();
        let loaded = store.load();
        let _ = std::fs::remove_file(path);
        assert!(loaded.is_ok());
        let terminal_turn_count = loaded
            .unwrap_or_default()
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::TurnFinished { turn_id, .. } if turn_id.0 == "turn-0"
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
        NativeTuiBackendSetup, RigSmokeConfigError, RigSmokeOutcome, RunnerConfigInput,
        TuiBackendSelection, dialog_smoke_requests, extension_store_path, native_backend_handshake,
        print_capabilities, provider_setup_error_message, run_extension_install_command,
        run_extension_list_command, run_extension_remove_command,
        run_extension_set_enabled_command, runner_config, tui_session_path_from_latest,
        tui_theme_path, unconfigured_launch_setup_error,
    };
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::process::Command as ProcessCommand;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::mpsc;
    use yach_backend::{
        BackendMetadata, ExtensionActivationState, ExtensionInstallScope, RunnerConfig,
        run_native_loop, session_log_path, start_backend_session,
    };
    use yach_connections::{
        ConnectionId, CredentialError, CredentialStore, JsonConnectionMetadataStore,
        NewConnectionDraft, ProviderKind, ProviderSecret,
    };
    use yach_proto::{BackendEvent, ClientEvent, PromptOutcome, ServerEvent};
    use yach_ui::negotiate_with;

    trait TestUnwrap {
        type Output;

        fn test_unwrap(self) -> Self::Output;
    }

    impl<T, E> TestUnwrap for Result<T, E> {
        type Output = T;

        fn test_unwrap(self) -> Self::Output {
            assert!(self.is_ok());
            match self {
                Ok(value) => value,
                Err(_) => unreachable!(),
            }
        }
    }

    impl<T> TestUnwrap for Option<T> {
        type Output = T;

        fn test_unwrap(self) -> Self::Output {
            assert!(self.is_some());
            match self {
                Some(value) => value,
                None => unreachable!(),
            }
        }
    }

    #[test]
    fn cli_defaults_to_interactive_tui_session() {
        let cli = CliArgs::from_args(std::iter::empty());

        assert_eq!(
            cli.command,
            Command::Tui {
                backend: TuiBackendSelection::Provider,
                resume: false,
            }
        );
        assert!(!cli.quiet);
    }

    #[test]
    fn native_launch_negotiates_provider_connections_only_with_runtime() {
        for available in [false, true] {
            let handshake = native_backend_handshake(&NativeTuiBackendSetup::Fixture, available);
            let negotiated = negotiate_with(&handshake);
            let mut session = start_backend_session(BackendMetadata::native(), negotiated);
            let event = session.channels.backend_rx.try_recv().test_unwrap();
            let BackendEvent::Connected { negotiated } = event else {
                unreachable!("native start must announce negotiated capabilities");
            };

            assert_eq!(
                negotiated.supports(yach_proto::Capability::ProviderConnections),
                available
            );
        }
    }

    #[test]
    fn cli_bare_flags_configure_the_default_tui_session() {
        let resume = CliArgs::from_args([String::from("--resume")].into_iter());
        let backend =
            CliArgs::from_args([String::from("--backend"), String::from("fixture")].into_iter());

        assert_eq!(
            resume.command,
            Command::Tui {
                backend: TuiBackendSelection::Provider,
                resume: true,
            }
        );
        assert_eq!(
            backend.command,
            Command::Tui {
                backend: TuiBackendSelection::Fixture,
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
    fn tui_theme_path_prefers_explicit_then_project_then_user() -> Result<(), String> {
        let root = TestTempDir::new("theme-path")?;
        let project = root.path().join("project");
        let home = root.path().join("home");
        let project_theme = project.join(".yach/theme.json");
        let user_theme = home.join(".yach/theme.json");
        expect_ok(std::fs::create_dir_all(
            project_theme.parent().ok_or("project parent")?,
        ))?;
        expect_ok(std::fs::create_dir_all(
            user_theme.parent().ok_or("user parent")?,
        ))?;
        expect_ok(std::fs::write(&project_theme, "{}"))?;
        expect_ok(std::fs::write(&user_theme, "{}"))?;

        expect_equal(
            &tui_theme_path(Some(&project), None, Some(&home)),
            &Some(project_theme.clone()),
        )?;
        expect_ok(std::fs::remove_file(project.join(".yach/theme.json")))?;
        expect_equal(
            &tui_theme_path(Some(&project), None, Some(&home)),
            &Some(user_theme),
        )?;
        let explicit = root.path().join("chosen-theme.json");
        expect_equal(
            &tui_theme_path(Some(&project), Some(&explicit), Some(&home)),
            &Some(explicit),
        )?;
        Ok(())
    }

    #[test]
    fn cli_parses_supported_commands() {
        let print = CliArgs::from_args([String::from("print-capabilities")].into_iter());
        let rig_smoke =
            CliArgs::from_args([String::from("smoke-rig-openai-compatible")].into_iter());
        let http_smoke =
            CliArgs::from_args([String::from("smoke-openai-compatible-http")].into_iter());
        let anthropic_smoke = CliArgs::from_args([String::from("smoke-rig-anthropic")].into_iter());
        let openai_smoke = CliArgs::from_args([String::from("smoke-rig-openai")].into_iter());
        let chatgpt_smoke =
            CliArgs::from_args([String::from("smoke-rig-chatgpt-subscription")].into_iter());
        let responses_compaction_smoke =
            CliArgs::from_args([String::from("smoke-responses-compaction")].into_iter());
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
        let fixture_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("fixture"),
            ]
            .into_iter(),
        );
        // An unrecognized backend value falls back to the provider runner.
        let unknown_backend_tui = CliArgs::from_args(
            [
                String::from("tui"),
                String::from("--backend"),
                String::from("nonsense"),
            ]
            .into_iter(),
        );

        assert_eq!(print.command, Command::PrintCapabilities);
        assert_eq!(rig_smoke.command, Command::SmokeRigOpenAiCompatible);
        assert_eq!(http_smoke.command, Command::SmokeOpenAiCompatibleHttp);
        assert_eq!(anthropic_smoke.command, Command::SmokeRigAnthropic);
        assert_eq!(openai_smoke.command, Command::SmokeRigOpenAi);
        assert_eq!(chatgpt_smoke.command, Command::SmokeRigChatGptSubscription);
        assert_eq!(
            provider_request_smoke.command,
            Command::SmokeRigProviderRequest
        );
        assert_eq!(
            responses_compaction_smoke.command,
            Command::SmokeResponsesCompaction
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
                backend: TuiBackendSelection::Provider,
                resume: false,
            }
        );
        assert_eq!(
            resume_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Provider,
                resume: true,
            }
        );
        assert_eq!(
            fixture_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Fixture,
                resume: false,
            }
        );
        assert_eq!(
            unknown_backend_tui.command,
            Command::Tui {
                backend: TuiBackendSelection::Provider,
                resume: false,
            }
        );
    }

    #[test]
    fn smoke_responses_compaction_missing_key_output_is_redacted() {
        let result = CommandResult::ResponsesCompactionSmoke {
            outcome: RigSmokeOutcome::MissingConfig,
            model: None,
            stages: Vec::new(),
            artifact_item_count: 0,
            token_count: 0,
        };

        assert_eq!(
            result.render_lines(),
            vec![
                String::from("responses_compaction_smoke=missing_config"),
                String::from("artifact_item_count=0"),
                String::from("token_count=0"),
                String::from("prerequisite=YACH_RIG_OPENAI_API_KEY"),
            ]
        );
    }

    #[test]
    fn smoke_responses_compaction_command_is_available() {
        let cli = CliArgs::from_args([String::from("smoke-responses-compaction")].into_iter());

        assert_eq!(cli.command, Command::SmokeResponsesCompaction);
    }
    #[test]
    fn responses_compaction_smoke_workspace_uses_a_low_kept_tail_budget() {
        let workspace = super::responses_compaction_smoke_workspace();
        assert!(workspace.is_ok());
        let Ok(workspace) = workspace else {
            return;
        };
        let config = yach_backend::CompactionConfig::load_for_project(Some(&workspace.root));

        assert_eq!(config.reserve_tokens, 0);
        assert_eq!(config.keep_recent_tokens, 1);
        assert_eq!(config.auto_threshold_percent, 10);
        assert_eq!(
            workspace.session_path.parent(),
            Some(workspace.root.as_path())
        );
    }

    #[test]
    fn responses_compaction_smoke_workspace_primes_foldable_history() {
        let workspace = super::responses_compaction_smoke_workspace();
        assert!(workspace.is_ok());
        let Ok(workspace) = workspace else {
            return;
        };
        let log = yach_backend::JsonlSessionStore::new(workspace.session_path.clone()).load();
        assert!(log.is_ok());
        let Ok(log) = log else {
            return;
        };

        assert!(yach_backend::estimate_current_context_tokens(&log) > 2_000);
        assert!(log.events.iter().any(|event| matches!(
            event,
            yach_backend::SessionEvent::TurnFinished {
                outcome: yach_backend::TurnOutcome::Completed,
                ..
            }
        )));
    }

    #[test]
    fn cli_parses_hidden_provider_connection_smoke_command() {
        let cli = CliArgs::from_args([String::from("tui-provider-connection-smoke")].into_iter());

        assert_eq!(cli.command, Command::TuiProviderConnectionSmoke);
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
    fn tui_provider_connection_smoke_fixture_serves_create_refresh_picker_and_prompt() {
        let fixture = super::TuiProviderConnectionSmokeFixture::start(
            super::TUI_PROVIDER_CONNECTION_SMOKE_SECRET,
        )
        .test_unwrap();
        for _ in 0..3 {
            let response =
                smoke_fixture_request(&fixture.base_url, "GET /v1/models HTTP/1.1\r\n\r\n");
            assert!(
                response.contains("200 OK"),
                "models request must receive a response"
            );
        }
        let response = smoke_fixture_request(
            &fixture.base_url,
            &format!(
                "POST /v1/chat/completions HTTP/1.1\r\nauthorization: Bearer {}\r\nContent-Length: 22\r\n\r\n{{\"model\":\"task-7-model\"}}",
                super::TUI_PROVIDER_CONNECTION_SMOKE_SECRET
            ),
        );
        assert!(
            response.contains("200 OK") && response.contains("task seven completion"),
            "prompt request must receive the streaming fixture response"
        );
        assert!(fixture.models.load(Ordering::SeqCst));
        assert!(fixture.prompt.load(Ordering::SeqCst));
        assert!(fixture.expected_auth_and_model.load(Ordering::SeqCst));
    }

    fn smoke_fixture_request(base_url: &str, request: &str) -> String {
        let address = base_url
            .strip_prefix("http://")
            .and_then(|value| value.strip_suffix("/v1"))
            .test_unwrap();
        let mut stream = std::net::TcpStream::connect(address).test_unwrap();
        stream.write_all(request.as_bytes()).test_unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).test_unwrap();
        response
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
    fn config_defers_installed_roots_to_first_render_loader() -> Result<(), String> {
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

            let config = runner_config(RunnerConfigInput {
                session_path: temp_native_log_path(),
                project_root: None,
                provider: None,
                provider_setup_error: None,
                startup_trace: None,
                catalog_refresh: Some(std::sync::mpsc::channel().1),
                model_discovery: None,
                provider_connections: None,
            });

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
    fn tui_fresh_launch_ignores_existing_latest_session() {
        let latest = std::env::temp_dir().join("latest-native-session.jsonl");
        assert_eq!(
            tui_session_path_from_latest(true, Some(latest.clone()), "fresh-session"),
            latest
        );
        assert_eq!(
            tui_session_path_from_latest(false, Some(latest), "fresh-session"),
            session_log_path("fresh-session")
        );
    }

    #[test]
    fn tui_resume_without_existing_session_uses_fresh_session() {
        assert_eq!(
            tui_session_path_from_latest(true, None, "fresh-session"),
            session_log_path("fresh-session")
        );
    }

    #[test]
    fn loop_streams_and_persists_prompt() {
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
            let handle = tokio::spawn(run_native_loop(
                client_rx,
                backend_tx,
                RunnerConfig {
                    session_path: path.clone(),
                    project_root: None,
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                    catalog_refresh: None,
                    model_discovery: None,
                    provider_connections: None,
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
    fn runner_config_uses_the_resolved_project_root() {
        let expected = Some(PathBuf::from("/tmp/yach-project-root"));
        let config = runner_config(RunnerConfigInput {
            session_path: temp_native_log_path(),
            project_root: expected.clone(),
            provider: None,
            provider_setup_error: None,
            startup_trace: None,
            catalog_refresh: Some(std::sync::mpsc::channel().1),
            model_discovery: None,
            provider_connections: None,
        });

        assert_eq!(config.project_root, expected);
    }

    #[test]
    fn provider_setup_error_copy_is_actionable() {
        let message = provider_setup_error_message(&RigSmokeConfigError::Missing(
            "YACH_RIG_ANTHROPIC_API_KEY",
        ));

        assert_eq!(
            message,
            "provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY"
        );
    }

    #[test]
    fn unconfigured_launch_setup_error_requires_both_missing_env_and_no_stored_connections() {
        let error = RigSmokeConfigError::Missing("YACH_RIG_ANTHROPIC_API_KEY");

        assert_eq!(
            unconfigured_launch_setup_error(&error, false).as_deref(),
            Some("provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY")
        );
        assert_eq!(unconfigured_launch_setup_error(&error, true), None);
    }

    #[test]
    fn loop_persists_failed_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-fail");

        assert!(persisted.contains("failed"));
        assert!(persisted.contains("provider_error kind=provider_internal"));
        assert!(!persisted.contains("fixture provider failure"));
    }

    #[test]
    fn loop_persists_malformed_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-malformed");

        assert!(persisted.contains("failed"));
        assert!(persisted.contains("provider_error kind=malformed_stream"));
        assert!(!persisted.contains("fixture malformed stream"));
    }

    #[test]
    fn loop_persists_cancelled_fixture_turn() {
        let persisted = run_native_fixture_prompt("/native-fixture-cancel");

        assert!(persisted.contains("cancelled"));
        assert!(persisted.contains("provider_error kind=cancelled"));
        assert!(!persisted.contains("fixture cancellation"));
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
            let handle = tokio::spawn(run_native_loop(
                client_rx,
                backend_tx,
                RunnerConfig {
                    session_path: path.clone(),
                    project_root: None,
                    provider: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    startup_trace: None,
                    catalog_refresh: None,
                    model_discovery: None,
                    provider_connections: None,
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
            "yach-native-test-{}-{unique}-{id}.jsonl",
            std::process::id()
        ))
    }

    const RESTART_FIXTURE_SECRET: &str = "task-7-secret-never-print";

    #[test]
    fn provider_connections_survive_restart_and_complete_a_real_provider_turn() {
        if let Ok(phase) = std::env::var("YACH_TASK_7_CHILD_PHASE") {
            let result = match phase.as_str() {
                "create" => provider_connection_restart_create_child(),
                "verify" => provider_connection_restart_verify_child(),
                "leak-secret-stdout" => provider_connection_restart_leak_secret_child(false),
                "leak-secret-stderr" => provider_connection_restart_leak_secret_child(true),
                _ => Err(String::from("unknown task-7 child phase")),
            };
            if let Err(error) = result {
                unreachable!("task-7 child failed: {error}");
            }
            return;
        }

        let root = TestTempDir::new("provider-connection-restart").test_unwrap();
        let registry = root.path().join("connections.json");
        let credentials = root.path().join("fixture-credentials");
        let (base_url, fixture) =
            local_openai_compatible_fixture(RESTART_FIXTURE_SECRET, Duration::from_secs(10));

        let create_output = run_provider_connection_restart_child(
            "create",
            &registry,
            &credentials,
            Some(&base_url),
            Some(RESTART_FIXTURE_SECRET),
            RESTART_FIXTURE_SECRET,
        );
        assert_child_succeeded("create", &create_output);
        let verify_output = run_provider_connection_restart_child(
            "verify",
            &registry,
            &credentials,
            Some(&base_url),
            None,
            RESTART_FIXTURE_SECRET,
        );
        let fixture = fixture
            .wait_for_completion(Duration::from_secs(11))
            .test_unwrap();
        assert!(
            verify_output.status.success(),
            "task-7 child phase verify failed: {} (status {}; fixture requests {}; prompt {}; complete {})",
            sanitized_child_failure_reason(&verify_output),
            verify_output.status,
            fixture.request_count.load(Ordering::SeqCst),
            fixture.prompt.load(Ordering::SeqCst),
            fixture.completed.load(Ordering::SeqCst),
        );

        assert_eq!(
            fixture.request_count.load(Ordering::SeqCst),
            FIXTURE_REQUEST_COUNT,
            "restart fixture must serve create validation, fresh discovery, and prompt"
        );
        assert_eq!(
            *fixture.request_sequence.lock().test_unwrap(),
            vec![
                FixtureRequestKind::Models,
                FixtureRequestKind::Models,
                FixtureRequestKind::Prompt,
            ],
            "restart fixture requests must use the expected sequence"
        );
        assert!(
            fixture.completed.load(Ordering::SeqCst),
            "fixture did not complete all requests"
        );
        assert!(
            fixture.models.load(Ordering::SeqCst),
            "fresh process did not discover models"
        );
        assert!(
            fixture.prompt.load(Ordering::SeqCst),
            "fresh process did not stream a provider prompt"
        );
        assert!(
            fixture.expected_auth_and_model.load(Ordering::SeqCst),
            "provider request did not use the persisted credential and exact model"
        );
    }

    fn run_provider_connection_restart_child(
        phase: &str,
        registry: &Path,
        credentials: &Path,
        base_url: Option<&str>,
        secret: Option<&str>,
        forbidden_secret: &str,
    ) -> std::process::Output {
        let executable = std::env::current_exe().test_unwrap();
        let mut command = ProcessCommand::new(executable);
        command
            .arg("--exact")
            .arg("tests::provider_connections_survive_restart_and_complete_a_real_provider_turn")
            .arg("--nocapture")
            .env("YACH_TASK_7_CHILD_PHASE", phase)
            .env("YACH_TASK_7_REGISTRY", registry)
            .env("YACH_TASK_7_CREDENTIALS", credentials);
        if let Some(base_url) = base_url {
            command.env("YACH_TASK_7_BASE_URL", base_url);
        }
        if let Some(secret) = secret {
            command.env("YACH_TASK_7_SECRET", secret);
        }
        let output = command.output().test_unwrap();
        assert_child_output_is_secret_free(phase, &output, forbidden_secret);
        output
    }

    fn assert_child_output_is_secret_free(
        phase: &str,
        output: &std::process::Output,
        forbidden_secret: &str,
    ) {
        let secret = forbidden_secret.as_bytes();
        assert!(!secret.is_empty(), "fixture credential must not be empty");
        assert!(
            !output
                .stdout
                .windows(secret.len())
                .any(|window| window == secret),
            "task-7 child phase {phase} emitted the provider credential to stdout"
        );
        assert!(
            !output
                .stderr
                .windows(secret.len())
                .any(|window| window == secret),
            "task-7 child phase {phase} emitted the provider credential to stderr"
        );
    }

    fn assert_child_succeeded(phase: &str, output: &std::process::Output) {
        assert!(
            output.status.success(),
            "task-7 child phase {phase} failed: {} (status {}; stdout {} bytes; stderr {} bytes)",
            sanitized_child_failure_reason(output),
            output.status,
            output.stdout.len(),
            output.stderr.len(),
        );
    }

    fn sanitized_child_failure_reason(output: &std::process::Output) -> &'static str {
        for (needle, reason) in [
            (
                b"connection creation connection validation failed".as_slice(),
                "connection validation failed",
            ),
            (
                b"connection creation connection authentication failed".as_slice(),
                "connection authentication failed",
            ),
            (
                b"connection creation connection network request failed".as_slice(),
                "connection network request failed",
            ),
            (
                b"connection creation connection operation unavailable".as_slice(),
                "connection operation unavailable",
            ),
            (
                b"restart connection dialog".as_slice(),
                "restart connection dialog did not open",
            ),
            (
                b"restart model discovery".as_slice(),
                "restart model discovery did not complete",
            ),
            (
                b"restart model activation".as_slice(),
                "restart model activation did not complete",
            ),
            (
                b"restart prompt completion".as_slice(),
                "restart prompt did not complete",
            ),
            (
                b"timed out waiting for backend event".as_slice(),
                "backend event timed out",
            ),
            (
                b"fresh runtime could not list persisted connection".as_slice(),
                "persisted connection was not listed",
            ),
            (
                b"fresh runtime listed no persisted connection".as_slice(),
                "persisted connection list was empty",
            ),
            (
                b"expected backend event was not emitted".as_slice(),
                "expected backend event was not emitted",
            ),
        ] {
            if output
                .stdout
                .windows(needle.len())
                .chain(output.stderr.windows(needle.len()))
                .any(|window| window == needle)
            {
                return reason;
            }
        }
        "child failure output suppressed"
    }

    #[test]
    fn provider_connection_restart_child_rejects_secret_output_without_echoing_it() {
        const SECRET: &str = RESTART_FIXTURE_SECRET;
        let root = TestTempDir::new("provider-connection-child-output").test_unwrap();

        for phase in ["leak-secret-stdout", "leak-secret-stderr"] {
            let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_provider_connection_restart_child(
                    phase,
                    &root.path().join("connections.json"),
                    &root.path().join("fixture-credentials"),
                    None,
                    Some(SECRET),
                    SECRET,
                );
            })) else {
                unreachable!("child output containing the credential must be rejected");
            };
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .test_unwrap();

            assert!(message.contains("provider credential"));
            assert!(
                !message.contains(SECRET),
                "failure diagnostics must redact the credential"
            );
        }
    }

    fn provider_connection_restart_create_child() -> Result<(), String> {
        let registry = child_path("YACH_TASK_7_REGISTRY")?;
        let credentials = child_path("YACH_TASK_7_CREDENTIALS")?;
        let base_url = child_env("YACH_TASK_7_BASE_URL")?;
        let secret = ProviderSecret::new(child_env("YACH_TASK_7_SECRET")?);
        let runtime = super::provider_connections::CliProviderConnectionRuntime::with_stores(
            Arc::new(JsonConnectionMetadataStore::new(registry)),
            Arc::new(FileCredentialStore::new(credentials)),
            super::model_layers_fixture(),
            None,
        );
        let draft = NewConnectionDraft::new(
            ProviderKind::OpenAiCompatible,
            Some(String::from("restart fixture")),
            Some(base_url),
        )
        .map_err(|error| format!("create draft rejected: {error}"))?;
        let outcome = tokio::runtime::Runtime::new()
            .map_err(|error| format!("test runtime unavailable: {error}"))?
            .block_on(yach_backend::ProviderConnectionRuntime::create(
                &runtime, draft, secret,
            ));
        match outcome {
            yach_backend::ConnectionMutationOutcome::Succeeded => Ok(()),
            yach_backend::ConnectionMutationOutcome::Failed(failure) => {
                Err(format!("connection creation {}", failure.status_message()))
            }
            yach_backend::ConnectionMutationOutcome::FailedAfterCreatePending {
                failure, ..
            } => Err(format!("connection creation {}", failure.status_message())),
            yach_backend::ConnectionMutationOutcome::Renamed { .. } => Err(String::from(
                "connection creation returned an invalid outcome",
            )),
        }
    }

    fn provider_connection_restart_leak_secret_child(stderr: bool) -> Result<(), String> {
        let secret = child_env("YACH_TASK_7_SECRET")?;
        if stderr {
            writeln!(std::io::stderr(), "{secret}")
        } else {
            writeln!(std::io::stdout(), "{secret}")
        }
        .map_err(|error| format!("child fixture cannot write output: {error}"))
    }

    fn provider_connection_restart_verify_child() -> Result<(), String> {
        let registry = child_path("YACH_TASK_7_REGISTRY")?;
        let credentials = child_path("YACH_TASK_7_CREDENTIALS")?;
        let runtime = Arc::new(
            super::provider_connections::CliProviderConnectionRuntime::with_stores(
                Arc::new(JsonConnectionMetadataStore::new(registry.clone())),
                Arc::new(FileCredentialStore::new(credentials)),
                super::model_layers_fixture(),
                None,
            ),
        );
        tokio::runtime::Runtime::new()
            .map_err(|error| format!("test runtime unavailable: {error}"))?
            .block_on(async move {
                let ui_handshake = yach_ui::alpha_handshake();
                let backend_handshake =
                    super::native_backend_handshake(&super::NativeTuiBackendSetup::Fixture, true);
                let negotiated = yach_ui::negotiate_with(&backend_handshake);
                let backend_session = start_backend_session(BackendMetadata::native(), negotiated);
                let session_path = registry.with_extension("session.jsonl");
                let backend = tokio::spawn(run_native_loop(
                    backend_session.endpoints.client_rx,
                    backend_session.endpoints.backend_tx.clone(),
                    super::runner_config(super::RunnerConfigInput {
                        session_path,
                        project_root: None,
                        provider: None,
                        provider_setup_error: None,
                        startup_trace: None,
                        catalog_refresh: None,
                        model_discovery: None,
                        provider_connections: Some(
                            runtime.clone() as Arc<dyn yach_backend::ProviderConnectionRuntime>
                        ),
                    }),
                ));
                let client = backend_session.channels.client_tx;
                let mut events = backend_session.channels.backend_rx;
                client
                    .send(ClientEvent::Initialize(ui_handshake))
                    .map_err(|_| String::from("backend rejected initialize"))?;
                client
                    .send(ClientEvent::FirstRenderCompleted)
                    .map_err(|_| String::from("backend rejected first-render marker"))?;
                client
                    .send(ClientEvent::ConnectionsRequested)
                    .map_err(|_| String::from("backend rejected connect request"))?;
                wait_for_server_event(&mut events, |event| {
                    matches!(
                        event,
                        ServerEvent::DialogRequested(request)
                            if request.id.as_deref() == Some("provider-connection:root")
                    )
                })
                .await
                .map_err(|error| format!("restart connection dialog: {error}"))?;
                client
                    .send(ClientEvent::AvailableModelsRequested)
                    .map_err(|_| String::from("backend rejected discovery request"))?;
                wait_for_server_event(&mut events, |event| {
                    matches!(
                        event,
                        ServerEvent::DiscoveredModelsUpdated { models }
                            if models.iter().any(|model| {
                                model.id == "task-7-model"
                                    && model.provider == "openai-compatible"
                                    && model.connection_id.is_some()
                            })
                    )
                })
                .await
                .map_err(|error| format!("restart model discovery: {error}"))?;
                let list = yach_backend::ProviderConnectionRuntime::list(runtime.as_ref()).await;
                let yach_backend::ConnectionListOutcome::Available(list) = list else {
                    return Err(String::from(
                        "fresh runtime could not list persisted connection",
                    ));
                };
                let Some(connection) = list.as_slice().first() else {
                    return Err(String::from("fresh runtime listed no persisted connection"));
                };
                let connection_id = connection.id.as_str().to_owned();
                client
                    .send(ClientEvent::ModelSelectedDetailed {
                        provider: String::from("openai-compatible"),
                        model_id: String::from("task-7-model"),
                        connection_id: Some(connection_id.clone()),
                        request_id: 0,
                    })
                    .map_err(|_| String::from("backend rejected exact model selection"))?;
                wait_for_server_event(&mut events, |event| {
                    matches!(
                        event,
                        ServerEvent::ModelChanged(target)
                            if target.model == "task-7-model"
                                && target.connection_id.as_deref() == Some(connection_id.as_str())
                    )
                })
                .await
                .map_err(|error| format!("restart model activation: {error}"))?;
                client
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from("restart durability prompt"),
                    })
                    .map_err(|_| String::from("backend rejected prompt"))?;
                wait_for_server_event(&mut events, |event| {
                    matches!(
                        event,
                        ServerEvent::PromptFinished {
                            outcome: PromptOutcome::Completed,
                            ..
                        }
                    )
                })
                .await
                .map_err(|error| format!("restart prompt completion: {error}"))?;
                backend.abort();
                Ok(())
            })
    }

    async fn wait_for_server_event(
        events: &mut mpsc::UnboundedReceiver<BackendEvent>,
        predicate: impl Fn(&ServerEvent) -> bool,
    ) -> Result<(), String> {
        for _ in 0..64 {
            let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .map_err(|_| String::from("timed out waiting for backend event"))?
                .ok_or_else(|| String::from("backend event channel closed"))?;
            if let BackendEvent::Server(event) = event
                && predicate(&event)
            {
                return Ok(());
            }
        }
        Err(String::from("expected backend event was not emitted"))
    }

    fn child_env(name: &str) -> Result<String, String> {
        std::env::var(name).map_err(|_| format!("missing task-7 child environment {name}"))
    }

    fn child_path(name: &str) -> Result<PathBuf, String> {
        child_env(name).map(PathBuf::from)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FixtureRequestKind {
        Models,
        Prompt,
    }

    #[derive(Default)]
    struct FixtureObservation {
        models: AtomicBool,
        prompt: AtomicBool,
        expected_auth_and_model: AtomicBool,
        request_count: AtomicUsize,
        request_sequence: Mutex<Vec<FixtureRequestKind>>,
        completed: AtomicBool,
    }
    const MAX_FIXTURE_REQUEST_BYTES: usize = 64 * 1024;
    const FIXTURE_REQUEST_COUNT: usize = 3;

    struct FixtureShutdown {
        cancelled: AtomicBool,
        active_stream: Mutex<Option<std::net::TcpStream>>,
    }

    impl FixtureShutdown {
        fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Some(stream) = self.active_stream.lock().test_unwrap().as_ref() {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }

        fn track(&self, stream: &std::net::TcpStream) -> Result<(), String> {
            let stream = stream
                .try_clone()
                .map_err(|error| format!("fixture failed tracking active request: {error}"))?;
            *self.active_stream.lock().test_unwrap() = Some(stream);
            if self.is_cancelled() {
                self.cancel();
                return Err(String::from("fixture shutdown requested"));
            }
            Ok(())
        }

        fn clear(&self) {
            *self.active_stream.lock().test_unwrap() = None;
        }
    }

    struct LocalOpenAiCompatibleFixture {
        observation: Arc<FixtureObservation>,
        completion: std::sync::mpsc::Receiver<Result<(), String>>,
        shutdown: Arc<FixtureShutdown>,
        worker: std::thread::JoinHandle<()>,
    }

    impl LocalOpenAiCompatibleFixture {
        fn wait_for_completion(self, timeout: Duration) -> Result<Arc<FixtureObservation>, String> {
            let LocalOpenAiCompatibleFixture {
                observation,
                completion,
                shutdown,
                worker,
            } = self;
            match completion.recv_timeout(timeout) {
                Ok(result) => {
                    worker
                        .join()
                        .map_err(|_| String::from("fixture worker panicked"))?;
                    result?;
                    Ok(observation)
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    worker
                        .join()
                        .map_err(|_| String::from("fixture worker panicked"))?;
                    Err(String::from(
                        "fixture worker exited without reporting completion",
                    ))
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    shutdown.cancel();
                    worker
                        .join()
                        .map_err(|_| String::from("fixture worker panicked"))?;
                    let _ = completion.recv();
                    Err(String::from(
                        "fixture did not complete before parent timeout",
                    ))
                }
            }
        }
    }

    fn local_openai_compatible_fixture(
        expected_secret: &'static str,
        deadline: Duration,
    ) -> (String, LocalOpenAiCompatibleFixture) {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        listener.set_nonblocking(true).test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let observation = Arc::new(FixtureObservation::default());
        let observed = observation.clone();
        let shutdown = Arc::new(FixtureShutdown {
            cancelled: AtomicBool::new(false),
            active_stream: Mutex::new(None),
        });
        let worker_shutdown = shutdown.clone();
        let (completion_tx, completion) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = run_local_openai_compatible_fixture(
                &listener,
                &observed,
                expected_secret,
                deadline,
                &worker_shutdown,
            );
            let _ = completion_tx.send(result);
        });
        (
            format!("http://{address}/v1"),
            LocalOpenAiCompatibleFixture {
                observation,
                completion,
                shutdown,
                worker,
            },
        )
    }

    fn run_local_openai_compatible_fixture(
        listener: &TcpListener,
        observed: &FixtureObservation,
        expected_secret: &'static str,
        deadline: Duration,
        shutdown: &FixtureShutdown,
    ) -> Result<(), String> {
        let deadline = std::time::Instant::now() + deadline;
        for (index, expected) in [
            FixtureRequestKind::Models,
            FixtureRequestKind::Models,
            FixtureRequestKind::Prompt,
        ]
        .into_iter()
        .enumerate()
        {
            if shutdown.is_cancelled() {
                return Err(String::from("fixture shutdown requested"));
            }
            let request_number = index + 1;
            let mut stream =
                accept_fixture_connection(listener, deadline, request_number, shutdown)?;
            shutdown.track(&stream)?;
            let request = read_fixture_request_until(&mut stream, deadline);
            let request = request.map_err(|error| {
                format!(
                    "fixture failed reading request {request_number} of {FIXTURE_REQUEST_COUNT}: {error}"
                )
            })?;
            let request_kind = if request.starts_with("GET /v1/models ") {
                FixtureRequestKind::Models
            } else if request.starts_with("POST /v1/chat/completions ") {
                FixtureRequestKind::Prompt
            } else {
                return Err(format!(
                    "fixture received an unexpected request {request_number} of {FIXTURE_REQUEST_COUNT}"
                ));
            };
            observed.request_count.fetch_add(1, Ordering::SeqCst);
            observed
                .request_sequence
                .lock()
                .test_unwrap()
                .push(request_kind);
            if request_kind != expected {
                return Err(format!(
                    "fixture request {request_number} of {FIXTURE_REQUEST_COUNT} used an unexpected endpoint"
                ));
            }

            if request_kind == FixtureRequestKind::Models {
                observed.models.store(true, Ordering::SeqCst);
                let body = r#"{"object":"list","data":[{"id":"task-7-model","object":"model","created":0,"owned_by":"fixture"}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                )
                .map_err(|error| {
                    format!(
                        "fixture failed writing response for request {request_number} of {FIXTURE_REQUEST_COUNT}: {error}"
                    )
                })?;
            } else {
                observed.prompt.store(true, Ordering::SeqCst);
                let matches_secret = request
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {expected_secret}"));
                let matches_model = request.contains(r#""model":"task-7-model""#);
                observed
                    .expected_auth_and_model
                    .store(matches_secret && matches_model, Ordering::SeqCst);
                let body = concat!(
                    "data: {\"id\":\"chatcmpl-task-7\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"task-7-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"task seven completion\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"id\":\"chatcmpl-task-7\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"task-7-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                )
                .map_err(|error| {
                    format!(
                        "fixture failed writing response for request {request_number} of {FIXTURE_REQUEST_COUNT}: {error}"
                    )
                })?;
            }
            shutdown.clear();
        }
        observed.completed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn accept_fixture_connection(
        listener: &TcpListener,
        deadline: std::time::Instant,
        request_number: usize,
        shutdown: &FixtureShutdown,
    ) -> Result<std::net::TcpStream, String> {
        loop {
            if shutdown.is_cancelled() {
                return Err(String::from("fixture shutdown requested"));
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "fixture timed out waiting for request {request_number} of {FIXTURE_REQUEST_COUNT}"
                ));
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).map_err(|error| {
                        format!(
                            "fixture failed configuring request {request_number} of {FIXTURE_REQUEST_COUNT}: {error}"
                        )
                    })?;
                    return Ok(stream);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(format!(
                        "fixture failed accepting request {request_number} of {FIXTURE_REQUEST_COUNT}: {error}"
                    ));
                }
            }
        }
    }

    fn read_fixture_request(stream: &mut std::net::TcpStream) -> Result<String, std::io::Error> {
        read_fixture_request_until(stream, std::time::Instant::now() + Duration::from_secs(5))
    }

    fn fixture_request_deadline_elapsed() -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "fixture request deadline elapsed",
        )
    }

    fn read_fixture_request_until(
        stream: &mut std::net::TcpStream,
        deadline: std::time::Instant,
    ) -> Result<String, std::io::Error> {
        let mut request = Vec::with_capacity(4096);
        let mut expected_length = None;
        let mut buffer = [0_u8; 4096];
        loop {
            let remaining = MAX_FIXTURE_REQUEST_BYTES
                .checked_sub(request.len())
                .ok_or_else(|| std::io::Error::other("fixture request exceeds byte limit"))?;
            if remaining == 0 {
                return Err(std::io::Error::other("fixture request exceeds byte limit"));
            }
            let timeout = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(fixture_request_deadline_elapsed)?;
            stream.set_read_timeout(Some(timeout))?;
            let read_limit = remaining.min(buffer.len());
            let count = match stream.read(&mut buffer[..read_limit]) {
                Ok(count) => count,
                Err(_) if std::time::Instant::now() >= deadline => {
                    return Err(fixture_request_deadline_elapsed());
                }
                Err(error) => return Err(error),
            };
            if count == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "fixture request ended before complete headers and body",
                ));
            }
            request.extend_from_slice(&buffer[..count]);

            if expected_length.is_none()
                && let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
            {
                let headers = std::str::from_utf8(&request[..header_end]).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "fixture headers are not UTF-8",
                    )
                })?;
                let mut content_lengths = headers.lines().filter_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim())
                });
                let content_length = match content_lengths.next() {
                    Some(value) => value.parse::<usize>().map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "fixture content length is invalid",
                        )
                    })?,
                    None => 0,
                };
                if content_lengths.next().is_some() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "fixture request has multiple content lengths",
                    ));
                }
                let total_length = header_end
                    .checked_add(4)
                    .and_then(|length| length.checked_add(content_length))
                    .filter(|length| *length <= MAX_FIXTURE_REQUEST_BYTES)
                    .ok_or_else(|| std::io::Error::other("fixture request exceeds byte limit"))?;
                expected_length = Some(total_length);
            }

            if expected_length.is_some_and(|length| request.len() >= length) {
                return String::from_utf8(request).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "fixture request is not UTF-8",
                    )
                });
            }
        }
    }

    #[test]
    fn restart_fixture_reader_waits_for_a_segmented_request_body() {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (request_tx, request_rx) = std::sync::mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().test_unwrap();
            ready_tx.send(()).test_unwrap();
            request_tx
                .send(read_fixture_request(&mut stream))
                .test_unwrap();
        });
        let mut stream = std::net::TcpStream::connect(address).test_unwrap();
        ready_rx.recv_timeout(Duration::from_secs(1)).test_unwrap();
        stream
            .write_all(b"POST /v1/chat/completions HTTP/1.1\r\nContent-Length: 22\r\n\r\n")
            .test_unwrap();
        stream.flush().test_unwrap();
        assert!(
            request_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "fixture reader must wait for the declared request body"
        );
        stream
            .write_all(br#"{"model":"task-7-model"}"#)
            .test_unwrap();
        let request = request_rx
            .recv_timeout(Duration::from_secs(1))
            .test_unwrap()
            .test_unwrap();
        server.join().test_unwrap();

        assert!(request.ends_with(r#"{"model":"task-7-model"}"#));
    }

    #[test]
    fn restart_fixture_reports_missing_request_before_parent_timeout() {
        let (_, fixture) =
            local_openai_compatible_fixture(RESTART_FIXTURE_SECRET, Duration::from_millis(200));

        let Err(error) = fixture.wait_for_completion(Duration::from_secs(1)) else {
            unreachable!("fixture must report its missing first request");
        };

        assert!(
            error.contains("timed out waiting for request 1 of 3"),
            "unexpected fixture completion error: {error}"
        );
    }

    #[test]
    fn restart_fixture_reader_enforces_one_absolute_deadline_for_a_slow_drip() {
        let (base_url, fixture) =
            local_openai_compatible_fixture(RESTART_FIXTURE_SECRET, Duration::from_millis(200));
        let address = base_url
            .strip_prefix("http://")
            .and_then(|value| value.strip_suffix("/v1"))
            .test_unwrap()
            .to_owned();
        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(address).test_unwrap();
            for byte in b"GET /v1/models HTTP/1.1\r\n" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                let _ = stream.flush();
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        let Err(error) = fixture.wait_for_completion(Duration::from_millis(400)) else {
            unreachable!("fixture must reject the slow-drip request");
        };
        client.join().test_unwrap();

        assert!(
            error.contains("fixture request deadline elapsed"),
            "fixture must report the reader deadline rather than parent timeout: {error}"
        );
    }

    #[test]
    fn restart_fixture_parent_timeout_wakes_and_joins_the_active_reader() {
        let (base_url, fixture) =
            local_openai_compatible_fixture(RESTART_FIXTURE_SECRET, Duration::from_secs(10));
        let address = base_url
            .strip_prefix("http://")
            .and_then(|value| value.strip_suffix("/v1"))
            .test_unwrap();
        let mut stream = std::net::TcpStream::connect(address).test_unwrap();
        stream.write_all(b"G").test_unwrap();
        stream.flush().test_unwrap();

        let Err(error) = fixture.wait_for_completion(Duration::from_millis(100)) else {
            unreachable!("parent timeout must fail the incomplete request");
        };
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .test_unwrap();
        let mut byte = [0_u8; 1];
        let wake = stream.read(&mut byte);

        assert_eq!(error, "fixture did not complete before parent timeout");
        let socket_woke = match &wake {
            Ok(0) => true,
            Err(error) => !matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ),
            Ok(_) => false,
        };
        assert!(
            socket_woke,
            "fixture cancellation must wake the active socket rather than detach the worker: {wake:?}"
        );
    }

    struct FileCredentialStore {
        path: PathBuf,
    }

    impl FileCredentialStore {
        fn new(path: PathBuf) -> Self {
            Self { path }
        }

        fn read(&self) -> Result<BTreeMap<String, String>, CredentialError> {
            let Ok(raw) = std::fs::read_to_string(&self.path) else {
                return Ok(BTreeMap::new());
            };
            serde_json::from_str(&raw).map_err(|_| CredentialError::Invalid)
        }

        fn write(&self, values: &BTreeMap<String, String>) -> Result<(), CredentialError> {
            let encoded = serde_json::to_vec(values).map_err(|_| CredentialError::Invalid)?;
            std::fs::write(&self.path, encoded).map_err(|_| CredentialError::Unavailable)
        }
    }

    impl CredentialStore for FileCredentialStore {
        fn put(&self, id: &ConnectionId, secret: &ProviderSecret) -> Result<(), CredentialError> {
            let mut values = self.read()?;
            let secret = secret.with_exposed(ToOwned::to_owned);
            values.insert(id.as_str().to_owned(), secret);
            self.write(&values)
        }

        fn get(&self, id: &ConnectionId) -> Result<Option<ProviderSecret>, CredentialError> {
            self.read()
                .map(|values| values.get(id.as_str()).cloned().map(ProviderSecret::new))
        }

        fn remove(&self, id: &ConnectionId) -> Result<(), CredentialError> {
            let mut values = self.read()?;
            values.remove(id.as_str());
            self.write(&values)
        }
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

    /// Wait up to `deadline` for the first `PromptFinished` and return
    /// its outcome. Unlike `wait_for_prompt_finished`, non-matching
    /// outcomes are returned rather than skipped, so callers can assert
    /// on exactly what arrived first.
    pub(super) async fn first_prompt_finished(
        backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
        deadline: std::time::Duration,
    ) -> Option<yach_proto::PromptOutcome> {
        let deadline = tokio::time::Instant::now() + deadline;
        loop {
            match tokio::time::timeout_at(deadline, backend_rx.recv()).await {
                Ok(Some(BackendEvent::Server(ServerEvent::PromptFinished { outcome, .. }))) => {
                    return Some(outcome);
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return None,
            }
        }
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
                yach_proto::DialogKind::SecretInput => {
                    unreachable!("secret input dialogs are not included in the smoke fixture")
                }
                yach_proto::DialogKind::DeviceCode { .. } => {
                    unreachable!("device code dialogs are not included in the smoke fixture")
                }
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
