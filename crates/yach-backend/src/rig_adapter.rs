use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use rig::agent::{MultiTurnStreamItem, StreamingError};
use rig::client::CompletionClient;
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequestBuilder, GetTokenUsage, Message,
    ToolDefinition,
};
use rig::providers::{anthropic, chatgpt, openai};
use rig::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamedAssistantContent, StreamingCompletion,
    StreamingCompletionResponse, StreamingPrompt, ToolCallDeltaContent,
};

use crate::{
    NativeRole, NativeTurnId, ProviderContinuationSubmission, ProviderContinuationToolResult,
    ProviderError, ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderRequest,
    ProviderStreamEvent, ProviderToolAdvertisingError, ProviderToolCall,
    parse_provider_tool_advertising_extensions,
};

const SMOKE_PROMPT: &str = "Reply with exactly: yach-rig-smoke-ok";
const EXPECTED_SMOKE_TEXT: &str = "yach-rig-smoke-ok";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigOpenAiCompatibleSmokeConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub provider_label: String,
    pub timeout: Duration,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigOpenAiCompatibleSmokeReport {
    pub provider_label: String,
    pub model: String,
    pub event_count: usize,
    pub text_delta_count: usize,
    pub completed: bool,
    pub matched_expected_text: bool,
    pub response_chars: usize,
    pub provider_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleHttpSmokeReport {
    pub status: u16,
    pub content_type: Option<String>,
    pub matched_expected_text: bool,
    pub response_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigAnthropicSmokeConfig {
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigChatGptSubscriptionSmokeConfig {
    pub model: String,
    pub token_dir: PathBuf,
    pub timeout: Duration,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigProviderConfig {
    Anthropic { api_key: String },
    ChatGptSubscription { token_dir: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigProviderAdapterConfig {
    pub provider: RigProviderConfig,
    pub timeout: Duration,
    pub max_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigStreamMapper {
    turn_id: NativeTurnId,
    provider_response_id: Option<String>,
}

impl RigStreamMapper {
    #[must_use]
    pub fn new(turn_id: NativeTurnId) -> Self {
        Self {
            turn_id,
            provider_response_id: None,
        }
    }

    #[must_use]
    pub fn provider_response_id(&self) -> Option<&str> {
        self.provider_response_id.as_deref()
    }

    pub fn map_choice<R: Clone>(
        &mut self,
        choice: RawStreamingChoice<R>,
    ) -> Option<ProviderStreamEvent> {
        match choice {
            RawStreamingChoice::Message(delta) => Some(ProviderStreamEvent::TextDelta {
                turn_id: self.turn_id.clone(),
                delta,
            }),
            RawStreamingChoice::ToolCall(tool_call) => {
                Some(ProviderStreamEvent::ToolCallCompleted {
                    turn_id: self.turn_id.clone(),
                    tool_call: map_raw_tool_call(tool_call),
                })
            }
            RawStreamingChoice::ToolCallDelta {
                id,
                internal_call_id,
                content,
            } => Some(map_tool_call_delta(
                &self.turn_id,
                id,
                internal_call_id,
                content,
            )),
            RawStreamingChoice::FinalResponse(_) => Some(ProviderStreamEvent::Completed {
                turn_id: self.turn_id.clone(),
                finish_reason: Some(ProviderFinishReason::Stop),
                usage: None,
                provider_response_id: self.provider_response_id.clone(),
            }),
            RawStreamingChoice::MessageId(message_id) => {
                self.provider_response_id = Some(message_id);
                None
            }
            RawStreamingChoice::Reasoning { .. } | RawStreamingChoice::ReasoningDelta { .. } => {
                None
            }
        }
    }
}

#[must_use]
pub fn map_raw_streaming_choice<R: Clone>(
    turn_id: &NativeTurnId,
    choice: RawStreamingChoice<R>,
) -> Option<ProviderStreamEvent> {
    let mut mapper = RigStreamMapper::new(turn_id.clone());
    mapper.map_choice(choice)
}

pub async fn run_provider_request(
    config: RigProviderAdapterConfig,
    request: ProviderRequest,
) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
    let prompt = prompt_from_request(&request)?;
    let rig_tools = rig_tool_definitions_from_request(&request)?;
    let tool_policy = RigToolCallPolicy::from_tool_definitions(&rig_tools);
    let timeout = config.timeout;
    match config.provider {
        RigProviderConfig::Anthropic { api_key } => {
            let client = anthropic::Client::builder()
                .api_key(&api_key)
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let preamble = preamble_from_request(&request);
            let agent = client
                .agent(request.model.model.clone())
                .preamble(&preamble)
                .max_tokens(config.max_tokens)
                .build();
            let stream = tokio::time::timeout(timeout, async {
                let mut builder = agent
                    .stream_completion(prompt, std::iter::empty::<Message>())
                    .await
                    .map_err(|error| map_completion_error(&error))?;
                builder = apply_rig_tool_definitions(builder, rig_tools);
                builder
                    .stream()
                    .await
                    .map_err(|error| map_completion_error(&error))
            })
            .await
            .map_err(|_| rig_provider_stream_timeout_error())??;
            collect_rig_completion_stream(
                stream,
                request.turn_id,
                request.model.provider,
                request.model.model,
                timeout,
                tool_policy,
            )
            .await
        }
        RigProviderConfig::ChatGptSubscription { token_dir } => {
            let client = chatgpt::Client::builder()
                .oauth()
                .token_dir(&token_dir)
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let preamble = preamble_from_request(&request);
            let agent = client
                .agent(request.model.model.clone())
                .preamble(&preamble)
                .max_tokens(config.max_tokens)
                .build();
            let stream = tokio::time::timeout(timeout, async {
                let mut builder = agent
                    .stream_completion(prompt, std::iter::empty::<Message>())
                    .await
                    .map_err(|error| map_completion_error(&error))?;
                builder = apply_rig_tool_definitions(builder, rig_tools);
                builder
                    .stream()
                    .await
                    .map_err(|error| map_completion_error(&error))
            })
            .await
            .map_err(|_| rig_provider_stream_timeout_error())??;
            collect_rig_completion_stream(
                stream,
                request.turn_id,
                request.model.provider,
                request.model.model,
                timeout,
                tool_policy,
            )
            .await
        }
    }
}

fn rig_provider_stream_timeout_error() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Timeout,
        message: String::from("Rig provider stream timed out"),
        redacted_debug: Some(String::from("timeout while starting provider stream")),
    }
}

#[must_use]
pub fn project_provider_continuation_request(
    submission: ProviderContinuationSubmission,
) -> ProviderRequest {
    let ProviderContinuationSubmission {
        turn_id,
        model,
        prior_messages,
        tool_results,
        extensions,
    } = submission;
    let mut messages = prior_messages;
    messages.extend(tool_results.iter().map(provider_tool_result_message));
    ProviderRequest {
        turn_id,
        model,
        messages,
        extensions,
    }
}

fn provider_tool_result_message(result: &ProviderContinuationToolResult) -> ProviderMessage {
    ProviderMessage {
        role: NativeRole::Tool,
        content: serde_json::json!({
            "provider_call_id": result.provider_call_id,
            "status": native_tool_outcome_label(result.status),
            "content": result.content,
            "byte_count": result.byte_count,
            "redacted": result.redacted,
            "truncated": result.truncated,
            "reason": result.reason,
        })
        .to_string(),
    }
}

const fn native_tool_outcome_label(status: crate::NativeToolOutcome) -> &'static str {
    match status {
        crate::NativeToolOutcome::Completed => "completed",
        crate::NativeToolOutcome::Failed => "failed",
        crate::NativeToolOutcome::Denied => "denied",
        crate::NativeToolOutcome::Cancelled => "cancelled",
        crate::NativeToolOutcome::ValidationFailed => "validation_failed",
    }
}

fn prompt_from_request(request: &ProviderRequest) -> Result<String, ProviderError> {
    let has_user_message = request.messages.iter().any(|message| {
        matches!(message.role, NativeRole::User) && !message.content.trim().is_empty()
    });
    if !has_user_message {
        return Err(ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider request requires at least one user message"),
            redacted_debug: None,
        });
    }

    let prompt = request
        .messages
        .iter()
        .filter(|message| !matches!(message.role, NativeRole::System))
        .filter(|message| !message.content.trim().is_empty())
        .map(|message| {
            format!(
                "{}:\n{}",
                rig_prompt_role_label(message.role),
                message.content
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if prompt.trim().is_empty() {
        Err(ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider request requires at least one user message"),
            redacted_debug: None,
        })
    } else {
        Ok(prompt)
    }
}

const fn rig_prompt_role_label(role: NativeRole) -> &'static str {
    match role {
        NativeRole::User => "User",
        NativeRole::Assistant => "Assistant",
        NativeRole::Tool => "Tool",
        NativeRole::System => "System",
    }
}

fn preamble_from_request(request: &ProviderRequest) -> String {
    let preamble = request
        .messages
        .iter()
        .filter(|message| matches!(message.role, NativeRole::System))
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if preamble.trim().is_empty() {
        String::from("Follow the user instruction exactly.")
    } else {
        preamble
    }
}

pub fn rig_tool_definitions_from_request(
    request: &ProviderRequest,
) -> Result<Vec<ToolDefinition>, ProviderError> {
    let Some(advertising) = parse_provider_tool_advertising_extensions(&request.extensions)
        .map_err(|error| ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider tool advertising is invalid"),
            redacted_debug: Some(provider_tool_advertising_error_label(&error)),
        })?
    else {
        return Ok(Vec::new());
    };

    Ok(advertising
        .tools
        .into_iter()
        .map(|tool| ToolDefinition {
            name: tool.name,
            description: tool.description,
            parameters: tool.parameters,
        })
        .collect())
}

fn provider_tool_advertising_error_label(error: &ProviderToolAdvertisingError) -> String {
    match error {
        ProviderToolAdvertisingError::Malformed => {
            String::from("provider_tool_advertising_error=malformed")
        }
        ProviderToolAdvertisingError::EmptyTools => {
            String::from("provider_tool_advertising_error=empty_tools")
        }
        ProviderToolAdvertisingError::DuplicateExtension => {
            String::from("provider_tool_advertising_error=duplicate_extension")
        }
        ProviderToolAdvertisingError::DuplicateToolName { .. } => {
            String::from("provider_tool_advertising_error=duplicate_tool_name")
        }
        ProviderToolAdvertisingError::UnsupportedTool { .. } => {
            String::from("provider_tool_advertising_error=unsupported_tool")
        }
        ProviderToolAdvertisingError::UnsupportedRisk { .. } => {
            String::from("provider_tool_advertising_error=unsupported_risk")
        }
        ProviderToolAdvertisingError::UnsupportedSchema { .. } => {
            String::from("provider_tool_advertising_error=unsupported_schema")
        }
    }
}

pub(crate) fn apply_rig_tool_definitions<M: CompletionModel>(
    builder: CompletionRequestBuilder<M>,
    tools: Vec<ToolDefinition>,
) -> CompletionRequestBuilder<M> {
    if tools.is_empty() {
        builder
    } else {
        builder.tools(tools)
    }
}

pub async fn run_chatgpt_subscription_smoke(
    config: RigChatGptSubscriptionSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = chatgpt::Client::builder()
        .oauth()
        .token_dir(&config.token_dir)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let agent = client
        .agent(config.model.clone())
        .preamble("Follow the user instruction exactly.")
        .max_tokens(config.max_tokens)
        .build();
    let stream = agent.stream_prompt(SMOKE_PROMPT).await;
    collect_rig_smoke_stream(stream, "chatgpt-subscription", config.model, config.timeout).await
}

pub async fn run_anthropic_smoke(
    config: RigAnthropicSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = anthropic::Client::builder()
        .api_key(&config.api_key)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let agent = client
        .agent(config.model.clone())
        .preamble("Follow the user instruction exactly.")
        .max_tokens(config.max_tokens)
        .build();
    let stream = agent.stream_prompt(SMOKE_PROMPT).await;
    collect_rig_smoke_stream(stream, "anthropic", config.model, config.timeout).await
}

pub async fn run_openai_compatible_smoke(
    config: RigOpenAiCompatibleSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = openai::Client::builder()
        .api_key(&config.api_key)
        .base_url(&config.base_url)
        .build()
        .map_err(|error| provider_internal_error(&error))?
        .completions_api();
    let agent = client
        .agent(config.model.clone())
        .preamble("Follow the user instruction exactly.")
        .max_tokens(config.max_tokens)
        .build();
    let stream = agent.stream_prompt(SMOKE_PROMPT).await;
    collect_rig_smoke_stream(stream, config.provider_label, config.model, config.timeout).await
}

async fn collect_rig_smoke_stream<R>(
    stream: rig::agent::StreamingResult<R>,
    provider_label: impl Into<String>,
    model: String,
    timeout: Duration,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError>
where
    R: Clone,
{
    let provider_label = provider_label.into();
    let (events, text, provider_response_id) = collect_rig_stream_text(
        stream,
        NativeTurnId(String::from("rig-smoke-turn")),
        provider_label.clone(),
        model.clone(),
        timeout,
    )
    .await?;
    let completed = events
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::Completed { .. }));
    let text_delta_count = events
        .iter()
        .filter(|event| matches!(event, ProviderStreamEvent::TextDelta { .. }))
        .count();
    Ok(RigOpenAiCompatibleSmokeReport {
        provider_label,
        model,
        event_count: events.len(),
        text_delta_count,
        completed,
        matched_expected_text: text.trim() == EXPECTED_SMOKE_TEXT
            || text.contains(EXPECTED_SMOKE_TEXT),
        response_chars: text.chars().count(),
        provider_response_id,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RigToolCallPolicy {
    Advertised { tool_names: BTreeSet<String> },
    Unexpected,
}

impl RigToolCallPolicy {
    fn from_tool_definitions(tools: &[ToolDefinition]) -> Self {
        if tools.is_empty() {
            Self::Unexpected
        } else {
            Self::Advertised {
                tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
            }
        }
    }

    fn allows_tool_name(&self, name: &str) -> bool {
        match self {
            Self::Advertised { tool_names } => tool_names.contains(name),
            Self::Unexpected => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RigToolCallCollection {
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    policy: RigToolCallPolicy,
    text: String,
    saw_tool_call: bool,
    partial_tool_call_ids: BTreeSet<String>,
    completed_tool_call_ids: BTreeSet<String>,
}

impl RigToolCallCollection {
    pub(crate) fn new(
        turn_id: NativeTurnId,
        provider_label: String,
        model: String,
        policy: RigToolCallPolicy,
    ) -> Self {
        Self {
            turn_id,
            provider_label,
            model,
            policy,
            text: String::new(),
            saw_tool_call: false,
            partial_tool_call_ids: BTreeSet::new(),
            completed_tool_call_ids: BTreeSet::new(),
        }
    }

    #[cfg(test)]
    pub(crate) const fn saw_tool_call(&self) -> bool {
        self.saw_tool_call
    }

    pub(crate) fn record_tool_call(&mut self) {
        self.saw_tool_call = true;
    }

    pub(crate) fn record_partial_tool_call(&mut self, internal_call_id: String) {
        self.record_tool_call();
        self.partial_tool_call_ids.insert(internal_call_id);
    }

    pub(crate) fn record_completed_tool_call(&mut self, internal_call_id: String) {
        self.record_tool_call();
        self.completed_tool_call_ids.insert(internal_call_id);
    }

    fn started_event(&self) -> ProviderStreamEvent {
        ProviderStreamEvent::Started {
            turn_id: self.turn_id.clone(),
            model: crate::ProviderModel {
                provider: self.provider_label.clone(),
                model: self.model.clone(),
            },
        }
    }

    pub(crate) fn completed_event(
        &self,
        provider_response_id: Option<String>,
    ) -> ProviderStreamEvent {
        ProviderStreamEvent::Completed {
            turn_id: self.turn_id.clone(),
            finish_reason: Some(if self.saw_tool_call {
                ProviderFinishReason::ToolCalls
            } else {
                ProviderFinishReason::Stop
            }),
            usage: None,
            provider_response_id,
        }
    }

    pub(crate) fn final_events(
        &self,
        provider_response_id: Option<String>,
    ) -> Vec<ProviderStreamEvent> {
        if let Some(internal_call_id) = self
            .partial_tool_call_ids
            .difference(&self.completed_tool_call_ids)
            .next()
        {
            vec![incomplete_rig_tool_call_failure(
                &self.turn_id,
                internal_call_id.clone(),
            )]
        } else {
            vec![self.completed_event(provider_response_id)]
        }
    }
}

pub(crate) async fn collect_rig_completion_stream<R>(
    mut stream: StreamingCompletionResponse<R>,
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    timeout: Duration,
    policy: RigToolCallPolicy,
) -> Result<Vec<ProviderStreamEvent>, ProviderError>
where
    R: Clone + Unpin + GetTokenUsage,
{
    let mut collection = RigToolCallCollection::new(turn_id, provider_label, model, policy);
    let mut events = vec![collection.started_event()];

    loop {
        let next = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| ProviderError {
                kind: ProviderErrorKind::Timeout,
                message: String::from("Rig provider stream timed out"),
                redacted_debug: Some(String::from("timeout while awaiting next stream event")),
            })?;
        let Some(item) = next else {
            break;
        };
        let item = item.map_err(|error| map_completion_error(&error))?;
        let mapped = collect_rig_stream_item(&mut collection, item);
        let failed = mapped
            .iter()
            .any(|event| matches!(event, ProviderStreamEvent::Failed { .. }));
        events.extend(mapped);
        if failed {
            break;
        }
    }

    Ok(events)
}

pub(crate) fn collect_rig_stream_item<R>(
    collection: &mut RigToolCallCollection,
    item: StreamedAssistantContent<R>,
) -> Vec<ProviderStreamEvent> {
    match item {
        StreamedAssistantContent::Text(delta) => {
            collection.text.push_str(&delta.text);
            vec![ProviderStreamEvent::TextDelta {
                turn_id: collection.turn_id.clone(),
                delta: delta.text,
            }]
        }
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id,
        } => {
            if !collection.policy.allows_tool_name(&tool_call.function.name) {
                return vec![unexpected_rig_tool_call_failure(
                    &collection.turn_id,
                    internal_call_id,
                )];
            }

            collection.record_completed_tool_call(internal_call_id);
            vec![ProviderStreamEvent::ToolCallCompleted {
                turn_id: collection.turn_id.clone(),
                tool_call: ProviderToolCall {
                    call_id: tool_call.call_id.unwrap_or(tool_call.id),
                    name: tool_call.function.name,
                    arguments_json: tool_call.function.arguments,
                },
            }]
        }
        StreamedAssistantContent::ToolCallDelta {
            id,
            internal_call_id,
            content,
        } => {
            if matches!(&collection.policy, RigToolCallPolicy::Unexpected) {
                return vec![unexpected_rig_tool_call_failure(
                    &collection.turn_id,
                    internal_call_id,
                )];
            }
            if let ToolCallDeltaContent::Name(name) = &content {
                if !collection.policy.allows_tool_name(name) {
                    return vec![unexpected_rig_tool_call_failure(
                        &collection.turn_id,
                        internal_call_id,
                    )];
                }
            }

            collection.record_partial_tool_call(internal_call_id.clone());
            vec![map_tool_call_delta(
                &collection.turn_id,
                id,
                internal_call_id,
                content,
            )]
        }
        StreamedAssistantContent::Final(_) => collection.final_events(None),
        StreamedAssistantContent::Reasoning(_)
        | StreamedAssistantContent::ReasoningDelta { .. } => Vec::new(),
    }
}

fn unexpected_rig_tool_call_failure(
    turn_id: &NativeTurnId,
    internal_call_id: String,
) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id: turn_id.clone(),
        error: ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider received an unexpected tool call"),
            redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
        },
    }
}

fn incomplete_rig_tool_call_failure(
    turn_id: &NativeTurnId,
    internal_call_id: String,
) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id: turn_id.clone(),
        error: ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider returned incomplete tool call"),
            redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
        },
    }
}

