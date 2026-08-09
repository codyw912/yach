use std::sync::{Arc, Mutex};

use rig::OneOrMany;
use rig::completion::Message;
use rig::completion::message::{
    AssistantContent, Text, ToolCall, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use rig::providers::openai::responses_api::InputItem;
use sha2::{Digest, Sha256};

use crate::{
    ProviderError, ProviderErrorKind, ProviderMessage, ProviderToolResultBlock, Role, SessionId,
    native_window_is_replayable,
};

const NATIVE_COMPACTION_ARTIFACT_VERSION: u8 = 1;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeReplayTarget {
    pub session_id: SessionId,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub connection: String,
}

impl std::fmt::Debug for NativeReplayTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let connection_digest = format!("{:x}", Sha256::digest(self.connection.as_bytes()));
        formatter
            .debug_struct("NativeReplayTarget")
            .field("session_id", &self.session_id)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("connection_fingerprint", &&connection_digest[..12])
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NativeReplayState {
    pub target: NativeReplayTarget,
    pub instructions: String,
    pub input: Vec<serde_json::Value>,
    pub synced_event_count: usize,
}

impl std::fmt::Debug for NativeReplayState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeReplayState")
            .field("target", &self.target)
            .field("instructions_bytes", &self.instructions.len())
            .field("input_items", &self.input.len())
            .field("synced_event_count", &self.synced_event_count)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Default)]
pub enum NativeReplayStoreState {
    #[default]
    Uninitialized,
    /// Replay is unsafe only for this target. A later target may restore its
    /// own checkpoint without reviving the failed chain.
    Invalidated(Option<NativeReplayTarget>),
    /// Capability absence is safe fallback, not malformed replay. A matching
    /// target may reload its persisted checkpoint when capability returns.
    CapabilityDisabled(Option<NativeReplayTarget>),
    Active(NativeReplayState),
}

impl std::fmt::Debug for NativeReplayStoreState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uninitialized => formatter.write_str("NativeReplayStoreState::Uninitialized"),
            Self::Invalidated(target) => formatter
                .debug_tuple("NativeReplayStoreState::Invalidated")
                .field(target)
                .finish(),
            Self::CapabilityDisabled(target) => formatter
                .debug_tuple("NativeReplayStoreState::CapabilityDisabled")
                .field(target)
                .finish(),
            Self::Active(state) => formatter
                .debug_tuple("NativeReplayStoreState::Active")
                .field(state)
                .finish(),
        }
    }
}

impl NativeReplayStoreState {
    pub fn active(&self) -> Option<&NativeReplayState> {
        match self {
            Self::Active(state) => Some(state),
            Self::Uninitialized | Self::Invalidated(_) | Self::CapabilityDisabled(_) => None,
        }
    }

    pub fn active_mut(&mut self) -> Option<&mut NativeReplayState> {
        match self {
            Self::Active(state) => Some(state),
            Self::Uninitialized | Self::Invalidated(_) | Self::CapabilityDisabled(_) => None,
        }
    }

    pub fn invalidated_for(target: NativeReplayTarget) -> Self {
        Self::Invalidated(Some(target))
    }

    pub fn capability_disabled_for(target: NativeReplayTarget) -> Self {
        Self::CapabilityDisabled(Some(target))
    }

    pub fn from_active_for(state: Option<NativeReplayState>, target: NativeReplayTarget) -> Self {
        state.map_or_else(|| Self::invalidated_for(target), Self::Active)
    }
}
pub type NativeReplayStore = Arc<Mutex<NativeReplayStoreState>>;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct NativeCompactionArtifact {
    version: u8,
    target: NativeReplayTarget,
    instructions: String,
    input: Vec<serde_json::Value>,
    synced_event_count: usize,
}

impl std::fmt::Debug for NativeCompactionArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeCompactionArtifact")
            .field("version", &self.version)
            .field("target", &self.target)
            .field("instructions_bytes", &self.instructions.len())
            .field("input_items", &self.input.len())
            .field("synced_event_count", &self.synced_event_count)
            .finish()
    }
}

