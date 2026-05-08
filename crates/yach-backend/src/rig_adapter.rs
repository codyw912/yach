use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use rig::agent::{MultiTurnStreamItem, StreamingError};
use rig::client::CompletionClient;
use rig::providers::{anthropic, chatgpt, openai};
use rig::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamedAssistantContent, StreamingPrompt,
    ToolCallDeltaContent,
};

use crate::{
    NativeRole, NativeTurnId, ProviderError, ProviderErrorKind, ProviderFinishReason,
    ProviderRequest, ProviderStreamEvent, ProviderToolCall,
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
            let stream = agent.stream_prompt(prompt).await;
            collect_rig_stream(
                stream,
                request.turn_id,
                request.model.provider,
                request.model.model,
                config.timeout,
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
            let stream = agent.stream_prompt(prompt).await;
            collect_rig_stream(
                stream,
                request.turn_id,
                request.model.provider,
                request.model.model,
                config.timeout,
            )
            .await
        }
    }
}

fn prompt_from_request(request: &ProviderRequest) -> Result<String, ProviderError> {
    let prompt = request
        .messages
        .iter()
        .filter(|message| matches!(message.role, NativeRole::User))
        .map(|message| message.content.as_str())
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

async fn collect_rig_stream<R>(
    stream: rig::agent::StreamingResult<R>,
    turn_id: NativeTurnId,
    provider_label: String,
    model: String,
    timeout: Duration,
) -> Result<Vec<ProviderStreamEvent>, ProviderError>
where
    R: Clone,
{
    collect_rig_stream_text(stream, turn_id, provider_label, model, timeout)
        .await
        .map(|(events, _, _)| events)
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