async fn collect_rig_stream_text<R>(
    mut stream: rig::agent::StreamingResult<R>,
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    timeout: Duration,
) -> Result<(Vec<ProviderStreamEvent>, String, Option<String>), ProviderError>
where
    R: Clone,
{
    let mut mapper = RigStreamMapper::new(turn_id.clone());
    let mut events = vec![ProviderStreamEvent::Started {
        turn_id,
        model: crate::ProviderModel {
            provider: provider_label,
            model,
        },
    }];
    let mut text = String::new();

    loop {
        let next = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| ProviderError {
                kind: ProviderErrorKind::Timeout,
                message: String::from("Rig provider stream timed out"),
                redacted_debug: Some(String::from("timeout while awaiting next stream event")),
            })?;
        let Some(item) = next else {
            break;
        };
        let item = item.map_err(|error| map_streaming_error(&error))?;
        match item {
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(delta)) => {
                let choice = RawStreamingChoice::<()>::Message(delta.text);
                if let Some(event) = mapper.map_choice(choice) {
                    if let ProviderStreamEvent::TextDelta { delta, .. } = &event {
                        text.push_str(delta);
                    }
                    events.push(event);
                }
            }
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                internal_call_id,
            }) => {
                events.push(ProviderStreamEvent::ToolCallCompleted {
                    turn_id: mapper.turn_id.clone(),
                    tool_call: ProviderToolCall {
                        call_id: tool_call.call_id.unwrap_or(tool_call.id),
                        name: tool_call.function.name,
                        arguments_json: tool_call.function.arguments,
                    },
                });
                events.push(ProviderStreamEvent::Failed {
                    turn_id: mapper.turn_id.clone(),
                    error: ProviderError {
                        kind: ProviderErrorKind::InvalidRequest,
                        message: String::from("Rig smoke received an unexpected tool call"),
                        redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
                    },
                });
                break;
            }
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCallDelta {
                id,
                internal_call_id,
                content,
            }) => {
                events.push(map_tool_call_delta(
                    &mapper.turn_id,
                    id,
                    internal_call_id,
                    content,
                ));
            }
            MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Final(_)) => {
                if let Some(event) = mapper.map_choice(RawStreamingChoice::FinalResponse(())) {
                    events.push(event);
                }
            }
            MultiTurnStreamItem::FinalResponse(response) => {
                response.response().clone_into(&mut text);
                if let Some(event) = mapper.map_choice(RawStreamingChoice::FinalResponse(())) {
                    events.push(event);
                }
            }
            _ => {}
        }
    }

    Ok((events, text, mapper.provider_response_id.clone()))
}