impl NativeReplayState {
    pub fn new(
        target: NativeReplayTarget,
        messages: &[ProviderMessage],
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            target,
            instructions: instructions_from_messages(messages),
            input: input_items_from_messages(messages)?,
            synced_event_count: 0,
        })
    }

    pub fn matches_target(&self, target: &NativeReplayTarget) -> bool {
        self.target == *target
    }

    pub fn artifact_json(&self) -> Result<serde_json::Value, ProviderError> {
        serde_json::to_value(NativeCompactionArtifact {
            version: NATIVE_COMPACTION_ARTIFACT_VERSION,
            target: self.target.clone(),
            instructions: self.instructions.clone(),
            input: self.input.clone(),
            synced_event_count: self.synced_event_count,
        })
        .map_err(|_| invalid_request("native compaction artifact encoding failed"))
    }

    pub fn from_artifact_json(value: serde_json::Value) -> Result<Self, ProviderError> {
        let artifact: NativeCompactionArtifact = serde_json::from_value(value)
            .map_err(|_| invalid_request("invalid native compaction artifact"))?;
        if artifact.version != NATIVE_COMPACTION_ARTIFACT_VERSION {
            return Err(invalid_request(
                "unsupported native compaction artifact version",
            ));
        }
        if !native_window_is_replayable(&artifact.input) {
            return Err(invalid_request("invalid native compaction artifact window"));
        }
        Ok(Self {
            target: artifact.target,
            instructions: artifact.instructions,
            input: artifact.input,
            synced_event_count: artifact.synced_event_count,
        })
    }
}

/// Assemble the exact top-level OpenAI Responses instructions string from yach
/// system messages.
pub(crate) fn instructions_from_messages(messages: &[ProviderMessage]) -> String {
    let instructions = messages
        .iter()
        .filter(|message| matches!(message.role, Role::System))
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if instructions.trim().is_empty() {
        String::from("Follow the user instruction exactly.")
    } else {
        instructions
    }
}

/// Construct a raw OpenAI Responses assistant output-text item.
pub fn assistant_output_text_item(text: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text.into()}],
    })
}

