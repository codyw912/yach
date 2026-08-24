use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    ProviderExtension, ProviderMessage, ProviderModel, ProviderToolCall, ResourceListPolicy,
    ResourcePathError, ResourceReadError, ResourceReadPolicy, ResourceRoot, ResourceSearchPolicy,
    SessionEvent, SessionId, SessionLog, ToolOutcome, ToolPayloadSummary, ToolRequestId, TurnId,
};

/// Risk class for yach-owned native tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    FixtureSafe,
    ReadsLocalMetadata,
    ReadsLocalContent,
    MutatesLocalState,
    UsesNetwork,
    RunsProcess,
}

/// Ownership boundary for a yach-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOwner {
    BuiltIn,
    Extension {
        extension_id: String,
        extension_version: Option<String>,
    },
}

/// Whether a native tool may be advertised to model providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderToolVisibility {
    Hidden,
    Visible,
}

/// Permission state assigned after validating a native tool request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermissionState {
    Allowed,
    Denied,
    NeedsApproval,
}

/// Normalized native tool validation/permission errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolError {
    UnknownTool,
    MalformedArguments,
    ArgumentsTooLarge,
    MissingRequiredField { field: String },
    InvalidFieldType { field: String },
    UnexpectedField { field: String },
    PermissionDenied,
}

/// Minimal allowlisted object schema for first native tool validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInputSchema {
    required_string_fields: BTreeSet<String>,
    optional_string_fields: BTreeSet<String>,
    optional_number_fields: BTreeSet<String>,
    max_serialized_bytes: usize,
}

impl ToolInputSchema {
    #[must_use]
    pub fn string_object(
        required: impl IntoIterator<Item = impl Into<String>>,
        optional: impl IntoIterator<Item = impl Into<String>>,
        max_serialized_bytes: usize,
    ) -> Self {
        Self {
            required_string_fields: required.into_iter().map(Into::into).collect(),
            optional_string_fields: optional.into_iter().map(Into::into).collect(),
            optional_number_fields: BTreeSet::new(),
            max_serialized_bytes,
        }
    }

    #[must_use]
    pub fn with_optional_number_fields(
        mut self,
        fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.optional_number_fields = fields.into_iter().map(Into::into).collect();
        self
    }

    pub fn validate(&self, arguments: &serde_json::Value) -> Result<(), ToolError> {
        let serialized_len = serde_json::to_vec(arguments)
            .map_err(|_| ToolError::MalformedArguments)?
            .len();
        if serialized_len > self.max_serialized_bytes {
            return Err(ToolError::ArgumentsTooLarge);
        }

        let Some(object) = arguments.as_object() else {
            return Err(ToolError::MalformedArguments);
        };

        for field in &self.required_string_fields {
            let Some(value) = object.get(field) else {
                return Err(ToolError::MissingRequiredField {
                    field: field.clone(),
                });
            };
            if !value.is_string() {
                return Err(ToolError::InvalidFieldType {
                    field: field.clone(),
                });
            }
        }

        for (field, value) in object {
            if self.optional_number_fields.contains(field) {
                if !value.is_u64() {
                    return Err(ToolError::InvalidFieldType {
                        field: field.clone(),
                    });
                }
                continue;
            }
            if !self.required_string_fields.contains(field)
                && !self.optional_string_fields.contains(field)
            {
                return Err(ToolError::UnexpectedField {
                    field: field.clone(),
                });
            }
            if !value.is_string() {
                return Err(ToolError::InvalidFieldType {
                    field: field.clone(),
                });
            }
        }

        Ok(())
    }

    pub fn to_provider_json_schema(
        &self,
        name: &str,
    ) -> Result<serde_json::Value, ProviderToolAdvertisingError> {
        let mut properties = serde_json::Map::new();
        for field in self
            .required_string_fields
            .iter()
            .chain(&self.optional_string_fields)
        {
            properties.insert(
                field.clone(),
                serde_json::json!({
                    "type": "string",
                    "description": provider_string_field_description(name, field),
                }),
            );
        }
        for field in &self.optional_number_fields {
            properties.insert(
                field.clone(),
                serde_json::json!({
                    "type": "number",
                    "description": provider_string_field_description(name, field),
                }),
            );
        }

        Ok(serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": self.required_string_fields.iter().cloned().collect::<Vec<_>>(),
            "additionalProperties": false
        }))
    }
}

fn provider_string_field_description(tool_name: &str, field: &str) -> String {
    match (tool_name, field) {
        ("project_path_info", "path") => String::from("Project-relative path to inspect."),
        ("read_text_file", "path") => {
            String::from("Project-relative UTF-8 text file path to read.")
        }
        ("search_project", "query") => {
            String::from("Literal text to search for in project UTF-8 files.")
        }
        ("list_project_paths", "path") => String::from("Project-relative directory path to list."),
        ("edit_text_file", "path") => {
            String::from("Project-relative UTF-8 text file path to edit.")
        }
        ("edit_text_file", "find") => {
            String::from("Exact text to replace. The match must be unique.")
        }
        ("edit_text_file", "replace") => String::from("Replacement text."),
        ("create_text_file", "path") => {
            String::from("Project-relative UTF-8 text file path to create.")
        }
        ("create_text_file", "content") => {
            String::from("Full content for the new UTF-8 text file.")
        }
        ("bash", "command") => String::from("Shell command line to run with bash -c."),
        ("bash", "timeout") => String::from(
            "Optional timeout in milliseconds; clamped to the configured maximum (default 120000).",
        ),
        ("bash", "workdir") => {
            String::from("Optional project-relative working directory. Use this instead of cd.")
        }
        _ => format!("{field} argument for {tool_name}."),
    }
}

/// Backend-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
    pub risk: ToolRisk,
    pub owner: ToolOwner,
    pub provider_visibility: ProviderToolVisibility,
}

impl ToolDefinition {
    #[must_use]
    pub fn fixture_echo_metadata() -> Self {
        Self {
            name: String::from("fixture_echo_metadata"),
            description: String::from("Fixture-safe tool that validates metadata arguments only."),
            input_schema: ToolInputSchema::string_object(["label"], ["note"], 1024),
            risk: ToolRisk::FixtureSafe,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Hidden,
        }
    }