fn provider_internal_error(error: &impl ToString) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::ProviderInternal,
        message: String::from("Rig smoke setup failed"),
        redacted_debug: Some(redact_secrets(&error.to_string())),
    }
}

fn map_streaming_error(error: &StreamingError) -> ProviderError {
    let debug = error_chain(error);
    ProviderError {
        kind: classify_provider_error_debug(&debug),
        message: String::from("Rig smoke provider call failed"),
        redacted_debug: Some(redact_secrets(&debug)),
    }
}

fn map_completion_error(error: &CompletionError) -> ProviderError {
    let debug = error_chain(error);
    ProviderError {
        kind: classify_provider_error_debug(&debug),
        message: String::from("Rig provider call failed"),
        redacted_debug: Some(redact_secrets(&debug)),
    }
}

#[must_use]
pub fn classify_provider_error_debug(debug: &str) -> ProviderErrorKind {
    let lower = debug.to_ascii_lowercase();
    if lower.contains("auth")
        || lower.contains("api key")
        || lower.contains("401")
        || lower.contains("unauthorized")
    {
        ProviderErrorKind::Authentication
    } else if lower.contains("rate") || lower.contains("429") {
        ProviderErrorKind::RateLimited
    } else if lower.contains("context") || lower.contains("token limit") {
        ProviderErrorKind::ContextLength
    } else if lower.contains("model")
        && (lower.contains("not found")
            || lower.contains("not_found")
            || lower.contains("unavailable")
            || lower.contains("does not exist")
            || lower.contains("not supported")
            || lower.contains("invalid"))
    {
        ProviderErrorKind::UnavailableModel
    } else if lower.contains("timeout") || lower.contains("timed out") {
        ProviderErrorKind::Timeout
    } else if lower.contains("network") || lower.contains("connect") {
        ProviderErrorKind::Network
    } else {
        ProviderErrorKind::ProviderInternal
    }
}

