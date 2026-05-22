use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc::{self, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use serde::{Deserialize, Serialize};

use crate::{
    NativeToolDefinition, NativeToolInputSchema, NativeToolRegistrationError, NativeToolRegistry,
    ProviderToolVisibility,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionToolCandidate {
    pub extension_id: ExtensionId,
    pub tool: ExtensionToolContribution,
}

impl ExtensionToolCandidate {
    #[must_use]
    pub fn to_native_definition(&self) -> NativeToolDefinition {
        NativeToolDefinition::extension_metadata_tool(
            self.extension_id.0.clone(),
            self.tool.name.clone(),
            self.tool.description.clone(),
            NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
            if self.tool.provider_visible {
                ProviderToolVisibility::Visible
            } else {
                ProviderToolVisibility::Hidden
            },
        )
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
    UnsupportedRisk,
    UnsupportedSchema,
    SpawnFailed,
    HostExited { status: Option<i32> },
    TimedOut,
    OutputTooLarge { max_bytes: usize },
    ToolRegistration(NativeToolRegistrationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHostCommand {
    pub command: String,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
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
        },
    })
}

pub fn process_extension_registration_messages(
    expected_extension_id: &str,
    messages: Vec<serde_json::Value>,
    registry: &mut NativeToolRegistry,
) -> Result<Vec<String>, ExtensionHostProtocolError> {
    let mut ready = false;
    let mut registered_tools = Vec::new();
    let mut staged_definitions = Vec::new();

    for value in messages {
        let message =
            serde_json::from_value(value).map_err(|_| ExtensionHostProtocolError::Malformed)?;
        match message {
            RawExtensionHostMessage::Ready {
                protocol,
                extension_id,
            } => {
                if protocol != "yach.extension-host.v1" {
                    return Err(ExtensionHostProtocolError::UnsupportedProtocol);
                }
                if extension_id != expected_extension_id {
                    return Err(ExtensionHostProtocolError::ExtensionIdMismatch);
                }
                ready = true;
            }
            RawExtensionHostMessage::ToolRegister {
                name,
                description,
                risk,
                provider_visible,
                input_schema,
            } => {
                if !ready {
                    return Err(ExtensionHostProtocolError::MissingReady);
                }
                if risk != "reads_local_metadata" {
                    return Err(ExtensionHostProtocolError::UnsupportedRisk);
                }
                validate_tool_name(&name).map_err(|_| ExtensionHostProtocolError::Malformed)?;

                let definition = NativeToolDefinition::extension_metadata_tool(
                    expected_extension_id,
                    name.clone(),
                    description,
                    parse_extension_tool_input_schema(input_schema)?,
                    if provider_visible {
                        ProviderToolVisibility::Visible
                    } else {
                        ProviderToolVisibility::Hidden
                    },
                );
                staged_definitions.push(definition);
                registered_tools.push(name);
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
                NativeToolRegistrationError::DuplicateToolName {
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

pub fn run_extension_host_registration_command(
    extension_id: &str,
    command: &ExtensionHostCommand,
    registry: &mut NativeToolRegistry,
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
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_extension_host_process(_command: &mut Command) {}

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
) -> Result<NativeToolInputSchema, ExtensionHostProtocolError> {
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

    Ok(NativeToolInputSchema::string_object(
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

    use std::fmt::Debug;
    #[cfg(unix)]
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::NativeToolOwner;

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

    fn parse_valid_manifest(value: serde_json::Value) -> Result<ExtensionManifest, String> {
        parse_extension_manifest(value).map_err(|error| format!("{error:?}"))
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
                },
            })
        );
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
    fn extension_host_registers_toy_tool_after_ready_handshake() -> Result<(), String> {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registered_tools = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v1",
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
            &Some(&NativeToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
        let ready = serde_json::json!({
            "type": "extension.ready",
            "protocol": "yach.extension-host.v1",
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
            &Some(&NativeToolOwner::Extension {
                extension_id: String::from("example.toy-tools"),
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
    fn extension_process_host_reports_exit_timeout_and_malformed_output() {
        let mut exited_registry = NativeToolRegistry::with_project_read_only_tools();
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

        let mut timed_out_registry = NativeToolRegistry::with_project_read_only_tools();
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

        let mut malformed_registry = NativeToolRegistry::with_project_read_only_tools();
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v1",
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v1",
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
                    "risk": "reads_local_content",
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
    fn extension_host_registration_requires_ready_before_tool_register() {
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![serde_json::json!({
                "type": "extension.ready",
                "protocol": "yach.extension-host.v2",
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![serde_json::json!({
                "type": "extension.ready",
                "protocol": "yach.extension-host.v1",
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
        let mut registry = NativeToolRegistry::with_project_read_only_tools();

        let registration = process_extension_registration_messages(
            "example.toy-tools",
            vec![
                serde_json::json!({
                    "type": "extension.ready",
                    "protocol": "yach.extension-host.v1",
                    "extension_id": "example.toy-tools"
                }),
                serde_json::json!({
                    "type": "tool.register",
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "risk": "reads_local_content",
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
