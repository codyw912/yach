use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    NativeResourceListPolicy, NativeResourcePathError, NativeResourceReadError,
    NativeResourceReadPolicy, NativeResourceRoot, NativeResourceSearchPolicy, NativeSessionEvent,
    NativeSessionId, NativeSessionLog, NativeToolOutcome, NativeToolPayloadSummary,
    NativeToolRequestId, NativeTurnId, ProviderExtension, ProviderMessage, ProviderModel,
    ProviderToolCall,
};

/// Risk class for yach-owned native tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeToolRisk {
    FixtureSafe,
    ReadsLocalMetadata,
    ReadsLocalContent,
    MutatesLocalState,
    UsesNetwork,
    RunsProcess,
}

/// Ownership boundary for a yach-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolOwner {
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
pub enum NativeToolPermissionState {
    Allowed,
    Denied,
    NeedsApproval,
}

/// Normalized native tool validation/permission errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeToolError {
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
pub struct NativeToolInputSchema {
    required_string_fields: BTreeSet<String>,
    optional_string_fields: BTreeSet<String>,
    max_serialized_bytes: usize,
}

impl NativeToolInputSchema {
    #[must_use]
    pub fn string_object(
        required: impl IntoIterator<Item = impl Into<String>>,
        optional: impl IntoIterator<Item = impl Into<String>>,
        max_serialized_bytes: usize,
    ) -> Self {
        Self {
            required_string_fields: required.into_iter().map(Into::into).collect(),
            optional_string_fields: optional.into_iter().map(Into::into).collect(),
            max_serialized_bytes,
        }
    }