/// Construct a raw OpenAI Responses user input-text item.
pub fn user_input_text_item(text: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": text.into()}],
    })
}
/// Convert one yach message into the Rig message(s) shared by generic and
/// OpenAI Responses replay request assembly.
pub fn rig_messages_from_provider_message(message: &ProviderMessage) -> Vec<Message> {
    match message.role {
        Role::System => Vec::new(),
        Role::User => (!message.content.trim().is_empty())
            .then(|| Message::user(message.content.clone()))
            .into_iter()
            .collect(),
        Role::Assistant => {
            let mut content = Vec::new();
            if !message.content.trim().is_empty() {
                content.push(AssistantContent::text(message.content.clone()));
            }
            content.extend(message.tool_calls.iter().map(|call| {
                AssistantContent::ToolCall(ToolCall {
                    id: call.call_id.clone(),
                    call_id: Some(call.call_id.clone()),
                    function: ToolFunction {
                        name: call.name.clone(),
                        arguments: call.arguments_json.clone(),
                    },
                    signature: None,
                    additional_params: None,
                })
            }));
            OneOrMany::many(content)
                .ok()
                .map(|content| Message::Assistant { id: None, content })
                .into_iter()
                .collect()
        }
        Role::Tool => OneOrMany::many(
            message
                .tool_results
                .iter()
                .map(|result| {
                    UserContent::ToolResult(ToolResult {
                        id: result.call_id.clone(),
                        call_id: Some(result.call_id.clone()),
                        content: OneOrMany::one(ToolResultContent::Text(Text {
                            text: result.content.clone(),
                            additional_params: None,
                        })),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .ok()
        .map(|content| Message::User { content })
        .into_iter()
        .collect(),
    }
}

pub fn rig_messages_from_messages(
    messages: &[ProviderMessage],
) -> Result<(Message, Vec<Message>), ProviderError> {
    let mut messages = messages
        .iter()
        .flat_map(rig_messages_from_provider_message)
        .collect::<Vec<_>>();
    let Some(prompt) = messages.pop() else {
        return Err(invalid_request(
            "Rig provider request requires at least one user message",
        ));
    };
    Ok((prompt, messages))
}

/// Build raw Responses input items through Rig's patched message conversion.
pub fn input_items_from_messages(
    messages: &[ProviderMessage],
) -> Result<Vec<serde_json::Value>, ProviderError> {
    messages
        .iter()
        .flat_map(rig_messages_from_provider_message)
        .map(Vec::<InputItem>::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_request("invalid native replay input"))?
        .into_iter()
        .flat_map(std::iter::IntoIterator::into_iter)
        .map(|item| {
            serde_json::to_value(item).map_err(|_| invalid_request("invalid native replay item"))
        })
        .collect()
}

/// Convert tool outputs through the same patched Rig Responses conversion.
pub fn function_call_output_items(
    results: &[ProviderToolResultBlock],
) -> Result<Vec<serde_json::Value>, ProviderError> {
    let message = ProviderMessage::tool_results(results.to_vec());
    input_items_from_messages(&[message])
}

fn invalid_request(message: impl Into<String>) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::InvalidRequest,
        message: message.into(),
        redacted_debug: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        NativeReplayState, NativeReplayStore, NativeReplayTarget, function_call_output_items,
        input_items_from_messages, instructions_from_messages,
    };
    use crate::{ProviderMessage, ProviderToolCall, ProviderToolResultBlock, Role, SessionId};

    #[test]
    fn replay_envelope_preserves_instruction_bytes_and_provider_items() {
        let messages = vec![
            ProviderMessage::text(Role::System, "first system\n"),
            ProviderMessage::text(Role::System, "second system"),
            ProviderMessage::text(Role::User, "question"),
            ProviderMessage::assistant(
                "",
                vec![ProviderToolCall {
                    call_id: String::from("call_provider_1"),
                    name: String::from("read_text_file"),
                    arguments_json: json!({"path":"Cargo.toml"}),
                }],
            ),
            ProviderMessage::tool_results(vec![ProviderToolResultBlock {
                call_id: String::from("call_provider_1"),
                content: String::from("Cargo.toml: file, 10 bytes"),
            }]),
        ];

        assert_eq!(
            instructions_from_messages(&messages),
            "first system\n\n\nsecond system"
        );
        assert_eq!(
            input_items_from_messages(&messages),
            Ok(vec![
                json!({"type":"message","role":"user","content":[{"type":"input_text","text":"question"}]}),
                json!({"type":"function_call","call_id":"call_provider_1","name":"read_text_file","arguments":"{\"path\":\"Cargo.toml\"}","status":"completed"}),
                json!({"type":"function_call_output","call_id":"call_provider_1","output":"Cargo.toml: file, 10 bytes","status":"completed"}),
            ])
        );
    }

    #[test]
    fn blank_system_messages_use_existing_default_instructions() {
        assert_eq!(
            instructions_from_messages(&[ProviderMessage::text(Role::System, "  ")]),
            "Follow the user instruction exactly."
        );
    }

    #[test]
    fn function_output_items_preserve_provider_result_text() {
        let values = function_call_output_items(&[ProviderToolResultBlock {
            call_id: String::from("call_provider_2"),
            content: String::from("\n"),
        }]);
        assert_eq!(
            values,
            Ok(vec![json!({
                "type":"function_call_output",
                "call_id":"call_provider_2",
                "output":"\n",
                "status":"completed"
            })])
        );
    }

    #[test]
    fn artifact_round_trip_preserves_replay_target_and_payload() {
        let target = NativeReplayTarget {
            session_id: SessionId(String::from("session-1")),
            provider: String::from("openai"),
            model: String::from("gpt-5"),
            connection: String::new(),
        };
        let state = NativeReplayState {
            target: target.clone(),
            instructions: String::from("instructions"),
            input: vec![json!({"type":"compaction","id":"window"})],
            synced_event_count: 0,
        };
        let store: NativeReplayStore = std::sync::Arc::new(std::sync::Mutex::new(
            super::NativeReplayStoreState::Active(state.clone()),
        ));
        assert!(store.lock().is_ok());
        assert!(state.matches_target(&target));
        let artifact = state.artifact_json();
        assert!(artifact.is_ok());
        let Ok(artifact) = artifact else {
            return;
        };
        assert_eq!(NativeReplayState::from_artifact_json(artifact), Ok(state));
    }
    #[test]
    fn replay_artifact_rejects_an_empty_window() {
        let state = NativeReplayState {
            target: NativeReplayTarget {
                session_id: SessionId(String::from("session-1")),
                provider: String::from("openai"),
                model: String::from("gpt-5"),
                connection: String::from("opaque-connection"),
            },
            instructions: String::from("instructions"),
            input: vec![json!({"type":"compaction","id":"window"})],
            synced_event_count: 0,
        };
        let artifact = state.artifact_json();
        assert!(artifact.is_ok());
        let Ok(mut artifact) = artifact else {
            return;
        };
        artifact["input"] = json!([]);
        assert!(NativeReplayState::from_artifact_json(artifact).is_err());
    }

    #[test]
    fn replay_target_debug_redacts_connection_identity() {
        let target = NativeReplayTarget {
            session_id: SessionId(String::from("session-1")),
            provider: String::from("openai"),
            model: String::from("gpt-5"),
            connection: String::from("opaque-connection-secret"),
        };

        let debug = format!("{target:?}");
        assert!(debug.contains("connection_fingerprint"));
        assert!(!debug.contains("opaque-connection-secret"));
    }
}
