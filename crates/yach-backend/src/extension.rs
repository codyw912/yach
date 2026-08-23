use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::{Deserialize, Serialize};

use crate::extension_install::ExtensionInstallRecord;
use crate::{
    ExtensionStaticContextFile, ProviderToolVisibility, ResolvedToolCatalog,
    StaticContextPlacement, ToolDefinition, ToolInputSchema, ToolPermissionPolicy,
    ToolRegistrationError, ToolRegistry, ToolReplacementPolicy, ToolReplacementRule,
    ToolReplacementSource, ToolResolutionError, ToolResolutionMode, ToolRisk,
    tools::{ExtensionToolExecutorRouter, ExtensionToolHandler},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtensionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionManifestSchema {
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifest {
    pub schema: ExtensionManifestSchema,
    pub id: ExtensionId,
    pub version: String,
    pub main: ExtensionMain,
    pub activation: ExtensionActivation,
    pub contributes: ExtensionContributions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionMain {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionActivation {
    pub events: Vec<ExtensionActivationEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionActivationEvent {
    Command(String),
    PostFirstPaint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionContributions {
    pub tools: Vec<ExtensionToolContribution>,
    pub static_context: Vec<ExtensionStaticContextContribution>,
    pub tool_replacement_bundles: Vec<ExtensionToolReplacementBundleContribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolContribution {
    pub name: String,
    pub description: String,
    pub risk: ExtensionToolRisk,
    pub provider_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionToolRisk {
    ReadsLocalMetadata,
    ReadsLocalContent,
    MutatesLocalState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionToolReplacementContract {
    Preserve,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolReplacementMember {
    pub builtin: String,
    pub tool: String,
    pub contract: ExtensionToolReplacementContract,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolReplacementBundleContribution {
    pub id: String,
    pub members: Vec<ExtensionToolReplacementMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionStaticContextContribution {
    pub id: String,
    pub title: String,
    pub source: ExtensionStaticContextSource,
    pub placement: ExtensionStaticContextPlacement,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionStaticContextSource {
    ExtensionFile { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionStaticContextPlacement {
    BackgroundContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionManifestError {
    Malformed,
    UnsupportedSchema,
    InvalidExtensionId,
    InvalidCommand,
    InvalidActivationEvent { event: String },
    InvalidToolName { name: String },
    ReservedToolName { name: String },
    UnsupportedToolRisk { risk: String },
    DuplicateToolName { name: String },
    UnsupportedStaticContextPlacement { placement: String },
    InvalidStaticContextId { id: String },
    InvalidStaticContextPath { path: String },
    InvalidReplacementBundleId { id: String },
    DuplicateReplacementBundleId { id: String },
    InvalidReplacementBundle { id: String },
    InvalidReplacementContract { contract: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolCandidate {
    pub extension_id: ExtensionId,
    pub extension_version: String,
    pub tool: ExtensionToolContribution,
}

impl ExtensionToolCandidate {
    #[must_use]
    pub fn to_native_definition(&self) -> ToolDefinition {
        ToolDefinition::extension_tool_with_version(
            self.extension_id.0.clone(),
            Some(self.extension_version.clone()),
            self.tool.name.clone(),
            self.tool.description.clone(),
            ToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            self.tool.risk.into(),
            if self.tool.provider_visible {
                ProviderToolVisibility::Visible
            } else {
                ProviderToolVisibility::Hidden
            },
        )
    }
}
impl From<ExtensionToolRisk> for ToolRisk {
    fn from(risk: ExtensionToolRisk) -> Self {
        match risk {
            ExtensionToolRisk::ReadsLocalMetadata => Self::ReadsLocalMetadata,
            ExtensionToolRisk::ReadsLocalContent => Self::ReadsLocalContent,
            ExtensionToolRisk::MutatesLocalState => Self::MutatesLocalState,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionCatalog {
    extensions: Vec<ExtensionManifest>,
    tool_candidates: BTreeMap<String, ExtensionToolCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionCatalogError {
    DuplicateExtensionId { id: ExtensionId },
    DuplicateToolName { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionHostProtocolError {
    Malformed,
    MissingReady,
    UnsupportedProtocol,
    ExtensionIdMismatch,
    RequestIdMismatch,
    UnsupportedRisk,
    UnsupportedSchema,
    SpawnFailed,
    HostExited { status: Option<i32> },
    TimedOut,
    OutputTooLarge { max_bytes: usize },
    ToolRegistration(ToolRegistrationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHostCommand {
    pub command: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionResourceRequest {
    ReadTextFile { path: String, max_bytes: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExtensionResourceResult {
    Completed {
        path: String,
        text: String,
        sha256: String,
    },
    Failed {
        reason: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtensionEditProposalOperation {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
        after_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionEditProposal {
    pub summary: String,
    pub operations: Vec<ExtensionEditProposalOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionToolResultStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionHostInvocation {
    ToolResult {
        content: String,
        status: ExtensionToolResultStatus,
        reason: Option<String>,
    },
    EditProposal(ExtensionEditProposal),
}

pub trait ExtensionResourceBroker {
    fn execute(&self, request: &ExtensionResourceRequest) -> ExtensionResourceResult;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DenyExtensionResources;

impl ExtensionResourceBroker for DenyExtensionResources {
    fn execute(&self, _request: &ExtensionResourceRequest) -> ExtensionResourceResult {
        ExtensionResourceResult::Failed {
            reason: String::from("resource_access_unavailable"),
            message: String::from("resource access is unavailable for this extension invocation"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtensionHostClientMessage {
    #[serde(rename = "extension.initialize")]
    Initialize {
        protocol: String,
        extension_id: String,
    },
    #[serde(rename = "tool.invoke")]
    ToolInvoke {
        request_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    #[serde(rename = "resource.result")]
    ResourceResult {
        request_id: String,
        result: ExtensionResourceResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionHostServerMessage {
    Ready {
        protocol: String,
        extension_id: String,
    },
    ToolRegister {
        name: String,
        description: String,
        risk: ExtensionToolRisk,
        provider_visible: bool,
        input_schema: ToolInputSchema,
    },
    ResourceRequest {
        request_id: String,
        operation: ExtensionResourceRequest,
    },
    EditProposal {
        request_id: String,
        proposal: ExtensionEditProposal,
    },
    ToolResult {
        request_id: String,
        content: String,
        status: ExtensionToolResultStatus,
        reason: Option<String>,
    },
}

pub trait ExtensionHostTransport {
    fn send(
        &mut self,
        message: ExtensionHostClientMessage,
    ) -> Result<(), ExtensionHostProtocolError>;

    fn recv(
        &mut self,
        timeout: Duration,
    ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError>;
}

pub trait ExtensionHostInvoker: Send {
    fn invoke(
        &mut self,
        request_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
        resources: &dyn ExtensionResourceBroker,
    ) -> Result<ExtensionHostInvocation, ExtensionHostProtocolError>;
}

#[derive(Debug)]
pub struct ExtensionHostSession<Transport> {
    extension_id: String,
    transport: Transport,
    max_result_bytes: usize,
}

impl<Transport> ExtensionHostSession<Transport> {
    pub fn new(
        extension_id: impl Into<String>,
        transport: Transport,
        max_result_bytes: usize,
    ) -> Self {
        Self {
            extension_id: extension_id.into(),
            transport,
            max_result_bytes,
        }
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub fn into_transport(self) -> Transport {
        self.transport
    }
}

#[derive(Debug)]
pub struct ExtensionProcessHostTransport {
    child: Child,
    stdin: ChildStdin,
    stdout_rx: Receiver<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>,
    stdout_reader: Option<JoinHandle<()>>,
}

impl ExtensionProcessHostTransport {
    pub fn spawn(
        main: &ExtensionMain,
        package_root: &Path,
        max_stdout_line_bytes: usize,
    ) -> Result<Self, ExtensionHostProtocolError> {
        let mut process = Command::new(&main.command);
        process
            .args(&main.args)
            .current_dir(package_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_extension_host_process(&mut process);

        let mut child = process
            .spawn()
            .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?;
        let stdin = child
            .stdin
            .take()
            .ok_or(ExtensionHostProtocolError::Malformed)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(ExtensionHostProtocolError::Malformed)?;
        let (stdout_tx, stdout_rx) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            read_extension_host_stdout_jsonl_lines(stdout, max_stdout_line_bytes, &stdout_tx);
        });

        Ok(Self {
            child,
            stdin,
            stdout_rx,
            stdout_reader: Some(stdout_reader),
        })
    }

    fn recv_message(
        &mut self,
        timeout: Duration,
    ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
        match self.stdout_rx.recv_timeout(timeout) {
            Ok(message) => message,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = self
                    .child
                    .try_wait()
                    .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?
                {
                    return Err(ExtensionHostProtocolError::HostExited {
                        status: status.code(),
                    });
                }
                Err(ExtensionHostProtocolError::TimedOut)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(status) = self
                    .child
                    .try_wait()
                    .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?
                {
                    return Err(ExtensionHostProtocolError::HostExited {
                        status: status.code(),
                    });
                }
                Err(ExtensionHostProtocolError::Malformed)
            }
        }
    }
}

impl ExtensionHostTransport for ExtensionProcessHostTransport {
    fn send(
        &mut self,
        message: ExtensionHostClientMessage,
    ) -> Result<(), ExtensionHostProtocolError> {
        let mut line =
            serde_json::to_vec(&message).map_err(|_| ExtensionHostProtocolError::Malformed)?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.flush())
            .map_err(|_| ExtensionHostProtocolError::SpawnFailed)
    }

    fn recv(
        &mut self,
        timeout: Duration,
    ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
        self.recv_message(timeout)
    }
}

impl Drop for ExtensionProcessHostTransport {
    fn drop(&mut self) {
        terminate_extension_host_process_tree(&mut self.child);
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionInstallScope {
    User,
    Project,
    Ephemeral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPackageRoot {
    pub root: PathBuf,
    pub scope: ExtensionInstallScope,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPackageRecord {
    pub manifest: ExtensionManifest,
    pub scope: ExtensionInstallScope,
    pub package_root: PathBuf,
    pub manifest_path: PathBuf,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionActivationState {
    Installed,
    Discovered,
    Blocked,
    Starting,
    Registering,
    Active,
    Failed,
    Stopping,
    Stopped,
    ReloadRequested,
}

impl ExtensionActivationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Discovered => "discovered",
            Self::Blocked => "blocked",
            Self::Starting => "starting",
            Self::Registering => "registering",
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::ReloadRequested => "reload_requested",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionActivationErrorKind {
    Disabled,
    ProjectTrustRequired,
    MissingPackageRoot,
    MissingManifest,
    InvalidManifest,
    HostStartFailed,
    HostTimedOut,
    ProtocolError,
    PolicyBlocked,
}

impl ExtensionActivationErrorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ProjectTrustRequired => "project_trust_required",
            Self::MissingPackageRoot => "missing_package_root",
            Self::MissingManifest => "missing_manifest",
            Self::InvalidManifest => "invalid_manifest",
            Self::HostStartFailed => "host_start_failed",
            Self::HostTimedOut => "host_timed_out",
            Self::ProtocolError => "protocol_error",
            Self::PolicyBlocked => "policy_blocked",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivatedToolReplacementBundle {
    pub extension_id: String,
    pub extension_version: String,
    pub bundle_id: String,
    pub source: ToolReplacementSource,
    pub members: Vec<ExtensionToolReplacementMember>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReplacementBundleDiagnostic {
    pub extension_id: String,
    pub bundle_id: String,
    pub member: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionActivationLifecycleError {
    NotFound { selector: String },
    NotActive { selector: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionActivationDiagnostic {
    pub extension_id: Option<String>,
    pub version: Option<String>,
    pub scope: ExtensionInstallScope,
    pub source_ref: Option<String>,
    pub install_source: Option<String>,
    pub package_root: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub activation_state: ExtensionActivationState,
    pub generation: u64,
    pub last_error_kind: Option<ExtensionActivationErrorKind>,
    pub last_error_summary: Option<String>,
    pub registered_tools: Vec<String>,
    pub provider_visible_tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExtensionActivationSnapshot {
    pub registry: ToolRegistry,
    pub executor: ExtensionToolExecutorRouter,
    pub diagnostics: Vec<ExtensionActivationDiagnostic>,
    pub replacement_bundles: Vec<ActivatedToolReplacementBundle>,
    pub host_start_count: usize,
}

impl Default for ExtensionActivationSnapshot {
    fn default() -> Self {
        Self {
            registry: ToolRegistry::with_project_read_only_and_agent_edit_tools(),
            executor: ExtensionToolExecutorRouter::default(),
            diagnostics: Vec::new(),
            replacement_bundles: Vec::new(),
            host_start_count: 0,
        }
    }
}

impl ExtensionActivationSnapshot {
    #[must_use]
    pub fn active_tool_names(&self) -> Vec<&str> {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.activation_state == ExtensionActivationState::Active)
            .flat_map(|diagnostic| diagnostic.registered_tools.iter().map(String::as_str))
            .collect()
    }
    #[must_use]
    pub fn resolve_provider_turn_catalog<'a>(
        &self,
        permission_policy: &ToolPermissionPolicy,
        executable_tools: impl IntoIterator<Item = &'a str>,
    ) -> (ResolvedToolCatalog, Vec<ToolReplacementBundleDiagnostic>) {
        let executable_tools = executable_tools
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let mut resolved_catalog = self.registry.resolve_provider_turn_catalog(
            permission_policy,
            executable_tools.iter().map(String::as_str),
        );
        let mut diagnostics = Vec::new();
        let mut accepted_rules = Vec::new();
        let mut builtin_counts = BTreeMap::<&str, usize>::new();
        for bundle in &self.replacement_bundles {
            for member in &bundle.members {
                *builtin_counts.entry(member.builtin.as_str()).or_default() += 1;
            }
        }

        for bundle in &self.replacement_bundles {
            let conflicting_members = bundle
                .members
                .iter()
                .filter(|member| builtin_counts.get(member.builtin.as_str()).copied() != Some(1))
                .collect::<Vec<_>>();
            if !conflicting_members.is_empty() {
                diagnostics.extend(conflicting_members.into_iter().map(|member| {
                    ToolReplacementBundleDiagnostic {
                        extension_id: bundle.extension_id.clone(),
                        bundle_id: bundle.bundle_id.clone(),
                        member: Some(member.builtin.clone()),
                        summary: String::from("replacement target claimed by multiple bundles"),
                    }
                }));
                continue;
            }
            if let Some(member) = bundle
                .members
                .iter()
                .find(|member| !executable_tools.contains(&member.tool))
            {
                diagnostics.push(ToolReplacementBundleDiagnostic {
                    extension_id: bundle.extension_id.clone(),
                    bundle_id: bundle.bundle_id.clone(),
                    member: Some(member.tool.clone()),
                    summary: String::from("replacement implementation is not executable"),
                });
                continue;
            }

            let bundle_rules = bundle.members.iter().map(|member| ToolReplacementRule {
                builtin_name: member.builtin.clone(),
                extension_id: bundle.extension_id.clone(),
                extension_tool: member.tool.clone(),
                mode: match member.contract {
                    ExtensionToolReplacementContract::Preserve => {
                        ToolResolutionMode::ReplaceBuiltin
                    }
                    ExtensionToolReplacementContract::Replace => {
                        ToolResolutionMode::ReplaceBuiltinWithExtensionContract
                    }
                },
                source: bundle.source.clone(),
            });
            let mut candidate_rules = accepted_rules.clone();
            candidate_rules.extend(bundle_rules);
            let candidate_policy = ToolReplacementPolicy::from_rules(candidate_rules.clone());
            match self
                .registry
                .resolve_provider_turn_catalog_with_replacements(
                    permission_policy,
                    executable_tools.iter().map(String::as_str),
                    &candidate_policy,
                ) {
                Ok(candidate_catalog) => {
                    if let Some(member) = bundle.members.iter().find(|member| {
                        candidate_catalog
                            .resolved_tool(&member.builtin)
                            .is_none_or(|tool| tool.implementation_name != member.tool)
                    }) {
                        diagnostics.push(ToolReplacementBundleDiagnostic {
                            extension_id: bundle.extension_id.clone(),
                            bundle_id: bundle.bundle_id.clone(),
                            member: Some(member.tool.clone()),
                            summary: String::from(
                                "replacement implementation is not provider-routable",
                            ),
                        });
                        continue;
                    }
                    accepted_rules = candidate_rules;
                    resolved_catalog = candidate_catalog;
                }
                Err(error) => diagnostics.push(ToolReplacementBundleDiagnostic {
                    extension_id: bundle.extension_id.clone(),
                    bundle_id: bundle.bundle_id.clone(),
                    member: Some(replacement_error_member(&error)),
                    summary: format!("{error:?}"),
                }),
            }
        }
        (resolved_catalog, diagnostics)
    }

    pub fn stop_extension(
        &mut self,
        selector: &str,
    ) -> Result<ExtensionActivationDiagnostic, ExtensionActivationLifecycleError> {
        let Some(index) = self
            .diagnostics
            .iter()
            .position(|diagnostic| diagnostic.matches_selector(selector))
        else {
            return Err(ExtensionActivationLifecycleError::NotFound {
                selector: selector.to_string(),
            });
        };

        let diagnostic = &mut self.diagnostics[index];
        if diagnostic.activation_state != ExtensionActivationState::Active {
            return Err(ExtensionActivationLifecycleError::NotActive {
                selector: selector.to_string(),
            });
        }
        let Some(extension_id) = diagnostic.extension_id.clone() else {
            return Err(ExtensionActivationLifecycleError::NotActive {
                selector: selector.to_string(),
            });
        };

        diagnostic.activation_state = ExtensionActivationState::Stopping;
        let registered_tools = diagnostic.registered_tools.clone();
        let removed_tools = self.registry.remove_extension_tools(&extension_id);
        self.executor.remove_tools(
            registered_tools
                .iter()
                .chain(removed_tools.iter())
                .map(String::as_str),
        );
        self.replacement_bundles
            .retain(|bundle| bundle.extension_id != extension_id);
        diagnostic.mark_stopped();
        Ok(diagnostic.clone())
    }

    pub fn reload_extension_from_record(
        &mut self,
        record: &ExtensionPackageRecord,
        config: ExtensionBackgroundActivationConfig,
    ) -> ExtensionActivationDiagnostic {
        let extension_id = &record.manifest.id.0;
        let next_generation = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.matches_package_record(record))
            .map(|diagnostic| diagnostic.generation)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let retired_tools = self
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.matches_package_record(record))
            .flat_map(|diagnostic| diagnostic.registered_tools.clone())
            .collect::<Vec<_>>();
        let removed_tools = self.registry.remove_extension_tools(extension_id);
        self.executor.remove_tools(
            retired_tools
                .iter()
                .chain(removed_tools.iter())
                .map(String::as_str),
        );
        self.replacement_bundles
            .retain(|bundle| bundle.extension_id != *extension_id);
        self.diagnostics
            .retain(|diagnostic| !diagnostic.matches_package_record(record));

        let mut diagnostic = ExtensionActivationDiagnostic::from_package_record(record, None);
        diagnostic.generation = next_generation;

        if record.scope == ExtensionInstallScope::Project {
            diagnostic.mark_blocked(
                ExtensionActivationErrorKind::ProjectTrustRequired,
                "project extension activation requires project trust",
            );
            self.diagnostics.push(diagnostic.clone());
            return diagnostic;
        }
        if !record
            .manifest
            .activation
            .events
            .contains(&ExtensionActivationEvent::PostFirstPaint)
        {
            self.diagnostics.push(diagnostic.clone());
            return diagnostic;
        }
        if record.manifest.contributes.tools.is_empty() {
            self.diagnostics.push(diagnostic.clone());
            return diagnostic;
        }

        self.host_start_count = self.host_start_count.saturating_add(1);
        let mut registry = self.registry.clone();
        match activate_extension_host_record(record, &mut registry, config) {
            Ok((session, registered_tools)) => {
                let shared_invoker: Arc<Mutex<Box<dyn ExtensionHostInvoker>>> =
                    Arc::new(Mutex::new(Box::new(session)));
                for tool_name in &registered_tools {
                    self.executor.insert_tool(
                        tool_name.clone(),
                        ExtensionToolHandler::shared_host_metadata(
                            extension_id.clone(),
                            shared_invoker.clone(),
                            config.invocation_timeout,
                        ),
                    );
                }
                diagnostic.mark_active_with_generation(
                    &registry,
                    registered_tools,
                    next_generation,
                );
                self.registry = registry;
                self.replacement_bundles
                    .extend(activated_replacement_bundles(record));
            }
            Err(error) => {
                let (kind, summary) = extension_host_activation_error(&error);
                diagnostic.mark_failed(kind, summary);
            }
        }

        self.diagnostics.push(diagnostic.clone());
        diagnostic
    }
}
fn activated_replacement_bundles(
    record: &ExtensionPackageRecord,
) -> impl Iterator<Item = ActivatedToolReplacementBundle> + '_ {
    let source = match record.scope {
        ExtensionInstallScope::User => ToolReplacementSource::User,
        ExtensionInstallScope::Project => ToolReplacementSource::Project { trusted: false },
        ExtensionInstallScope::Ephemeral => ToolReplacementSource::Ephemeral,
    };
    record
        .manifest
        .contributes
        .tool_replacement_bundles
        .iter()
        .map(move |bundle| ActivatedToolReplacementBundle {
            extension_id: record.manifest.id.0.clone(),
            extension_version: record.manifest.version.clone(),
            bundle_id: bundle.id.clone(),
            source: source.clone(),
            members: bundle.members.clone(),
        })
}
fn replacement_error_member(error: &ToolResolutionError) -> String {
    match error {
        ToolResolutionError::MissingBuiltIn { name }
        | ToolResolutionError::MissingExtensionTool { name } => name.clone(),
        ToolResolutionError::ExtensionIdMismatch { expected, .. } => expected.clone(),
        ToolResolutionError::ReplacementLowersRisk { extension_tool, .. }
        | ToolResolutionError::ReplacementSchemaMismatch { extension_tool, .. } => {
            extension_tool.clone()
        }
        ToolResolutionError::UntrustedProjectReplacement { builtin_name } => builtin_name.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtensionBackgroundActivationConfig {
    pub registration_timeout: Duration,
    pub invocation_timeout: Duration,
    pub max_stdout_line_bytes: usize,
    pub max_result_bytes: usize,
}

impl ExtensionBackgroundActivationConfig {
    #[must_use]
    pub const fn conservative() -> Self {
        Self {
            registration_timeout: Duration::from_millis(750),
            invocation_timeout: Duration::from_secs(5),
            max_stdout_line_bytes: 64 * 1024,
            max_result_bytes: 64 * 1024,
        }
    }
}

impl ExtensionActivationDiagnostic {
    #[must_use]
    pub fn from_package_record(
        record: &ExtensionPackageRecord,
        install: Option<&ExtensionInstallRecord>,
    ) -> Self {
        let (activation_state, last_error_kind, last_error_summary) =
            if install.is_some_and(|record| !record.enabled) {
                (
                    ExtensionActivationState::Blocked,
                    Some(ExtensionActivationErrorKind::Disabled),
                    Some(String::from("install record is disabled")),
                )
            } else {
                (ExtensionActivationState::Discovered, None, None)
            };
        Self {
            extension_id: Some(record.manifest.id.0.clone()),
            version: Some(record.manifest.version.clone()),
            scope: record.scope,
            source_ref: record.source_ref.clone(),
            install_source: install.map(|record| record.source.clone()),
            package_root: record.package_root.clone(),
            manifest_path: Some(record.manifest_path.clone()),
            activation_state,
            generation: 0,
            last_error_kind,
            last_error_summary,
            registered_tools: Vec::new(),
            provider_visible_tools: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_install_record(install: &ExtensionInstallRecord) -> Self {
        let (activation_state, last_error_kind, last_error_summary) = if install.enabled {
            (ExtensionActivationState::Installed, None, None)
        } else {
            (
                ExtensionActivationState::Blocked,
                Some(ExtensionActivationErrorKind::Disabled),
                Some(String::from("install record is disabled")),
            )
        };

        Self {
            extension_id: None,
            version: None,
            scope: install.scope,
            source_ref: None,
            install_source: Some(install.source.clone()),
            package_root: install.package_root.clone(),
            manifest_path: None,
            activation_state,
            generation: 0,
            last_error_kind,
            last_error_summary,
            registered_tools: Vec::new(),
            provider_visible_tools: Vec::new(),
        }
    }

    #[must_use]
    pub fn registered_tool_count(&self) -> usize {
        self.registered_tools.len()
    }

    fn mark_blocked(
        &mut self,
        error_kind: ExtensionActivationErrorKind,
        summary: impl Into<String>,
    ) {
        self.activation_state = ExtensionActivationState::Blocked;
        self.last_error_kind = Some(error_kind);
        self.last_error_summary = Some(summary.into());
    }

    fn mark_failed(
        &mut self,
        error_kind: ExtensionActivationErrorKind,
        summary: impl Into<String>,
    ) {
        self.activation_state = ExtensionActivationState::Failed;
        self.last_error_kind = Some(error_kind);
        self.last_error_summary = Some(summary.into());
    }

    fn mark_active(&mut self, registry: &ToolRegistry, registered_tools: Vec<String>) {
        self.mark_active_with_generation(registry, registered_tools, 1);
    }

    fn mark_active_with_generation(
        &mut self,
        registry: &ToolRegistry,
        registered_tools: Vec<String>,
        generation: u64,
    ) {
        self.activation_state = ExtensionActivationState::Active;
        self.generation = generation.max(1);
        self.last_error_kind = None;
        self.last_error_summary = None;
        self.provider_visible_tools = registered_tools
            .iter()
            .filter(|tool_name| {
                registry.get(tool_name).is_some_and(|definition| {
                    definition.provider_visibility == ProviderToolVisibility::Visible
                })
            })
            .cloned()
            .collect();
        self.registered_tools = registered_tools;
    }

    fn mark_stopped(&mut self) {
        self.activation_state = ExtensionActivationState::Stopped;
        self.generation = self.generation.saturating_add(1);
        self.last_error_kind = None;
        self.last_error_summary = None;
        self.registered_tools.clear();
        self.provider_visible_tools.clear();
    }

    fn matches_selector(&self, selector: &str) -> bool {
        self.extension_id.as_deref() == Some(selector)
            || self.source_ref.as_deref() == Some(selector)
            || self.install_source.as_deref() == Some(selector)
            || self.package_root == Path::new(selector)
            || self.package_root.to_string_lossy() == selector
    }

    fn matches_package_record(&self, record: &ExtensionPackageRecord) -> bool {
        self.extension_id.as_deref() == Some(record.manifest.id.0.as_str())
            || self.package_root == record.package_root
            || self.manifest_path.as_ref() == Some(&record.manifest_path)
            || record
                .source_ref
                .as_deref()
                .is_some_and(|source_ref| self.source_ref.as_deref() == Some(source_ref))
    }
}

pub fn activate_background_metadata_extensions(
    package_records: &[ExtensionPackageRecord],
    config: ExtensionBackgroundActivationConfig,
) -> ExtensionActivationSnapshot {
    let mut snapshot = ExtensionActivationSnapshot::default();
    let mut handlers = BTreeMap::new();

    for record in package_records {
        let mut diagnostic = ExtensionActivationDiagnostic::from_package_record(record, None);
        if record.scope == ExtensionInstallScope::Project {
            diagnostic.mark_blocked(
                ExtensionActivationErrorKind::ProjectTrustRequired,
                "project extension activation requires project trust",
            );
            snapshot.diagnostics.push(diagnostic);
            continue;
        }
        if !record
            .manifest
            .activation
            .events
            .contains(&ExtensionActivationEvent::PostFirstPaint)
        {
            snapshot.diagnostics.push(diagnostic);
            continue;
        }
        if record.manifest.contributes.tools.is_empty() {
            snapshot.diagnostics.push(diagnostic);
            continue;
        }

        snapshot.host_start_count = snapshot.host_start_count.saturating_add(1);
        let mut registry = snapshot.registry.clone();
        let activation = activate_extension_host_record(record, &mut registry, config);
        match activation {
            Ok((session, registered_tools)) => {
                let shared_invoker: Arc<Mutex<Box<dyn ExtensionHostInvoker>>> =
                    Arc::new(Mutex::new(Box::new(session)));
                for tool_name in &registered_tools {
                    handlers.insert(
                        tool_name.clone(),
                        ExtensionToolHandler::shared_host_metadata(
                            record.manifest.id.0.clone(),
                            shared_invoker.clone(),
                            config.invocation_timeout,
                        ),
                    );
                }
                diagnostic.mark_active(&registry, registered_tools);
                snapshot.registry = registry;
                snapshot
                    .replacement_bundles
                    .extend(activated_replacement_bundles(record));
            }
            Err(error) => {
                let (kind, summary) = extension_host_activation_error(&error);
                diagnostic.mark_failed(kind, summary);
            }
        }
        snapshot.diagnostics.push(diagnostic);
    }

    snapshot.executor = ExtensionToolExecutorRouter::from_handlers(handlers);
    snapshot
}

fn activate_extension_host_record(
    record: &ExtensionPackageRecord,
    registry: &mut ToolRegistry,
    config: ExtensionBackgroundActivationConfig,
) -> Result<
    (
        ExtensionHostSession<ExtensionProcessHostTransport>,
        Vec<String>,
    ),
    ExtensionHostProtocolError,
> {
    let transport = ExtensionProcessHostTransport::spawn(
        &record.manifest.main,
        &record.package_root,
        config.max_stdout_line_bytes,
    )?;
    let mut session = ExtensionHostSession::new(
        record.manifest.id.0.clone(),
        transport,
        config.max_result_bytes,
    );
    let expected_tool_count = record.manifest.contributes.tools.len();
    let registered_tools = session.initialize_and_register(
        registry,
        Some(&record.manifest.version),
        expected_tool_count,
        config.registration_timeout,
    )?;
    Ok((session, registered_tools))
}

fn extension_host_activation_error(
    error: &ExtensionHostProtocolError,
) -> (ExtensionActivationErrorKind, String) {
    let kind = match error {
        ExtensionHostProtocolError::SpawnFailed | ExtensionHostProtocolError::HostExited { .. } => {
            ExtensionActivationErrorKind::HostStartFailed
        }
        ExtensionHostProtocolError::TimedOut
        | ExtensionHostProtocolError::OutputTooLarge { .. } => {
            ExtensionActivationErrorKind::HostTimedOut
        }
        ExtensionHostProtocolError::ToolRegistration(_) => {
            ExtensionActivationErrorKind::PolicyBlocked
        }
        ExtensionHostProtocolError::Malformed
        | ExtensionHostProtocolError::MissingReady
        | ExtensionHostProtocolError::UnsupportedProtocol
        | ExtensionHostProtocolError::ExtensionIdMismatch
        | ExtensionHostProtocolError::RequestIdMismatch
        | ExtensionHostProtocolError::UnsupportedRisk
        | ExtensionHostProtocolError::UnsupportedSchema => {
            ExtensionActivationErrorKind::ProtocolError
        }
    };
    (kind, extension_host_protocol_error_label(error).to_owned())
}

fn extension_host_protocol_error_label(error: &ExtensionHostProtocolError) -> &'static str {
    match error {
        ExtensionHostProtocolError::Malformed => "malformed",
        ExtensionHostProtocolError::MissingReady => "missing_ready",
        ExtensionHostProtocolError::UnsupportedProtocol => "unsupported_protocol",
        ExtensionHostProtocolError::ExtensionIdMismatch => "extension_id_mismatch",
        ExtensionHostProtocolError::RequestIdMismatch => "request_id_mismatch",
        ExtensionHostProtocolError::UnsupportedRisk => "unsupported_risk",
        ExtensionHostProtocolError::UnsupportedSchema => "unsupported_schema",
        ExtensionHostProtocolError::SpawnFailed => "spawn_failed",
        ExtensionHostProtocolError::HostExited { .. } => "host_exited",
        ExtensionHostProtocolError::TimedOut => "timed_out",
        ExtensionHostProtocolError::OutputTooLarge { .. } => "output_too_large",
        ExtensionHostProtocolError::ToolRegistration(_) => "tool_registration",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifestIndex {
    records: Vec<ExtensionPackageRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionPackageIndexError {
    MissingPackageRoot {
        root: PathBuf,
    },
    MissingManifest {
        root: PathBuf,
    },
    MissingManifestFile {
        path: PathBuf,
    },
    MalformedPackageJson {
        path: PathBuf,
    },
    InvalidManifestPointer {
        path: PathBuf,
        pointer: String,
    },
    ManifestPathEscapedPackageRoot {
        root: PathBuf,
        path: PathBuf,
    },
    Manifest {
        path: PathBuf,
        error: ExtensionManifestError,
    },
    Catalog(ExtensionCatalogError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifestIndexCache {
    pub records: Vec<ExtensionManifestIndexCacheRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifestIndexCacheRecord {
    pub extension_id: String,
    pub version: String,
    pub scope: ExtensionInstallScope,
    pub package_root: PathBuf,
    pub manifest_path: PathBuf,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionManifestIndexCacheRecordStatus {
    pub extension_id: String,
    pub manifest_path: PathBuf,
    pub state: ExtensionManifestIndexCacheRecordState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionManifestIndexCacheRecordState {
    Present,
    StaleMissingManifest,
    StaleEscapedPackageRoot,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionManifest {
    schema: String,
    id: String,
    version: String,
    main: RawExtensionMain,
    #[serde(default)]
    activation: RawExtensionActivation,
    #[serde(default)]
    contributes: RawExtensionContributions,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionMain {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionActivation {
    #[serde(default)]
    events: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionContributions {
    #[serde(default)]
    tools: Vec<RawExtensionToolContribution>,
    #[serde(default)]
    static_context: Vec<RawExtensionStaticContextContribution>,
    #[serde(default)]
    tool_replacement_bundles: Vec<RawExtensionToolReplacementBundle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionToolContribution {
    name: String,
    description: String,
    risk: String,
    provider_visible: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionToolReplacementBundle {
    id: String,
    members: Vec<RawExtensionToolReplacementMember>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionToolReplacementMember {
    builtin: String,
    tool: String,
    contract: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionStaticContextContribution {
    id: String,
    title: String,
    source: RawExtensionStaticContextSource,
    placement: Option<String>,
    max_bytes: u64,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum RawExtensionStaticContextSource {
    #[serde(rename = "extension_file")]
    ExtensionFile { path: String },
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum RawExtensionHostMessage {
    #[serde(rename = "extension.ready")]
    Ready {
        protocol: String,
        extension_id: String,
    },
    #[serde(rename = "tool.register")]
    ToolRegister {
        name: String,
        description: String,
        risk: String,
        provider_visible: bool,
        input_schema: RawExtensionToolInputSchema,
    },
    #[serde(rename = "resource.request")]
    ResourceRequest {
        request_id: String,
        operation: ExtensionResourceRequest,
    },
    #[serde(rename = "tool.edit_proposal")]
    EditProposal {
        request_id: String,
        summary: String,
        operations: Vec<ExtensionEditProposalOperation>,
    },
    #[serde(rename = "tool.result")]
    ToolResult {
        request_id: String,
        content: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionToolInputSchema {
    #[serde(rename = "type")]
    schema_type: String,
    #[serde(rename = "additionalProperties")]
    additional_properties: bool,
    required: Vec<String>,
    properties: BTreeMap<String, RawExtensionToolInputProperty>,
    #[serde(rename = "maxSerializedBytes")]
    max_serialized_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionToolInputProperty {
    #[serde(rename = "type")]
    schema_type: String,
}

#[derive(Deserialize)]
struct RawExtensionPackageJson {
    yach: Option<RawExtensionPackageJsonYach>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtensionPackageJsonYach {
    manifests: Vec<String>,
}

pub fn parse_extension_manifest(
    value: serde_json::Value,
) -> Result<ExtensionManifest, ExtensionManifestError> {
    let raw: RawExtensionManifest =
        serde_json::from_value(value).map_err(|_| ExtensionManifestError::Malformed)?;

    if raw.schema != "yach.extension.v1" {
        return Err(ExtensionManifestError::UnsupportedSchema);
    }

    if !is_valid_extension_id(&raw.id) {
        return Err(ExtensionManifestError::InvalidExtensionId);
    }

    if !is_valid_process_command(&raw.main.command) {
        return Err(ExtensionManifestError::InvalidCommand);
    }

    let activation_events = raw
        .activation
        .events
        .into_iter()
        .map(parse_activation_event)
        .collect::<Result<Vec<_>, _>>()?;

    let mut seen_tool_names = BTreeSet::new();
    let mut tools = Vec::with_capacity(raw.contributes.tools.len());
    for raw_tool in raw.contributes.tools {
        let name = raw_tool.name;
        validate_tool_name(&name)?;
        if !seen_tool_names.insert(name.clone()) {
            return Err(ExtensionManifestError::DuplicateToolName { name });
        }

        tools.push(ExtensionToolContribution {
            name,
            description: raw_tool.description,
            risk: parse_tool_risk(raw_tool.risk)?,
            provider_visible: raw_tool.provider_visible,
        });
    }
    let mut seen_bundle_ids = BTreeSet::new();
    let mut tool_replacement_bundles =
        Vec::with_capacity(raw.contributes.tool_replacement_bundles.len());
    for raw_bundle in raw.contributes.tool_replacement_bundles {
        if !is_valid_tool_name(&raw_bundle.id) {
            return Err(ExtensionManifestError::InvalidReplacementBundleId { id: raw_bundle.id });
        }
        if !seen_bundle_ids.insert(raw_bundle.id.clone()) {
            return Err(ExtensionManifestError::DuplicateReplacementBundleId { id: raw_bundle.id });
        }
        if raw_bundle.members.len() < 2 {
            return Err(ExtensionManifestError::InvalidReplacementBundle { id: raw_bundle.id });
        }
        let mut builtins = BTreeSet::new();
        let mut implementation_tools = BTreeSet::new();
        let mut members = Vec::with_capacity(raw_bundle.members.len());
        for raw_member in raw_bundle.members {
            if !is_valid_tool_name(&raw_member.builtin)
                || !seen_tool_names.contains(&raw_member.tool)
                || !builtins.insert(raw_member.builtin.clone())
                || !implementation_tools.insert(raw_member.tool.clone())
            {
                return Err(ExtensionManifestError::InvalidReplacementBundle { id: raw_bundle.id });
            }
            let contract = match raw_member.contract.as_str() {
                "preserve" => ExtensionToolReplacementContract::Preserve,
                "replace" => ExtensionToolReplacementContract::Replace,
                _ => {
                    return Err(ExtensionManifestError::InvalidReplacementContract {
                        contract: raw_member.contract,
                    });
                }
            };
            members.push(ExtensionToolReplacementMember {
                builtin: raw_member.builtin,
                tool: raw_member.tool,
                contract,
            });
        }
        tool_replacement_bundles.push(ExtensionToolReplacementBundleContribution {
            id: raw_bundle.id,
            members,
        });
    }

    let static_context = raw
        .contributes
        .static_context
        .into_iter()
        .map(parse_static_context_contribution)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ExtensionManifest {
        schema: ExtensionManifestSchema::V1,
        id: ExtensionId(raw.id),
        version: raw.version,
        main: ExtensionMain {
            command: raw.main.command,
            args: raw.main.args,
        },
        activation: ExtensionActivation {
            events: activation_events,
        },
        contributes: ExtensionContributions {
            tools,
            static_context,
            tool_replacement_bundles,
        },
    })
}

pub fn parse_extension_host_server_message(
    value: serde_json::Value,
) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
    let message =
        serde_json::from_value(value).map_err(|_| ExtensionHostProtocolError::Malformed)?;
    match message {
        RawExtensionHostMessage::Ready {
            protocol,
            extension_id,
        } => Ok(ExtensionHostServerMessage::Ready {
            protocol,
            extension_id,
        }),
        RawExtensionHostMessage::ToolRegister {
            name,
            description,
            risk,
            provider_visible,
            input_schema,
        } => {
            validate_tool_name(&name).map_err(|_| ExtensionHostProtocolError::Malformed)?;
            Ok(ExtensionHostServerMessage::ToolRegister {
                name,
                description,
                risk: parse_tool_risk(risk)
                    .map_err(|_| ExtensionHostProtocolError::UnsupportedRisk)?,
                provider_visible,
                input_schema: parse_extension_tool_input_schema(input_schema)?,
            })
        }
        RawExtensionHostMessage::ToolResult {
            request_id,
            content,
            status,
            reason,
        } => {
            let (status, reason) = match (status.as_deref(), reason) {
                (None | Some("completed"), None) => (ExtensionToolResultStatus::Completed, None),
                (Some("failed"), Some(reason)) if is_valid_tool_name(&reason) => {
                    (ExtensionToolResultStatus::Failed, Some(reason))
                }
                _ => return Err(ExtensionHostProtocolError::Malformed),
            };
            Ok(ExtensionHostServerMessage::ToolResult {
                request_id,
                content,
                status,
                reason,
            })
        }
        RawExtensionHostMessage::ResourceRequest {
            request_id,
            operation,
        } => Ok(ExtensionHostServerMessage::ResourceRequest {
            request_id,
            operation,
        }),
        RawExtensionHostMessage::EditProposal {
            request_id,
            summary,
            operations,
        } => Ok(ExtensionHostServerMessage::EditProposal {
            request_id,
            proposal: ExtensionEditProposal {
                summary,
                operations,
            },
        }),
    }
}

pub fn process_extension_registration_messages(
    expected_extension_id: &str,
    messages: Vec<serde_json::Value>,
    registry: &mut ToolRegistry,
) -> Result<Vec<String>, ExtensionHostProtocolError> {
    let mut ready = false;
    let mut registered_tools = Vec::new();
    let mut staged_definitions = Vec::new();

    for value in messages {
        let message = parse_extension_host_server_message(value)?;
        match message {
            ExtensionHostServerMessage::Ready {
                protocol,
                extension_id,
            } => {
                if protocol != "yach.extension-host.v2" {
                    return Err(ExtensionHostProtocolError::UnsupportedProtocol);
                }
                if extension_id != expected_extension_id {
                    return Err(ExtensionHostProtocolError::ExtensionIdMismatch);
                }
                ready = true;
            }
            ExtensionHostServerMessage::ToolRegister {
                name,
                description,
                risk,
                provider_visible,
                input_schema,
            } => {
                if !ready {
                    return Err(ExtensionHostProtocolError::MissingReady);
                }
                let definition = ToolDefinition::extension_tool_with_version(
                    expected_extension_id,
                    None::<String>,
                    name.clone(),
                    description,
                    input_schema,
                    risk.into(),
                    if provider_visible {
                        ProviderToolVisibility::Visible
                    } else {
                        ProviderToolVisibility::Hidden
                    },
                );
                staged_definitions.push(definition);
                registered_tools.push(name);
            }
            ExtensionHostServerMessage::ToolResult { .. }
            | ExtensionHostServerMessage::ResourceRequest { .. }
            | ExtensionHostServerMessage::EditProposal { .. } => {
                return Err(ExtensionHostProtocolError::Malformed);
            }
        }
    }

    if !ready {
        return Err(ExtensionHostProtocolError::MissingReady);
    }

    let mut staged_names = BTreeSet::new();
    for definition in &staged_definitions {
        if registry.get(&definition.name).is_some() || !staged_names.insert(&definition.name) {
            return Err(ExtensionHostProtocolError::ToolRegistration(
                ToolRegistrationError::DuplicateToolName {
                    name: definition.name.clone(),
                },
            ));
        }
    }

    for definition in staged_definitions {
        registry
            .register_extension_tool(definition)
            .map_err(ExtensionHostProtocolError::ToolRegistration)?;
    }

    Ok(registered_tools)
}

impl<Transport> ExtensionHostSession<Transport>
where
    Transport: ExtensionHostTransport,
{
    pub fn initialize_and_register(
        &mut self,
        registry: &mut ToolRegistry,
        extension_version: Option<&str>,
        expected_tool_count: usize,
        timeout: Duration,
    ) -> Result<Vec<String>, ExtensionHostProtocolError> {
        self.transport
            .send(ExtensionHostClientMessage::Initialize {
                protocol: String::from("yach.extension-host.v2"),
                extension_id: self.extension_id.clone(),
            })?;

        match self.transport.recv(timeout)? {
            ExtensionHostServerMessage::Ready {
                protocol,
                extension_id,
            } => {
                if protocol != "yach.extension-host.v2" {
                    return Err(ExtensionHostProtocolError::UnsupportedProtocol);
                }
                if extension_id != self.extension_id {
                    return Err(ExtensionHostProtocolError::ExtensionIdMismatch);
                }
            }
            ExtensionHostServerMessage::ToolRegister { .. }
            | ExtensionHostServerMessage::ToolResult { .. }
            | ExtensionHostServerMessage::ResourceRequest { .. }
            | ExtensionHostServerMessage::EditProposal { .. } => {
                return Err(ExtensionHostProtocolError::MissingReady);
            }
        }

        let mut registered_tools = Vec::new();
        for _ in 0..expected_tool_count {
            let ExtensionHostServerMessage::ToolRegister {
                name,
                description,
                risk,
                provider_visible,
                input_schema,
            } = self.transport.recv(timeout)?
            else {
                return Err(ExtensionHostProtocolError::Malformed);
            };
            registry
                .register_extension_tool(ToolDefinition::extension_tool_with_version(
                    &self.extension_id,
                    extension_version.map(String::from),
                    name.clone(),
                    description,
                    input_schema,
                    risk.into(),
                    if provider_visible {
                        ProviderToolVisibility::Visible
                    } else {
                        ProviderToolVisibility::Hidden
                    },
                ))
                .map_err(ExtensionHostProtocolError::ToolRegistration)?;
            registered_tools.push(name);
        }

        Ok(registered_tools)
    }

    pub fn invoke_tool(
        &mut self,
        request_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
        resources: &dyn ExtensionResourceBroker,
    ) -> Result<ExtensionHostInvocation, ExtensionHostProtocolError> {
        self.transport
            .send(ExtensionHostClientMessage::ToolInvoke {
                request_id: request_id.to_owned(),
                name: tool_name.to_owned(),
                arguments,
            })?;

        let started = Instant::now();
        let mut resource_request_ids = BTreeSet::new();
        for _ in 0..64 {
            let remaining = timeout
                .checked_sub(started.elapsed())
                .ok_or(ExtensionHostProtocolError::TimedOut)?;
            match self.transport.recv(remaining)? {
                ExtensionHostServerMessage::ToolResult {
                    request_id: result_request_id,
                    content,
                    status,
                    reason,
                } => {
                    if result_request_id != request_id {
                        return Err(ExtensionHostProtocolError::RequestIdMismatch);
                    }
                    if content.len() > self.max_result_bytes {
                        return Err(ExtensionHostProtocolError::OutputTooLarge {
                            max_bytes: self.max_result_bytes,
                        });
                    }
                    return Ok(ExtensionHostInvocation::ToolResult {
                        content,
                        status,
                        reason,
                    });
                }
                ExtensionHostServerMessage::EditProposal {
                    request_id: result_request_id,
                    proposal,
                } => {
                    if result_request_id != request_id {
                        return Err(ExtensionHostProtocolError::RequestIdMismatch);
                    }
                    let proposal_bytes = serde_json::to_vec(&proposal)
                        .map_err(|_| ExtensionHostProtocolError::Malformed)?
                        .len();
                    if proposal_bytes > self.max_result_bytes {
                        return Err(ExtensionHostProtocolError::OutputTooLarge {
                            max_bytes: self.max_result_bytes,
                        });
                    }
                    return Ok(ExtensionHostInvocation::EditProposal(proposal));
                }
                ExtensionHostServerMessage::ResourceRequest {
                    request_id: resource_request_id,
                    operation,
                } => {
                    if resource_request_id.is_empty()
                        || !resource_request_ids.insert(resource_request_id.clone())
                    {
                        return Err(ExtensionHostProtocolError::RequestIdMismatch);
                    }
                    let result = resources.execute(&operation);
                    self.transport
                        .send(ExtensionHostClientMessage::ResourceResult {
                            request_id: resource_request_id,
                            result,
                        })?;
                }
                ExtensionHostServerMessage::Ready { .. }
                | ExtensionHostServerMessage::ToolRegister { .. } => {
                    return Err(ExtensionHostProtocolError::Malformed);
                }
            }
        }
        Err(ExtensionHostProtocolError::Malformed)
    }
}

impl<Transport> ExtensionHostInvoker for ExtensionHostSession<Transport>
where
    Transport: ExtensionHostTransport + Send,
{
    fn invoke(
        &mut self,
        request_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
        timeout: Duration,
        resources: &dyn ExtensionResourceBroker,
    ) -> Result<ExtensionHostInvocation, ExtensionHostProtocolError> {
        self.invoke_tool(request_id, tool_name, arguments, timeout, resources)
    }
}

pub fn run_extension_host_registration_command(
    extension_id: &str,
    command: &ExtensionHostCommand,
    registry: &mut ToolRegistry,
) -> Result<Vec<String>, ExtensionHostProtocolError> {
    let max_stdout_bytes = command.max_stdout_bytes;
    let timeout = command.timeout;
    let mut process = Command::new(&command.command);
    process
        .args(&command.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_extension_host_process(&mut process);

    let mut child = process
        .spawn()
        .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?;

    let stdout = child
        .stdout
        .take()
        .ok_or(ExtensionHostProtocolError::Malformed)?;
    let (stdout_sender, stdout_receiver) = mpsc::channel();
    let stdout_reader = thread::spawn(move || {
        let _ = stdout_sender.send(read_extension_host_stdout(stdout, max_stdout_bytes));
    });

    let started_at = Instant::now();
    let mut stdout_bytes = None;
    let mut exited_successfully = false;
    loop {
        if stdout_bytes.is_none() {
            match stdout_receiver.try_recv() {
                Ok(Ok(bytes)) => {
                    stdout_bytes = Some(bytes);
                }
                Ok(Err(error)) => {
                    terminate_extension_host_process_tree(&mut child);
                    let _ = child.wait();
                    join_stdout_reader(stdout_reader);
                    return Err(error);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    terminate_extension_host_process_tree(&mut child);
                    let _ = child.wait();
                    join_stdout_reader(stdout_reader);
                    return Err(ExtensionHostProtocolError::Malformed);
                }
            }
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|_| ExtensionHostProtocolError::SpawnFailed)?
        {
            if !status.success() {
                terminate_extension_host_process_tree(&mut child);
                join_stdout_reader(stdout_reader);
                return Err(ExtensionHostProtocolError::HostExited {
                    status: status.code(),
                });
            }

            exited_successfully = true;
        }

        if exited_successfully && let Some(bytes) = stdout_bytes {
            join_stdout_reader(stdout_reader);
            let messages = parse_extension_host_stdout_jsonl(bytes)?;
            return process_extension_registration_messages(extension_id, messages, registry);
        }

        if started_at.elapsed() >= timeout {
            terminate_extension_host_process_tree(&mut child);
            let _ = child.wait();
            join_stdout_reader(stdout_reader);
            return Err(ExtensionHostProtocolError::TimedOut);
        }

        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn configure_extension_host_process(command: &mut Command) {
    configure_extension_host_environment(command);
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_extension_host_process(command: &mut Command) {
    configure_extension_host_environment(command);
}

fn configure_extension_host_environment(command: &mut Command) {
    command.env_clear();
    copy_parent_env_if_present(command, "PATH");
    copy_parent_env_if_present(command, "HOME");
    copy_parent_env_if_present(command, "LANG");
    copy_parent_env_if_present(command, "LC_ALL");
    copy_parent_env_if_present(command, "LC_CTYPE");

    #[cfg(windows)]
    {
        copy_parent_env_if_present(command, "Path");
        copy_parent_env_if_present(command, "PATHEXT");
        copy_parent_env_if_present(command, "SystemRoot");
        copy_parent_env_if_present(command, "ComSpec");
        copy_parent_env_if_present(command, "TEMP");
        copy_parent_env_if_present(command, "TMP");
    }
}

fn copy_parent_env_if_present(command: &mut Command, key: &str) {
    if let Some(value) = std::env::var_os(key) {
        command.env(key, value);
    }
}

#[cfg(unix)]
fn terminate_extension_host_process_tree(child: &mut Child) {
    if let Ok(process_group_id) = libc::pid_t::try_from(child.id()) {
        unsafe {
            libc::kill(-process_group_id, libc::SIGKILL);
        }
    } else {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn terminate_extension_host_process_tree(child: &mut Child) {
    let _ = child.kill();
}

fn read_extension_host_stdout(
    mut stdout: ChildStdout,
    max_stdout_bytes: usize,
) -> Result<Vec<u8>, ExtensionHostProtocolError> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];

    loop {
        let remaining = max_stdout_bytes
            .saturating_add(1)
            .saturating_sub(bytes.len());
        let read_len = remaining.min(buffer.len());
        if read_len == 0 {
            return Err(ExtensionHostProtocolError::OutputTooLarge {
                max_bytes: max_stdout_bytes,
            });
        }

        match stdout.read(&mut buffer[..read_len]) {
            Ok(0) => return Ok(bytes),
            Ok(read) => {
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.len() > max_stdout_bytes {
                    return Err(ExtensionHostProtocolError::OutputTooLarge {
                        max_bytes: max_stdout_bytes,
                    });
                }
            }
            Err(_) => return Err(ExtensionHostProtocolError::Malformed),
        }
    }
}

fn read_extension_host_stdout_jsonl_lines(
    stdout: ChildStdout,
    max_line_bytes: usize,
    sender: &mpsc::Sender<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) if line.len() > max_line_bytes => {
                let _ = sender.send(Err(ExtensionHostProtocolError::OutputTooLarge {
                    max_bytes: max_line_bytes,
                }));
                return;
            }
            Ok(_) => {
                let parsed = serde_json::from_str::<serde_json::Value>(line.trim_end())
                    .map_err(|_| ExtensionHostProtocolError::Malformed)
                    .and_then(parse_extension_host_server_message);
                if sender.send(parsed).is_err() {
                    return;
                }
            }
            Err(_) => {
                let _ = sender.send(Err(ExtensionHostProtocolError::Malformed));
                return;
            }
        }
    }
}

fn parse_extension_host_stdout_jsonl(
    bytes: Vec<u8>,
) -> Result<Vec<serde_json::Value>, ExtensionHostProtocolError> {
    let output = String::from_utf8(bytes).map_err(|_| ExtensionHostProtocolError::Malformed)?;
    output
        .lines()
        .map(|line| serde_json::from_str(line).map_err(|_| ExtensionHostProtocolError::Malformed))
        .collect()
}

fn join_stdout_reader(stdout_reader: JoinHandle<()>) {
    let _ = stdout_reader.join();
}

impl ExtensionCatalog {
    pub fn from_manifests(
        manifests: Vec<ExtensionManifest>,
    ) -> Result<Self, ExtensionCatalogError> {
        let mut extension_ids = BTreeSet::new();
        let mut tool_candidates = BTreeMap::new();

        for manifest in &manifests {
            if !extension_ids.insert(manifest.id.clone()) {
                return Err(ExtensionCatalogError::DuplicateExtensionId {
                    id: manifest.id.clone(),
                });
            }

            for tool in &manifest.contributes.tools {
                let candidate = ExtensionToolCandidate {
                    extension_id: manifest.id.clone(),
                    extension_version: manifest.version.clone(),
                    tool: tool.clone(),
                };
                if tool_candidates
                    .insert(tool.name.clone(), candidate)
                    .is_some()
                {
                    return Err(ExtensionCatalogError::DuplicateToolName {
                        name: tool.name.clone(),
                    });
                }
            }
        }

        Ok(Self {
            extensions: manifests,
            tool_candidates,
        })
    }

    pub fn extensions(&self) -> &[ExtensionManifest] {
        &self.extensions
    }

    pub fn host_start_count(&self) -> usize {
        0
    }

    pub fn tool_candidates(&self, name: &str) -> Option<&ExtensionToolCandidate> {
        self.tool_candidates.get(name)
    }
}

impl ExtensionManifestIndex {
    pub fn from_package_roots(
        package_roots: impl IntoIterator<Item = ExtensionPackageRoot>,
    ) -> Result<Self, ExtensionPackageIndexError> {
        let mut records = Vec::new();
        for package_root in package_roots {
            records.extend(load_extension_package_root(package_root)?);
        }

        ExtensionCatalog::from_manifests(
            records
                .iter()
                .map(|record| record.manifest.clone())
                .collect(),
        )
        .map_err(ExtensionPackageIndexError::Catalog)?;

        Ok(Self { records })
    }

    pub fn records(&self) -> &[ExtensionPackageRecord] {
        &self.records
    }

    pub fn static_context_files(&self) -> Vec<ExtensionStaticContextFile> {
        self.records
            .iter()
            .flat_map(|record| {
                record
                    .manifest
                    .contributes
                    .static_context
                    .iter()
                    .map(|contribution| {
                        let ExtensionStaticContextSource::ExtensionFile { path } =
                            &contribution.source;
                        ExtensionStaticContextFile {
                            extension_id: record.manifest.id.0.clone(),
                            item_id: contribution.id.clone(),
                            package_root: record.package_root.clone(),
                            relative_path: path.clone(),
                            title: contribution.title.clone(),
                            placement: match contribution.placement {
                                ExtensionStaticContextPlacement::BackgroundContext => {
                                    StaticContextPlacement::BackgroundContext
                                }
                            },
                            max_bytes: contribution.max_bytes,
                        }
                    })
            })
            .collect()
    }

    pub fn host_start_count(&self) -> usize {
        0
    }

    pub fn to_cache(&self) -> ExtensionManifestIndexCache {
        ExtensionManifestIndexCache {
            records: self
                .records
                .iter()
                .map(|record| ExtensionManifestIndexCacheRecord {
                    extension_id: record.manifest.id.0.clone(),
                    version: record.manifest.version.clone(),
                    scope: record.scope,
                    package_root: record.package_root.clone(),
                    manifest_path: record.manifest_path.clone(),
                    source_ref: record.source_ref.clone(),
                })
                .collect(),
        }
    }
}

impl ExtensionManifestIndexCache {
    pub fn record_statuses(&self) -> Vec<ExtensionManifestIndexCacheRecordStatus> {
        self.records
            .iter()
            .map(|record| ExtensionManifestIndexCacheRecordStatus {
                extension_id: record.extension_id.clone(),
                manifest_path: record.manifest_path.clone(),
                state: cache_record_state(record),
            })
            .collect()
    }

    pub fn host_start_count(&self) -> usize {
        0
    }
}

fn load_extension_package_root(
    package_root: ExtensionPackageRoot,
) -> Result<Vec<ExtensionPackageRecord>, ExtensionPackageIndexError> {
    if !package_root.root.is_dir() {
        return Err(ExtensionPackageIndexError::MissingPackageRoot {
            root: package_root.root,
        });
    }

    let manifest_paths = discover_extension_manifest_paths(&package_root.root)?;
    if manifest_paths.is_empty() {
        return Err(ExtensionPackageIndexError::MissingManifest {
            root: package_root.root,
        });
    }

    manifest_paths
        .into_iter()
        .map(|manifest_path| {
            ensure_manifest_path_stays_inside_package_root(&package_root.root, &manifest_path)?;
            let manifest = read_extension_manifest_file(&manifest_path)?;
            Ok(ExtensionPackageRecord {
                manifest,
                scope: package_root.scope,
                package_root: package_root.root.clone(),
                manifest_path,
                source_ref: package_root.source_ref.clone(),
            })
        })
        .collect()
}

fn discover_extension_manifest_paths(
    package_root: &Path,
) -> Result<Vec<PathBuf>, ExtensionPackageIndexError> {
    let mut manifest_paths = Vec::new();
    let default_manifest_path = package_root.join("yach.extension.json");
    if default_manifest_path.is_file() {
        manifest_paths.push(default_manifest_path);
    }

    let package_json_path = package_root.join("package.json");
    if package_json_path.is_file() {
        let package_json = read_extension_package_json(&package_json_path)?;
        if let Some(yach) = package_json.yach {
            for pointer in yach.manifests {
                let manifest_path =
                    package_manifest_pointer_path(package_root, &package_json_path, pointer)?;
                manifest_paths.push(manifest_path);
            }
        }
    }

    Ok(manifest_paths)
}

fn read_extension_package_json(
    package_json_path: &Path,
) -> Result<RawExtensionPackageJson, ExtensionPackageIndexError> {
    let contents = fs::read_to_string(package_json_path).map_err(|_| {
        ExtensionPackageIndexError::MalformedPackageJson {
            path: package_json_path.to_path_buf(),
        }
    })?;
    serde_json::from_str(&contents).map_err(|_| ExtensionPackageIndexError::MalformedPackageJson {
        path: package_json_path.to_path_buf(),
    })
}

fn package_manifest_pointer_path(
    package_root: &Path,
    package_json_path: &Path,
    pointer: String,
) -> Result<PathBuf, ExtensionPackageIndexError> {
    if !is_valid_package_manifest_pointer(&pointer) {
        return Err(ExtensionPackageIndexError::InvalidManifestPointer {
            path: package_json_path.to_path_buf(),
            pointer,
        });
    }

    Ok(package_root.join(pointer))
}

fn read_extension_manifest_file(
    manifest_path: &Path,
) -> Result<ExtensionManifest, ExtensionPackageIndexError> {
    let contents = fs::read_to_string(manifest_path).map_err(|_| {
        ExtensionPackageIndexError::MissingManifestFile {
            path: manifest_path.to_path_buf(),
        }
    })?;
    let value =
        serde_json::from_str(&contents).map_err(|_| ExtensionPackageIndexError::Manifest {
            path: manifest_path.to_path_buf(),
            error: ExtensionManifestError::Malformed,
        })?;
    parse_extension_manifest(value).map_err(|error| ExtensionPackageIndexError::Manifest {
        path: manifest_path.to_path_buf(),
        error,
    })
}

fn ensure_manifest_path_stays_inside_package_root(
    package_root: &Path,
    manifest_path: &Path,
) -> Result<(), ExtensionPackageIndexError> {
    let canonical_root = fs::canonicalize(package_root).map_err(|_| {
        ExtensionPackageIndexError::MissingPackageRoot {
            root: package_root.to_path_buf(),
        }
    })?;
    let canonical_manifest = fs::canonicalize(manifest_path).map_err(|_| {
        ExtensionPackageIndexError::MissingManifestFile {
            path: manifest_path.to_path_buf(),
        }
    })?;

    if canonical_manifest.starts_with(canonical_root) {
        Ok(())
    } else {
        Err(ExtensionPackageIndexError::ManifestPathEscapedPackageRoot {
            root: package_root.to_path_buf(),
            path: manifest_path.to_path_buf(),
        })
    }
}

fn cache_record_state(
    record: &ExtensionManifestIndexCacheRecord,
) -> ExtensionManifestIndexCacheRecordState {
    if !record.manifest_path.is_file() {
        return ExtensionManifestIndexCacheRecordState::StaleMissingManifest;
    }

    if ensure_manifest_path_stays_inside_package_root(&record.package_root, &record.manifest_path)
        .is_err()
    {
        return ExtensionManifestIndexCacheRecordState::StaleEscapedPackageRoot;
    }

    ExtensionManifestIndexCacheRecordState::Present
}

fn is_valid_package_manifest_pointer(pointer: &str) -> bool {
    let pointer = Path::new(pointer);
    !pointer.as_os_str().is_empty()
        && !pointer.is_absolute()
        && pointer
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn parse_activation_event(
    event: String,
) -> Result<ExtensionActivationEvent, ExtensionManifestError> {
    if event == "postFirstPaint" {
        return Ok(ExtensionActivationEvent::PostFirstPaint);
    }

    if let Some(command) = event.strip_prefix("onCommand:")
        && is_valid_activation_command(command)
    {
        return Ok(ExtensionActivationEvent::Command(command.to_owned()));
    }

    Err(ExtensionManifestError::InvalidActivationEvent { event })
}

fn parse_tool_risk(risk: String) -> Result<ExtensionToolRisk, ExtensionManifestError> {
    match risk.as_str() {
        "reads_local_metadata" => Ok(ExtensionToolRisk::ReadsLocalMetadata),
        "reads_local_content" => Ok(ExtensionToolRisk::ReadsLocalContent),
        "mutates_local_state" => Ok(ExtensionToolRisk::MutatesLocalState),
        _ => Err(ExtensionManifestError::UnsupportedToolRisk { risk }),
    }
}

fn parse_static_context_contribution(
    contribution: RawExtensionStaticContextContribution,
) -> Result<ExtensionStaticContextContribution, ExtensionManifestError> {
    if !is_valid_static_context_id(&contribution.id) {
        return Err(ExtensionManifestError::InvalidStaticContextId {
            id: contribution.id,
        });
    }

    let source = match contribution.source {
        RawExtensionStaticContextSource::ExtensionFile { path } => {
            if !is_valid_static_context_path(&path) {
                return Err(ExtensionManifestError::InvalidStaticContextPath { path });
            }
            ExtensionStaticContextSource::ExtensionFile { path }
        }
    };

    Ok(ExtensionStaticContextContribution {
        id: contribution.id,
        title: contribution.title,
        source,
        placement: parse_static_context_placement(
            contribution
                .placement
                .unwrap_or_else(|| String::from("background_context")),
        )?,
        max_bytes: contribution.max_bytes,
    })
}

fn parse_static_context_placement(
    placement: String,
) -> Result<ExtensionStaticContextPlacement, ExtensionManifestError> {
    match placement.as_str() {
        "background_context" => Ok(ExtensionStaticContextPlacement::BackgroundContext),
        _ => Err(ExtensionManifestError::UnsupportedStaticContextPlacement { placement }),
    }
}

fn parse_extension_tool_input_schema(
    schema: RawExtensionToolInputSchema,
) -> Result<ToolInputSchema, ExtensionHostProtocolError> {
    if schema.schema_type != "object" || schema.additional_properties {
        return Err(ExtensionHostProtocolError::UnsupportedSchema);
    }

    let required_count = schema.required.len();
    let required = schema.required.into_iter().collect::<BTreeSet<_>>();
    if required.len() != required_count
        || required.len() != schema.properties.len()
        || !schema.properties.keys().all(|name| required.contains(name))
        || schema
            .properties
            .values()
            .any(|property| property.schema_type != "string")
    {
        return Err(ExtensionHostProtocolError::UnsupportedSchema);
    }

    Ok(ToolInputSchema::string_object(
        required,
        std::iter::empty::<&str>(),
        schema.max_serialized_bytes,
    ))
}

fn validate_tool_name(name: &str) -> Result<(), ExtensionManifestError> {
    if is_reserved_tool_name(name) {
        return Err(ExtensionManifestError::ReservedToolName {
            name: name.to_owned(),
        });
    }

    if !is_valid_tool_name(name) {
        return Err(ExtensionManifestError::InvalidToolName {
            name: name.to_owned(),
        });
    }

    Ok(())
}

fn is_reserved_tool_name(name: &str) -> bool {
    matches!(name, "project_path_info" | "fixture_echo_metadata")
}

fn is_valid_process_command(command: &str) -> bool {
    let command = command.trim();
    !command.is_empty() && !command.chars().any(char::is_whitespace)
}

fn is_valid_extension_id(id: &str) -> bool {
    !id.is_empty()
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
                && part
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                && part
                    .chars()
                    .last()
                    .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        })
}

fn is_valid_activation_command(command: &str) -> bool {
    !command.is_empty()
        && command.split('.').all(|part| {
            !part.is_empty()
                && part.chars().all(|ch| {
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_'
                })
        })
}

fn is_valid_tool_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_')
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn is_valid_static_context_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn is_valid_static_context_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use std::{collections::VecDeque, fmt::Debug};

    use crate::{
        ProviderToolCall, SessionId, ToolContinuationContext, ToolContinuationPolicy,
        ToolContinuationWorkflow, ToolOwner, ToolPermissionPolicy, TurnId,
        extension_install::ExtensionInstallRefKind,
    };

    fn toy_tool_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.toy-tools",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "activation": {
                "events": ["onCommand:yach.extensions.activate.example.toy-tools"]
            },
            "contributes": {
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": false
                }]
            }
        })
    }

    fn post_first_paint_toy_tool_manifest_json() -> serde_json::Value {
        serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.toy-tools",
            "version": "0.1.0",
            "main": {
                "command": "sh",
                "args": ["host.sh"]
            },
            "activation": {
                "events": ["postFirstPaint"]
            },
            "contributes": {
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": true
                }]
            }
        })
    }

    fn toy_extension_host_script() -> String {
        let ready = serde_json::json!({
            "type": "extension.ready",
            "protocol": "yach.extension-host.v2",
            "extension_id": "example.toy-tools"
        })
        .to_string();
        let register = serde_json::json!({
            "type": "tool.register",
            "name": "toy_tool",
            "description": "Return static fixture metadata.",
            "risk": "reads_local_metadata",
            "provider_visible": true,
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["label"],
                "properties": {
                    "label": { "type": "string" }
                },
                "maxSerializedBytes": 512
            }
        })
        .to_string();
        let result = serde_json::json!({
            "type": "tool.result",
            "request_id": "tool-request-1",
            "content": "{\"kind\":\"toy\",\"label\":\"fixture\"}"
        })
        .to_string();
        format!(
            r#"while IFS= read -r line; do
case "$line" in
  *extension.initialize*) printf '%s\n' '{ready}' '{register}' ;;
  *tool.invoke*) printf '%s\n' '{result}' ;;
esac
done
"#
        )
    }

    fn parse_valid_manifest(value: serde_json::Value) -> Result<ExtensionManifest, String> {
        parse_extension_manifest(value).map_err(|error| format!("{error:?}"))
    }

    #[test]
    fn activation_diagnostic_from_package_record_starts_discovered() -> Result<(), String> {
        let package_root = PathBuf::from("/tmp/yach-extension");
        let manifest_path = package_root.join("yach.extension.json");
        let manifest = parse_valid_manifest(toy_tool_manifest_json())?;
        let install = ExtensionInstallRecord {
            source: String::from("./extension"),
            kind: ExtensionInstallRefKind::LocalPath,
            scope: ExtensionInstallScope::User,
            enabled: true,
            package_root: package_root.clone(),
        };
        let record = ExtensionPackageRecord {
            manifest,
            scope: ExtensionInstallScope::User,
            package_root,
            manifest_path: manifest_path.clone(),
            source_ref: Some(String::from("./extension")),
        };

        let diagnostic =
            ExtensionActivationDiagnostic::from_package_record(&record, Some(&install));

        expect_equal(
            &diagnostic.extension_id.as_deref(),
            &Some("example.toy-tools"),
        )?;
        expect_equal(
            &diagnostic.activation_state,
            &ExtensionActivationState::Discovered,
        )?;
        expect_equal(&diagnostic.generation, &0)?;
        expect_equal(&diagnostic.manifest_path.as_ref(), &Some(&manifest_path))?;
        if !diagnostic.registered_tools.is_empty() {
            return Err(String::from(
                "discovered diagnostics must not invent active registrations",
            ));
        }
        if !diagnostic.provider_visible_tools.is_empty() {
            return Err(String::from(
                "discovered diagnostics must not advertise provider-visible tools",
            ));
        }
        Ok(())
    }

    #[test]
    fn activation_diagnostic_from_disabled_install_record_is_blocked() -> Result<(), String> {
        let install = ExtensionInstallRecord {
            source: String::from("./disabled-extension"),
            kind: ExtensionInstallRefKind::LocalPath,
            scope: ExtensionInstallScope::Project,
            enabled: false,
            package_root: PathBuf::from("/tmp/disabled-extension"),
        };

        let diagnostic = ExtensionActivationDiagnostic::from_install_record(&install);

        expect_equal(
            &diagnostic.activation_state,
            &ExtensionActivationState::Blocked,
        )?;
        expect_equal(
            &diagnostic.last_error_kind,
            &Some(ExtensionActivationErrorKind::Disabled),
        )?;
        expect_equal(
            &diagnostic.last_error_summary.as_deref(),
            &Some("install record is disabled"),
        )
    }

    struct TestPackageRoot {
        path: PathBuf,
    }

    impl TestPackageRoot {
        fn new(test_name: &str) -> Result<Self, String> {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("{error:?}"))?;
            let path = std::env::temp_dir().join(format!(
                "yach_{test_name}_{}_{}",
                std::process::id(),
                now.as_nanos()
            ));
            fs::create_dir_all(&path).map_err(|error| format!("{error:?}"))?;
            Ok(Self { path })
        }

        fn write_json_file(
            &self,
            relative_path: &str,
            value: &serde_json::Value,
        ) -> Result<PathBuf, String> {
            self.write_file(relative_path, &value.to_string())
        }

        fn write_file(&self, relative_path: &str, contents: &str) -> Result<PathBuf, String> {
            let path = self.path.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| format!("{error:?}"))?;
            }
            fs::write(&path, contents).map_err(|error| format!("{error:?}"))?;
            Ok(path)
        }

        fn package_root(&self, scope: ExtensionInstallScope) -> ExtensionPackageRoot {
            ExtensionPackageRoot {
                root: self.path.clone(),
                scope,
                source_ref: Some(String::from("github:example/toy-tools")),
            }
        }
    }

    impl Drop for TestPackageRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Debug)]
    struct FakeExtensionHostTransport {
        sent: Vec<ExtensionHostClientMessage>,
        received: VecDeque<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>,
    }

    impl FakeExtensionHostTransport {
        fn new(
            received: impl IntoIterator<
                Item = Result<ExtensionHostServerMessage, ExtensionHostProtocolError>,
            >,
        ) -> Self {
            Self {
                sent: Vec::new(),
                received: received.into_iter().collect(),
            }
        }

        fn sent(&self) -> &[ExtensionHostClientMessage] {
            &self.sent
        }
    }

    impl ExtensionHostTransport for FakeExtensionHostTransport {
        fn send(
            &mut self,
            message: ExtensionHostClientMessage,
        ) -> Result<(), ExtensionHostProtocolError> {
            self.sent.push(message);
            Ok(())
        }

        fn recv(
            &mut self,
            _timeout: Duration,
        ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
            self.received
                .pop_front()
                .unwrap_or(Err(ExtensionHostProtocolError::TimedOut))
        }
    }
    #[derive(Debug, Clone, Copy)]
    struct FixtureResourceBroker;

    impl ExtensionResourceBroker for FixtureResourceBroker {
        fn execute(&self, request: &ExtensionResourceRequest) -> ExtensionResourceResult {
            match request {
                ExtensionResourceRequest::ReadTextFile { path, .. } => {
                    ExtensionResourceResult::Completed {
                        path: path.clone(),
                        text: String::from("alpha\n"),
                        sha256: String::from("fixture-sha256"),
                    }
                }
            }
        }
    }

    fn ready_message(extension_id: &str) -> ExtensionHostServerMessage {
        ExtensionHostServerMessage::Ready {
            protocol: String::from("yach.extension-host.v2"),
            extension_id: extension_id.to_owned(),
        }
    }

    fn toy_tool_register_message() -> ExtensionHostServerMessage {
        ExtensionHostServerMessage::ToolRegister {
            name: String::from("toy_tool"),
            description: String::from("Return static fixture metadata."),
            risk: ExtensionToolRisk::ReadsLocalMetadata,
            provider_visible: true,
            input_schema: ToolInputSchema::string_object(
                ["label"],
                std::iter::empty::<&str>(),
                512,
            ),
        }
    }

    fn tool_result_message(request_id: &str, content: &str) -> ExtensionHostServerMessage {
        ExtensionHostServerMessage::ToolResult {
            request_id: request_id.to_owned(),
            content: content.to_owned(),
            status: ExtensionToolResultStatus::Completed,
            reason: None,
        }
    }

    fn catalog_from_valid_manifests(
        manifests: Vec<ExtensionManifest>,
    ) -> Result<ExtensionCatalog, String> {
        ExtensionCatalog::from_manifests(manifests).map_err(|error| format!("{error:?}"))
    }

    fn expect_equal<T>(actual: &T, expected: &T) -> Result<(), String>
    where
        T: Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(format!("expected {expected:?}, got {actual:?}"))
        }
    }

    fn extension_host_env_lock() -> Result<std::sync::MutexGuard<'static, ()>, String> {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ENV_LOCK.lock().map_err(|error| format!("{error:?}"))
    }

    #[cfg(unix)]
    fn process_marker(test_name: &str) -> String {
        format!("yach_extension_host_{test_name}_{}", std::process::id())
    }

    #[cfg(unix)]
    fn process_matching_marker_exists(marker: &str) -> bool {
        std::process::Command::new("pgrep")
            .args(["-f", marker])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(unix)]
    fn terminate_marker_processes(marker: &str) {
        let _ = std::process::Command::new("pkill")
            .args(["-TERM", "-f", marker])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[cfg(unix)]
    fn assert_no_process_matching_marker(marker: &str) {
        let process_was_running = process_matching_marker_exists(marker);
        if process_was_running {
            terminate_marker_processes(marker);
        }
        assert!(
            !process_was_running,
            "process matching marker {marker} was still running"
        );
    }

    #[test]
    fn extension_manifest_parses_toy_tool_without_executing_code() {
        let manifest = parse_extension_manifest(toy_tool_manifest_json());

        assert_eq!(
            manifest,
            Ok(ExtensionManifest {
                schema: ExtensionManifestSchema::V1,
                id: ExtensionId(String::from("example.toy-tools")),
                version: String::from("0.1.0"),
                main: ExtensionMain {
                    command: String::from("node"),
                    args: vec![String::from("./extension.js")],
                },
                activation: ExtensionActivation {
                    events: vec![ExtensionActivationEvent::Command(String::from(
                        "yach.extensions.activate.example.toy-tools"
                    ))],
                },
                contributes: ExtensionContributions {
                    tools: vec![ExtensionToolContribution {
                        name: String::from("toy_tool"),
                        description: String::from("Return static fixture metadata."),
                        risk: ExtensionToolRisk::ReadsLocalMetadata,
                        provider_visible: false,
                    }],
                    static_context: Vec::new(),
                    tool_replacement_bundles: Vec::new(),
                },
            })
        );
    }
    #[test]
    fn extension_manifest_parses_coordinated_tool_replacement_bundle() -> Result<(), String> {
        let manifest = parse_valid_manifest(serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.hashline",
            "version": "0.1.0",
            "main": {"command": "hashline-host"},
            "contributes": {
                "tools": [
                    {
                        "name": "hashline_read",
                        "description": "Read text with hashline anchors.",
                        "risk": "reads_local_content",
                        "provider_visible": true
                    },
                    {
                        "name": "hashline_edit",
                        "description": "Apply hashline edits.",
                        "risk": "mutates_local_state",
                        "provider_visible": true
                    }
                ],
                "tool_replacement_bundles": [{
                    "id": "hashline",
                    "members": [
                        {
                            "builtin": "read_text_file",
                            "tool": "hashline_read",
                            "contract": "preserve"
                        },
                        {
                            "builtin": "edit_text_file",
                            "tool": "hashline_edit",
                            "contract": "replace"
                        }
                    ]
                }]
            }
        }))?;

        expect_equal(
            &manifest.contributes.tool_replacement_bundles,
            &vec![ExtensionToolReplacementBundleContribution {
                id: String::from("hashline"),
                members: vec![
                    ExtensionToolReplacementMember {
                        builtin: String::from("read_text_file"),
                        tool: String::from("hashline_read"),
                        contract: ExtensionToolReplacementContract::Preserve,
                    },
                    ExtensionToolReplacementMember {
                        builtin: String::from("edit_text_file"),
                        tool: String::from("hashline_edit"),
                        contract: ExtensionToolReplacementContract::Replace,
                    },
                ],
            }],
        )
    }

    #[test]
    fn replacement_bundle_resolves_atomically_and_projects_member_contracts() -> Result<(), String>
    {
        let mut snapshot = ExtensionActivationSnapshot::default();
        let read_schema = snapshot
            .registry
            .get("read_text_file")
            .map(|definition| definition.input_schema.clone())
            .ok_or_else(|| String::from("missing read_text_file"))?;
        let hashline_edit_schema =
            ToolInputSchema::string_object(["patch"], std::iter::empty::<&str>(), 512 * 1024);
        snapshot
            .registry
            .register_extension_tool(ToolDefinition::extension_tool_with_version(
                "example.hashline",
                Some("0.1.0"),
                "hashline_read",
                "Read text with hashline anchors.",
                read_schema,
                ToolRisk::ReadsLocalContent,
                ProviderToolVisibility::Visible,
            ))
            .map_err(|error| format!("{error:?}"))?;
        snapshot
            .registry
            .register_extension_tool(ToolDefinition::extension_tool_with_version(
                "example.hashline",
                Some("0.1.0"),
                "hashline_edit",
                "Apply hashline edits.",
                hashline_edit_schema.clone(),
                ToolRisk::MutatesLocalState,
                ProviderToolVisibility::Visible,
            ))
            .map_err(|error| format!("{error:?}"))?;
        snapshot.replacement_bundles = vec![ActivatedToolReplacementBundle {
            extension_id: String::from("example.hashline"),
            extension_version: String::from("0.1.0"),
            bundle_id: String::from("hashline"),
            source: ToolReplacementSource::User,
            members: vec![
                ExtensionToolReplacementMember {
                    builtin: String::from("read_text_file"),
                    tool: String::from("hashline_read"),
                    contract: ExtensionToolReplacementContract::Preserve,
                },
                ExtensionToolReplacementMember {
                    builtin: String::from("edit_text_file"),
                    tool: String::from("hashline_edit"),
                    contract: ExtensionToolReplacementContract::Replace,
                },
            ],
        }];
        let policy = ToolPermissionPolicy::allow_project_metadata_content_and_agent_edit_tools(
            ["project_path_info"],
            ["read_text_file", "hashline_read"],
            ["edit_text_file", "hashline_edit"],
        );

        let (incomplete, incomplete_diagnostics) = snapshot.resolve_provider_turn_catalog(
            &policy,
            ["read_text_file", "edit_text_file", "hashline_read"],
        );
        expect_equal(
            &incomplete.implementation_name_for_provider_tool("read_text_file"),
            &Some("read_text_file"),
        )?;
        expect_equal(
            &incomplete.implementation_name_for_provider_tool("edit_text_file"),
            &Some("edit_text_file"),
        )?;
        expect_equal(&incomplete_diagnostics.len(), &1)?;

        let (resolved, diagnostics) = snapshot.resolve_provider_turn_catalog(
            &policy,
            [
                "read_text_file",
                "edit_text_file",
                "hashline_read",
                "hashline_edit",
            ],
        );
        expect_equal(&diagnostics, &Vec::new())?;
        expect_equal(
            &resolved.implementation_name_for_provider_tool("read_text_file"),
            &Some("hashline_read"),
        )?;
        expect_equal(
            &resolved.implementation_name_for_provider_tool("edit_text_file"),
            &Some("hashline_edit"),
        )?;
        expect_equal(
            &resolved
                .resolved_tool("edit_text_file")
                .map(|tool| &tool.definition.input_schema),
            &Some(&hashline_edit_schema),
        )
    }

    #[test]
    fn extension_manifest_rejects_malformed_identity_and_tool_names() {
        let mut invalid_id = toy_tool_manifest_json();
        invalid_id["id"] = serde_json::json!("bad id with spaces");
        assert_eq!(
            parse_extension_manifest(invalid_id),
            Err(ExtensionManifestError::InvalidExtensionId)
        );

        let mut invalid_tool = toy_tool_manifest_json();
        invalid_tool["contributes"]["tools"][0]["name"] = serde_json::json!("project_path_info");
        assert_eq!(
            parse_extension_manifest(invalid_tool),
            Err(ExtensionManifestError::ReservedToolName {
                name: String::from("project_path_info")
            })
        );
    }

    #[test]
    fn extension_manifest_rejects_unsupported_schema_and_risk() {
        let mut unsupported_schema = toy_tool_manifest_json();
        unsupported_schema["schema"] = serde_json::json!("yach.extension.v2");
        assert_eq!(
            parse_extension_manifest(unsupported_schema),
            Err(ExtensionManifestError::UnsupportedSchema)
        );

        let mut unsupported_risk = toy_tool_manifest_json();
        unsupported_risk["contributes"]["tools"][0]["risk"] =
            serde_json::json!("writes_local_files");
        assert_eq!(
            parse_extension_manifest(unsupported_risk),
            Err(ExtensionManifestError::UnsupportedToolRisk {
                risk: String::from("writes_local_files")
            })
        );
    }

    #[test]
    fn extension_manifest_rejects_duplicate_tool_names_within_one_manifest() {
        let mut duplicate_tool = toy_tool_manifest_json();
        duplicate_tool["contributes"]["tools"] = serde_json::json!([
            {
                "name": "toy_tool",
                "description": "Return static fixture metadata.",
                "risk": "reads_local_metadata",
                "provider_visible": false
            },
            {
                "name": "toy_tool",
                "description": "Return more static fixture metadata.",
                "risk": "reads_local_metadata",
                "provider_visible": false
            }
        ]);

        assert_eq!(
            parse_extension_manifest(duplicate_tool),
            Err(ExtensionManifestError::DuplicateToolName {
                name: String::from("toy_tool")
            })
        );
    }

    #[test]
    fn extension_catalog_rejects_duplicate_tool_names_across_manifests() -> Result<(), String> {
        let first = parse_valid_manifest(toy_tool_manifest_json())?;
        let mut second_json = toy_tool_manifest_json();
        second_json["id"] = serde_json::json!("example.more-tools");
        let second = parse_valid_manifest(second_json)?;

        expect_equal(
            &ExtensionCatalog::from_manifests(vec![first, second]),
            &Err(ExtensionCatalogError::DuplicateToolName {
                name: String::from("toy_tool"),
            }),
        )?;
        Ok(())
    }

    #[test]
    fn extension_catalog_rejects_duplicate_extension_ids_across_manifests() -> Result<(), String> {
        let first = parse_valid_manifest(toy_tool_manifest_json())?;
        let mut second_json = toy_tool_manifest_json();
        second_json["contributes"]["tools"][0]["name"] = serde_json::json!("second_toy_tool");
        let second = parse_valid_manifest(second_json)?;

        expect_equal(
            &ExtensionCatalog::from_manifests(vec![first, second]),
            &Err(ExtensionCatalogError::DuplicateExtensionId {
                id: ExtensionId(String::from("example.toy-tools")),
            }),
        )?;
        Ok(())
    }

    #[test]
    fn extension_manifest_rejects_unknown_fields_as_malformed() {
        let mut unknown_root_field = toy_tool_manifest_json();
        unknown_root_field["unexpected"] = serde_json::json!(true);
        assert_eq!(
            parse_extension_manifest(unknown_root_field),
            Err(ExtensionManifestError::Malformed)
        );

        let mut unknown_nested_field = toy_tool_manifest_json();
        unknown_nested_field["main"]["env"] = serde_json::json!({});
        assert_eq!(
            parse_extension_manifest(unknown_nested_field),
            Err(ExtensionManifestError::Malformed)
        );
    }

    #[test]
    fn extension_manifest_defaults_empty_activation_events_and_contributes() -> Result<(), String> {
        let minimal_manifest = serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.empty-tools",
            "version": "0.1.0",
            "main": {
                "command": "node"
            }
        });

        let manifest = parse_valid_manifest(minimal_manifest)?;

        expect_equal(
            &manifest,
            &ExtensionManifest {
                schema: ExtensionManifestSchema::V1,
                id: ExtensionId(String::from("example.empty-tools")),
                version: String::from("0.1.0"),
                main: ExtensionMain {
                    command: String::from("node"),
                    args: Vec::new(),
                },
                activation: ExtensionActivation { events: Vec::new() },
                contributes: ExtensionContributions {
                    tools: Vec::new(),
                    static_context: Vec::new(),
                    tool_replacement_bundles: Vec::new(),
                },
            },
        )?;

        let catalog = catalog_from_valid_manifests(vec![manifest])?;
        expect_equal(&catalog.extensions().len(), &1)?;
        expect_equal(&catalog.host_start_count(), &0)?;
        expect_equal(&catalog.tool_candidates("toy_tool"), &None)?;
        Ok(())
    }

    #[test]
    fn extension_manifest_accepts_packaged_background_static_context_contribution()
    -> Result<(), String> {
        let manifest = parse_valid_manifest(serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.context-pack",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "contributes": {
                "static_context": [{
                    "id": "rust-style-guide",
                    "title": "Rust style guide",
                    "source": {
                        "type": "extension_file",
                        "path": "context/rust.md"
                    },
                    "placement": "background_context",
                    "max_bytes": 12000
                }]
            }
        }))?;

        expect_equal(
            &manifest.contributes.static_context,
            &vec![ExtensionStaticContextContribution {
                id: String::from("rust-style-guide"),
                title: String::from("Rust style guide"),
                source: ExtensionStaticContextSource::ExtensionFile {
                    path: String::from("context/rust.md"),
                },
                placement: ExtensionStaticContextPlacement::BackgroundContext,
                max_bytes: 12000,
            }],
        )
    }

    #[test]
    fn extension_manifest_defaults_static_context_placement_to_background_context()
    -> Result<(), String> {
        let manifest = parse_valid_manifest(serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.context-pack",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "contributes": {
                "static_context": [{
                    "id": "rust-style-guide",
                    "title": "Rust style guide",
                    "source": {
                        "type": "extension_file",
                        "path": "context/rust.md"
                    },
                    "max_bytes": 12000
                }]
            }
        }))?;

        expect_equal(
            &manifest.contributes.static_context[0].placement,
            &ExtensionStaticContextPlacement::BackgroundContext,
        )
    }

    #[test]
    fn extension_manifest_rejects_static_context_append_system_placement_for_now() {
        let error = parse_extension_manifest(serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.context-pack",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "contributes": {
                "static_context": [{
                    "id": "system-guide",
                    "title": "System guide",
                    "source": {
                        "type": "extension_file",
                        "path": "context/system.md"
                    },
                    "placement": "append_system",
                    "max_bytes": 1024
                }]
            }
        }));

        assert_eq!(
            error,
            Err(ExtensionManifestError::UnsupportedStaticContextPlacement {
                placement: String::from("append_system")
            })
        );
    }

    #[test]
    fn extension_manifest_index_maps_static_context_files_without_starting_hosts()
    -> Result<(), String> {
        let package = TestPackageRoot::new("static_context_files")?;
        let manifest = serde_json::json!({
            "schema": "yach.extension.v1",
            "id": "example.context-pack",
            "version": "0.1.0",
            "main": {
                "command": "node",
                "args": ["./extension.js"]
            },
            "contributes": {
                "static_context": [{
                    "id": "rust-style-guide",
                    "title": "Rust style guide",
                    "source": {
                        "type": "extension_file",
                        "path": "context/rust.md"
                    },
                    "placement": "background_context",
                    "max_bytes": 12000
                }]
            }
        });
        package.write_json_file("yach.extension.json", &manifest)?;

        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;

        expect_equal(&index.host_start_count(), &0)?;
        expect_equal(
            &index.static_context_files(),
            &vec![ExtensionStaticContextFile {
                extension_id: String::from("example.context-pack"),
                item_id: String::from("rust-style-guide"),
                package_root: package.path.clone(),
                relative_path: String::from("context/rust.md"),
                title: String::from("Rust style guide"),
                placement: StaticContextPlacement::BackgroundContext,
                max_bytes: 12000,
            }],
        )
    }

    #[test]
    fn extension_catalog_discovery_is_manifest_only() -> Result<(), String> {
        let manifest = parse_valid_manifest(toy_tool_manifest_json())?;
        let catalog = catalog_from_valid_manifests(vec![manifest])?;

        expect_equal(&catalog.extensions().len(), &1)?;
        expect_equal(&catalog.host_start_count(), &0)?;
        expect_equal(
            &catalog
                .tool_candidates("toy_tool")
                .map(|candidate| &candidate.extension_id),
            &Some(&ExtensionId(String::from("example.toy-tools"))),
        )?;
        Ok(())
    }

    #[test]
    fn extension_package_root_loads_yach_extension_manifest() -> Result<(), String> {
        let package = TestPackageRoot::new("default_manifest")?;
        let manifest_path =
            package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;

        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;

        expect_equal(&index.records().len(), &1)?;
        expect_equal(
            &index.records()[0].manifest.id,
            &ExtensionId(String::from("example.toy-tools")),
        )?;
        expect_equal(&index.records()[0].manifest_path, &manifest_path)
    }

    #[test]
    fn extension_package_root_loads_package_json_yach_manifest_pointer() -> Result<(), String> {
        let package = TestPackageRoot::new("package_json_manifest")?;
        let manifest_path =
            package.write_json_file("manifests/toy.extension.json", &toy_tool_manifest_json())?;
        package.write_json_file(
            "package.json",
            &serde_json::json!({
                "name": "example-package",
                "version": "0.1.0",
                "yach": {
                    "manifests": ["manifests/toy.extension.json"]
                }
            }),
        )?;

        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::User)
        ])
        .map_err(|error| format!("{error:?}"))?;

        expect_equal(&index.records().len(), &1)?;
        expect_equal(&index.records()[0].manifest_path, &manifest_path)?;
        expect_equal(
            &index.records()[0].manifest.id,
            &ExtensionId(String::from("example.toy-tools")),
        )
    }

    #[test]
    fn extension_package_index_records_source_scope_and_manifest_path() -> Result<(), String> {
        let package = TestPackageRoot::new("records_source_scope")?;
        let manifest_path =
            package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;

        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Ephemeral)
        ])
        .map_err(|error| format!("{error:?}"))?;
        let record = &index.records()[0];

        expect_equal(&record.scope, &ExtensionInstallScope::Ephemeral)?;
        expect_equal(&record.package_root, &package.path)?;
        expect_equal(&record.manifest_path, &manifest_path)?;
        expect_equal(
            &record.source_ref,
            &Some(String::from("github:example/toy-tools")),
        )
    }

    #[test]
    fn extension_package_index_rejects_duplicate_extension_ids() -> Result<(), String> {
        let first = TestPackageRoot::new("duplicate_extension_first")?;
        first.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;
        let second = TestPackageRoot::new("duplicate_extension_second")?;
        let mut second_manifest = toy_tool_manifest_json();
        second_manifest["contributes"]["tools"][0]["name"] = serde_json::json!("second_toy_tool");
        second.write_json_file("yach.extension.json", &second_manifest)?;

        let error = ExtensionManifestIndex::from_package_roots([
            first.package_root(ExtensionInstallScope::Project),
            second.package_root(ExtensionInstallScope::Project),
        ]);

        expect_equal(
            &error,
            &Err(ExtensionPackageIndexError::Catalog(
                ExtensionCatalogError::DuplicateExtensionId {
                    id: ExtensionId(String::from("example.toy-tools")),
                },
            )),
        )
    }

    #[test]
    fn extension_package_index_rejects_manifest_pointers_outside_package_root() -> Result<(), String>
    {
        let package = TestPackageRoot::new("invalid_manifest_pointer")?;
        package.write_json_file(
            "package.json",
            &serde_json::json!({
                "yach": {
                    "manifests": ["../outside.extension.json"]
                }
            }),
        )?;

        let error = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::User)
        ]);

        expect_equal(
            &error,
            &Err(ExtensionPackageIndexError::InvalidManifestPointer {
                path: package.path.join("package.json"),
                pointer: String::from("../outside.extension.json"),
            }),
        )
    }

    #[test]
    fn extension_package_index_does_not_start_hosts() -> Result<(), String> {
        let package = TestPackageRoot::new("index_does_not_start_hosts")?;
        package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;

        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;

        expect_equal(&index.host_start_count(), &0)
    }

    #[test]
    fn extension_package_index_cache_round_trips_valid_records() -> Result<(), String> {
        let package = TestPackageRoot::new("cache_round_trip")?;
        package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;
        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;

        let cache = index.to_cache();
        let encoded = serde_json::to_value(&cache).map_err(|error| format!("{error:?}"))?;
        let decoded: ExtensionManifestIndexCache =
            serde_json::from_value(encoded).map_err(|error| format!("{error:?}"))?;

        expect_equal(&decoded, &cache)
    }

    #[test]
    fn extension_package_index_cache_marks_missing_manifest_path_stale() -> Result<(), String> {
        let package = TestPackageRoot::new("cache_missing_manifest")?;
        let manifest_path =
            package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;
        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;
        let cache = index.to_cache();
        fs::remove_file(manifest_path).map_err(|error| format!("{error:?}"))?;

        let statuses = cache.record_statuses();

        expect_equal(&statuses.len(), &1)?;
        expect_equal(
            &statuses[0].state,
            &ExtensionManifestIndexCacheRecordState::StaleMissingManifest,
        )
    }

    #[test]
    fn extension_package_index_cache_load_does_not_start_hosts() -> Result<(), String> {
        let package = TestPackageRoot::new("cache_does_not_start_hosts")?;
        package.write_json_file("yach.extension.json", &toy_tool_manifest_json())?;
        let index = ExtensionManifestIndex::from_package_roots([
            package.package_root(ExtensionInstallScope::Project)
        ])
        .map_err(|error| format!("{error:?}"))?;
        let cache = index.to_cache();

        expect_equal(&cache.host_start_count(), &0)
    }
    #[test]
    fn extension_host_tool_result_requires_a_reason_for_failures() -> Result<(), String> {
        let failed = parse_extension_host_server_message(serde_json::json!({
            "type": "tool.result",
            "request_id": "tool-request-1",
            "status": "failed",
            "reason": "malformed_patch",
            "content": "[hashline error: malformed hashline patch]"
        }))
        .map_err(|error| format!("{error:?}"))?;
        expect_equal(
            &failed,
            &ExtensionHostServerMessage::ToolResult {
                request_id: String::from("tool-request-1"),
                content: String::from("[hashline error: malformed hashline patch]"),
                status: ExtensionToolResultStatus::Failed,
                reason: Some(String::from("malformed_patch")),
            },
        )?;
        expect_equal(
            &parse_extension_host_server_message(serde_json::json!({
                "type": "tool.result",
                "request_id": "tool-request-1",
                "status": "failed",
                "content": "missing categorical reason"
            })),
            &Err(ExtensionHostProtocolError::Malformed),
        )
    }

    #[test]
    fn extension_host_session_initializes_registers_and_invokes_toy_tool() -> Result<(), String> {
        let transport = FakeExtensionHostTransport::new([
            Ok(ready_message("example.toy-tools")),
            Ok(toy_tool_register_message()),
            Ok(tool_result_message(
                "tool-request-1",
                "{\"kind\":\"toy\",\"label\":\"fixture\"}",
            )),
        ]);
        let mut session = ExtensionHostSession::new("example.toy-tools", transport, 1024);
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registered = session
            .initialize_and_register(&mut registry, Some("0.1.0"), 1, Duration::from_secs(1))
            .map_err(|error| format!("{error:?}"))?;
        let response = session
            .invoke_tool(
                "tool-request-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
                Duration::from_secs(1),
                &DenyExtensionResources,
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(&registered, &vec![String::from("toy_tool")])?;
        expect_equal(
            &registry.get("toy_tool").map(|definition| &definition.owner),
            &Some(&ToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: Some(String::from("0.1.0")),
            }),
        )?;
        expect_equal(
            &response,
            &ExtensionHostInvocation::ToolResult {
                content: String::from("{\"kind\":\"toy\",\"label\":\"fixture\"}"),
                status: ExtensionToolResultStatus::Completed,
                reason: None,
            },
        )?;
        expect_equal(
            &serde_json::to_value(&session.transport().sent()[0])
                .map_err(|error| format!("{error:?}"))?,
            &serde_json::json!({
                "type": "extension.initialize",
                "protocol": "yach.extension-host.v2",
                "extension_id": "example.toy-tools"
            }),
        )?;
        expect_equal(
            &serde_json::to_value(&session.transport().sent()[1])
                .map_err(|error| format!("{error:?}"))?,
            &serde_json::json!({
                "type": "tool.invoke",
                "request_id": "tool-request-1",
                "name": "toy_tool",
                "arguments": {"label":"fixture"}
            }),
        )
    }

    #[test]
    fn extension_host_invocation_accepts_plain_text_result() -> Result<(), String> {
        let mut session = ExtensionHostSession::new(
            "example.hashline",
            FakeExtensionHostTransport::new([Ok(tool_result_message(
                "tool-request-1",
                "[src/lib.rs#0123456789abcdef]\n1:alpha",
            ))]),
            1024,
        );

        let response = session
            .invoke_tool(
                "tool-request-1",
                "hashline_read",
                serde_json::json!({"path":"src/lib.rs"}),
                Duration::from_secs(1),
                &DenyExtensionResources,
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(
            &response,
            &ExtensionHostInvocation::ToolResult {
                content: String::from("[src/lib.rs#0123456789abcdef]\n1:alpha"),
                status: ExtensionToolResultStatus::Completed,
                reason: None,
            },
        )
    }
    #[test]
    fn extension_host_invocation_brokers_resource_requests_before_result() -> Result<(), String> {
        let transport = FakeExtensionHostTransport::new([
            Ok(ExtensionHostServerMessage::ResourceRequest {
                request_id: String::from("resource-1"),
                operation: ExtensionResourceRequest::ReadTextFile {
                    path: String::from("src/lib.rs"),
                    max_bytes: 4096,
                },
            }),
            Ok(tool_result_message("tool-request-1", "{\"lines\":1}")),
        ]);
        let mut session = ExtensionHostSession::new("example.hashline", transport, 4096);

        let response = session
            .invoke_tool(
                "tool-request-1",
                "hashline_read",
                serde_json::json!({"path":"src/lib.rs"}),
                Duration::from_secs(1),
                &FixtureResourceBroker,
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(
            &response,
            &ExtensionHostInvocation::ToolResult {
                content: String::from("{\"lines\":1}"),
                status: ExtensionToolResultStatus::Completed,
                reason: None,
            },
        )?;
        expect_equal(
            &serde_json::to_value(&session.transport().sent()[1])
                .map_err(|error| format!("{error:?}"))?,
            &serde_json::json!({
                "type": "resource.result",
                "request_id": "resource-1",
                "result": {
                    "status": "completed",
                    "path": "src/lib.rs",
                    "text": "alpha\n",
                    "sha256": "fixture-sha256"
                }
            }),
        )
    }

    #[test]
    fn extension_host_invocation_returns_structured_edit_proposal() -> Result<(), String> {
        let proposal = ExtensionEditProposal {
            summary: String::from("Update two files"),
            operations: vec![ExtensionEditProposalOperation::ModifyTextFile {
                path: String::from("src/lib.rs"),
                expected_sha256: String::from("before-sha256"),
                after_text: String::from("updated\n"),
            }],
        };
        let transport =
            FakeExtensionHostTransport::new([Ok(ExtensionHostServerMessage::EditProposal {
                request_id: String::from("tool-request-1"),
                proposal: proposal.clone(),
            })]);
        let mut session = ExtensionHostSession::new("example.hashline", transport, 4096);

        let response = session
            .invoke_tool(
                "tool-request-1",
                "hashline_edit",
                serde_json::json!({"patch":"fixture"}),
                Duration::from_secs(1),
                &DenyExtensionResources,
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(&response, &ExtensionHostInvocation::EditProposal(proposal))
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_transport_invokes_live_stdio_tool() -> Result<(), String> {
        let package = TestPackageRoot::new("live-stdio-host")?;
        package.write_file("host.sh", &toy_extension_host_script())?;
        let transport = ExtensionProcessHostTransport::spawn(
            &ExtensionMain {
                command: String::from("sh"),
                args: vec![String::from("host.sh")],
            },
            &package.path,
            4096,
        )
        .map_err(|error| format!("{error:?}"))?;
        let mut session = ExtensionHostSession::new("example.toy-tools", transport, 4096);
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registered = session
            .initialize_and_register(&mut registry, None, 1, Duration::from_secs(1))
            .map_err(|error| format!("{error:?}"))?;
        let response = session
            .invoke_tool(
                "tool-request-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
                Duration::from_secs(1),
                &DenyExtensionResources,
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(&registered, &vec![String::from("toy_tool")])?;
        expect_equal(
            &response,
            &ExtensionHostInvocation::ToolResult {
                content: String::from("{\"kind\":\"toy\",\"label\":\"fixture\"}"),
                status: ExtensionToolResultStatus::Completed,
                reason: None,
            },
        )
    }

    #[cfg(unix)]
    #[test]
    fn background_activation_snapshot_routes_active_metadata_tool() -> Result<(), String> {
        let package = TestPackageRoot::new("background-activation")?;
        package.write_json_file(
            "yach.extension.json",
            &post_first_paint_toy_tool_manifest_json(),
        )?;
        package.write_file("host.sh", &toy_extension_host_script())?;
        let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
            root: package.path.clone(),
            scope: ExtensionInstallScope::User,
            source_ref: Some(String::from("test-package-root")),
        }])
        .map_err(|error| format!("{error:?}"))?;

        let snapshot = activate_background_metadata_extensions(
            index.records(),
            ExtensionBackgroundActivationConfig {
                registration_timeout: Duration::from_secs(1),
                invocation_timeout: Duration::from_secs(1),
                max_stdout_line_bytes: 4096,
                max_result_bytes: 4096,
            },
        );

        expect_equal(&snapshot.host_start_count, &1)?;
        expect_equal(&snapshot.active_tool_names(), &vec!["toy_tool"])?;
        expect_equal(&snapshot.diagnostics.len(), &1)?;
        expect_equal(
            &snapshot.diagnostics[0].activation_state,
            &ExtensionActivationState::Active,
        )?;
        expect_equal(
            &snapshot.diagnostics[0].provider_visible_tools,
            &vec![String::from("toy_tool")],
        )?;

        let policy = ToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]);
        let workflow = ToolContinuationWorkflow {
            registry: &snapshot.registry,
            permission_policy: &policy,
            executor: &snapshot.executor,
            continuation_policy: ToolContinuationPolicy {
                max_tool_calls: 1,
                max_result_bytes: 4096,
            },
        };
        let mut log = crate::SessionLog::default();
        let results = workflow
            .build_provider_tool_results(
                &mut log,
                &ToolContinuationContext {
                    session_id: SessionId(String::from("default")),
                    turn_id: TurnId(String::from("turn-1")),
                },
                vec![ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("toy_tool"),
                    arguments_json: serde_json::json!({"label":"fixture"}),
                }],
            )
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(&results.len(), &1)?;
        expect_equal(
            &results[0].content,
            &String::from("{\"kind\":\"toy\",\"label\":\"fixture\"}"),
        )
    }

    #[cfg(unix)]
    #[test]
    fn background_activation_snapshot_stop_removes_active_metadata_tool() -> Result<(), String> {
        let package = TestPackageRoot::new("background-activation-stop")?;
        package.write_json_file(
            "yach.extension.json",
            &post_first_paint_toy_tool_manifest_json(),
        )?;
        package.write_file("host.sh", &toy_extension_host_script())?;
        let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
            root: package.path.clone(),
            scope: ExtensionInstallScope::User,
            source_ref: Some(String::from("test-package-root")),
        }])
        .map_err(|error| format!("{error:?}"))?;

        let mut snapshot = activate_background_metadata_extensions(
            index.records(),
            ExtensionBackgroundActivationConfig {
                registration_timeout: Duration::from_secs(1),
                invocation_timeout: Duration::from_secs(1),
                max_stdout_line_bytes: 4096,
                max_result_bytes: 4096,
            },
        );

        let diagnostic = snapshot
            .stop_extension("example.toy-tools")
            .map_err(|error| format!("{error:?}"))?;

        expect_equal(
            &diagnostic.activation_state,
            &ExtensionActivationState::Stopped,
        )?;
        expect_equal(&diagnostic.generation, &2)?;
        expect_equal(&diagnostic.registered_tools, &Vec::<String>::new())?;
        expect_equal(&diagnostic.provider_visible_tools, &Vec::<String>::new())?;
        expect_equal(&snapshot.active_tool_names(), &Vec::<&str>::new())?;
        expect_equal(&snapshot.executor.handler_count(), &0)?;
        expect_equal(&snapshot.registry.get("toy_tool").is_none(), &true)?;

        let policy = ToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]);
        let catalog = snapshot
            .registry
            .resolve_provider_turn_catalog(&policy, ["toy_tool"]);
        expect_equal(&catalog.provider_definitions(), &Vec::new())
    }

    #[cfg(unix)]
    #[test]
    fn background_activation_snapshot_reload_reactivates_stopped_metadata_tool()
    -> Result<(), String> {
        let package = TestPackageRoot::new("background-activation-reload")?;
        package.write_json_file(
            "yach.extension.json",
            &post_first_paint_toy_tool_manifest_json(),
        )?;
        package.write_file("host.sh", &toy_extension_host_script())?;
        let index = ExtensionManifestIndex::from_package_roots([ExtensionPackageRoot {
            root: package.path.clone(),
            scope: ExtensionInstallScope::User,
            source_ref: Some(String::from("test-package-root")),
        }])
        .map_err(|error| format!("{error:?}"))?;
        let config = ExtensionBackgroundActivationConfig {
            registration_timeout: Duration::from_secs(1),
            invocation_timeout: Duration::from_secs(1),
            max_stdout_line_bytes: 4096,
            max_result_bytes: 4096,
        };
        let mut snapshot = activate_background_metadata_extensions(index.records(), config);

        snapshot
            .stop_extension("example.toy-tools")
            .map_err(|error| format!("{error:?}"))?;
        let diagnostic = snapshot.reload_extension_from_record(&index.records()[0], config);

        expect_equal(
            &diagnostic.activation_state,
            &ExtensionActivationState::Active,
        )?;
        expect_equal(&diagnostic.generation, &3)?;
        expect_equal(
            &diagnostic.registered_tools,
            &vec![String::from("toy_tool")],
        )?;
        expect_equal(
            &diagnostic.provider_visible_tools,
            &vec![String::from("toy_tool")],
        )?;
        expect_equal(&snapshot.active_tool_names(), &vec!["toy_tool"])?;
        expect_equal(&snapshot.executor.handler_count(), &1)?;
        expect_equal(&snapshot.registry.get("toy_tool").is_some(), &true)?;

        let policy = ToolPermissionPolicy::allow_project_metadata_tools(["toy_tool"]);
        let catalog = snapshot
            .registry
            .resolve_provider_turn_catalog(&policy, ["toy_tool"]);
        expect_equal(&catalog.provider_definitions().len(), &1)?;

        let workflow = ToolContinuationWorkflow {
            registry: &snapshot.registry,
            permission_policy: &policy,
            executor: &snapshot.executor,
            continuation_policy: ToolContinuationPolicy {
                max_tool_calls: 4,
                max_result_bytes: 4096,
            },
        };
        let mut log = crate::SessionLog::default();
        let results = workflow
            .build_provider_tool_results(
                &mut log,
                &ToolContinuationContext {
                    session_id: SessionId(String::from("default")),
                    turn_id: TurnId(String::from("turn-1")),
                },
                vec![ProviderToolCall {
                    call_id: String::from("provider-call-1"),
                    name: String::from("toy_tool"),
                    arguments_json: serde_json::json!({"label":"fixture"}),
                }],
            )
            .map_err(|error| format!("{error:?}"))?;
        expect_equal(&results.len(), &1)?;
        expect_equal(
            &results[0].content,
            &String::from("{\"kind\":\"toy\",\"label\":\"fixture\"}"),
        )
    }

    #[test]
    fn extension_host_session_categorizes_transport_failures() -> Result<(), String> {
        let mut timed_out = ExtensionHostSession::new(
            "example.toy-tools",
            FakeExtensionHostTransport::new([Err(ExtensionHostProtocolError::TimedOut)]),
            1024,
        );
        let mut exited = ExtensionHostSession::new(
            "example.toy-tools",
            FakeExtensionHostTransport::new([Err(ExtensionHostProtocolError::HostExited {
                status: Some(7),
            })]),
            1024,
        );
        let mut oversized = ExtensionHostSession::new(
            "example.toy-tools",
            FakeExtensionHostTransport::new([Ok(tool_result_message(
                "tool-request-1",
                "{\"kind\":\"toy\"}",
            ))]),
            4,
        );
        let mut mismatched_request = ExtensionHostSession::new(
            "example.toy-tools",
            FakeExtensionHostTransport::new([Ok(tool_result_message(
                "tool-request-2",
                "{\"kind\":\"toy\"}",
            ))]),
            1024,
        );
        let mut registry = ToolRegistry::with_project_read_only_tools();

        expect_equal(
            &timed_out.initialize_and_register(&mut registry, None, 1, Duration::from_millis(1)),
            &Err(ExtensionHostProtocolError::TimedOut),
        )?;
        expect_equal(
            &exited.initialize_and_register(&mut registry, None, 1, Duration::from_millis(1)),
            &Err(ExtensionHostProtocolError::HostExited { status: Some(7) }),
        )?;
        expect_equal(
            &oversized.invoke_tool(
                "tool-request-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
                Duration::from_millis(1),
                &DenyExtensionResources,
            ),
            &Err(ExtensionHostProtocolError::OutputTooLarge { max_bytes: 4 }),
        )?;
        expect_equal(
            &mismatched_request.invoke_tool(
                "tool-request-1",
                "toy_tool",
                serde_json::json!({"label":"fixture"}),
                Duration::from_millis(1),
                &DenyExtensionResources,
            ),
            &Err(ExtensionHostProtocolError::RequestIdMismatch),
        )
    }

    #[test]
    fn extension_host_registers_toy_tool_after_ready_handshake() -> Result<(), String> {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registered_tools = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v2",
                    "extension_id": "example.toy-tools"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": true,
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label"],
                        "properties": {
                            "label": { "type": "string" }
                        },
                        "maxSerializedBytes": 512
                    }
                }),
            ],
            &mut registry,
        )
        .map_err(|error| format!("{error:?}"))?;

        let definition = registry.get("toy_tool");

        expect_equal(&registered_tools, &vec![String::from("toy_tool")])?;
        expect_equal(
            &definition.map(|definition| &definition.owner),
            &Some(&ToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: None,
            }),
        )?;
        expect_equal(
            &definition.map(|definition| definition.provider_visibility),
            &Some(ProviderToolVisibility::Visible),
        )?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_registers_toy_tool_from_stdout_jsonl() -> Result<(), String> {
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let ready = serde_json::json!({
            "type": "extension.ready",
            "protocol": "yach.extension-host.v2",
            "extension_id": "example.toy-tools"
        })
        .to_string();
        let register = serde_json::json!({
            "type": "tool.register",
            "name": "toy_tool",
            "description": "Return static fixture metadata.",
            "risk": "reads_local_metadata",
            "provider_visible": true,
            "input_schema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["label"],
                "properties": {
                    "label": { "type": "string" }
                },
                "maxSerializedBytes": 512
            }
        })
        .to_string();

        let registered_tools = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![
                    String::from("-c"),
                    format!("printf '%s\\n' '{}' '{}'", ready, register),
                ],
                timeout: Duration::from_secs(1),
                max_stdout_bytes: 4096,
            },
            &mut registry,
        )
        .map_err(|error| format!("{error:?}"))?;

        let definition = registry.get("toy_tool");
        expect_equal(&registered_tools, &vec![String::from("toy_tool")])?;
        expect_equal(
            &definition.map(|definition| &definition.owner),
            &Some(&ToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
                extension_version: None,
            }),
        )?;
        expect_equal(
            &definition.map(|definition| definition.provider_visibility),
            &Some(ProviderToolVisibility::Visible),
        )?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn extension_host_env_does_not_inherit_parent_sentinel() -> Result<(), String> {
        let _guard = extension_host_env_lock()?;
        unsafe {
            std::env::set_var(
                "YACH_TEST_EXTENSION_HOST_SECRET_SENTINEL",
                "provider-secret-fixture",
            );
        }
        let result = (|| {
            let mut registry = ToolRegistry::with_project_read_only_tools();
            let ready = serde_json::json!({
                "type": "extension.ready",
                "protocol": "yach.extension-host.v2",
                "extension_id": "example.toy-tools"
            })
            .to_string();
            let register = serde_json::json!({
                "type": "tool.register",
                "name": "toy_tool",
                "description": "Return static fixture metadata.",
                "risk": "reads_local_metadata",
                "provider_visible": true,
                "input_schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["label"],
                    "properties": {
                        "label": { "type": "string" }
                    },
                    "maxSerializedBytes": 512
                }
            })
            .to_string();

            let registered_tools = run_extension_host_registration_command(
                "example.toy-tools",
                &ExtensionHostCommand {
                    command: String::from("sh"),
                    args: vec![
                        String::from("-c"),
                        format!(
                            "if [ -n \"$YACH_TEST_EXTENSION_HOST_SECRET_SENTINEL\" ]; then exit 7; fi; if [ -z \"$PATH\" ]; then exit 8; fi; printf '%s\\n' '{}' '{}'",
                            ready, register
                        ),
                    ],
                    timeout: Duration::from_secs(1),
                    max_stdout_bytes: 4096,
                },
                &mut registry,
            )
            .map_err(|error| format!("{error:?}"))?;

            expect_equal(&registered_tools, &vec![String::from("toy_tool")])
        })();
        unsafe {
            std::env::remove_var("YACH_TEST_EXTENSION_HOST_SECRET_SENTINEL");
        }
        result
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_reports_exit_timeout_and_malformed_output() {
        let mut exited_registry = ToolRegistry::with_project_read_only_tools();
        let exited = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![String::from("-c"), String::from("exit 7")],
                timeout: Duration::from_secs(1),
                max_stdout_bytes: 4096,
            },
            &mut exited_registry,
        );
        assert_eq!(
            exited,
            Err(ExtensionHostProtocolError::HostExited { status: Some(7) })
        );

        let mut timed_out_registry = ToolRegistry::with_project_read_only_tools();
        let timed_out = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![String::from("-c"), String::from("sleep 2")],
                timeout: Duration::from_millis(20),
                max_stdout_bytes: 4096,
            },
            &mut timed_out_registry,
        );
        assert_eq!(timed_out, Err(ExtensionHostProtocolError::TimedOut));

        let mut malformed_registry = ToolRegistry::with_project_read_only_tools();
        let malformed = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![String::from("-c"), String::from("printf '%s\\n' not-json")],
                timeout: Duration::from_secs(1),
                max_stdout_bytes: 4096,
            },
            &mut malformed_registry,
        );
        assert_eq!(malformed, Err(ExtensionHostProtocolError::Malformed));
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_cleans_up_descendant_stdout_after_host_exit() {
        let marker = process_marker("host_exit_descendant_stdout");
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let exited = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![
                    String::from("-c"),
                    format!("sh -c 'sleep 5' {marker} & exit 7"),
                ],
                timeout: Duration::from_millis(200),
                max_stdout_bytes: 4096,
            },
            &mut registry,
        );

        assert_eq!(
            exited,
            Err(ExtensionHostProtocolError::HostExited { status: Some(7) })
        );
        assert_no_process_matching_marker(&marker);
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_cleans_up_descendant_stdout_after_timeout() {
        let marker = process_marker("timeout_descendant_stdout");
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let timed_out = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![
                    String::from("-c"),
                    format!("sh -c 'sleep 5' {marker} & wait"),
                ],
                timeout: Duration::from_millis(50),
                max_stdout_bytes: 4096,
            },
            &mut registry,
        );

        assert_eq!(timed_out, Err(ExtensionHostProtocolError::TimedOut));
        assert_no_process_matching_marker(&marker);
    }

    #[cfg(unix)]
    #[test]
    fn extension_process_host_reports_oversized_output_before_timeout() {
        let mut registry = ToolRegistry::with_project_read_only_tools();
        let oversized = run_extension_host_registration_command(
            "example.toy-tools",
            &ExtensionHostCommand {
                command: String::from("sh"),
                args: vec![
                    String::from("-c"),
                    String::from(
                        "i=0; while [ \"$i\" -lt 8192 ]; do printf 1234567890abcdef; i=$((i + 1)); done",
                    ),
                ],
                timeout: Duration::from_millis(100),
                max_stdout_bytes: 4,
            },
            &mut registry,
        );

        assert_eq!(
            oversized,
            Err(ExtensionHostProtocolError::OutputTooLarge { max_bytes: 4 })
        );
    }

    #[test]
    fn extension_host_registration_rejects_unsupported_schema_features() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v2",
                    "extension_id": "example.toy-tools"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": false,
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label"],
                        "properties": {
                            "label": { "type": "string" },
                            "note": { "type": "string" }
                        },
                        "maxSerializedBytes": 512
                    }
                }),
            ],
            &mut registry,
        );

        assert_eq!(
            registration,
            Err(ExtensionHostProtocolError::UnsupportedSchema)
        );
        assert!(registry.get("toy_tool").is_none());
    }

    #[test]
    fn extension_host_registration_is_atomic_when_later_message_fails() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v2",
                    "extension_id": "example.toy-tools"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_metadata",
                    "provider_visible": false,
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label"],
                        "properties": {
                            "label": { "type": "string" }
                        },
                        "maxSerializedBytes": 512
                    }
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "unsafe_tool",
                    "description": "Attempts unsupported access.",
                    "risk": "uses_network",
                    "provider_visible": false,
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label"],
                        "properties": {
                            "label": { "type": "string" }
                        },
                        "maxSerializedBytes": 512
                    }
                }),
            ],
            &mut registry,
        );

        assert_eq!(
            registration,
            Err(ExtensionHostProtocolError::UnsupportedRisk)
        );
        assert!(registry.get("toy_tool").is_none());
    }
    #[test]
    fn extension_host_registration_accepts_read_and_mutating_tools() -> Result<(), String> {
        let mut registry = ToolRegistry::with_project_read_only_and_agent_edit_tools();
        let schema = |field: &str| {
            serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": [field],
                "properties": {(field): { "type": "string" }},
                "maxSerializedBytes": 4096
            })
        };
        let registered = process_extension_registration_messages(
            "example.hashline",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v2",
                    "extension_id": "example.hashline"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "hashline_read",
                    "description": "Read text with hashline anchors.",
                    "risk": "reads_local_content",
                    "provider_visible": true,
                    "input_schema": schema("path")
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "hashline_edit",
                    "description": "Apply hashline edits.",
                    "risk": "mutates_local_state",
                    "provider_visible": true,
                    "input_schema": schema("patch")
                }),
            ],
            &mut registry,
        )
        .map_err(|error| format!("{error:?}"))?;

        expect_equal(
            &registered,
            &vec![String::from("hashline_read"), String::from("hashline_edit")],
        )?;
        expect_equal(
            &registry.get("hashline_read").map(|tool| tool.risk),
            &Some(ToolRisk::ReadsLocalContent),
        )?;
        expect_equal(
            &registry.get("hashline_edit").map(|tool| tool.risk),
            &Some(ToolRisk::MutatesLocalState),
        )
    }

    #[test]
    fn extension_host_registration_requires_ready_before_tool_register() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![serde_json::json!({
                "type": "tool.register",
                "name": "toy_tool",
                "description": "Return static fixture metadata.",
                "risk": "reads_local_metadata",
                "provider_visible": false,
                "input_schema": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["label"],
                    "properties": {
                        "label": { "type": "string" }
                    },
                    "maxSerializedBytes": 512
                }
            })],
            &mut registry,
        );

        assert_eq!(registration, Err(ExtensionHostProtocolError::MissingReady));
        assert!(registry.get("toy_tool").is_none());
    }

    #[test]
    fn extension_host_registration_rejects_unsupported_protocol() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![serde_json::json!({
                "type": "extension.ready",
                "protocol": "yach.extension-host.v1",
                "extension_id": "example.toy-tools"
            })],
            &mut registry,
        );

        assert_eq!(
            registration,
            Err(ExtensionHostProtocolError::UnsupportedProtocol)
        );
    }

    #[test]
    fn extension_host_registration_rejects_extension_id_mismatch() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![serde_json::json!({
                "type": "extension.ready",
                "protocol": "yach.extension-host.v2",
                "extension_id": "example.other-tools"
            })],
            &mut registry,
        );

        assert_eq!(
            registration,
            Err(ExtensionHostProtocolError::ExtensionIdMismatch)
        );
    }

    #[test]
    fn extension_host_registration_rejects_unsupported_risk() {
        let mut registry = ToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v2",
                    "extension_id": "example.toy-tools"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "uses_network",
                    "provider_visible": false,
                    "input_schema": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["label"],
                        "properties": {
                            "label": { "type": "string" }
                        },
                        "maxSerializedBytes": 512
                    }
                }),
            ],
            &mut registry,
        );

        assert_eq!(
            registration,
            Err(ExtensionHostProtocolError::UnsupportedRisk)
        );
        assert!(registry.get("toy_tool").is_none());
    }
}