    pub fn validate(&self, arguments: &serde_json::Value) -> Result<(), NativeToolError> {
        let serialized_len = serde_json::to_vec(arguments)
            .map_err(|_| NativeToolError::MalformedArguments)?
            .len();
        if serialized_len > self.max_serialized_bytes {
            return Err(NativeToolError::ArgumentsTooLarge);
        }

        let Some(object) = arguments.as_object() else {
            return Err(NativeToolError::MalformedArguments);
        };

        for field in &self.required_string_fields {
            let Some(value) = object.get(field) else {
                return Err(NativeToolError::MissingRequiredField {
                    field: field.clone(),
                });
            };
            if !value.is_string() {
                return Err(NativeToolError::InvalidFieldType {
                    field: field.clone(),
                });
            }
        }

        for (field, value) in object {
            if !self.required_string_fields.contains(field)
                && !self.optional_string_fields.contains(field)
            {
                return Err(NativeToolError::UnexpectedField {
                    field: field.clone(),
                });
            }
            if !value.is_string() {
                return Err(NativeToolError::InvalidFieldType {
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
        if !self.optional_string_fields.is_empty() {
            return Err(ProviderToolAdvertisingError::UnsupportedSchema {
                name: String::from(name),
            });
        }

        let mut properties = serde_json::Map::new();
        for field in &self.required_string_fields {
            properties.insert(
                field.clone(),
                serde_json::json!({
                    "type": "string",
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
        _ => format!("{field} argument for {tool_name}."),
    }
}

/// Backend-owned native tool definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: NativeToolInputSchema,
    pub risk: NativeToolRisk,
    pub owner: NativeToolOwner,
    pub provider_visibility: ProviderToolVisibility,
}

impl NativeToolDefinition {
    #[must_use]
    pub fn fixture_echo_metadata() -> Self {
        Self {
            name: String::from("fixture_echo_metadata"),
            description: String::from("Fixture-safe tool that validates metadata arguments only."),
            input_schema: NativeToolInputSchema::string_object(["label"], ["note"], 1024),
            risk: NativeToolRisk::FixtureSafe,
            owner: NativeToolOwner::BuiltIn,
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
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::BuiltIn,
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
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn search_project() -> Self {
        Self {
            name: String::from("search_project"),
            description: String::from("Search bounded UTF-8 project files for a literal query."),
            input_schema: NativeToolInputSchema::string_object(
                ["query"],
                std::iter::empty::<&str>(),
                4 * 1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
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
            input_schema: NativeToolInputSchema::string_object(
                ["path"],
                std::iter::empty::<&str>(),
                1024,
            ),
            risk: NativeToolRisk::ReadsLocalContent,
            owner: NativeToolOwner::BuiltIn,
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
            input_schema: NativeToolInputSchema::string_object(
                ["path", "find", "replace"],
                std::iter::empty::<&str>(),
                16 * 1024,
            ),
            risk: NativeToolRisk::MutatesLocalState,
            owner: NativeToolOwner::BuiltIn,
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
            input_schema: NativeToolInputSchema::string_object(
                ["path", "content"],
                std::iter::empty::<&str>(),
                128 * 1024,
            ),
            risk: NativeToolRisk::MutatesLocalState,
            owner: NativeToolOwner::BuiltIn,
            provider_visibility: ProviderToolVisibility::Visible,
        }
    }

    #[must_use]
    pub fn extension_metadata_tool(
        extension_id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: NativeToolInputSchema,
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
        input_schema: NativeToolInputSchema,
        provider_visibility: ProviderToolVisibility,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            risk: NativeToolRisk::ReadsLocalMetadata,
            owner: NativeToolOwner::Extension {
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
    UnsupportedRisk { name: String, risk: NativeToolRisk },
    UnsupportedSchema { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolRegistrationError {
    DuplicateToolName { name: String },
    UnsupportedOwner { name: String },
    UnsupportedRisk { name: String, risk: NativeToolRisk },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeToolResolutionMode {
    Deny,
    AliasOnly,
    ReplaceBuiltin,
    DisableBuiltin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolReplacementSource {
    User,
    Profile,
    Project { trusted: bool },
    Ephemeral,
}

impl NativeToolReplacementSource {
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
pub struct NativeToolReplacementRule {
    pub builtin_name: String,
    pub extension_id: String,
    pub extension_tool: String,
    pub mode: NativeToolResolutionMode,
    pub source: NativeToolReplacementSource,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolReplacementPolicy {
    rules: Vec<NativeToolReplacementRule>,
}

impl NativeToolReplacementPolicy {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_rules(rules: impl IntoIterator<Item = NativeToolReplacementRule>) -> Self {
        Self {
            rules: rules.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn rules(&self) -> &[NativeToolReplacementRule] {
        &self.rules
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolResolutionError {
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
        builtin_risk: NativeToolRisk,
        extension_tool: String,
        extension_risk: NativeToolRisk,
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
pub enum NativeToolProvenance {
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
pub struct ResolvedNativeTool {
    pub provider_name: String,
    pub implementation_name: String,
    pub definition: NativeToolDefinition,
    pub provenance: NativeToolProvenance,
}

/// Snapshot of the tools visible and executable for one provider turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedNativeToolCatalog {
    tools: Vec<ResolvedNativeTool>,
}

impl ResolvedNativeToolCatalog {
    #[must_use]
    pub fn new(tools: Vec<ResolvedNativeTool>) -> Self {
        Self { tools }
    }

    #[must_use]
    pub fn tools(&self) -> &[ResolvedNativeTool] {
        &self.tools
    }

    #[must_use]
    pub fn provider_definitions(&self) -> Vec<NativeToolDefinition> {
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
    pub fn resolved_tool(&self, provider_name: &str) -> Option<&ResolvedNativeTool> {
        self.tools
            .iter()
            .find(|tool| tool.provider_name == provider_name)
    }
}

pub fn build_provider_tool_advertising_extension(
    tools: &[NativeToolDefinition],
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
    build_provider_tool_advertising_extension(&[NativeToolDefinition::project_path_info()])
}

pub fn parse_provider_tool_advertising_extensions(
    extensions: &[ProviderExtension],
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
        validate_provider_tool_advertising(&advertising)?;
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
) -> Result<(), ProviderToolAdvertisingError> {
    if advertising.tools.is_empty() {
        return Err(ProviderToolAdvertisingError::EmptyTools);
    }

    let mut names = BTreeSet::new();
    for tool in &advertising.tools {
        validate_unique_tool_name(&mut names, &tool.name)?;
        validate_provider_advertised_tool_schema(tool)?;
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
    tool: &NativeToolDefinition,
) -> Result<ProviderAdvertisedToolSchema, ProviderToolAdvertisingError> {
    if tool.provider_visibility != ProviderToolVisibility::Visible {
        return Err(ProviderToolAdvertisingError::UnsupportedTool {
            name: tool.name.clone(),
        });
    }

    if tool.owner == NativeToolOwner::BuiltIn {
        match tool.name.as_str() {
            "project_path_info" => {
                if tool.risk != NativeToolRisk::ReadsLocalMetadata {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            "read_text_file" | "search_project" | "list_project_paths" => {
                if tool.risk != NativeToolRisk::ReadsLocalContent {
                    return Err(ProviderToolAdvertisingError::UnsupportedRisk {
                        name: tool.name.clone(),
                        risk: tool.risk,
                    });
                }
            }
            "edit_text_file" | "create_text_file" => {
                if tool.risk != NativeToolRisk::MutatesLocalState {
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
    } else if tool.risk != NativeToolRisk::ReadsLocalMetadata {
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

fn is_canonical_builtin_provider_tool(tool: &NativeToolDefinition) -> bool {
    match tool.name.as_str() {
        "project_path_info" => {
            let canonical = NativeToolDefinition::project_path_info();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "read_text_file" => {
            let canonical = NativeToolDefinition::read_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "search_project" => {
            let canonical = NativeToolDefinition::search_project();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "list_project_paths" => {
            let canonical = NativeToolDefinition::list_project_paths();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "edit_text_file" => {
            let canonical = NativeToolDefinition::edit_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        "create_text_file" => {
            let canonical = NativeToolDefinition::create_text_file();
            tool.risk == canonical.risk
                && tool.description == canonical.description
                && tool.input_schema == canonical.input_schema
        }
        _ => false,
    }
}

fn is_provider_advertising_routable(tool: &NativeToolDefinition) -> bool {
    project_provider_advertised_tool(tool).is_ok()
}

fn validate_provider_advertised_tool_schema(
    tool: &ProviderAdvertisedToolSchema,
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
            || property.get("type").and_then(serde_json::Value::as_str) != Some("string")
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
    if properties
        .keys()
        .any(|field| !required_fields.contains(field.as_str()))
    {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    if parameters.get("additionalProperties") != Some(&serde_json::json!(false)) {
        return Err(ProviderToolAdvertisingError::UnsupportedSchema {
            name: tool.name.clone(),
        });
    }

    let canonical = match tool.name.as_str() {
        "project_path_info" => Some(NativeToolDefinition::project_path_info()),
        "read_text_file" => Some(NativeToolDefinition::read_text_file()),
        "search_project" => Some(NativeToolDefinition::search_project()),
        "list_project_paths" => Some(NativeToolDefinition::list_project_paths()),
        "edit_text_file" => Some(NativeToolDefinition::edit_text_file()),
        "create_text_file" => Some(NativeToolDefinition::create_text_file()),
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

    Ok(())
}

/// Yach-owned pending native tool request derived from provider/tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingNativeToolRequest {
    pub request_id: String,
    pub turn_id: NativeTurnId,
    pub tool_name: String,
    pub provider_call_id: Option<String>,
    pub arguments: serde_json::Value,
}

/// Result of validating and authorizing a pending native tool request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolValidation {
    pub request_id: String,
    pub tool_name: String,
    pub permission: NativeToolPermissionState,
}

/// Backend-internal native tool execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolExecutionResult {
    pub request_id: String,
    pub summary: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
}

/// Provider-bound yach-owned tool result after validation/execution/redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeProviderToolResult {
    pub tool_request_id: String,
    pub provider_call_id: Option<String>,
    pub status: NativeToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Backend-owned request for a provider continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationRequest {
    pub turn_id: NativeTurnId,
    pub model: ProviderModel,
    pub prior_messages: Vec<ProviderMessage>,
    pub tool_results: Vec<NativeProviderToolResult>,
    pub extensions: Vec<ProviderExtension>,
}

/// Provider-independent adapter submission for a validated continuation round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderContinuationSubmission {
    pub turn_id: NativeTurnId,
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
    pub status: NativeToolOutcome,
    pub content: String,
    pub byte_count: usize,
    pub redacted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}

/// Adapter-independent provider continuation validation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContinuationValidationPolicy {
    pub require_provider_call_id: bool,
    pub max_result_content_bytes: usize,
    pub allow_redacted_results: bool,
    pub allow_truncated_results: bool,
}

impl ProviderContinuationValidationPolicy {
    #[must_use]
    pub const fn strict_tool_results(max_result_content_bytes: usize) -> Self {
        Self {
            require_provider_call_id: true,
            max_result_content_bytes,
            allow_redacted_results: true,
            allow_truncated_results: false,
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
        status: NativeToolOutcome,
    },
}

/// Session/turn context for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeToolContinuationContext {
    pub session_id: NativeSessionId,
    pub turn_id: NativeTurnId,
}

/// Limits for backend-only provider tool-result continuation fixtures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeToolContinuationPolicy {
    pub max_tool_calls: usize,
    pub max_result_bytes: usize,
}

impl NativeToolContinuationPolicy {
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
pub enum NativeToolContinuationError {
    TooManyToolCalls {
        max: usize,
        actual: usize,
    },
    Validation(NativeToolError),
    Execution(NativeToolExecutionError),
    ResultTooLarge {
        tool_call_id: String,
        max_bytes: usize,
        actual_bytes: usize,
    },
}

/// Normalized native tool execution errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeToolExecutionError {
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
        error: NativeResourcePathError,
    },
}

/// Backend-internal execution boundary for yach-owned native tools.
pub trait NativeToolExecutor {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError>;
}

/// Deep workflow for provider tool-call validation, execution, recording, and result building.
pub struct NativeToolContinuationWorkflow<'a, Executor>
where
    Executor: NativeToolExecutor,
{
    pub registry: &'a NativeToolRegistry,
    pub permission_policy: &'a NativeToolPermissionPolicy,
    pub executor: &'a Executor,
    pub continuation_policy: NativeToolContinuationPolicy,
}

impl<Executor> NativeToolContinuationWorkflow<'_, Executor>
where
    Executor: NativeToolExecutor,
{
    pub fn build_provider_tool_results(
        &self,
        log: &mut NativeSessionLog,
        context: &NativeToolContinuationContext,
        tool_calls: Vec<ProviderToolCall>,
    ) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
        if tool_calls.len() > self.continuation_policy.max_tool_calls {
            return Err(NativeToolContinuationError::TooManyToolCalls {
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
            .map_err(NativeToolContinuationError::Validation)?;
            let execution = match self.executor.execute(self.registry, &request, &validation) {
                Ok(execution) => execution,
                Err(error) => {
                    log.push(NativeSessionEvent::ToolExecutionFinished {
                        session_id: context.session_id.clone(),
                        turn_id: context.turn_id.clone(),
                        tool_request_id: NativeToolRequestId(request.request_id.clone()),
                        outcome: NativeToolOutcome::Failed,
                        reason: Some(native_tool_execution_error_label(&error).to_string()),
                        result_summary: None,
                    });
                    return Err(NativeToolContinuationError::Execution(error));
                }
            };
            if execution.byte_count > self.continuation_policy.max_result_bytes {
                log.push(NativeSessionEvent::ToolExecutionFinished {
                    session_id: context.session_id.clone(),
                    turn_id: context.turn_id.clone(),
                    tool_request_id: NativeToolRequestId(request.request_id.clone()),
                    outcome: NativeToolOutcome::Failed,
                    reason: Some(String::from("result_too_large")),
                    result_summary: None,
                });
                return Err(NativeToolContinuationError::ResultTooLarge {
                    tool_call_id: request
                        .provider_call_id
                        .clone()
                        .unwrap_or_else(|| request.request_id.clone()),
                    max_bytes: self.continuation_policy.max_result_bytes,
                    actual_bytes: execution.byte_count,
                });
            }

            let result_summary = provider_tool_result_summary(&request.tool_name, &execution);
            log.push(NativeSessionEvent::ToolExecutionFinished {
                session_id: context.session_id.clone(),
                turn_id: context.turn_id.clone(),
                tool_request_id: NativeToolRequestId(request.request_id.clone()),
                outcome: NativeToolOutcome::Completed,
                reason: None,
                result_summary: Some(result_summary),
            });
            results.push(NativeProviderToolResult {
                tool_request_id: request.request_id,
                provider_call_id: request.provider_call_id,
                status: NativeToolOutcome::Completed,
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
pub struct FixtureNativeToolExecutor;

impl NativeToolExecutor for FixtureNativeToolExecutor {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnknownTool);
        };
        if validation.permission != NativeToolPermissionState::Allowed {
            return Err(NativeToolExecutionError::PermissionDenied);
        }
        if definition.name != "fixture_echo_metadata"
            || definition.risk != NativeToolRisk::FixtureSafe
        {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }

        let byte_count = serde_json::to_vec(&request.arguments).map_or(0, |bytes| bytes.len());
        Ok(NativeToolExecutionResult {
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
    root: Option<NativeResourceRoot>,
}

impl ProjectReadOnlyToolExecutor {
    #[must_use]
    pub fn new(root: NativeResourceRoot) -> Self {
        Self { root: Some(root) }
    }

    #[must_use]
    pub fn unavailable_root() -> Self {
        Self { root: None }
    }
}

impl NativeToolExecutor for ProjectReadOnlyToolExecutor {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnknownTool);
        };
        if validation.permission != NativeToolPermissionState::Allowed {
            return Err(NativeToolExecutionError::PermissionDenied);
        }
        let Some(root) = &self.root else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        match definition.name.as_str() {
            "project_path_info" if definition.risk == NativeToolRisk::ReadsLocalMetadata => {
                execute_project_path_info(root, request)
            }
            "read_text_file" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_read_text_file(root, request)
            }
            "search_project" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_search_project(root, request)
            }
            "list_project_paths" if definition.risk == NativeToolRisk::ReadsLocalContent => {
                execute_list_project_paths(root, request)
            }
            _ => Err(NativeToolExecutionError::UnsupportedTool),
        }
    }
}

fn execute_project_path_info(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let metadata = root
        .path_metadata(path)
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let summary = serde_json::json!({
        "relative_path": metadata.relative_path,
        "kind": resource_entry_kind_label(metadata.kind),
        "byte_size": metadata.byte_size,
        "provider_visibility": "never",
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: false,
    })
}

fn execute_read_text_file(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let read = root
        .read_text_file(
            &path,
            NativeResourceReadPolicy::local_only(PROVIDER_READ_TEXT_MAX_BYTES),
        )
        .map_err(|error| native_read_error_to_execution_error(&error))?;
    let relative_path = root
        .path_metadata(&path)
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?
        .relative_path;
    let summary = serde_json::json!({
        "outcome": "read",
        "path": relative_path,
        "text": read.text,
        "byte_count": read.byte_count,
        "truncated": false,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: false,
    })
}

fn execute_search_project(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let query = required_string_argument(request, "query")?;
    let result = root
        .search_text(
            &query,
            NativeResourceSearchPolicy {
                max_file_bytes: PROVIDER_SEARCH_MAX_FILE_BYTES,
                max_files: PROVIDER_SEARCH_MAX_FILES,
                max_matches: PROVIDER_SEARCH_MAX_MATCHES,
            },
        )
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let mut line_truncated = false;
    let matches = result
        .matches
        .into_iter()
        .map(|matched| {
            let (line, truncated) = bounded_provider_line(&matched.line);
            line_truncated |= truncated;
            serde_json::json!({
                "path": matched.relative_path,
                "line_number": matched.line_number,
                "line": line,
                "line_truncated": truncated,
            })
        })
        .collect::<Vec<_>>();
    let truncated = result.truncated || line_truncated;
    let summary = serde_json::json!({
        "outcome": "search",
        "matches": matches,
        "searched_files": result.searched_files,
        "truncated": truncated,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated,
    })
}

fn execute_list_project_paths(
    root: &NativeResourceRoot,
    request: &PendingNativeToolRequest,
) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
    let path = required_string_argument(request, "path")?;
    let result = root
        .list_paths(
            &path,
            NativeResourceListPolicy {
                max_entries: PROVIDER_LIST_MAX_ENTRIES,
            },
        )
        .map_err(|error| NativeToolExecutionError::ResourcePath { error })?;
    let entries = result
        .entries
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.relative_path,
                "kind": resource_entry_kind_label(entry.kind),
                "byte_size": entry.byte_size,
            })
        })
        .collect::<Vec<_>>();
    let summary = serde_json::json!({
        "outcome": "list",
        "path": result.relative_path,
        "entries": entries,
        "truncated": result.truncated,
    })
    .to_string();
    Ok(NativeToolExecutionResult {
        request_id: request.request_id.clone(),
        byte_count: summary.len(),
        summary,
        redacted: false,
        truncated: result.truncated,
    })
}

fn required_string_argument(
    request: &PendingNativeToolRequest,
    field: &str,
) -> Result<String, NativeToolExecutionError> {
    request
        .arguments
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .ok_or(NativeToolExecutionError::MalformedResult)
}

fn resource_entry_kind_label(kind: crate::NativeResourceEntryKind) -> &'static str {
    match kind {
        crate::NativeResourceEntryKind::File => "file",
        crate::NativeResourceEntryKind::Directory => "directory",
        crate::NativeResourceEntryKind::Other => "other",
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

fn native_read_error_to_execution_error(
    error: &NativeResourceReadError,
) -> NativeToolExecutionError {
    match error {
        NativeResourceReadError::Path(error) => {
            NativeToolExecutionError::ResourcePath { error: *error }
        }
        NativeResourceReadError::TooLarge { .. } => NativeToolExecutionError::ResourceReadTooLarge,
        NativeResourceReadError::NotUtf8 => NativeToolExecutionError::ResourceReadNotUtf8,
        NativeResourceReadError::Io => NativeToolExecutionError::MalformedResult,
    }
}

fn provider_tool_result_summary(
    tool_name: &str,
    execution: &NativeToolExecutionResult,
) -> NativeToolPayloadSummary {
    let summary = match tool_name {
        "read_text_file" => String::from("read_text_file result redacted"),
        "search_project" => content_result_count_summary("search_project", &execution.summary)
            .unwrap_or_else(|| String::from("search_project result redacted")),
        "list_project_paths" => {
            content_result_count_summary("list_project_paths", &execution.summary)
                .unwrap_or_else(|| String::from("list_project_paths result redacted"))
        }
        _ => execution.summary.clone(),
    };
    NativeToolPayloadSummary {
        summary,
        byte_count: execution.byte_count,
        redacted: matches!(
            tool_name,
            "read_text_file" | "search_project" | "list_project_paths"
        ),
        truncated: execution.truncated,
    }
}

fn content_result_count_summary(tool_name: &str, content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    match tool_name {
        "search_project" => Some(format!(
            "search_project matches={} truncated={}",
            value.get("matches")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        "list_project_paths" => Some(format!(
            "list_project_paths entries={} truncated={}",
            value.get("entries")?.as_array()?.len(),
            value.get("truncated")?.as_bool()?
        )),
        _ => None,
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
}

impl NativeToolExecutor for ExtensionToolExecutorRouter {
    fn execute(
        &self,
        registry: &NativeToolRegistry,
        request: &PendingNativeToolRequest,
        validation: &NativeToolValidation,
    ) -> Result<NativeToolExecutionResult, NativeToolExecutionError> {
        let Some(definition) = registry.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnknownTool);
        };
        if validation.permission != NativeToolPermissionState::Allowed {
            return Err(NativeToolExecutionError::PermissionDenied);
        }
        let NativeToolOwner::Extension {
            extension_id: definition_extension_id,
            ..
        } = &definition.owner
        else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        let Some(handler) = self.handlers.get(&request.tool_name) else {
            return Err(NativeToolExecutionError::UnsupportedTool);
        };
        if handler.extension_id != *definition_extension_id {
            return Err(NativeToolExecutionError::UnsupportedTool);
        }
        match &handler.route {
            ExtensionToolRoute::Static {
                response,
                malformed,
            } => {
                if *malformed {
                    return Err(NativeToolExecutionError::MalformedResult);
                }
                if serde_json::from_str::<serde_json::Value>(response).is_err() {
                    return Err(NativeToolExecutionError::MalformedResult);
                }

                Ok(NativeToolExecutionResult {
                    request_id: request.request_id.clone(),
                    byte_count: response.len(),
                    summary: response.clone(),
                    redacted: false,
                    truncated: false,
                })
            }
            ExtensionToolRoute::Host { invoker, timeout } => {
                let mut invoker =
                    invoker
                        .lock()
                        .map_err(|_| NativeToolExecutionError::ExtensionHost {
                            error: crate::ExtensionHostProtocolError::Malformed,
                        })?;
                let response = invoker
                    .invoke(
                        &request.request_id,
                        &request.tool_name,
                        request.arguments.clone(),
                        *timeout,
                    )
                    .map_err(|error| NativeToolExecutionError::ExtensionHost { error })?;
                Ok(NativeToolExecutionResult {
                    request_id: request.request_id.clone(),
                    byte_count: response.len(),
                    summary: response,
                    redacted: false,
                    truncated: false,
                })
            }
        }
    }
}

/// Explicit allowlist policy for first native tool slices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolPermissionPolicy {
    fixture_execution: BTreeSet<String>,
    metadata_advertising: BTreeSet<String>,
    content_advertising: BTreeSet<String>,
    agent_edit_advertising: BTreeSet<String>,
}

impl NativeToolPermissionPolicy {
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
        }
    }

    #[must_use]
    pub fn authorize(&self, definition: &NativeToolDefinition) -> NativeToolPermissionState {
        let allowed = match definition.risk {
            NativeToolRisk::FixtureSafe => self.fixture_execution.contains(&definition.name),
            NativeToolRisk::ReadsLocalMetadata => {
                self.metadata_advertising.contains(&definition.name)
            }
            NativeToolRisk::ReadsLocalContent => {
                self.content_advertising.contains(&definition.name)
            }
            NativeToolRisk::MutatesLocalState
            | NativeToolRisk::UsesNetwork
            | NativeToolRisk::RunsProcess => false,
        };

        if allowed {
            NativeToolPermissionState::Allowed
        } else {
            NativeToolPermissionState::Denied
        }
    }

    #[must_use]
    pub fn allows_provider_advertising(&self, definition: &NativeToolDefinition) -> bool {
        match definition.risk {
            NativeToolRisk::ReadsLocalMetadata => {
                self.metadata_advertising.contains(&definition.name)
            }
            NativeToolRisk::ReadsLocalContent => {
                self.content_advertising.contains(&definition.name)
            }
            NativeToolRisk::MutatesLocalState => {
                self.agent_edit_advertising.contains(&definition.name)
            }
            NativeToolRisk::FixtureSafe
            | NativeToolRisk::RunsProcess
            | NativeToolRisk::UsesNetwork => false,
        }
    }
}

/// Backend-owned native tool registry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeToolRegistry {
    definitions: Vec<NativeToolDefinition>,
}

impl NativeToolRegistry {
    #[must_use]
    pub fn with_fixture_tools() -> Self {
        Self {
            definitions: vec![NativeToolDefinition::fixture_echo_metadata()],
        }
    }

    #[must_use]
    pub fn with_project_read_only_tools() -> Self {
        Self {
            definitions: vec![
                NativeToolDefinition::project_path_info(),
                NativeToolDefinition::read_text_file(),
                NativeToolDefinition::search_project(),
                NativeToolDefinition::list_project_paths(),
            ],
        }
    }

    #[must_use]
    pub fn with_agent_edit_tools() -> Self {
        Self {
            definitions: vec![
                NativeToolDefinition::edit_text_file(),
                NativeToolDefinition::create_text_file(),
            ],
        }
    }

    #[must_use]
    pub fn with_project_read_only_and_agent_edit_tools() -> Self {
        Self {
            definitions: vec![
                NativeToolDefinition::project_path_info(),
                NativeToolDefinition::read_text_file(),
                NativeToolDefinition::search_project(),
                NativeToolDefinition::list_project_paths(),
                NativeToolDefinition::edit_text_file(),
                NativeToolDefinition::create_text_file(),
            ],
        }
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&NativeToolDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.name == name)
    }

    #[must_use]
    pub fn definitions(&self) -> &[NativeToolDefinition] {
        &self.definitions
    }

    pub fn register_extension_tool(
        &mut self,
        definition: NativeToolDefinition,
    ) -> Result<(), NativeToolRegistrationError> {
        if self.get(&definition.name).is_some() {
            return Err(NativeToolRegistrationError::DuplicateToolName {
                name: definition.name,
            });
        }

        if !matches!(&definition.owner, NativeToolOwner::Extension { .. }) {
            return Err(NativeToolRegistrationError::UnsupportedOwner {
                name: definition.name,
            });
        }

        if definition.risk != NativeToolRisk::ReadsLocalMetadata {
            return Err(NativeToolRegistrationError::UnsupportedRisk {
                name: definition.name,
                risk: definition.risk,
            });
        }

        self.definitions.push(definition);
        Ok(())
    }

    #[must_use]
    pub fn provider_advertising_candidates<'a>(
        &self,
        policy: &NativeToolPermissionPolicy,
        routable_tools: impl IntoIterator<Item = &'a str>,
    ) -> Vec<NativeToolDefinition> {
        self.resolve_provider_turn_catalog(policy, routable_tools)
            .provider_definitions()
    }

    #[must_use]
    pub fn resolve_provider_turn_catalog<'a>(
        &self,
        policy: &NativeToolPermissionPolicy,
        executable_tools: impl IntoIterator<Item = &'a str>,
    ) -> ResolvedNativeToolCatalog {
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
            .map(|definition| ResolvedNativeTool {
                provider_name: definition.name.clone(),
                implementation_name: definition.name.clone(),
                definition: definition.clone(),
                provenance: native_tool_provenance(definition),
            })
            .collect();
        ResolvedNativeToolCatalog::new(tools)
    }

    pub fn resolve_provider_turn_catalog_with_replacements<'a>(
        &self,
        policy: &NativeToolPermissionPolicy,
        executable_tools: impl IntoIterator<Item = &'a str>,
        replacement_policy: &NativeToolReplacementPolicy,
    ) -> Result<ResolvedNativeToolCatalog, NativeToolResolutionError> {
        let executable_tools = executable_tools.into_iter().collect::<BTreeSet<_>>();
        let mut disabled_builtins = BTreeSet::new();
        let mut replaced_builtins = BTreeSet::new();
        let mut replacement_implementations = BTreeSet::new();
        let mut denied_extension_tools = BTreeSet::new();
        let mut replacement_tools = Vec::new();

        for rule in replacement_policy.rules() {
            validate_replacement_rule_source(rule)?;
            match rule.mode {
                NativeToolResolutionMode::Deny => {
                    denied_extension_tools.insert(rule.extension_tool.as_str());
                }
                NativeToolResolutionMode::AliasOnly => {}
                NativeToolResolutionMode::DisableBuiltin => {
                    let builtin = self.builtin_definition(&rule.builtin_name)?;
                    disabled_builtins.insert(builtin.name.as_str());
                }
                NativeToolResolutionMode::ReplaceBuiltin => {
                    let builtin = self.builtin_definition(&rule.builtin_name)?;
                    let extension = self.extension_definition(rule)?;
                    validate_replacement_shape(builtin, extension, rule)?;
                    if executable_tools.contains(extension.name.as_str())
                        && policy.allows_provider_advertising(builtin)
                        && is_provider_advertising_routable(builtin)
                    {
                        replaced_builtins.insert(builtin.name.as_str());
                        replacement_implementations.insert(extension.name.as_str());
                        let extension_version = native_tool_extension_version(extension);
                        replacement_tools.push(ResolvedNativeTool {
                            provider_name: builtin.name.clone(),
                            implementation_name: extension.name.clone(),
                            definition: builtin.clone(),
                            provenance: NativeToolProvenance::ExtensionReplacement {
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
                if definition.owner == NativeToolOwner::BuiltIn
                    && (disabled_builtins.contains(definition.name.as_str())
                        || replaced_builtins.contains(definition.name.as_str()))
                {
                    return false;
                }
                if matches!(definition.owner, NativeToolOwner::Extension { .. })
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
            .map(|definition| ResolvedNativeTool {
                provider_name: definition.name.clone(),
                implementation_name: definition.name.clone(),
                definition: definition.clone(),
                provenance: native_tool_provenance(definition),
            })
            .collect::<Vec<_>>();
        tools.extend(replacement_tools);
        tools.sort_by(|left, right| left.provider_name.cmp(&right.provider_name));
        Ok(ResolvedNativeToolCatalog::new(tools))
    }

    pub fn validate_request_schema_only(
        &self,
        request: &PendingNativeToolRequest,
    ) -> Result<&NativeToolDefinition, NativeToolError> {
        let definition = self
            .get(&request.tool_name)
            .ok_or(NativeToolError::UnknownTool)?;
        definition.input_schema.validate(&request.arguments)?;
        Ok(definition)
    }

    pub fn validate_request(
        &self,
        request: &PendingNativeToolRequest,
        policy: &NativeToolPermissionPolicy,
    ) -> Result<NativeToolValidation, NativeToolError> {
        let definition = self
            .get(&request.tool_name)
            .ok_or(NativeToolError::UnknownTool)?;
        definition.input_schema.validate(&request.arguments)?;
        let permission = policy.authorize(definition);
        if permission == NativeToolPermissionState::Denied {
            return Err(NativeToolError::PermissionDenied);
        }

        Ok(NativeToolValidation {
            request_id: request.request_id.clone(),
            tool_name: request.tool_name.clone(),
            permission,
        })
    }

    fn builtin_definition(
        &self,
        name: &str,
    ) -> Result<&NativeToolDefinition, NativeToolResolutionError> {
        let definition =
            self.get(name)
                .ok_or_else(|| NativeToolResolutionError::MissingBuiltIn {
                    name: String::from(name),
                })?;
        if definition.owner != NativeToolOwner::BuiltIn {
            return Err(NativeToolResolutionError::MissingBuiltIn {
                name: String::from(name),
            });
        }
        Ok(definition)
    }

    fn extension_definition(
        &self,
        rule: &NativeToolReplacementRule,
    ) -> Result<&NativeToolDefinition, NativeToolResolutionError> {
        let definition = self.get(&rule.extension_tool).ok_or_else(|| {
            NativeToolResolutionError::MissingExtensionTool {
                name: rule.extension_tool.clone(),
            }
        })?;
        let NativeToolOwner::Extension { extension_id, .. } = &definition.owner else {
            return Err(NativeToolResolutionError::MissingExtensionTool {
                name: rule.extension_tool.clone(),
            });
        };
        if extension_id != &rule.extension_id {
            return Err(NativeToolResolutionError::ExtensionIdMismatch {
                expected: rule.extension_id.clone(),
                actual: extension_id.clone(),
            });
        }
        Ok(definition)
    }
}

fn validate_replacement_rule_source(
    rule: &NativeToolReplacementRule,
) -> Result<(), NativeToolResolutionError> {
    if rule.source.is_trusted() {
        Ok(())
    } else {
        Err(NativeToolResolutionError::UntrustedProjectReplacement {
            builtin_name: rule.builtin_name.clone(),
        })
    }
}

fn validate_replacement_shape(
    builtin: &NativeToolDefinition,
    extension: &NativeToolDefinition,
    rule: &NativeToolReplacementRule,
) -> Result<(), NativeToolResolutionError> {
    if builtin.risk != extension.risk {
        return Err(NativeToolResolutionError::ReplacementLowersRisk {
            builtin_name: builtin.name.clone(),
            builtin_risk: builtin.risk,
            extension_tool: extension.name.clone(),
            extension_risk: extension.risk,
        });
    }
    if builtin.input_schema != extension.input_schema {
        return Err(NativeToolResolutionError::ReplacementSchemaMismatch {
            builtin_name: builtin.name.clone(),
            extension_tool: rule.extension_tool.clone(),
        });
    }
    Ok(())
}

fn native_tool_provenance(definition: &NativeToolDefinition) -> NativeToolProvenance {
    match &definition.owner {
        NativeToolOwner::BuiltIn => NativeToolProvenance::BuiltIn,
        NativeToolOwner::Extension {
            extension_id,
            extension_version,
        } => NativeToolProvenance::Extension {
            extension_id: extension_id.clone(),
            extension_version: extension_version
                .clone()
                .unwrap_or_else(|| String::from("unknown")),
        },
    }
}

fn native_tool_extension_version(definition: &NativeToolDefinition) -> String {
    match &definition.owner {
        NativeToolOwner::Extension {
            extension_version, ..
        } => extension_version
            .clone()
            .unwrap_or_else(|| String::from("unknown")),
        NativeToolOwner::BuiltIn => String::from("unknown"),
    }
}

/// Build a yach-owned pending tool request from provider-emitted tool-call metadata.
#[must_use]
pub fn pending_tool_request_from_provider_call(
    request_id: impl Into<String>,
    turn_id: NativeTurnId,
    tool_call: ProviderToolCall,
) -> PendingNativeToolRequest {
    PendingNativeToolRequest {
        request_id: request_id.into(),
        turn_id,
        tool_name: tool_call.name,
        provider_call_id: Some(tool_call.call_id),
        arguments: tool_call.arguments_json,
    }
}

/// Validate a pending tool request and append provisional redacted session records.
pub fn record_native_tool_validation(
    log: &mut NativeSessionLog,
    session_id: NativeSessionId,
    request: &PendingNativeToolRequest,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
) -> Result<NativeToolValidation, NativeToolError> {
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
    log: &mut NativeSessionLog,
    session_id: NativeSessionId,
    request: &PendingNativeToolRequest,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    catalog: &ResolvedNativeToolCatalog,
) -> Result<NativeToolValidation, NativeToolError> {
    let mut summary = summarize_tool_payload(&request.arguments);
    if let Some(tool) = catalog.resolved_tool(&request.tool_name)
        && let Some(provenance) = resolved_tool_provenance_summary(tool)
    {
        summary.summary = format!("{}; {provenance}", summary.summary);
    }
    record_native_tool_validation_with_summary(log, session_id, request, registry, policy, summary)
}

fn record_native_tool_validation_with_summary(
    log: &mut NativeSessionLog,
    session_id: NativeSessionId,
    request: &PendingNativeToolRequest,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    argument_summary: NativeToolPayloadSummary,
) -> Result<NativeToolValidation, NativeToolError> {
    let validation = registry.validate_request(request, policy);
    let permission = if validation.is_ok() {
        NativeToolPermissionState::Allowed
    } else {
        NativeToolPermissionState::Denied
    };
    log.push(NativeSessionEvent::ToolRequestRecorded {
        session_id: session_id.clone(),
        turn_id: request.turn_id.clone(),
        tool_request_id: NativeToolRequestId(request.request_id.clone()),
        tool_name: request.tool_name.clone(),
        provider_call_id: request.provider_call_id.clone(),
        validation: validation.as_ref().map(|_| ()).map_err(Clone::clone),
        permission,
        argument_summary,
    });
    if let Err(error) = &validation {
        log.push(NativeSessionEvent::ToolExecutionFinished {
            session_id,
            turn_id: request.turn_id.clone(),
            tool_request_id: NativeToolRequestId(request.request_id.clone()),
            outcome: match error {
                NativeToolError::PermissionDenied => NativeToolOutcome::Denied,
                _ => NativeToolOutcome::ValidationFailed,
            },
            reason: Some(native_tool_error_label(error)),
            result_summary: None,
        });
    }
    validation
}

fn resolved_tool_provenance_summary(tool: &ResolvedNativeTool) -> Option<String> {
    match &tool.provenance {
        NativeToolProvenance::BuiltIn => None,
        NativeToolProvenance::Extension {
            extension_id,
            extension_version,
        } => Some(format!(
            "resolved_tool=extension extension_id={extension_id} extension_version={extension_version} implementation={}",
            tool.implementation_name
        )),
        NativeToolProvenance::ExtensionReplacement {
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

fn summarize_tool_payload(value: &serde_json::Value) -> NativeToolPayloadSummary {
    let byte_count = serde_json::to_vec(value).map_or(0, |bytes| bytes.len());
    NativeToolPayloadSummary {
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
        if result.status != NativeToolOutcome::Completed {
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
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    executor: &impl NativeToolExecutor,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    NativeToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}

pub fn build_project_readonly_provider_tool_results(
    log: &mut NativeSessionLog,
    context: &NativeToolContinuationContext,
    tool_calls: Vec<ProviderToolCall>,
    project_root: NativeResourceRoot,
    registry: &NativeToolRegistry,
    policy: &NativeToolPermissionPolicy,
    continuation_policy: NativeToolContinuationPolicy,
) -> Result<Vec<NativeProviderToolResult>, NativeToolContinuationError> {
    let executor = ProjectReadOnlyToolExecutor::new(project_root);
    NativeToolContinuationWorkflow {
        registry,
        permission_policy: policy,
        executor: &executor,
        continuation_policy,
    }
    .build_provider_tool_results(log, context, tool_calls)
}

fn native_tool_error_label(error: &NativeToolError) -> String {
    match error {
        NativeToolError::UnknownTool => String::from("unknown_tool"),
        NativeToolError::MalformedArguments => String::from("malformed_arguments"),
        NativeToolError::ArgumentsTooLarge => String::from("arguments_too_large"),
        NativeToolError::MissingRequiredField { .. } => String::from("missing_required_field"),
        NativeToolError::InvalidFieldType { .. } => String::from("invalid_field_type"),
        NativeToolError::UnexpectedField { .. } => String::from("unexpected_field"),
        NativeToolError::PermissionDenied => String::from("permission_denied"),
    }
}

fn native_tool_execution_error_label(error: &NativeToolExecutionError) -> &'static str {
    match error {
        NativeToolExecutionError::UnknownTool => "unknown_tool",
        NativeToolExecutionError::PermissionDenied => "permission_denied",
        NativeToolExecutionError::UnsupportedTool => "unsupported_tool",
        NativeToolExecutionError::MalformedResult => "malformed_result",
        NativeToolExecutionError::ExtensionHost { error } => extension_host_error_label(error),
        NativeToolExecutionError::ResourceReadTooLarge => "resource_read_too_large",
        NativeToolExecutionError::ResourceReadNotUtf8 => "resource_read_not_utf8",
        NativeToolExecutionError::ResourcePath { error } => {
            native_resource_path_error_label(*error)
        }
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

fn native_resource_path_error_label(error: NativeResourcePathError) -> &'static str {
    match error {
        NativeResourcePathError::RootUnavailable => "resource_path_root_unavailable",
        NativeResourcePathError::Missing => "resource_path_missing",
        NativeResourcePathError::EscapesRoot => "resource_path_outside_root",
        NativeResourcePathError::ExpectedFile => "resource_path_directory",
        NativeResourcePathError::ExpectedDirectory => "resource_path_not_directory",
    }
}