    #[must_use]
    pub fn project_path_info() -> Self {
        Self {
            name: String::from("project_path_info"),
            description: String::from(
                "Return local-only project path metadata without reading file contents.",
            ),
            input_schema: ToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: ToolRisk::ReadsLocalMetadata,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn read_text_file() -> Self {
        Self {
            name: String::from("read_text_file"),
            description: String::from(
                "Read a bounded UTF-8 project file through yach-owned resource policy.",
            ),
            input_schema: ToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: ToolRisk::ReadsLocalContent,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn search_project() -> Self {
        Self {
            name: String::from("search_project"),
            description: String::from("Search bounded UTF-8 project files for a literal query."),
            input_schema: ToolInputSchema::string_object(
                ["query"],
                std::iter::empty::<&str>(),
                4 * 1024,
            ),
            risk: ToolRisk::ReadsLocalContent,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn list_project_paths() -> Self {
        Self {
            name: String::from("list_project_paths"),
            description: String::from(
                "List bounded immediate project directory entries without file bodies.",
            ),
            input_schema: ToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: ToolRisk::ReadsLocalContent,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn edit_text_file() -> Self {
        Self {
            name: String::from("edit_text_file"),
            description: String::from(
                "Replace exact text in an existing UTF-8 project file. Yach computes the current file hash before applying.",
            ),
            input_schema: ToolInputSchema::string_object(
                ["path", "find", "replace"],
                std::iter::empty::<&str>(),
                16 * 1024,
            ),
            risk: ToolRisk::MutatesLocalState,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn create_text_file() -> Self {
        Self {
            name: String::from("create_text_file"),
            description: String::from(
                "Create a new UTF-8 project file. Fails if the target already exists.",
            ),
            input_schema: ToolInputSchema::string_object(
                ["path", "content"],
                std::iter::empty::<&str>(),
                128 * 1024,
            ),
            risk: ToolRisk::MutatesLocalState,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn bash() -> Self {
        Self {
            name: String::from("bash"),
            description: String::from(
                "Run a shell command in the project (bash -c, own process group, per-command \
process: shell state does not persist between calls). Returns bounded merged \
stdout/stderr and the exit code; a nonzero exit is a normal result to reason about. \
Use the workdir parameter instead of cd. Avoid cat/grep/ls/find for project files; \
prefer read_text_file, search_project, and list_project_paths. Commands run after \
user review unless allowlisted in config.",
            ),
            input_schema: ToolInputSchema::string_object(["command"], ["workdir"], 16 * 1024)
                .with_optional_number_fields(["timeout"]),
            risk: ToolRisk::RunsProcess,
            owner: ToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn extension_metadata_tool(
        extension_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: ToolInputSchema,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self::extension_metadata_tool_with_version(
            extension_id,
            None::<String>,
            name,
            description,
            input_schema,
            provider_visibility,
        )
    }

    #[must_use]
    pub fn extension_metadata_tool_with_version(
        extension_id: impl Into<String>,
        extension_version: Option<impl Into<String>>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: ToolInputSchema,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self::extension_tool_with_version(
            extension_id,
            extension_version,
            name,
            description,
            input_schema,
            ToolRisk::ReadsLocalMetadata,
            provider_visibility,
        )
    }

    #[must_use]
    pub fn extension_tool_with_version(
        extension_id: impl Into<String>,
        extension_version: Option<impl Into<String>>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: ToolInputSchema,
        risk: ToolRisk,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            risk,
            owner: ToolOwner::Extension {
                extension_id: extension_id.into(),
                extension_version: extension_version.map(Into::into),
            },
            provider_visibility,
        }
    }
}

pub const PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY: &str = "yach.provider_tool_advertising.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAdvertisedToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderToolAdvertising {
    pub tools: Vec<ProviderAdvertisedToolSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderToolAdvertisingError {
    Malformed,
    EmptyTools,
    DuplicateExtension,
    DuplicateToolName { name: String },
    UnsupportedTool { name: String },
    UnsupportedRisk { name: String, risk: ToolRisk },
    UnsupportedSchema { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistrationError {
    DuplicateToolName { name: String },
    UnsupportedOwner { name: String },
    UnsupportedRisk { name: String, risk: ToolRisk },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResolutionMode {
    Deny,
    AliasOnly,
    ReplaceBuiltin,
    ReplaceBuiltinWithExtensionContract,
    DisableBuiltin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolReplacementSource {
    User,
    Profile,
    Project { trusted: bool },
    Ephemeral,
}

impl ToolReplacementSource {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Profile => "profile",
            Self::Project { .. } => "project",
            Self::Ephemeral => "ephemeral",
        }
    }

    #[must_use]
    pub const fn is_trusted(&self) -> bool {
        !matches!(self, Self::Project { trusted: false })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReplacementRule {
    pub builtin_name: String,
    pub extension_id: String,
    pub extension_tool: String,
    pub mode: ToolResolutionMode,
    pub source: ToolReplacementSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolReplacementPolicy {
    rules: Vec<ToolReplacementRule>,
}

impl ToolReplacementPolicy {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_rules(rules: impl IntoIterator<Item = ToolReplacementRule>) -> Self {
        Self {
            rules: rules.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn rules(&self) -> &[ToolReplacementRule] {
        &self.rules
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolResolutionError {
    MissingBuiltIn {
        name: String,
    },
    MissingExtensionTool {
        name: String,
    },
    ExtensionIdMismatch {
        expected: String,
        actual: String,
    },
    ReplacementLowersRisk {
        builtin_name: String,
        builtin_risk: ToolRisk,
        extension_tool: String,
        extension_risk: ToolRisk,
    },
    ReplacementSchemaMismatch {
        builtin_name: String,
        extension_tool: String,
    },
    UntrustedProjectReplacement {
        builtin_name: String,
    },
}

/// Provenance for a provider-turn resolved native tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolProvenance {
    BuiltIn,
    Extension {
        extension_id: String,
        extension_version: String,
    },
    ExtensionReplacement {
        extension_id: String,
        extension_version: String,
        replaced_builtin: String,
        replacement_source: String,
    },
}

/// Provider-turn tool entry after policy and route availability resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTool {
    pub provider_name: String,
    pub implementation_name: String,
    pub definition: ToolDefinition,
    pub provenance: ToolProvenance,
}

/// Snapshot of the tools visible and executable for one provider turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedToolCatalog {
    tools: Vec<ResolvedTool>,
}

impl ResolvedToolCatalog {
    #[must_use]
    pub fn new(tools: Vec<ResolvedTool>) -> Self {
        Self { tools }
    }

    #[must_use]
    pub fn tools(&self) -> &[ResolvedTool] {
        &self.tools
    }

    #[must_use]
    pub fn provider_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| {
                let mut definition = tool.definition.clone();
                definition.name.clone_from(&tool.provider_name);
                definition
            })
            .collect()
    }

    #[must_use]
    pub fn implementation_name_for_provider_tool(&self, provider_name: &str) -> Option<&str> {
        self.tools
            .iter()
            .find(|tool| tool.provider_name == provider_name)
            .map(|tool| tool.implementation_name.as_str())
    }

    #[must_use]
    pub fn resolved_tool(&self, provider_name: &str) -> Option<&ResolvedTool> {
        self.tools
            .iter()
            .find(|tool| tool.provider_name == provider_name)
    }
}

pub fn build_provider_tool_advertising_extension(
    tools: &[ToolDefinition],
) -> Result<ProviderExtension, ProviderToolAdvertisingError> {
    if tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }

    let mut names = BTreeSet::new();
    let mut advertised_tools = Vec::with_capacity(tools.len());
    for tool in tools {
        validate_unique_tool_name(&mut names, &tool.name)?;
        advertised_tools.push(project_provider_advertised_tool(tool)?);
    }

    let advertising = ProviderToolAdvertising {
        tools: advertised_tools,
    };
    let value =
        serde_json::to_value(advertising).map_err(|_| ProviderToolAdvertisingError::Malformed)?;
    Ok(ProviderExtension {
        key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
        value,
    })
}

pub fn build_project_path_info_provider_tool_advertising_extension()
-> Result<ProviderExtension, ProviderToolAdvertisingError> {
    build_provider_tool_advertising_extension(&[ToolDefinition::project_path_info()])
}

pub fn parse_provider_tool_advertising_extensions(
    extensions: &[ProviderExtension],
) -> Result<Option<ProviderToolAdvertising>, ProviderToolAdvertisingError> {
    parse_provider_tool_advertising_extensions_inner(extensions, false)
}

/// Parse dynamically replaced built-in contracts after the caller has matched
/// the serialized advertising extension against its in-process approval.
///
/// Structural schema validation still applies. This only relaxes the canonical
/// built-in description/schema equality required for unapproved requests.
pub(crate) fn parse_provider_tool_advertising_extensions_with_approved_contracts(
    extensions: &[ProviderExtension],
) -> Result<Option<ProviderToolAdvertising>, ProviderToolAdvertisingError> {
    parse_provider_tool_advertising_extensions_inner(extensions, true)
}

fn parse_provider_tool_advertising_extensions_inner(
    extensions: &[ProviderExtension],
    allow_replacement_contracts: bool,
) -> Result<Option<ProviderToolAdvertising>, ProviderToolAdvertisingError> {
    let mut parsed = None;
    for extension in extensions {
        if extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY {
            continue;
        }
        if parsed.is_some() {
            return Err(ProviderToolAdvertisingError::DuplicateExtension);
        }
        let advertising =
            serde_json::from_value::<ProviderToolAdvertising>(extension.value.clone())
                .map_err(|_| ProviderToolAdvertisingError::Malformed)?;
        validate_provider_tool_advertising(&advertising, allow_replacement_contracts)?;
        parsed = Some(advertising);
    }

    Ok(parsed)
}

#[must_use]
pub fn strip_provider_tool_advertising_extensions(
    extensions: Vec<ProviderExtension>,
) -> Vec<ProviderExtension> {
    extensions
        .into_iter()
        .filter(|extension| extension.key != PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY)
        .collect()
}

fn validate_provider_tool_advertising(
    advertising: &ProviderToolAdvertising,
    allow_replacement_contracts: bool,
) -> Result<(), ProviderToolAdvertisingError> {
    if advertising.tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }

    let mut names = BTreeSet::new();
    for tool in &advertising.tools {
        validate_unique_tool_name(&mut names, &tool.name)?;
        validate_provider_advertised_tool_schema(tool, allow_replacement_contracts)?;
    }
    Ok(())
}

fn validate_unique_tool_name(
    names: &mut BTreeSet<String>,
    name: &str,
) -> Result<(), ProviderToolAdvertisingError> {
    if !names.insert(String::from(name)) {
        return Err(ProviderToolAdvertisingError::DuplicateToolName {
            name: String::from(name),
        });
    }
    Ok(())
}

fn project_provider_advertised_tool(
    tool: &ToolDefinition,
) -> Result<ProviderAdvertisedToolSchema, ProviderToolAdvertisingError> {
    if tool.provider_visibility != ProviderToolVisibility::Visible {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }

    if tool.owner == ToolOwner::BuiltIn {
        match tool.name.as_str() {
            "project_path_info" => {
                if tool.risk != ToolRisk::ReadsLocalMetadata {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            "read_text_file" | "search_project" | "list_project_paths" => {
                if tool.risk != ToolRisk::ReadsLocalContent {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            "edit_text_file" | "create_text_file" => {
                if tool.risk != ToolRisk::MutatesLocalState {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            "bash" => {
                if tool.risk != ToolRisk::RunsProcess {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            _ => {
                return Err(ProviderToolAdvertisingError::UnsupportedTool {
                    name: tool.name.clone(),
                });
            }
        }

        if !is_canonical_builtin_provider_tool(tool) {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        }
    } else if !matches!(
        tool.risk,
        ToolRisk::ReadsLocalMetadata | ToolRisk::ReadsLocalContent | ToolRisk::MutatesLocalState
    ) {
        return Err(ProviderToolAdvertisingError::UnsupportedRisk {
            name: tool.name.clone(),
            risk: tool.risk,
        });
    }

    Ok(ProviderAdvertisedToolSchema {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.input_schema.to_provider_json_schema(&tool.name)?,
    })
}

fn is_canonical_builtin_provider_tool(tool: &ToolDefinition) -> bool {
    match tool.name.as_str() {
        "project_path_info" => {
            let canonical = ToolDefinition::project_path_info();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "read_text_file" => {
            let canonical = ToolDefinition::read_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "search_project" => {
            let canonical = ToolDefinition::search_project();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "list_project_paths" => {
            let canonical = ToolDefinition::list_project_paths();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "edit_text_file" => {
            let canonical = ToolDefinition::edit_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "create_text_file" => {
            let canonical = ToolDefinition::create_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "bash" => {
            let canonical = ToolDefinition::bash();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        _ => false,
    }
}

fn is_provider_advertising_routable(tool: &ToolDefinition) -> bool {
    project_provider_advertised_tool(tool).is_ok()
}

fn validate_provider_advertised_tool_schema(
    tool: &ProviderAdvertisedToolSchema,
    allow_replacement_contracts: bool,
) -> Result<(), ProviderToolAdvertisingError> {
    let Some(parameters) = tool.parameters.as_object() else {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    };
    let allowed_root_keys = ["additionalProperties", "properties", "required", "type"];
    if parameters.len() != allowed_root_keys.len()
        || !allowed_root_keys
            .iter()
            .all(|key| parameters.contains_key(*key))
    {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    if parameters.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    let Some(properties) = parameters
        .get("properties")
        .and_then(serde_json::Value::as_object)
    else {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    };
    for property in properties.values() {
        let Some(property) = property.as_object() else {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        };
        let allowed_property_keys = ["description", "type"];
        if property.len() != allowed_property_keys.len()
            || !allowed_property_keys
                .iter()
                .all(|key| property.contains_key(*key))
            || !matches!(
                property.get("type").and_then(serde_json::Value::as_str),
                Some("string" | "number")
            )
            || property
                .get("description")
                .and_then(serde_json::Value::as_str)
                .is_none()
        {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        }
    }

    let Some(required) = parameters
        .get("required")
        .and_then(serde_json::Value::as_array)
    else {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    };
    let mut required_fields = BTreeSet::new();
    for field in required {
        let Some(field) = field.as_str() else {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        };
        if !required_fields.insert(field) || !properties.contains_key(field) {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        }
    }
    if parameters.get("additionalProperties") != Some(&serde_json::json!(false)) {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    if !allow_replacement_contracts {
        let canonical = match tool.name.as_str() {
            "project_path_info" => Some(ToolDefinition::project_path_info()),
            "read_text_file" => Some(ToolDefinition::read_text_file()),
            "search_project" => Some(ToolDefinition::search_project()),
            "list_project_paths" => Some(ToolDefinition::list_project_paths()),
            "edit_text_file" => Some(ToolDefinition::edit_text_file()),
            "create_text_file" => Some(ToolDefinition::create_text_file()),
            _ => None,
        };
        if let Some(canonical) = canonical
            && (tool.description != canonical.description
                || tool.parameters
                    != canonical
                        .input_schema
                        .to_provider_json_schema(&canonical.name)?)
        {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: tool.name.clone(),
            });
        }
    }

    Ok(())
}

/// Yach-owned pending native tool request derived from provider/tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingToolRequest {
    pub request_id: String,
    pub turn_id: TurnId,
    pub tool_name: String,
    pub provider_call_id: Option<String>,
    pub arguments: serde_json::Value,
}

/// Result of validating and authorizing a pending native tool request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolValidation {
    pub request_id: String,
    pub tool_name: String,
    pub permission: ToolPermissionState,
}

/// Backend-internal native tool execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResult {
    pub request_id: String,
    pub summary: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
}

/// Provider-bound yach-owned tool result after validation/execution/redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolResult {
    pub tool_request_id: String,
    pub provider_call_id: Option<String>,
    pub status: ToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Backend-owned request for a provider continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationRequest {
    pub turn_id: TurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<ProviderToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-independent adapter submission for a validated continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationSubmission {
    pub turn_id: TurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<ProviderContinuationToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-bound tool result normalized for adapter continuation mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationToolResult {
    pub tool_request_id: String,
    pub provider_call_id: String,
    pub status: ToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Adapter-independent provider continuation validation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Independent allow/deny switches, not encodable states of one machine.
#[expect(clippy::struct_excessive_bools)]
pub struct ProviderContinuationValidationPolicy {
    pub require_provider_call_id: bool,
    pub max_result_content_bytes: usize,
    pub allow_redacted_results: bool,
    pub allow_truncated_results: bool,
    /// Allow `Failed` tool results to flow back to the provider so the model
    /// can react to recoverable failures instead of the turn aborting.
    /// Denied, cancelled, and validation-failed results remain unsupported.
    pub allow_failed_results: bool,
}

impl ProviderContinuationValidationPolicy {
    #[must_use]
    pub const fn strict_tool_results(max_result_content_bytes: usize) -> Self {
        Self {
            require_provider_call_id: true,
            max_result_content_bytes,
            allow_redacted_results: true,
            allow_truncated_results: false,
            allow_failed_results: false,
        }
    }

    #[must_use]
    pub const fn agent_tool_results(max_result_content_bytes: usize) -> Self {
        Self {
            require_provider_call_id: true,
            max_result_content_bytes,
            allow_redacted_results: true,
            // Truncation is a designed result shape, not an error: the bash
            // tool's bounded head+tail capture flags truncated=true for any
            // long command output, and the model reasons about it. Rejecting
            // it here killed a dogfood turn the moment a command printed
            // more than the capture budget.
            allow_truncated_results: true,
            allow_failed_results: true,
        }
    }
}

/// Adapter-independent provider continuation validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderContinuationValidationError {
    MissingProviderCallId {
        tool_request_id: String,
    },
    ResultContentTooLarge {
        tool_request_id: String,
        max_bytes: usize,
        actual_bytes: usize,
    },
    RedactedResultRejected {
        tool_request_id: String,
    },
    TruncatedResultRejected {
        tool_request_id: String,
    },
}

/// Fail-closed errors while preparing adapter continuation input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderContinuationMappingError {
    Validation(ProviderContinuationValidationError),
    EmptyToolResults,
    UnsupportedToolResultStatus {
        tool_request_id: String,
        status: ToolOutcome,
    },
}

/// Session/turn context for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContinuationContext {
    pub session_id: SessionId,
    pub turn_id: TurnId,
}

/// Limits for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolContinuationPolicy {
    pub max_tool_calls: usize,
    pub max_result_bytes: usize,
}

impl ToolContinuationPolicy {
    #[must_use]
    pub const fn fixture_default() -> Self {
        Self {
            max_tool_calls: 4,
            max_result_bytes: 256,
        }
    }
}

/// Normalized continuation-loop errors before any real provider continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolContinuationError {
    TooManyToolCalls {
        max: usize,
        actual: usize,
    },
    Validation(ToolError),
    Execution(ToolExecutionError),
    ResultTooLarge {
        tool_call_id: String,
        max_bytes: usize,
        actual_bytes: usize,
    },
}

/// Normalized native tool execution errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionError {
    UnknownTool,
    PermissionDenied,
    UnsupportedTool,
    MalformedResult,
    ExtensionHost {
        error: crate::ExtensionHostProtocolError,
    },
    ResourceReadTooLarge,
    ResourceReadNotUtf8,
    ResourcePath {
        error: ResourcePathError,
    },
}

/// Backend-internal execution boundary for yach-owned native tools.
pub trait ToolExecutor {
    fn execute(
        &self,
        registry: &ToolRegistry,
        request: &PendingToolRequest,
        validation: &ToolValidation,
    ) -> Result<ToolExecutionResult, ToolExecutionError>;
}

/// Deep workflow for provider tool-call validation, execution, recording, and result building.
pub struct ToolContinuationWorkflow<'a, Executor>
where
    Executor: ToolExecutor,
{
    pub registry: &'a ToolRegistry,
    pub permission_policy: &'a ToolPermissionPolicy,
    pub executor: &'a Executor,
    pub continuation_policy: ToolContinuationPolicy,
}

impl<Executor> ToolContinuationWorkflow<'_, Executor>
where
    Executor: ToolExecutor,
{
    pub fn build_provider_tool_results(
        &self,
        log: &mut SessionLog,
        context: &ToolContinuationContext,
        tool_calls: Vec<ProviderToolCall>,
    ) -> Result<Vec<ProviderToolResult>, ToolContinuationError> {
        if tool_calls.len() > self.continuation_policy.max_tool_calls {
            return Err(ToolContinuationError::TooManyToolCalls {
                max: self.continuation_policy.max_tool_calls,
                actual: tool_calls.len(),
            });
        }

        let mut results = Vec::new();
        for (index, tool_call) in tool_calls.into_iter().enumerate() {
            let request = pending_tool_request_from_provider_call(
                format!("tool-request-{}", index + 1),
                context.turn_id.clone(),
                tool_call,
            );
            let validation = record_native_tool_validation(
                log,
                context.session_id.clone(),
                &request,
                self.registry,
                self.permission_policy,
            )
            .map_err(ToolContinuationError::Validation)?;
            let execution = match self.executor.execute(self.registry, &request, &validation) {
                Ok(execution) => execution,
                Err(error) => {
                    log.push(SessionEvent::ToolExecutionFinished {
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        tool_request_id: ToolRequestId(request.request_id.clone()),
                        outcome: ToolOutcome::Failed,
                        reason: Some(tool_execution_error_label(&error).to_string()),
                        result_summary: None,
                        result_content: None,
                    });
                    return Err(ToolContinuationError::Execution(error));
                }
            };
            if execution.byte_count > self.continuation_policy.max_result_bytes {
                log.push(SessionEvent::ToolExecutionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: ToolRequestId(request.request_id.clone()),
                    outcome: ToolOutcome::Failed,
                    reason: Some(String::from("result_too_large")),
                    result_summary: None,
                    result_content: None,
                });
                return Err(ToolContinuationError::ResultTooLarge {
                    tool_call_id: request
                        .provider_call_id
                        .clone()
                        .unwrap_or_else(|| request.request_id.clone()),
                    max_bytes: self.continuation_policy.max_result_bytes,
                    actual_bytes: execution.byte_count,
                });
            }

            let result_summary = provider_tool_result_summary(&request.tool_name, &execution);
            log.push(SessionEvent::ToolExecutionFinished {
                session_id: context.session_id.clone(),
                turn_id: context.turn_id.clone(),
                tool_request_id: ToolRequestId(request.request_id.clone()),
                outcome: ToolOutcome::Completed,
                reason: None,
                result_summary: Some(result_summary),
                result_content: Some(execution.summary.clone()),
            });
            results.push(ProviderToolResult {
                tool_request_id: request.request_id,
                provider_call_id: request.provider_call_id,
                status: ToolOutcome::Completed,
                content: execution.summary,
                byte_count: execution.byte_count,
                redacted: execution.redacted,
                truncated: execution.truncated,
                reason: None,
            });
        }

        Ok(results)
    }
}

/// Fixture-only native tool executor used to prove the execution boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixtureToolExecutor;

impl ToolExecutor for FixtureToolExecutor {
    fn execute(
        &self,
        registry: &ToolRegistry,
        request: &PendingToolRequest,
        validation: &ToolValidation,
    ) -> Result<ToolExecutionResult, ToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(ToolExecutionError::UnknownTool);
        };
        if validation.permission != ToolPermissionState::Allowed {
            return Err(ToolExecutionError::PermissionDenied);
        }
        if definition.name != "fixture_echo_metadata" || definition.risk != ToolRisk::FixtureSafe {
            return Err(ToolExecutionError::UnsupportedTool);
        }

        let byte_count = serde_json::to_vec(&request.arguments).map_or(0, |bytes| bytes.len());
        Ok(ToolExecutionResult {
            request_id: request.request_id.clone(),
            summary: String::from("fixture tool executed with redacted arguments"),
            byte_count,
            redacted: true,
            truncated: false,
        })
    }
}

const PROVIDER_READ_TEXT_MAX_BYTES: u64 = 32 * 1024;
const PROVIDER_SEARCH_MAX_FILE_BYTES: u64 = 64 * 1024;
const PROVIDER_SEARCH_MAX_FILES: usize = 512;
const PROVIDER_SEARCH_MAX_MATCHES: usize = 64;
const PROVIDER_SEARCH_LINE_MAX_BYTES: usize = 240;
const PROVIDER_LIST_MAX_ENTRIES: usize = 200;

/// Read-only project tool executor for local metadata and content tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReadOnlyToolExecutor {
    root: Option<ResourceRoot>,
}

impl ProjectReadOnlyToolExecutor {
    #[must_use]
    pub fn new(root: ResourceRoot) -> Self {
        Self { root: Some(root) }
    }

    #[must_use]
    pub fn unavailable_root() -> Self {
        Self { root: None }
    }
}

impl ToolExecutor for ProjectReadOnlyToolExecutor {
    fn execute(
        &self,
        registry: &ToolRegistry,
        request: &PendingToolRequest,
        validation: &ToolValidation,
    ) -> Result<ToolExecutionResult, ToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(ToolExecutionError::UnknownTool);
        };
        if validation.permission != ToolPermissionState::Allowed {
            return Err(ToolExecutionError::PermissionDenied);
        }
        let Some(root) = &self.root else {
            return Err(ToolExecutionError::UnsupportedTool);
        };
        match definition.name.as_str() {
            "project_path_info" if definition.risk == ToolRisk::ReadsLocalMetadata => {
                execute_project_path_info(root, request)
            }
            "read_text_file" if definition.risk == ToolRisk::ReadsLocalContent => {
                execute_read_text_file(root, request)
            }
            "search_project" if definition.risk == ToolRisk::ReadsLocalContent => {
                execute_search_project(root, request)
            }
            "list_project_paths" if definition.risk == ToolRisk::ReadsLocalContent => {
                execute_list_project_paths(root, request)
            }
            _ => Err(ToolExecutionError::UnsupportedTool),
        }
    }
}

fn execute_project_path_info(
    root: &ResourceRoot,
    request: &PendingToolRequest,
) -> Result<ToolExecutionResult, ToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let metadata = root
        .path_metadata(path)
        .map_err(|error| ToolExecutionError::ResourcePath { error })?;
    let summary = match metadata.byte_size {
        Some(bytes) => format!(
            "{}: {}, {bytes} bytes",
            metadata.relative_path,
            resource_entry_kind_label(metadata.kind)
        ),
        None => format!(
            "{}: {}",
            metadata.relative_path,
            resource_entry_kind_label(metadata.kind)
        ),
    };
    Ok(ToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: false,
    })
}

fn execute_read_text_file(
    root: &ResourceRoot,
    request: &PendingToolRequest,
) -> Result<ToolExecutionResult, ToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let read = root
        .read_text_file(
            &path,
            ResourceReadPolicy::local_only(PROVIDER_READ_TEXT_MAX_BYTES),
        )
        .map_err(|error| read_error_to_execution_error(&error))?;
    let summary = if read.text.is_empty() {
        // An empty tool-result string is ambiguous (blank file or
        // missing result?) and some provider shapes handle it poorly,
        // so the one read notice marks it explicitly.
        crate::tool_text::notice("empty file")
    } else {
        read.text
    };
    Ok(ToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: false,
    })
}

fn execute_search_project(
    root: &ResourceRoot,
    request: &PendingToolRequest,
) -> Result<ToolExecutionResult, ToolExecutionError> {
    let query = required_string_argument(request, "query")?;
    let result = root
        .search_text(
            &query,
            ResourceSearchPolicy {
                max_file_bytes: PROVIDER_SEARCH_MAX_FILE_BYTES,
                max_files: PROVIDER_SEARCH_MAX_FILES,
                max_matches: PROVIDER_SEARCH_MAX_MATCHES,
            },
        )
        .map_err(|error| ToolExecutionError::ResourcePath { error })?;
    let mut line_truncated = false;
    let lines = result
        .matches
        .into_iter()
        .map(|matched| {
            let (line, truncated) = bounded_provider_line(&matched.line);
            line_truncated |= truncated;
            let ellipsis = if truncated { "…" } else { "" };
            format!(
                "{}:{}: {line}{ellipsis}",
                matched.relative_path, matched.line_number
            )
        })
        .collect::<Vec<_>>();
    let notices = search_result_notices(
        lines.is_empty(),
        result.searched_files,
        result.truncated,
        result.denied_paths_excluded,
    );
    let truncated = result.truncated || line_truncated;
    let summary = crate::tool_text::append_notices(&lines.join("\n"), &notices);
    Ok(ToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated,
    })
}

fn search_result_notices(
    no_matches: bool,
    searched_files: usize,
    truncated: bool,
    denied_paths_excluded: bool,
) -> Vec<String> {
    let mut notices = Vec::new();
    if no_matches {
        if truncated {
            notices.push(crate::tool_text::notice(
                "search incomplete: file budget exhausted before any matches; narrow the path or pattern",
            ));
        } else {
            notices.push(crate::tool_text::notice(&format!(
                "no matches; {searched_files} files searched"
            )));
        }
    } else if truncated {
        notices.push(crate::tool_text::notice(
            "results incomplete (budget exhausted)",
        ));
    }
    if denied_paths_excluded {
        notices.push(crate::tool_text::notice("some paths excluded by policy"));
    }
    notices
}

fn execute_list_project_paths(
    root: &ResourceRoot,
    request: &PendingToolRequest,
) -> Result<ToolExecutionResult, ToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let result = root
        .list_paths(
            &path,
            ResourceListPolicy {
                max_entries: PROVIDER_LIST_MAX_ENTRIES,
            },
        )
        .map_err(|error| ToolExecutionError::ResourcePath { error })?;
    let lines = result
        .entries
        .into_iter()
        .map(|entry| match entry.kind {
            crate::ResourceEntryKind::Directory => format!("{}/", entry.relative_path),
            _ => match entry.byte_size {
                Some(bytes) => format!("{}  {bytes} bytes", entry.relative_path),
                None => entry.relative_path,
            },
        })
        .collect::<Vec<_>>();
    let mut notices = Vec::new();
    if lines.is_empty() {
        notices.push(crate::tool_text::notice("empty directory"));
    }
    if result.truncated {
        notices.push(crate::tool_text::notice("truncated: entry limit reached"));
    }
    if result.denied_paths_excluded {
        notices.push(crate::tool_text::notice("some paths excluded by policy"));
    }
    let summary = crate::tool_text::append_notices(&lines.join("\n"), &notices);
    Ok(ToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: result.truncated,
    })
}

fn required_string_argument(
    request: &PendingToolRequest,
    field: &str,
) -> Result<String, ToolExecutionError> {
    request
        .arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(ToolExecutionError::MalformedResult)
}

fn resource_entry_kind_label(kind: crate::ResourceEntryKind) -> &'static str {
    match kind {
        crate::ResourceEntryKind::File => "file",
        crate::ResourceEntryKind::Directory => "directory",
        crate::ResourceEntryKind::Other => "other",
    }
}

fn bounded_provider_line(value: &str) -> (String, bool) {
    if value.len() <= PROVIDER_SEARCH_LINE_MAX_BYTES {
        return (value.to_owned(), false);
    }

    let mut end = 0;
    for (index, _) in value.char_indices() {
        if index > PROVIDER_SEARCH_LINE_MAX_BYTES {
            break;
        }
        end = index;
    }
    if end == 0 {
        return (String::new(), true);
    }
    (value[..end].to_owned(), true)
}

fn read_error_to_execution_error(error: &ResourceReadError) -> ToolExecutionError {
    match error {
        ResourceReadError::Path(error) => ToolExecutionError::ResourcePath { error: *error },
        ResourceReadError::TooLarge { .. } => ToolExecutionError::ResourceReadTooLarge,
        ResourceReadError::NotUtf8 => ToolExecutionError::ResourceReadNotUtf8,
        ResourceReadError::Io => ToolExecutionError::MalformedResult,
    }
}

fn provider_tool_result_summary(
    tool_name: &str,
    execution: &ToolExecutionResult,
) -> ToolPayloadSummary {
    let summary = match tool_name {
        "read_text_file" => String::from("read_text_file result redacted"),
        "search_project" => crate::tool_text::content_line_count_summary(
            "search_project",
            "matches",
            &execution.summary,
            execution.truncated,
        ),
        "list_project_paths" => crate::tool_text::content_line_count_summary(
            "list_project_paths",
            "entries",
            &execution.summary,
            execution.truncated,
        ),
        _ => execution.summary.clone(),
    };
    ToolPayloadSummary {
        summary,
        byte_count: execution.byte_count,
        redacted: matches!(
            tool_name,
            "read_text_file" | "search_project" | "list_project_paths"
        ),
        truncated: execution.truncated,
    }
}

/// Extension tool handler used by native workflow routing.
#[derive(Clone)]
pub struct ExtensionToolHandler {
    extension_id: String,
    route: ExtensionToolRoute,
}

#[derive(Clone)]
enum ExtensionToolRoute {
    Static {
        response: String,
        malformed: bool,
    },
    Host {
        invoker: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
        timeout: Duration,
    },
}

impl std::fmt::Debug for ExtensionToolHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionToolHandler")
            .field("extension_id", &self.extension_id)
            .field("route", &self.route.kind())
            .finish()
    }
}

impl ExtensionToolRoute {
    const fn kind(&self) -> &'static str {
        match self {
            Self::Static { .. } => "static",
            Self::Host { .. } => "host",
        }
    }
}

impl ExtensionToolHandler {
    #[must_use]
    pub fn static_metadata(extension_id: impl Into<String>, response: impl Into<String>) -> Self {
        Self {
            extension_id: extension_id.into(),
            route: ExtensionToolRoute::Static {
                response: response.into(),
                malformed: false,
            },
        }
    }

    #[must_use]
    pub fn malformed_result(extension_id: impl Into<String>) -> Self {
        Self {
            extension_id: extension_id.into(),
            route: ExtensionToolRoute::Static {
                response: String::new(),
                malformed: true,
            },
        }
    }

    #[must_use]
    pub fn host_metadata(
        extension_id: impl Into<String>,
        invoker: impl crate::ExtensionHostInvoker + 'static,
        timeout: Duration,
    ) -> Self {
        Self {
            extension_id: extension_id.into(),
            route: ExtensionToolRoute::Host {
                invoker: Arc::new(Mutex::new(Box::new(invoker))),
                timeout,
            },
        }
    }

    #[must_use]
    pub fn shared_host_metadata(
        extension_id: impl Into<String>,
        invoker: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
        timeout: Duration,
    ) -> Self {
        Self {
            extension_id: extension_id.into(),
            route: ExtensionToolRoute::Host { invoker, timeout },
        }
    }
}

/// Extension-owned native tool executor router.
#[derive(Debug, Clone, Default)]
pub struct ExtensionToolExecutorRouter {
    handlers: BTreeMap<String, ExtensionToolHandler>,
}

impl ExtensionToolExecutorRouter {
    #[must_use]
    pub fn from_handlers(
        handlers: impl IntoIterator<Item = (impl Into<String>, ExtensionToolHandler)>,
    ) -> Self {
        Self {
            handlers: handlers
                .into_iter()
                .map(|(name, handler)| (name.into(), handler))
                .collect(),
        }
    }

    pub fn remove_tools<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            self.handlers.remove(name);
        }
    }

    pub fn insert_tool(&mut self, name: impl Into<String>, handler: ExtensionToolHandler) {
        self.handlers.insert(name.into(), handler);
    }

    #[must_use]
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionToolExecution {
    Result {
        result: ToolExecutionResult,
        status: crate::ExtensionToolResultStatus,
        reason: Option<String>,
    },
    EditProposal(crate::ExtensionEditProposal),
}

impl ExtensionToolExecutorRouter {
    pub fn execute_with_resources(
        &self,
        registry: &ToolRegistry,
        request: &PendingToolRequest,
        validation: &ToolValidation,
        resources: &dyn crate::ExtensionResourceBroker,
    ) -> Result<ExtensionToolExecution, ToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(ToolExecutionError::UnknownTool);
        };
        if validation.permission != ToolPermissionState::Allowed {
            return Err(ToolExecutionError::PermissionDenied);
        }
        let ToolOwner::Extension {
            extension_id: definition_extension_id,
            ..
        } = &definition.owner
        else {
            return Err(ToolExecutionError::UnsupportedTool);
        };
        let Some(handler) = self.handlers.get(&request.tool_name) else {
            return Err(ToolExecutionError::UnsupportedTool);
        };
        if handler.extension_id != *definition_extension_id {
            return Err(ToolExecutionError::UnsupportedTool);
        }
        match &handler.route {
            ExtensionToolRoute::Static {
                response,
                malformed,
            } => {
                if *malformed || serde_json::from_str::<serde_json::Value>(response).is_err() {
                    return Err(ToolExecutionError::MalformedResult);
                }
                Ok(ExtensionToolExecution::Result {
                    result: ToolExecutionResult {
                        request_id: request.request_id.clone(),
                        byte_count: response.len(),
                        summary: response.clone(),
                        redacted: false,
                        truncated: false,
                    },
                    status: crate::ExtensionToolResultStatus::Completed,
                    reason: None,
                })
            }
            ExtensionToolRoute::Host { invoker, timeout } => {
                let mut invoker =
                    invoker
                        .lock()
                        .map_err(|_| ToolExecutionError::ExtensionHost {
                            error: crate::ExtensionHostProtocolError::Malformed,
                        })?;
                match invoker
                    .invoke(
                        &request.request_id,
                        &request.tool_name,
                        request.arguments.clone(),
                        *timeout,
                        resources,
                    )
                    .map_err(|error| ToolExecutionError::ExtensionHost { error })?
                {
                    crate::ExtensionHostInvocation::ToolResult {
                        content,
                        status,
                        reason,
                    } => Ok(ExtensionToolExecution::Result {
                        result: ToolExecutionResult {
                            request_id: request.request_id.clone(),
                            byte_count: content.len(),
                            summary: content,
                            redacted: false,
                            truncated: false,
                        },
                        status,
                        reason,
                    }),
                    crate::ExtensionHostInvocation::EditProposal(proposal) => {
                        Ok(ExtensionToolExecution::EditProposal(proposal))
                    }
                }
            }
        }
    }
}

impl ToolExecutor for ExtensionToolExecutorRouter {
    fn execute(
        &self,
        registry: &ToolRegistry,
        request: &PendingToolRequest,
        validation: &ToolValidation,
    ) -> Result<ToolExecutionResult, ToolExecutionError> {
        match self.execute_with_resources(
            registry,
            request,
            validation,
            &crate::DenyExtensionResources,
        )? {
            ExtensionToolExecution::Result {
                result,
                status: crate::ExtensionToolResultStatus::Completed,
                ..
            } => Ok(result),
            ExtensionToolExecution::Result {
                status: crate::ExtensionToolResultStatus::Failed,
                ..
            }
            | ExtensionToolExecution::EditProposal(_) => Err(ToolExecutionError::MalformedResult),
        }
    }
}

/// Explicit allowlist policy for first native tool slices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolPermissionPolicy {
    fixture_execution: BTreeSet<String>,
    metadata_advertising: BTreeSet<String>,
    content_advertising: BTreeSet<String>,
    agent_edit_advertising: BTreeSet<String>,
    process_execution: BTreeSet<String>,
}

impl ToolPermissionPolicy {
    #[must_use]
    pub fn deny_all() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn allow_fixture_tool(name: impl Into<String>) -> Self {
        Self {
            fixture_execution: BTreeSet::from([name.into()]),
            metadata_advertising: BTreeSet::new(),
            content_advertising: BTreeSet::new(),
            agent_edit_advertising: BTreeSet::new(),
            process_execution: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn allow_project_metadata_tool(name: impl Into<String>) -> Self {
        Self::allow_project_metadata_tools([name])
    }

    #[must_use]
    pub fn allow_project_metadata_tools(
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            fixture_execution: BTreeSet::new(),
            metadata_advertising: names.into_iter().map(Into::into).collect(),
            content_advertising: BTreeSet::new(),
            agent_edit_advertising: BTreeSet::new(),
            process_execution: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn allow_project_metadata_and_agent_edit_tools(
        metadata_names: impl IntoIterator<Item = impl Into<String>>,
        edit_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            fixture_execution: BTreeSet::new(),
            metadata_advertising: metadata_names.into_iter().map(Into::into).collect(),
            content_advertising: BTreeSet::new(),
            agent_edit_advertising: edit_names.into_iter().map(Into::into).collect(),
            process_execution: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn allow_project_metadata_content_and_agent_edit_tools(
        metadata_names: impl IntoIterator<Item = impl Into<String>>,
        content_names: impl IntoIterator<Item = impl Into<String>>,
        edit_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            fixture_execution: BTreeSet::new(),
            metadata_advertising: metadata_names.into_iter().map(Into::into).collect(),
            content_advertising: content_names.into_iter().map(Into::into).collect(),
            agent_edit_advertising: edit_names.into_iter().map(Into::into).collect(),
            process_execution: BTreeSet::new(),
        }
    }

    /// Add process-execution tools (the `bash` tool) to the allow policy.
    #[must_use]
    pub fn with_process_tools(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.process_execution = names.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn authorize(&self, definition: &ToolDefinition) -> ToolPermissionState {
        let allowed = match definition.risk {
            ToolRisk::FixtureSafe => self.fixture_execution.contains(&definition.name),
            ToolRisk::ReadsLocalMetadata => self.metadata_advertising.contains(&definition.name),
            ToolRisk::ReadsLocalContent => self.content_advertising.contains(&definition.name),
            ToolRisk::RunsProcess => self.process_execution.contains(&definition.name),
            ToolRisk::MutatesLocalState | ToolRisk::UsesNetwork => false,
        };

        if allowed {
            ToolPermissionState::Allowed
        } else {
            ToolPermissionState::Denied
        }
    }

    #[must_use]
    pub fn allows_provider_advertising(&self, definition: &ToolDefinition) -> bool {
        match definition.risk {
            ToolRisk::ReadsLocalMetadata => self.metadata_advertising.contains(&definition.name),
            ToolRisk::ReadsLocalContent => self.content_advertising.contains(&definition.name),
            ToolRisk::MutatesLocalState => self.agent_edit_advertising.contains(&definition.name),
            ToolRisk::RunsProcess => self.process_execution.contains(&definition.name),
            ToolRisk::FixtureSafe | ToolRisk::UsesNetwork => false,
        }
    }
}

/// Backend-owned native tool registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolRegistry {
    definitions: Vec<ToolDefinition>,
}

impl ToolRegistry {
    #[must_use]
    pub fn with_fixture_tools() -> Self {
        Self {
            definitions: vec![ToolDefinition::fixture_echo_metadata()],
        }
    }

    #[must_use]
    pub fn with_project_read_only_tools() -> Self {
        Self {
            definitions: vec![
                ToolDefinition::project_path_info(),
                ToolDefinition::read_text_file(),
                ToolDefinition::search_project(),
                ToolDefinition::list_project_paths(),
            ],
        }
    }

    #[must_use]
    pub fn with_agent_edit_tools() -> Self {
        Self {
            definitions: vec![
                ToolDefinition::edit_text_file(),
                ToolDefinition::create_text_file(),
            ],
        }
    }

    #[must_use]
    pub fn with_project_read_only_and_agent_edit_tools() -> Self {
        Self {
            definitions: vec![
                ToolDefinition::project_path_info(),
                ToolDefinition::read_text_file(),
                ToolDefinition::search_project(),
                ToolDefinition::list_project_paths(),
                ToolDefinition::edit_text_file(),
                ToolDefinition::create_text_file(),
                ToolDefinition::bash(),
            ],
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    #[must_use]
    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub fn register_extension_tool(
        &mut self,
        definition: ToolDefinition,
    ) -> Result<(), ToolRegistrationError> {
        if self.get(&definition.name).is_some() {
            return Err(ToolRegistrationError::DuplicateToolName {
                name: definition.name,
            });
        }

        if !matches!(&definition.owner, ToolOwner::Extension { .. }) {
            return Err(ToolRegistrationError::UnsupportedOwner {
                name: definition.name,
            });
        }

        if !matches!(
            definition.risk,
            ToolRisk::ReadsLocalMetadata
                | ToolRisk::ReadsLocalContent
                | ToolRisk::MutatesLocalState
        ) {
            return Err(ToolRegistrationError::UnsupportedRisk {
                name: definition.name,
                risk: definition.risk,
            });
        }

        self.definitions.push(definition);
        Ok(())
    }

    pub fn remove_extension_tools(&mut self, extension_id: &str) -> Vec<String> {
        let mut removed_tools = Vec::new();
        self.definitions.retain(|definition| {
            if matches!(
                &definition.owner,
                ToolOwner::Extension {
                    extension_id: owner_extension_id,
                    ..
                } if owner_extension_id == extension_id
            ) {
                removed_tools.push(definition.name.clone());
                false
            } else {
                true
            }
        });
        removed_tools
    }

    #[must_use]
    pub fn provider_advertising_candidates<'a>(
        &self,
        policy: &ToolPermissionPolicy,
        routable_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<ToolDefinition> {
        self.resolve_provider_turn_catalog(policy, routable_tools)
            .provider_definitions()
    }

    #[must_use]
    pub fn resolve_provider_turn_catalog<'a>(
        &self,
        policy: &ToolPermissionPolicy,
        executable_tools: impl IntoIterator<Item = &'a str>,
    ) -> ResolvedToolCatalog {
        let executable_tools = executable_tools.into_iter().collect::<BTreeSet<_>>();
        let tools = self
            .definitions
            .iter()
            .filter(|definition| {
                definition.provider_visibility == ProviderToolVisibility::Visible
                    && policy.allows_provider_advertising(definition)
                    && executable_tools.contains(definition.name.as_str())
                    && is_provider_advertising_routable(definition)
            })
            .map(|definition| ResolvedTool {
                provider_name: definition.name.clone(),
                implementation_name: definition.name.clone(),
                definition: definition.clone(),
                provenance: tool_provenance(definition),
            })
            .collect();
        ResolvedToolCatalog::new(tools)
    }

    pub fn resolve_provider_turn_catalog_with_replacements<'a>(
        &self,
        policy: &ToolPermissionPolicy,
        executable_tools: impl IntoIterator<Item = &'a str>,
        replacement_policy: &ToolReplacementPolicy,
    ) -> Result<ResolvedToolCatalog, ToolResolutionError> {
        let executable_tools = executable_tools.into_iter().collect::<BTreeSet<_>>();
        let mut disabled_builtins = BTreeSet::new();
        let mut replaced_builtins = BTreeSet::new();
        let mut replacement_implementations = BTreeSet::new();
        let mut denied_extension_tools = BTreeSet::new();
        let mut replacement_tools = Vec::new();

        for rule in replacement_policy.rules() {
            validate_replacement_rule_source(rule)?;
            match rule.mode {
                ToolResolutionMode::Deny => {
                    denied_extension_tools.insert(rule.extension_tool.as_str());
                }
                ToolResolutionMode::AliasOnly => {}
                ToolResolutionMode::DisableBuiltin => {
                    let builtin = self.builtin_definition(&rule.builtin_name)?;
                    disabled_builtins.insert(builtin.name.as_str());
                }
                ToolResolutionMode::ReplaceBuiltin
                | ToolResolutionMode::ReplaceBuiltinWithExtensionContract => {
                    let builtin = self.builtin_definition(&rule.builtin_name)?;
                    let extension = self.extension_definition(rule)?;
                    if rule.mode == ToolResolutionMode::ReplaceBuiltin {
                        validate_replacement_shape(builtin, extension, rule)?;
                    } else if builtin.risk != extension.risk {
                        return Err(ToolResolutionError::ReplacementLowersRisk {
                            builtin_name: builtin.name.clone(),
                            builtin_risk: builtin.risk,
                            extension_tool: extension.name.clone(),
                            extension_risk: extension.risk,
                        });
                    }
                    let mut provider_definition =
                        if rule.mode == ToolResolutionMode::ReplaceBuiltinWithExtensionContract {
                            extension.clone()
                        } else {
                            builtin.clone()
                        };
                    provider_definition.name.clone_from(&builtin.name);
                    if executable_tools.contains(extension.name.as_str())
                        && policy.allows_provider_advertising(&provider_definition)
                        && is_provider_advertising_routable(&provider_definition)
                    {
                        replaced_builtins.insert(builtin.name.as_str());
                        replacement_implementations.insert(extension.name.as_str());
                        let extension_version = tool_extension_version(extension);
                        replacement_tools.push(ResolvedTool {
                            provider_name: builtin.name.clone(),
                            implementation_name: extension.name.clone(),
                            definition: provider_definition,
                            provenance: ToolProvenance::ExtensionReplacement {
                                extension_id: rule.extension_id.clone(),
                                extension_version,
                                replaced_builtin: builtin.name.clone(),
                                replacement_source: String::from(rule.source.label()),
                            },
                        });
                    }
                }
            }
        }

        let mut tools = self
            .definitions
            .iter()
            .filter(|definition| {
                if definition.owner == ToolOwner::BuiltIn
                    && (disabled_builtins.contains(definition.name.as_str())
                        || replaced_builtins.contains(definition.name.as_str()))
                {
                    return false;
                }
                if matches!(definition.owner, ToolOwner::Extension { .. })
                    && (denied_extension_tools.contains(definition.name.as_str())
                        || replacement_implementations.contains(definition.name.as_str()))
                {
                    return false;
                }
                definition.provider_visibility == ProviderToolVisibility::Visible
                    && policy.allows_provider_advertising(definition)
                    && executable_tools.contains(definition.name.as_str())
                    && is_provider_advertising_routable(definition)
            })
            .map(|definition| ResolvedTool {
                provider_name: definition.name.clone(),
                implementation_name: definition.name.clone(),
                definition: definition.clone(),
                provenance: tool_provenance(definition),
            })
            .collect::<Vec<_>>();
        tools.extend(replacement_tools);
        tools.sort_by(|left, right| left.provider_name.cmp(&right.provider_name));
        Ok(ResolvedToolCatalog::new(tools))
    }

    pub fn validate_request_schema_only(
        &self,
        request: &PendingToolRequest,
    ) -> Result<&ToolDefinition, ToolError> {
        let definition = self.get(&request.tool_name).ok_or(ToolError::UnknownTool)?;
        definition.input_schema.validate(&request.arguments)?;
        Ok(definition)
    }

    pub fn validate_request(
        &self,
        request: &PendingToolRequest,
        policy: &ToolPermissionPolicy,
    ) -> Result<ToolValidation, ToolError> {
        let definition = self.get(&request.tool_name).ok_or(ToolError::UnknownTool)?;
        definition.input_schema.validate(&request.arguments)?;
        let permission = policy.authorize(definition);
        if permission == ToolPermissionState::Denied {
            return Err(ToolError::PermissionDenied);
        }

        Ok(ToolValidation {
            request_id: request.request_id.clone(),
            tool_name: request.tool_name.clone(),
            permission,
        })
    }

    fn builtin_definition(&self, name: &str) -> Result<&ToolDefinition, ToolResolutionError> {
        let definition = self
            .get(name)
            .ok_or_else(|| ToolResolutionError::MissingBuiltIn {
                name: String::from(name),
            })?;
        if definition.owner != ToolOwner::BuiltIn {
            return Err(ToolResolutionError::MissingBuiltIn {
                name: String::from(name),
            });
        }
        Ok(definition)
    }

    fn extension_definition(
        &self,
        rule: &ToolReplacementRule,
    ) -> Result<&ToolDefinition, ToolResolutionError> {
        let definition = self.get(&rule.extension_tool).ok_or_else(|| {
            ToolResolutionError::MissingExtensionTool {
                name: rule.extension_tool.clone(),
            }
        })?;
        let ToolOwner::Extension { extension_id, .. } = &definition.owner else {
            return Err(ToolResolutionError::MissingExtensionTool {
                name: rule.extension_tool.clone(),
            });
        };
        if extension_id != &rule.extension_id {
            return Err(ToolResolutionError::ExtensionIdMismatch {
                expected: rule.extension_id.clone(),
                actual: extension_id.clone(),
            });
        }
        Ok(definition)
    }
}

fn validate_replacement_rule_source(rule: &ToolReplacementRule) -> Result<(), ToolResolutionError> {
    if rule.source.is_trusted() {
        Ok(())
    } else {
        Err(ToolResolutionError::UntrustedProjectReplacement {
            builtin_name: rule.builtin_name.clone(),
        })
    }
}

fn validate_replacement_shape(
    builtin: &ToolDefinition,
    extension: &ToolDefinition,
    rule: &ToolReplacementRule,
) -> Result<(), ToolResolutionError> {
    if builtin.risk != extension.risk {
        return Err(ToolResolutionError::ReplacementLowersRisk {
            builtin_name: builtin.name.clone(),
            builtin_risk: builtin.risk,
            extension_tool: extension.name.clone(),
            extension_risk: extension.risk,
        });
    }
    if builtin.input_schema != extension.input_schema {
        return Err(ToolResolutionError::ReplacementSchemaMismatch {
            builtin_name: builtin.name.clone(),
            extension_tool: rule.extension_tool.clone(),
        });
    }
    Ok(())
}

fn tool_provenance(definition: &ToolDefinition) -> ToolProvenance {
    match &definition.owner {
        ToolOwner::BuiltIn => ToolProvenance::BuiltIn,
        ToolOwner::Extension {
            extension_id,
            extension_version,
        } => ToolProvenance::Extension {
            extension_id: extension_id.clone(),
            extension_version: extension_version
                .clone()
                .unwrap_or_else(|| String::from("unknown")),
        },
    }
}

fn tool_extension_version(definition: &ToolDefinition) -> String {
    match &definition.owner {
        ToolOwner::Extension {
            extension_version, ..
        } => extension_version
            .clone()
            .unwrap_or_else(|| String::from("unknown")),
        ToolOwner::BuiltIn => String::from("unknown"),
    }
}

/// Build a yach-owned pending tool request from provider-emitted tool-call metadata.
#[must_use]
pub fn pending_tool_request_from_provider_call(
    request_id: impl Into<String>,
    turn_id: TurnId,
    tool_call: ProviderToolCall,
) -> PendingToolRequest {
    PendingToolRequest {
        request_id: request_id.into(),
        turn_id,
        tool_name: tool_call.name,
        provider_call_id: Some(tool_call.call_id),
        arguments: tool_call.arguments_json,
    }
}

/// Validate a pending tool request and append provisional redacted session records.
pub fn record_native_tool_validation(
    log: &mut SessionLog,
    session_id: SessionId,
    request: &PendingToolRequest,
    registry: &ToolRegistry,
    policy: &ToolPermissionPolicy,
) -> Result<ToolValidation, ToolError> {
    record_native_tool_validation_with_summary(
        log,
        session_id,
        request,
        registry,
        policy,
        summarize_tool_payload(&request.arguments),
    )
}

pub fn record_native_tool_validation_with_resolved_catalog(
    log: &mut SessionLog,
    session_id: SessionId,
    request: &PendingToolRequest,
    _registry: &ToolRegistry,
    policy: &ToolPermissionPolicy,
    catalog: &ResolvedToolCatalog,
) -> Result<ToolValidation, ToolError> {
    let mut summary = summarize_tool_payload(&request.arguments);
    let validation = catalog
        .resolved_tool(&request.tool_name)
        .ok_or(ToolError::UnknownTool)
        .and_then(|tool| {
            if let Some(provenance) = resolved_tool_provenance_summary(tool) {
                summary.summary = format!("{}; {provenance}", summary.summary);
            }
            tool.definition.input_schema.validate(&request.arguments)?;
            let permission = if tool.definition.risk == ToolRisk::MutatesLocalState
                && matches!(
                    tool.provenance,
                    ToolProvenance::Extension { .. } | ToolProvenance::ExtensionReplacement { .. }
                )
                && policy.allows_provider_advertising(&tool.definition)
            {
                ToolPermissionState::Allowed
            } else {
                policy.authorize(&tool.definition)
            };
            if permission == ToolPermissionState::Denied {
                return Err(ToolError::PermissionDenied);
            }
            Ok(ToolValidation {
                request_id: request.request_id.clone(),
                tool_name: request.tool_name.clone(),
                permission,
            })
        });
    record_tool_validation_result(log, session_id, request, summary, validation)
}

fn record_native_tool_validation_with_summary(
    log: &mut SessionLog,
    session_id: SessionId,
    request: &PendingToolRequest,
    registry: &ToolRegistry,
    policy: &ToolPermissionPolicy,
    argument_summary: ToolPayloadSummary,
) -> Result<ToolValidation, ToolError> {
    let validation = registry.validate_request(request, policy);
    record_tool_validation_result(log, session_id, request, argument_summary, validation)
}

fn record_tool_validation_result(
    log: &mut SessionLog,
    session_id: SessionId,
    request: &PendingToolRequest,
    argument_summary: ToolPayloadSummary,
    validation: Result<ToolValidation, ToolError>,
) -> Result<ToolValidation, ToolError> {
    let permission = validation
        .as_ref()
        .map_or(ToolPermissionState::Denied, |validation| {
            validation.permission
        });
    log.push(SessionEvent::ToolRequestRecorded {
        session_id: session_id.clone(),
        turn_id: request.turn_id.clone(),
        tool_request_id: ToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: request.provider_call_id.clone(),
        validation: validation.as_ref().map(|_| ()).map_err(Clone::clone),
        permission,
        argument_summary,
        argument_content: validation.is_ok().then(|| request.arguments.to_string()),
    });
    if let Err(error) = &validation {
        log.push(SessionEvent::ToolExecutionFinished {
            session_id,
            turn_id: request.turn_id.clone(),
            tool_request_id: ToolRequestId(request.request_id.clone()),
            outcome: match error {
                ToolError::PermissionDenied => ToolOutcome::Denied,
                _ => ToolOutcome::ValidationFailed,
            },
            reason: Some(tool_error_label(error)),
            result_summary: None,
            result_content: None,
        });
    }
    validation
}

fn resolved_tool_provenance_summary(tool: &ResolvedTool) -> Option<String> {
    match &tool.provenance {
        ToolProvenance::BuiltIn => None,
        ToolProvenance::Extension {
            extension_id,
            extension_version,
        } => Some(format!(
            "resolved_tool=extension extension_id={extension_id} extension_version={extension_version} implementation={}",
            tool.implementation_name
        )),
        ToolProvenance::ExtensionReplacement {
            extension_id,
            extension_version,
            replaced_builtin,
            replacement_source,
        } => Some(format!(
            "resolved_tool=extension_replacement extension_id={extension_id} extension_version={extension_version} provider_name={} implementation={} replaced_builtin={replaced_builtin} replacement_source={replacement_source}",
            tool.provider_name, tool.implementation_name
        )),
    }
}

fn summarize_tool_payload(value: &serde_json::Value) -> ToolPayloadSummary {
    let byte_count = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    ToolPayloadSummary {
        summary: String::from("tool payload redacted"),
        byte_count,
        redacted: true,
        truncated: false,
    }
}

/// Execute fixture-safe provider tool calls and return provider-bound redacted results.
pub fn validate_provider_continuation_request(
    request: &ProviderContinuationRequest,
    policy: ProviderContinuationValidationPolicy,
) -> Result<(), ProviderContinuationValidationError> {
    for result in &request.tool_results {
        if policy.require_provider_call_id && result.provider_call_id.is_none() {
            return Err(ProviderContinuationValidationError::MissingProviderCallId {
                tool_request_id: result.tool_request_id.clone(),
            });
        }
        let actual_bytes = result.content.len();
        if actual_bytes > policy.max_result_content_bytes {
            return Err(ProviderContinuationValidationError::ResultContentTooLarge {
                tool_request_id: result.tool_request_id.clone(),
                max_bytes: policy.max_result_content_bytes,
                actual_bytes,
            });
        }
        if result.redacted && !policy.allow_redacted_results {
            return Err(
                ProviderContinuationValidationError::RedactedResultRejected {
                    tool_request_id: result.tool_request_id.clone(),
                },
            );
        }
        if result.truncated && !policy.allow_truncated_results {
            return Err(
                ProviderContinuationValidationError::TruncatedResultRejected {
                    tool_request_id: result.tool_request_id.clone(),
                },
            );
        }
    }
    Ok(())
}

pub fn build_provider_continuation_submission(
    request: &ProviderContinuationRequest,
    policy: ProviderContinuationValidationPolicy,
) -> Result<ProviderContinuationSubmission, ProviderContinuationMappingError> {
    validate_provider_continuation_request(request, policy)
        .map_err(ProviderContinuationMappingError::Validation)?;
    if request.tool_results.is_empty() {
        return Err(ProviderContinuationMappingError::EmptyToolResults);
    }

    let mut tool_results = Vec::with_capacity(request.tool_results.len());
    for result in &request.tool_results {
        let status_supported = match result.status {
            ToolOutcome::Completed => true,
            ToolOutcome::Failed => policy.allow_failed_results,
            ToolOutcome::Denied | ToolOutcome::Cancelled | ToolOutcome::ValidationFailed => false,
        };
        if !status_supported {
            return Err(
                ProviderContinuationMappingError::UnsupportedToolResultStatus {
                    tool_request_id: result.tool_request_id.clone(),
                    status: result.status,
                },
            );
        }
        let Some(provider_call_id) = result.provider_call_id.clone() else {
            return Err(ProviderContinuationMappingError::Validation(
                ProviderContinuationValidationError::MissingProviderCallId {
                    tool_request_id: result.tool_request_id.clone(),
                },
            ));
        };
        tool_results.push(ProviderContinuationToolResult {
            tool_request_id: result.tool_request_id.clone(),
            provider_call_id,
            status: result.status,
            content: result.content.clone(),
            byte_count: result.byte_count,
            redacted: result.redacted,
            truncated: result.truncated,
            reason: result.reason.clone(),
        });
    }

    Ok(ProviderContinuationSubmission {
        turn_id: request.turn_id.clone(),
        model: request.model.clone(),
        prior_messages: request.prior_messages.clone(),
        tool_results,
        extensions: request.extensions.clone(),
    })
}

pub fn build_fixture_provider_tool_results(
    log: &mut SessionLog,
    context: &ToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    registry: &ToolRegistry,
    policy: &ToolPermissionPolicy,
    executor: &impl ToolExecutor,
    continuation_policy: ToolContinuationPolicy,
) -> Result<Vec<ProviderToolResult>, ToolContinuationError> {
    ToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}

pub fn build_project_readonly_provider_tool_results(
    log: &mut SessionLog,
    context: &ToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    project_root: ResourceRoot,
    registry: &ToolRegistry,
    policy: &ToolPermissionPolicy,
    continuation_policy: ToolContinuationPolicy,
) -> Result<Vec<ProviderToolResult>, ToolContinuationError> {
    let executor = ProjectReadOnlyToolExecutor::new(project_root);
    ToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor: &executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}

fn tool_error_label(error: &ToolError) -> String {
    match error {
        ToolError::UnknownTool => String::from("unknown_tool"),
        ToolError::MalformedArguments => String::from("malformed_arguments"),
        ToolError::ArgumentsTooLarge => String::from("arguments_too_large"),
        ToolError::MissingRequiredField { .. } => String::from("missing_required_field"),
        ToolError::InvalidFieldType { .. } => String::from("invalid_field_type"),
        ToolError::UnexpectedField { .. } => String::from("unexpected_field"),
        ToolError::PermissionDenied => String::from("permission_denied"),
    }
}

fn tool_execution_error_label(error: &ToolExecutionError) -> &'static str {
    match error {
        ToolExecutionError::UnknownTool => "unknown_tool",
        ToolExecutionError::PermissionDenied => "permission_denied",
        ToolExecutionError::UnsupportedTool => "unsupported_tool",
        ToolExecutionError::MalformedResult => "malformed_result",
        ToolExecutionError::ExtensionHost { error } => extension_host_error_label(error),
        ToolExecutionError::ResourceReadTooLarge => "resource_read_too_large",
        ToolExecutionError::ResourceReadNotUtf8 => "resource_read_not_utf8",
        ToolExecutionError::ResourcePath { error } => resource_path_error_label(*error),
    }
}

fn extension_host_error_label(error: &crate::ExtensionHostProtocolError) -> &'static str {
    match error {
        crate::ExtensionHostProtocolError::Malformed => "extension_host_malformed",
        crate::ExtensionHostProtocolError::MissingReady => "extension_host_missing_ready",
        crate::ExtensionHostProtocolError::UnsupportedProtocol => {
            "extension_host_unsupported_protocol"
        }
        crate::ExtensionHostProtocolError::ExtensionIdMismatch => {
            "extension_host_extension_id_mismatch"
        }
        crate::ExtensionHostProtocolError::RequestIdMismatch => {
            "extension_host_request_id_mismatch"
        }
        crate::ExtensionHostProtocolError::UnsupportedRisk => "extension_host_unsupported_risk",
        crate::ExtensionHostProtocolError::UnsupportedSchema => "extension_host_unsupported_schema",
        crate::ExtensionHostProtocolError::SpawnFailed => "extension_host_spawn_failed",
        crate::ExtensionHostProtocolError::HostExited { .. } => "extension_host_exited",
        crate::ExtensionHostProtocolError::TimedOut => "extension_host_timed_out",
        crate::ExtensionHostProtocolError::OutputTooLarge { .. } => {
            "extension_host_output_too_large"
        }
        crate::ExtensionHostProtocolError::ToolRegistration(_) => {
            "extension_host_tool_registration_failed"
        }
    }
}

fn resource_path_error_label(error: ResourcePathError) -> &'static str {
    match error {
        ResourcePathError::RootUnavailable => "resource_path_root_unavailable",
        ResourcePathError::Missing => "resource_path_missing",
        ResourcePathError::EscapesRoot => "resource_path_outside_root",
        ResourcePathError::SymlinkEscapesRoot => "resource_path_symlink_outside_root",
        ResourcePathError::ExpectedFile => "resource_path_directory",
        ResourcePathError::ExpectedDirectory => "resource_path_not_directory",
        ResourcePathError::SensitiveDenied => "sensitive_path_denied",
    }
}

#[cfg(test)]
mod tests {
    use super::search_result_notices;

    #[test]
    fn truncated_search_without_matches_leads_with_incomplete_notice() {
        assert_eq!(
            search_result_notices(true, 512, true, false),
            vec![
                "[search incomplete: file budget exhausted before any matches; narrow the path or pattern]"
            ]
        );
    }

    #[test]
    fn truncated_search_with_matches_appends_budget_notice() {
        assert_eq!(
            search_result_notices(false, 512, true, false),
            vec!["[results incomplete (budget exhausted)]"]
        );
    }

    #[test]
    fn complete_search_keeps_no_match_notice() {
        assert_eq!(
            search_result_notices(true, 2, false, false),
            vec!["[no matches; 2 files searched]"]
        );
    }
}