fn error_chain(error: &(dyn Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        parts.push(error.to_string());
        source = error.source();
    }
    parts.join("; caused_by: ")
}

#[must_use]
pub fn redact_secrets(input: &str) -> String {
    input
        .split_whitespace()
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            if part.starts_with("sk-")
                || lower.contains("authorization")
                || lower.contains("api_key")
                || lower.contains("api-key")
                || lower.contains("apikey")
                || lower.contains("bearer")
            {
                "<redacted>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn run_openai_compatible_http_smoke(
    config: RigOpenAiCompatibleSmokeConfig,
) -> Result<OpenAiCompatibleHttpSmokeReport, ProviderError> {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let response = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()
        .map_err(|error| provider_internal_error(&error))?
        .post(url)
        .bearer_auth(&config.api_key)
        .json(&serde_json::json!({
            "model": config.model,
            "messages": [{"role": "user", "content": SMOKE_PROMPT}],
            "max_tokens": config.max_tokens,
            "stream": false,
        }))
        .send()
        .await
        .map_err(|error| ProviderError {
            kind: ProviderErrorKind::Network,
            message: String::from("OpenAI-compatible HTTP smoke request failed"),
            redacted_debug: Some(redact_secrets(&error_chain(&error))),
        })?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.map_err(|error| ProviderError {
        kind: ProviderErrorKind::Network,
        message: String::from("OpenAI-compatible HTTP smoke response read failed"),
        redacted_debug: Some(redact_secrets(&error_chain(&error))),
    })?;
    if !status.is_success() {
        return Err(ProviderError {
            kind: ProviderErrorKind::ProviderInternal,
            message: format!("OpenAI-compatible HTTP smoke returned status {status}"),
            redacted_debug: Some(redact_secrets(&body)),
        });
    }
    let text = extract_chat_completion_text(&body).unwrap_or_default();
    Ok(OpenAiCompatibleHttpSmokeReport {
        status: status.as_u16(),
        content_type,
        matched_expected_text: text.trim() == EXPECTED_SMOKE_TEXT
            || text.contains(EXPECTED_SMOKE_TEXT),
        response_chars: text.chars().count(),
    })
}

fn extract_chat_completion_text(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    value
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")?
        .as_str()
        .map(str::to_owned)
}

#[must_use]
pub fn map_raw_tool_call(tool_call: RawStreamingToolCall) -> ProviderToolCall {
    ProviderToolCall {
        call_id: tool_call.call_id.unwrap_or(tool_call.id),
        name: tool_call.name,
        arguments_json: tool_call.arguments,
    }
}

#[must_use]
pub fn map_backpressure_error(turn_id: NativeTurnId) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id,
        error: ProviderError::backpressure(),
    }
}

#[must_use]
pub fn map_cancelled(turn_id: NativeTurnId, reason: impl Into<String>) -> ProviderStreamEvent {
    ProviderStreamEvent::Cancelled {
        turn_id,
        reason: Some(reason.into()),
    }
}

fn map_tool_call_delta(
    turn_id: &NativeTurnId,
    id: String,
    internal_call_id: String,
    content: ToolCallDeltaContent,
) -> ProviderStreamEvent {
    let call_id = id.if_empty(internal_call_id);
    match content {
        ToolCallDeltaContent::Name(name) => ProviderStreamEvent::ToolCallStarted {
            turn_id: turn_id.clone(),
            call_id,
            name,
        },
        ToolCallDeltaContent::Delta(arguments_delta) => ProviderStreamEvent::ToolCallDelta {
            turn_id: turn_id.clone(),
            call_id,
            arguments_delta,
        },
    }
}

trait IfEmpty {
    fn if_empty(self, fallback: String) -> String;
}

impl IfEmpty for String {
    fn if_empty(self, fallback: String) -> String {
        if self.is_empty() { fallback } else { self }
    }
}

#[cfg(test)]
mod tests {
    use rig::client::CompletionClient;
    use rig::completion::CompletionModel;
    use rig::completion::message::{ToolCall, ToolFunction};
    use rig::providers::anthropic;
    use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};

    use super::{
        RigToolCallCollection, RigToolCallPolicy, apply_rig_tool_definitions,
        collect_rig_stream_item, preamble_from_request, prompt_from_request,
        provider_tool_advertising_error_label, rig_tool_definitions_from_request,
    };
    use crate::{
        NativeRole, NativeToolDefinition, NativeToolInputSchema, NativeTurnId,
        PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, ProviderErrorKind, ProviderExtension,
        ProviderFinishReason, ProviderMessage, ProviderModel, ProviderRequest, ProviderStreamEvent,
        ProviderToolVisibility, build_project_path_info_provider_tool_advertising_extension,
        build_provider_tool_advertising_extension,
    };

    fn provider_request(messages: Vec<ProviderMessage>) -> ProviderRequest {
        ProviderRequest {
            turn_id: NativeTurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture-provider"),
                model: String::from("fixture-model"),
            },
            messages,
            extensions: Vec::new(),
        }
    }

    fn provider_request_with_extensions(extensions: Vec<ProviderExtension>) -> ProviderRequest {
        ProviderRequest {
            extensions,
            ..provider_request(vec![ProviderMessage {
                role: NativeRole::User,
                content: String::from("inspect cargo"),
            }])
        }
    }

    fn advertised_tool_call(call_id: Option<&str>) -> ToolCall {
        named_tool_call("project_path_info", call_id)
    }

    fn named_tool_call(name: &str, call_id: Option<&str>) -> ToolCall {
        let call = ToolCall::new(
            String::from("provider-call-1"),
            ToolFunction::new(
                String::from(name),
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
        );
        match call_id {
            Some(call_id) => call.with_call_id(String::from(call_id)),
            None => call,
        }
    }

    fn advertised_project_path_info_policy() -> RigToolCallPolicy {
        RigToolCallPolicy::Advertised {
            tool_names: [String::from("project_path_info")].into_iter().collect(),
        }
    }

    #[test]
    fn rig_provider_prompt_preserves_ordered_transcript_context() {
        let request = provider_request(vec![
            ProviderMessage {
                role: NativeRole::User,
                content: String::from("first question"),
            },
            ProviderMessage {
                role: NativeRole::Assistant,
                content: String::from("first answer"),
            },
            ProviderMessage {
                role: NativeRole::User,
                content: String::from("follow up"),
            },
        ]);

        let prompt = prompt_from_request(&request).ok();

        assert_eq!(
            prompt.as_deref(),
            Some("User:\nfirst question\n\nAssistant:\nfirst answer\n\nUser:\nfollow up")
        );
    }

    #[test]
    fn rig_provider_prompt_keeps_system_messages_in_preamble_only() {
        let request = provider_request(vec![
            ProviderMessage {
                role: NativeRole::System,
                content: String::from("system guidance"),
            },
            ProviderMessage {
                role: NativeRole::User,
                content: String::from("visible prompt"),
            },
        ]);

        let prompt = prompt_from_request(&request).ok();

        assert_eq!(prompt.as_deref(), Some("User:\nvisible prompt"));
    }

    #[test]
    fn rig_provider_prompt_requires_non_empty_user_message() {
        let request = provider_request(vec![ProviderMessage {
            role: NativeRole::Assistant,
            content: String::from("orphan answer"),
        }]);

        let error = prompt_from_request(&request).err();

        assert_eq!(
            error.as_ref().map(|error| error.kind),
            Some(crate::ProviderErrorKind::InvalidRequest)
        );
    }

    #[test]
    fn rig_adapter_projects_advertising_to_schema_only_tool_definition() {
        let extension = build_project_path_info_provider_tool_advertising_extension()
            .expect("canonical advertising extension");
        let request = provider_request_with_extensions(vec![extension]);

        let tools = rig_tool_definitions_from_request(&request)
            .expect("advertising should project to rig tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "project_path_info");
        assert_eq!(
            tools[0]
                .parameters
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .and_then(|properties| properties.get("path"))
                .and_then(|path| path.get("type"))
                .and_then(serde_json::Value::as_str),
            Some("string")
        );
    }

    #[test]
    fn rig_adapter_projects_extension_advertising_to_schema_only_tool_definition() {
        let extension = build_provider_tool_advertising_extension(&[
            NativeToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                NativeToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            ),
        ])
        .expect("extension tool should advertise");
        let request = provider_request_with_extensions(vec![extension]);

        let tools = rig_tool_definitions_from_request(&request)
            .expect("advertising should project to rig tools");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "toy_tool");
    }

    #[test]
    fn rig_adapter_rejects_malformed_known_advertising_extension() {
        let request = provider_request_with_extensions(vec![ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({ "tools": [] }),
        }]);

        let error = rig_tool_definitions_from_request(&request).err();

        assert_eq!(
            error.as_ref().map(|error| error.kind),
            Some(ProviderErrorKind::InvalidRequest)
        );
    }

    #[test]
    fn rig_adapter_rejects_unsupported_advertised_tool_projection() {
        let request = provider_request_with_extensions(vec![ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "toy_tool",
                    "description": "Return static fixture metadata.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "description": "label argument for toy_tool."
                            }
                        },
                        "required": ["missing"],
                        "additionalProperties": false
                    }
                }]
            }),
        }]);

        let error = rig_tool_definitions_from_request(&request).err();

        assert_eq!(
            error.as_ref().map(|error| error.kind),
            Some(ProviderErrorKind::InvalidRequest)
        );
        assert_eq!(
            error.and_then(|error| error.redacted_debug),
            Some(String::from(
                "provider_tool_advertising_error=unsupported_schema"
            ))
        );
    }

    #[test]
    fn rig_adapter_provider_tool_advertising_error_labels_are_categorical() {
        assert_eq!(
            provider_tool_advertising_error_label(
                &crate::ProviderToolAdvertisingError::DuplicateToolName {
                    name: String::from("duplicate-sk-test-secret"),
                }
            ),
            "provider_tool_advertising_error=duplicate_tool_name"
        );
        assert_eq!(
            provider_tool_advertising_error_label(
                &crate::ProviderToolAdvertisingError::UnsupportedRisk {
                    name: String::from("risk-sk-test-secret"),
                    risk: crate::NativeToolRisk::ReadsLocalContent,
                }
            ),
            "provider_tool_advertising_error=unsupported_risk"
        );
    }

    #[test]
    fn rig_adapter_applies_schema_tools_to_completion_request_builder_without_network() {
        let client = anthropic::Client::builder()
            .api_key("sk-ant-test")
            .build()
            .expect("test client should build without network");
        let model = client.completion_model("claude-test-model");
        let tool = rig_tool_definitions_from_request(&provider_request_with_extensions(vec![
            build_project_path_info_provider_tool_advertising_extension()
                .expect("canonical advertising extension"),
        ]))
        .expect("advertising should project")
        .remove(0);

        let request =
            apply_rig_tool_definitions(model.completion_request("inspect cargo"), vec![tool])
                .build();

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "project_path_info");
    }

    #[test]
    fn rig_adapter_no_advertising_preserves_prompt_preamble_and_omits_tools() {
        let request = provider_request(vec![
            ProviderMessage {
                role: NativeRole::System,
                content: String::from("system guidance"),
            },
            ProviderMessage {
                role: NativeRole::User,
                content: String::from("visible prompt"),
            },
        ]);

        let prompt = prompt_from_request(&request).expect("prompt");
        let preamble = preamble_from_request(&request);
        let tools = rig_tool_definitions_from_request(&request).expect("no tools");

        let client = anthropic::Client::builder()
            .api_key("sk-ant-test")
            .build()
            .expect("test client should build without network");
        let model = client.completion_model("claude-test-model");
        let completion = apply_rig_tool_definitions(
            model
                .completion_request(prompt.clone())
                .preamble(preamble.clone())
                .max_tokens(64),
            tools,
        )
        .build();
        let serialized = serde_json::to_string(&completion).expect("serialize completion request");

        assert_eq!(prompt, "User:\nvisible prompt");
        assert_eq!(preamble, "system guidance");
        assert!(completion.tools.is_empty());
        assert_eq!(completion.max_tokens, Some(64));
        assert!(serialized.contains("system guidance"));
        assert!(serialized.contains("visible prompt"));
    }

    #[test]
    fn rig_adapter_collects_advertised_tool_call_without_failure() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        let events = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCall {
                tool_call: advertised_tool_call(Some("call-1")),
                internal_call_id: String::from("internal-call-1"),
            },
        );

        assert!(collection.saw_tool_call());
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::ToolCallCompleted { tool_call, .. }]
                if tool_call.call_id == "call-1"
                    && tool_call.name == "project_path_info"
                    && tool_call.arguments_json == serde_json::json!({ "path": "Cargo.toml" })
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProviderStreamEvent::Failed { .. }))
        );
    }

    #[test]
    fn rig_adapter_allows_completed_tool_call_when_stream_id_differs_from_provider_call_id() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        let started = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCallDelta {
                id: String::from("stream-item-1"),
                internal_call_id: String::from("internal-call-1"),
                content: ToolCallDeltaContent::Name(String::from("project_path_info")),
            },
        );
        let completed = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCall {
                tool_call: ToolCall::new(
                    String::from("stream-item-1"),
                    ToolFunction::new(
                        String::from("project_path_info"),
                        serde_json::json!({ "path": "Cargo.toml" }),
                    ),
                )
                .with_call_id(String::from("provider-call-1")),
                internal_call_id: String::from("internal-call-1"),
            },
        );
        let final_events =
            collect_rig_stream_item::<()>(&mut collection, StreamedAssistantContent::Final(()));

        assert!(matches!(
            started.as_slice(),
            [ProviderStreamEvent::ToolCallStarted { call_id, .. }]
                if call_id == "stream-item-1"
        ));
        assert!(matches!(
            completed.as_slice(),
            [ProviderStreamEvent::ToolCallCompleted { tool_call, .. }]
                if tool_call.call_id == "provider-call-1"
        ));
        assert!(matches!(
            final_events.as_slice(),
            [ProviderStreamEvent::Completed {
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                ..
            }]
        ));
    }

    #[test]
    fn rig_adapter_rejects_mixed_completed_and_incomplete_tool_calls() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        let completed = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCall {
                tool_call: advertised_tool_call(Some("provider-call-1")),
                internal_call_id: String::from("internal-call-1"),
            },
        );
        let partial = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCallDelta {
                id: String::from("stream-item-2"),
                internal_call_id: String::from("internal-call-2"),
                content: ToolCallDeltaContent::Name(String::from("project_path_info")),
            },
        );
        let final_events =
            collect_rig_stream_item::<()>(&mut collection, StreamedAssistantContent::Final(()));

        assert!(matches!(
            completed.as_slice(),
            [ProviderStreamEvent::ToolCallCompleted { tool_call, .. }]
                if tool_call.call_id == "provider-call-1"
        ));
        assert!(matches!(
            partial.as_slice(),
            [ProviderStreamEvent::ToolCallStarted { call_id, .. }]
                if call_id == "stream-item-2"
        ));
        assert!(matches!(
            final_events.as_slice(),
            [ProviderStreamEvent::Failed { error, .. }]
                if error.kind == ProviderErrorKind::InvalidRequest
                    && error.redacted_debug.as_deref()
                        == Some("internal_call_id=internal-call-2")
        ));
    }

    #[test]
    fn rig_adapter_rejects_tool_call_name_not_in_advertised_policy() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        let events = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCall {
                tool_call: named_tool_call("read", Some("call-1")),
                internal_call_id: String::from("internal-call-1"),
            },
        );

        assert!(!collection.saw_tool_call());
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Failed { error, .. }]
                if error.kind == ProviderErrorKind::InvalidRequest
        ));
    }

    #[test]
    fn rig_adapter_rejects_tool_call_name_delta_not_in_advertised_policy() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        let events = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCallDelta {
                id: String::from("call-1"),
                internal_call_id: String::from("internal-call-1"),
                content: ToolCallDeltaContent::Name(String::from("read")),
            },
        );

        assert!(!collection.saw_tool_call());
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Failed { error, .. }]
                if error.kind == ProviderErrorKind::InvalidRequest
        ));
    }

    #[test]
    fn rig_adapter_fails_unadvertised_tool_call() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            RigToolCallPolicy::Unexpected,
        );

        let events = collect_rig_stream_item::<()>(
            &mut collection,
            StreamedAssistantContent::ToolCall {
                tool_call: advertised_tool_call(None),
                internal_call_id: String::from("internal-call-1"),
            },
        );

        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Failed { error, .. }]
                if error.kind == ProviderErrorKind::InvalidRequest
        ));
    }

    #[test]
    fn rig_adapter_finish_reason_tracks_advertised_tool_calls() {
        let mut collection = RigToolCallCollection::new(
            NativeTurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        collection.record_tool_call();
        let completed = collection.completed_event(None);

        assert!(matches!(
            completed,
            ProviderStreamEvent::Completed {
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                ..
            }
        ));
    }
}
