use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use rig::OneOrMany;
use rig::client::CompletionClient;
use rig::completion::message::{
    AssistantContent, Text, ToolCall, ToolFunction, ToolResult, ToolResultContent, UserContent,
};
use rig::completion::{
    CompletionError, CompletionModel, CompletionRequestBuilder, GetTokenUsage, Message,
    ToolDefinition,
};
use rig::providers::{anthropic, chatgpt, openai};
use rig::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamedAssistantContent,
    StreamingCompletionResponse, ToolCallDeltaContent,
};

use crate::{
    ProviderContinuationSubmission, ProviderContinuationToolResult, ProviderError,
    ProviderErrorKind, ProviderFinishReason, ProviderMessage, ProviderRequest, ProviderStreamEvent,
    ProviderToolAdvertisingError, ProviderToolCall, ProviderToolResultBlock, Role, TurnId,
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
pub struct RigOpenAiSmokeConfig {
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
    Anthropic {
        api_key: String,
        /// Override for Anthropic-messages-compatible aggregators (e.g.
        /// opencode Zen's `/zen/v1` messages endpoint). `None` uses the
        /// Anthropic API proper. Env-var provider wiring is a stopgap;
        /// the provider/model product surface is a slated design item.
        base_url: Option<String>,
    },
    /// OpenAI proper over the Responses API — rig's default client, the
    /// canonical endpoint. Aggregators wearing the chat-completions shape
    /// use `OpenAiCompatible` instead. No base-URL override until a
    /// Responses-speaking aggregator exists (design:
    /// `docs/superpowers/specs/2026-08-02-openai-responses-provider-design.md`).
    OpenAi {
        api_key: String,
    },
    ChatGptSubscription {
        token_dir: PathBuf,
    },
    /// OpenAI-chat-completions-shaped endpoints (Fireworks, opencode Zen's
    /// `/zen/v1/chat/completions` roster, and similar aggregators).
    OpenAiCompatible {
        base_url: String,
        api_key: String,
    },
}

/// Which request field carries the per-turn output budget.
///
/// Current OpenAI models reject `max_tokens` outright — the API answers
/// `unsupported_parameter` and names `max_completion_tokens` instead —
/// while aggregators wearing the chat-completions shape still take
/// `max_tokens`. Both spellings therefore have to be reachable.
///
/// This is per-provider capability data, resolved catalog-side as
/// `yach_catalog::OutputTokensParam` (baked -> user -> project -> env,
/// same layering as `max_tokens`) and converted to this transport enum
/// at the CLI boundary. The enum itself stays here rather than folding
/// into a single spelling because rig has no native concept of the
/// alternate parameter name — retiring it waits on upstream rig, not on
/// the catalog (same upstream gap as the missing tool-result error
/// flag, see `docs/superpowers/specs/2026-08-01-text-tool-results-design.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaxTokensParam {
    /// The original chat-completions spelling, and every path yach has
    /// measured working. Default so existing providers are unaffected.
    #[default]
    MaxTokens,
    /// Required by current OpenAI models.
    MaxCompletionTokens,
}

impl MaxTokensParam {
    /// Parses the configured spelling, falling back to the default for
    /// anything unrecognized.
    #[must_use]
    pub fn from_config_value(value: &str) -> Self {
        match value.trim() {
            "max_completion_tokens" => Self::MaxCompletionTokens,
            _ => Self::MaxTokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigProviderAdapterConfig {
    pub provider: RigProviderConfig,
    pub timeout: Duration,
    /// Per-turn output budget. Arrives catalog-resolved from the CLI
    /// (`yach_catalog::effective_output_budget`, layered over baked /
    /// user / project / env); this field is deliberately a plain number —
    /// resolution and provenance live in `yach-catalog`, not here.
    pub max_tokens: u64,
    /// Model context window used for compaction accounting. Arrives
    /// catalog-resolved from the CLI (`yach_catalog::resolve`), same
    /// layering as `max_tokens`; this field is deliberately a plain
    /// number for the same reason.
    pub context_window: u64,
    /// Which field carries `max_tokens` on the openai-compatible shape.
    pub max_tokens_param: MaxTokensParam,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigStreamMapper {
    turn_id: TurnId,
    provider_response_id: Option<String>,
}

impl RigStreamMapper {
    #[must_use]
    pub fn new(turn_id: TurnId) -> Self {
        Self {
            turn_id,
            provider_response_id: None,
        }
    }

    #[must_use]
    pub fn provider_response_id(&self) -> Option<&str> {
        self.provider_response_id.as_deref()
    }

    pub fn map_choice<R: Clone + GetTokenUsage>(
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
            RawStreamingChoice::FinalResponse(final_response) => {
                Some(ProviderStreamEvent::Completed {
                    turn_id: self.turn_id.clone(),
                    finish_reason: Some(ProviderFinishReason::Stop),
                    // Provider-reported usage when the final response
                    // carries it (yacht evidence requires real token
                    // counts; also the first step of the hybrid-accounting
                    // upgrade). An all-zero Usage maps to None at the
                    // boundary — the unreported case.
                    usage: provider_usage_from_rig(final_response.token_usage()),
                    provider_response_id: self.provider_response_id.clone(),
                })
            }
            RawStreamingChoice::MessageId(message_id) => {
                self.provider_response_id = Some(message_id);
                None
            }
            RawStreamingChoice::Reasoning { .. }
            | RawStreamingChoice::ReasoningDelta { .. }
            | RawStreamingChoice::TextStart { .. }
            | RawStreamingChoice::TextAdditionalParams(_)
            | RawStreamingChoice::Unknown(_) => None,
        }
    }
}

#[must_use]
pub fn map_raw_streaming_choice<R: Clone + GetTokenUsage>(
    turn_id: &TurnId,
    choice: RawStreamingChoice<R>,
) -> Option<ProviderStreamEvent> {
    let mut mapper = RigStreamMapper::new(turn_id.clone());
    mapper.map_choice(choice)
}

pub async fn run_provider_request(
    config: RigProviderAdapterConfig,
    request: ProviderRequest,
) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
    run_provider_request_with_approved_tools(config, request, ["project_path_info"]).await
}

pub async fn run_provider_request_with_approved_tools(
    config: RigProviderAdapterConfig,
    request: ProviderRequest,
    approved_tools: impl IntoIterator<Item = impl AsRef<str>>,
) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
    let (prompt, chat_history) = rig_messages_from_request(&request)?;
    let rig_tools =
        rig_tool_definitions_from_request_with_approved_tools(&request, approved_tools)?;
    let tool_policy = RigToolCallPolicy::from_tool_definitions(&rig_tools);
    let preamble = preamble_from_request(&request);
    let attempt = PreparedCompletion {
        request,
        prompt,
        chat_history,
        preamble,
        rig_tools,
        tool_policy,
        max_tokens: config.max_tokens,
        max_tokens_param: config.max_tokens_param,
        timeout: config.timeout,
    };
    match config.provider {
        RigProviderConfig::Anthropic { api_key, base_url } => {
            let mut builder = anthropic::Client::builder().api_key(&api_key);
            if let Some(base_url) = base_url.as_deref() {
                builder = builder.base_url(base_url);
            }
            let client = builder
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let model = client.completion_model(attempt.request.model.model.clone());
            attempt.run(model).await
        }
        RigProviderConfig::OpenAi { api_key } => {
            let client = openai::Client::builder()
                .api_key(&api_key)
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let model = client.completion_model(attempt.request.model.model.clone());
            attempt.run(model).await
        }
        RigProviderConfig::OpenAiCompatible { base_url, api_key } => {
            let client = openai::Client::builder()
                .api_key(&api_key)
                .base_url(&base_url)
                .build()
                .map_err(|error| provider_internal_error(&error))?
                .completions_api();
            let model = client.completion_model(attempt.request.model.model.clone());
            attempt.run(model).await
        }
        RigProviderConfig::ChatGptSubscription { token_dir } => {
            let client = chatgpt::Client::builder()
                .oauth()
                .token_dir(&token_dir)
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let model = client.completion_model(attempt.request.model.model.clone());
            attempt.run(model).await
        }
    }
}

/// The provider-independent share of one completion attempt: everything
/// `run_provider_request_with_approved_tools` prepares before the provider
/// branch picks a concrete client and model.
struct PreparedCompletion {
    request: ProviderRequest,
    prompt: Message,
    chat_history: Vec<Message>,
    preamble: String,
    rig_tools: Vec<ToolDefinition>,
    tool_policy: RigToolCallPolicy,
    max_tokens: u64,
    max_tokens_param: MaxTokensParam,
    timeout: Duration,
}

impl PreparedCompletion {
    /// Build the request on the given model, stream it, and collect the
    /// events. The provider branches were identical from this point on;
    /// only client and model construction differ per provider.
    async fn run<M>(self, model: M) -> Result<Vec<ProviderStreamEvent>, ProviderError>
    where
        M: CompletionModel,
        M::StreamingResponse: Clone + Unpin + GetTokenUsage,
    {
        let completion = build_completion_request(
            &model,
            self.prompt,
            self.chat_history,
            self.preamble,
            self.max_tokens,
            self.max_tokens_param,
            self.rig_tools,
        );
        let stream = tokio::time::timeout(self.timeout, async {
            model
                .stream(completion)
                .await
                .map_err(|error| map_completion_error(&error))
        })
        .await
        .map_err(|_| rig_provider_stream_timeout_error())??;
        collect_rig_completion_stream(
            stream,
            self.request.turn_id,
            self.request.model.provider,
            self.request.model.model,
            self.timeout,
            self.tool_policy,
        )
        .await
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
    messages.push(provider_continuation_guard_message());
    // All results answering one assistant turn ride a single message, which
    // is the shape providers expect: several `tool_result` blocks bound by
    // call id to the `tool_use` blocks of the turn before.
    if !tool_results.is_empty() {
        messages.push(ProviderMessage::tool_results(
            tool_results
                .iter()
                .map(provider_tool_result_block)
                .collect(),
        ));
    }
    ProviderRequest {
        turn_id,
        model,
        messages,
        extensions,
    }
}

fn provider_continuation_guard_message() -> ProviderMessage {
    ProviderMessage::text(
        Role::System,
        String::from(
            "Yach has executed exactly the tool results included in this continuation. \
You may call more advertised tools if more work is required, or answer only from executed \
evidence. Do not claim local effects unless they are present in the tool results.",
        ),
    )
}

/// One native `tool_result` block.
///
/// When native blocks replaced the flattened `Tool:` message, the
/// payload carried over byte-identical: that change moved where the
/// result sits (a native block bound by call id) without also
/// rewriting what it says, so the before/after measurement isolated
/// the structural variable. The payload's shape has since moved on —
/// it is now plain text (design:
/// `docs/superpowers/specs/2026-08-01-text-tool-results-design.md`)
/// rather than escaped JSON. Slimming the envelope — `provider_call_id`
/// on `ProviderToolResult` is now redundant with the block's own id —
/// is a separate, separately measured change.
fn provider_tool_result_block(result: &ProviderContinuationToolResult) -> ProviderToolResultBlock {
    // The tool's own result is already self-describing plain text:
    // failures carry their error label and guidance inline. The envelope
    // this replaces repeated a version of that as an escaped JSON string
    // nested one level deeper, so a model that wanted the content had to
    // unwrap twice.
    //
    // Dropped deliberately: `provider_call_id` (the block carries the
    // id), `status` (duplicated the payload's `outcome`, back when the
    // payload was JSON), `byte_count` and `truncated` (duplicated
    // inside), and `redacted` — which is a
    // session-log presentation flag meaning the summary line omits a
    // content-bearing payload, not a statement that the model's copy was
    // withheld. The model receives full content either way, so sending
    // it said nothing.
    let content = if result.content.is_empty() {
        // Denied and cancelled calls carry no payload at all, so the
        // verdict is the only thing left worth sending. Byte-emptiness,
        // not whitespace-emptiness: a completed read of a file containing
        // exactly "\n" (or a bash capture that is a lone blank line) is
        // one real byte of content, and the builders already guard that
        // case (`execute_read_text_file`'s `read.text.is_empty()`, the
        // bash `outcome.output.is_empty()` notice). Trimming here would
        // silently replace that byte with a synthesized verdict the
        // model never asked for.
        match &result.reason {
            Some(reason) if !reason.is_empty() => crate::tool_text::notice(&format!(
                "{}: {reason}",
                tool_outcome_label(result.status)
            )),
            _ => crate::tool_text::notice(tool_outcome_label(result.status)),
        }
    } else {
        result.content.clone()
    };
    ProviderToolResultBlock {
        call_id: result.provider_call_id.clone(),
        content,
    }
}

pub(crate) const fn tool_outcome_label(status: crate::ToolOutcome) -> &'static str {
    match status {
        crate::ToolOutcome::Completed => "completed",
        crate::ToolOutcome::Failed => "failed",
        crate::ToolOutcome::Denied => "denied",
        crate::ToolOutcome::Cancelled => "cancelled",
        crate::ToolOutcome::ValidationFailed => "validation_failed",
    }
}

/// Map yach's messages onto rig's native message array.
///
/// Returns the trailing turn and the history before it, because rig takes
/// the new turn separately from prior context. System messages are
/// excluded here; they become the preamble.
///
/// This replaced a flattening pass that rendered the whole conversation
/// as one `"User:\n...\n\nAssistant:\n...\n\nTool:\n{json}"` string.
/// That format was reproduced verbatim by a model that then called no
/// tools at all, and it left nothing binding a call to its result
/// (`records/2026-07-28-tool-call-baseline.md`).
fn rig_messages_from_request(
    request: &ProviderRequest,
) -> Result<(Message, Vec<Message>), ProviderError> {
    let mut history: Vec<Message> = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::System => {}
            Role::User => {
                if !message.content.trim().is_empty() {
                    history.push(Message::user(message.content.clone()));
                }
            }
            Role::Assistant => {
                let mut content: Vec<AssistantContent> = Vec::new();
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
                if let Ok(content) = OneOrMany::many(content) {
                    history.push(Message::Assistant { id: None, content });
                }
            }
            Role::Tool => {
                let results = message
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
                    .collect::<Vec<_>>();
                if let Ok(content) = OneOrMany::many(results) {
                    history.push(Message::User { content });
                }
            }
        }
    }

    // Providers require the exchange to end on a turn they can answer;
    // rig models that as a separate `prompt` argument.
    let Some(prompt) = history.pop() else {
        return Err(ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from("Rig provider request requires at least one user message"),
            redacted_debug: None,
        });
    };
    Ok((prompt, history))
}

fn preamble_from_request(request: &ProviderRequest) -> String {
    let preamble = request
        .messages
        .iter()
        .filter(|message| matches!(message.role, Role::System))
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
    rig_tool_definitions_from_request_with_approved_tools(request, ["project_path_info"])
}

pub fn rig_tool_definitions_from_request_with_approved_tools(
    request: &ProviderRequest,
    approved_names: impl IntoIterator<Item = impl AsRef<str>>,
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

    let approved_names = approved_names
        .into_iter()
        .map(|name| String::from(name.as_ref()))
        .collect::<BTreeSet<_>>();
    for tool in &advertising.tools {
        if !approved_names.contains(&tool.name) {
            return Err(ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                message: String::from("Rig provider tool advertising is not approved"),
                redacted_debug: Some(provider_tool_advertising_error_label(
                    &ProviderToolAdvertisingError::UnsupportedTool {
                        name: tool.name.clone(),
                    },
                )),
            });
        }
    }

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

/// Build one provider request from yach's own message array.
///
/// This is the seam yach actually operates at. rig's Agent was only
/// ever doing this and handing back a stream — it never ran a tool,
/// because yach executes every call itself behind review gating, the
/// sensitive-path chokepoint, and session persistence. Constructing the
/// request directly drops an abstraction that was already vestigial
/// (design: `specs/2026-07-31-rig-upgrade-own-the-loop-design.md`).
///
/// The output-budget spelling applies on every provider rather than
/// only the openai-compatible one: it is per-provider configuration,
/// and the default is the spelling every measured path already uses.
fn build_completion_request<M: CompletionModel>(
    model: &M,
    prompt: Message,
    chat_history: Vec<Message>,
    preamble: String,
    max_tokens: u64,
    max_tokens_param: MaxTokensParam,
    tools: Vec<ToolDefinition>,
) -> rig::completion::CompletionRequest {
    let builder = model
        .completion_request(prompt)
        .preamble(preamble)
        .messages(chat_history);
    // rig models only the `max_tokens` spelling, but skips that field
    // when None and flattens `additional_params` into the request body,
    // so the alternative is reachable without forking or upgrading it.
    let builder = match max_tokens_param {
        MaxTokensParam::MaxTokens => builder.max_tokens(max_tokens),
        MaxTokensParam::MaxCompletionTokens => {
            builder.additional_params(serde_json::json!({ "max_completion_tokens": max_tokens }))
        }
    };
    apply_rig_tool_definitions(builder, tools).build()
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
    let model = client.completion_model(config.model.clone());
    let stream = stream_smoke_completion(&model, config.max_tokens).await?;
    collect_rig_smoke_stream(stream, "chatgpt-subscription", config.model, config.timeout).await
}

pub async fn run_anthropic_smoke(
    config: RigAnthropicSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = anthropic::Client::builder()
        .api_key(&config.api_key)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let model = client.completion_model(config.model.clone());
    let stream = stream_smoke_completion(&model, config.max_tokens).await?;
    collect_rig_smoke_stream(stream, "anthropic", config.model, config.timeout).await
}

pub async fn run_openai_smoke(
    config: RigOpenAiSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = openai::Client::builder()
        .api_key(&config.api_key)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let model = client.completion_model(config.model.clone());
    let stream = stream_smoke_completion(&model, config.max_tokens).await?;
    collect_rig_smoke_stream(stream, "openai", config.model, config.timeout).await
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
    let model = client.completion_model(config.model.clone());
    let stream = stream_smoke_completion(&model, config.max_tokens).await?;
    collect_rig_smoke_stream(stream, config.provider_label, config.model, config.timeout).await
}

/// Build and start the one-prompt smoke request on the given model —
/// the smoke path's share of the Agent retirement: same model-level
/// seam as the production path, no tools, fixed preamble.
async fn stream_smoke_completion<M: CompletionModel>(
    model: &M,
    max_tokens: u64,
) -> Result<StreamingCompletionResponse<M::StreamingResponse>, ProviderError> {
    let completion = model
        .completion_request(Message::user(SMOKE_PROMPT))
        .preamble(String::from("Follow the user instruction exactly."))
        .max_tokens(max_tokens)
        .build();
    model
        .stream(completion)
        .await
        .map_err(|error| map_completion_error(&error))
}

async fn collect_rig_smoke_stream<R>(
    stream: StreamingCompletionResponse<R>,
    provider_label: impl Into<String>,
    model: String,
    timeout: Duration,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError>
where
    R: Clone + Unpin + GetTokenUsage,
{
    let provider_label = provider_label.into();
    let (events, text, provider_response_id) = collect_rig_stream_text(
        stream,
        TurnId(String::from("rig-smoke-turn")),
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
    turn_id: TurnId,
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
        turn_id: TurnId,
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
        usage: rig::completion::Usage,
    ) -> ProviderStreamEvent {
        ProviderStreamEvent::Completed {
            turn_id: self.turn_id.clone(),
            finish_reason: Some(if self.saw_tool_call {
                ProviderFinishReason::ToolCalls
            } else {
                ProviderFinishReason::Stop
            }),
            usage: provider_usage_from_rig(usage),
            provider_response_id,
        }
    }

    pub(crate) fn final_events(
        &self,
        provider_response_id: Option<String>,
        usage: rig::completion::Usage,
    ) -> Vec<ProviderStreamEvent> {
        if let Some(internal_call_id) = self
            .partial_tool_call_ids
            .difference(&self.completed_tool_call_ids)
            .next()
        {
            vec![incomplete_rig_tool_call_failure(
                &self.turn_id,
                internal_call_id,
            )]
        } else {
            vec![self.completed_event(provider_response_id, usage)]
        }
    }
}

pub(crate) async fn collect_rig_completion_stream<R>(
    mut stream: StreamingCompletionResponse<R>,
    turn_id: TurnId,
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

pub(crate) fn collect_rig_stream_item<R: GetTokenUsage>(
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
                    &internal_call_id,
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
                    &internal_call_id,
                )];
            }
            if let ToolCallDeltaContent::Name(name) = &content
                && !collection.policy.allows_tool_name(name)
            {
                return vec![unexpected_rig_tool_call_failure(
                    &collection.turn_id,
                    &internal_call_id,
                )];
            }

            collection.record_partial_tool_call(internal_call_id.clone());
            vec![map_tool_call_delta(
                &collection.turn_id,
                id,
                internal_call_id,
                content,
            )]
        }
        StreamedAssistantContent::Final(final_payload) => {
            collection.final_events(None, final_payload.token_usage())
        }
        StreamedAssistantContent::Reasoning(_)
        | StreamedAssistantContent::ReasoningDelta { .. }
        | StreamedAssistantContent::Unknown(_) => Vec::new(),
    }
}

fn unexpected_rig_tool_call_failure(
    turn_id: &TurnId,
    internal_call_id: &str,
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
    turn_id: &TurnId,
    internal_call_id: &str,
) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id: turn_id.clone(),
        error: ProviderError {
            kind: ProviderErrorKind::InvalidRequest,
            message: String::from(
                "provider stream ended before completing a tool call \
(usually output-token truncation; raise YACH_RIG_PROVIDER_MAX_TOKENS)",
            ),
            redacted_debug: Some(format!("internal_call_id={internal_call_id}")),
        },
    }
}

async fn collect_rig_stream_text<R>(
    mut stream: StreamingCompletionResponse<R>,
    turn_id: TurnId,
    provider_label: String,
    model: String,
    timeout: Duration,
) -> Result<(Vec<ProviderStreamEvent>, String, Option<String>), ProviderError>
where
    R: Clone + Unpin + GetTokenUsage,
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
        let item = item.map_err(|error| map_completion_error(&error))?;
        match item {
            StreamedAssistantContent::Text(delta) => {
                let choice = RawStreamingChoice::<R>::Message(delta.text);
                if let Some(event) = mapper.map_choice(choice) {
                    if let ProviderStreamEvent::TextDelta { delta, .. } = &event {
                        text.push_str(delta);
                    }
                    events.push(event);
                }
            }
            StreamedAssistantContent::ToolCall {
                tool_call,
                internal_call_id,
            } => {
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
            StreamedAssistantContent::ToolCallDelta {
                id,
                internal_call_id,
                content,
            } => {
                events.push(map_tool_call_delta(
                    &mapper.turn_id,
                    id,
                    internal_call_id,
                    content,
                ));
            }
            StreamedAssistantContent::Final(final_payload) => {
                if let Some(event) =
                    mapper.map_choice(RawStreamingChoice::FinalResponse(final_payload))
                {
                    events.push(event);
                }
            }
            StreamedAssistantContent::Reasoning(_)
            | StreamedAssistantContent::ReasoningDelta { .. }
            | StreamedAssistantContent::Unknown(_) => {}
        }
    }

    Ok((events, text, mapper.provider_response_id.clone()))
}

fn provider_usage_from_rig(usage: rig::completion::Usage) -> Option<crate::ProviderUsage> {
    // rig 0.41 dropped the Option that carried "the provider reported
    // nothing", so that signal is recovered at this boundary: a completed
    // response always consumes input tokens, making an all-zero Usage the
    // unreported case. The heuristic errs toward "do not trust this
    // number" — the safe direction for the context meter and for yacht
    // evidence (specs/2026-07-31-rig-upgrade-own-the-loop-design.md,
    // owner decision 1).
    if usage.input_tokens == 0 && usage.output_tokens == 0 && usage.total_tokens == 0 {
        return None;
    }
    Some(crate::ProviderUsage {
        input_tokens: Some(usage.input_tokens),
        output_tokens: Some(usage.output_tokens),
        total_tokens: Some(usage.total_tokens),
    })
}

fn provider_internal_error(error: &impl ToString) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::ProviderInternal,
        message: String::from("Rig smoke setup failed"),
        redacted_debug: Some(redact_secrets(&error.to_string())),
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
    } else if lower.contains("context")
        || lower.contains("token limit")
        // Anthropic overflow 400s say "prompt is too long: X tokens > Y
        // maximum" — no "context" anywhere; overflow recovery keys on this
        // kind, so the phrasing must classify here.
        || lower.contains("prompt is too long")
        || lower.contains("too many tokens")
    {
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
    // After the model/context branches: those errors also arrive typed
    // invalid_request_error. Billing and other request-shaped 400s must
    // fail fast, not retry as transient provider_internal.
    } else if lower.contains("credit balance")
        || lower.contains("billing")
        || lower.contains("invalid_request_error")
    {
        ProviderErrorKind::InvalidRequest
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
pub fn map_backpressure_error(turn_id: TurnId) -> ProviderStreamEvent {
    ProviderStreamEvent::Failed {
        turn_id,
        error: ProviderError::backpressure(),
    }
}

#[must_use]
pub fn map_cancelled(turn_id: TurnId, reason: impl Into<String>) -> ProviderStreamEvent {
    ProviderStreamEvent::Cancelled {
        turn_id,
        reason: Some(reason.into()),
    }
}

fn map_tool_call_delta(
    turn_id: &TurnId,
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
    use rig::completion::Message;
    use rig::completion::message::{AssistantContent, ToolCall, ToolFunction, UserContent};
    use rig::providers::anthropic;
    use rig::streaming::{StreamedAssistantContent, ToolCallDeltaContent};

    use super::{
        MaxTokensParam, RigToolCallCollection, RigToolCallPolicy, apply_rig_tool_definitions,
        collect_rig_stream_item, preamble_from_request, provider_tool_advertising_error_label,
        provider_tool_result_block, rig_messages_from_request, rig_tool_definitions_from_request,
        rig_tool_definitions_from_request_with_approved_tools,
    };
    use crate::{
        PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY, ProviderContinuationToolResult, ProviderError,
        ProviderErrorKind, ProviderExtension, ProviderFinishReason, ProviderMessage, ProviderModel,
        ProviderRequest, ProviderStreamEvent, ProviderToolCall, ProviderToolResultBlock,
        ProviderToolVisibility, Role, ToolDefinition, ToolInputSchema, ToolOutcome, TurnId,
        build_project_path_info_provider_tool_advertising_extension,
        build_provider_tool_advertising_extension,
    };

    fn provider_request(messages: Vec<ProviderMessage>) -> ProviderRequest {
        ProviderRequest {
            turn_id: TurnId(String::from("turn-1")),
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
            ..provider_request(vec![ProviderMessage::text(
                Role::User,
                String::from("inspect cargo"),
            )])
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
    fn rig_provider_messages_preserve_ordered_transcript_context() {
        let request = provider_request(vec![
            ProviderMessage::text(Role::User, String::from("first question")),
            ProviderMessage::text(Role::Assistant, String::from("first answer")),
            ProviderMessage::text(Role::User, String::from("follow up")),
        ]);

        let mapped = rig_messages_from_request(&request).ok();
        let Some((prompt, history)) = mapped else {
            unreachable!("mapping should succeed");
        };

        // Prior turns become real history; the trailing turn is the prompt.
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0], Message::User { .. }));
        assert!(matches!(history[1], Message::Assistant { .. }));
        assert!(matches!(prompt, Message::User { .. }));
    }

    fn empty_content_continuation_result(
        status: ToolOutcome,
        reason: Option<&str>,
    ) -> ProviderContinuationToolResult {
        ProviderContinuationToolResult {
            tool_request_id: String::from("tool-request-1"),
            provider_call_id: String::from("call-1"),
            status,
            content: String::new(),
            byte_count: 0,
            redacted: true,
            truncated: false,
            reason: reason.map(String::from),
        }
    }

    #[test]
    fn provider_tool_result_block_synthesizes_denied_verdict_from_empty_content() {
        let result = empty_content_continuation_result(ToolOutcome::Denied, Some("user_denied"));

        let block = provider_tool_result_block(&result);

        assert_eq!(block.content, "[denied: user_denied]");
    }

    #[test]
    fn provider_tool_result_block_synthesizes_bare_verdict_when_reason_is_absent() {
        let result = empty_content_continuation_result(ToolOutcome::Cancelled, None);

        let block = provider_tool_result_block(&result);

        assert_eq!(block.content, "[cancelled]");
    }

    #[test]
    fn provider_tool_result_block_passes_whitespace_only_completed_content_through_byte_exact() {
        let result = ProviderContinuationToolResult {
            tool_request_id: String::from("tool-request-1"),
            provider_call_id: String::from("call-1"),
            status: ToolOutcome::Completed,
            content: String::from("\n"),
            byte_count: 1,
            redacted: false,
            truncated: false,
            reason: None,
        };

        let block = provider_tool_result_block(&result);

        // A byte of real content (a file that is exactly one blank line)
        // must not be swallowed by whitespace-trimming into a synthesized
        // "[completed]" the model would mistake for "nothing happened".
        assert_eq!(block.content, "\n");
    }

    #[test]
    fn rig_provider_messages_carry_tool_calls_and_results_natively() {
        let request = provider_request(vec![
            ProviderMessage::text(Role::User, String::from("do the thing")),
            ProviderMessage::assistant(
                "on it",
                vec![ProviderToolCall {
                    call_id: String::from("call-1"),
                    name: String::from("read_text_file"),
                    arguments_json: serde_json::json!({"path": "a.txt"}),
                }],
            ),
            ProviderMessage::tool_results(vec![ProviderToolResultBlock {
                call_id: String::from("call-1"),
                content: String::from("file body"),
            }]),
        ]);

        let mapped = rig_messages_from_request(&request).ok();
        let Some((prompt, history)) = mapped else {
            unreachable!("mapping should succeed");
        };

        // The assistant turn carries a real tool_use block...
        let Some(Message::Assistant { content, .. }) = history.get(1) else {
            unreachable!("second history entry should be the assistant turn");
        };
        assert!(
            content.iter().any(
                |part| matches!(part, AssistantContent::ToolCall(call) if call.id == "call-1")
            )
        );
        // ...and the trailing turn answers it with a bound tool_result.
        let Message::User { content } = prompt else {
            unreachable!("tool results ride a user-role message");
        };
        assert!(
            content.iter().any(
                |part| matches!(part, UserContent::ToolResult(result) if result.id == "call-1")
            )
        );
    }

    #[test]
    fn max_tokens_param_defaults_to_the_measured_spelling() {
        // Unrecognized values fall back rather than failing a run: the
        // wrong spelling is a loud provider error, not a silent one.
        assert_eq!(MaxTokensParam::default(), MaxTokensParam::MaxTokens);
        assert_eq!(
            MaxTokensParam::from_config_value("max_tokens"),
            MaxTokensParam::MaxTokens
        );
        assert_eq!(
            MaxTokensParam::from_config_value("  max_completion_tokens "),
            MaxTokensParam::MaxCompletionTokens
        );
        assert_eq!(
            MaxTokensParam::from_config_value("nonsense"),
            MaxTokensParam::MaxTokens
        );
    }

    #[test]
    fn rig_provider_messages_keep_system_messages_in_preamble_only() {
        let request = provider_request(vec![
            ProviderMessage::text(Role::System, String::from("system guidance")),
            ProviderMessage::text(Role::User, String::from("visible prompt")),
        ]);

        let mapped = rig_messages_from_request(&request).ok();
        let Some((_, history)) = mapped else {
            unreachable!("mapping should succeed");
        };

        assert!(
            history.is_empty(),
            "system guidance belongs to the preamble"
        );
    }

    #[test]
    fn rig_provider_preamble_preserves_static_context_system_message() {
        let request = provider_request(vec![
            ProviderMessage::text(
                Role::System,
                String::from("# AGENTS.md instructions for .\n\nroot rules"),
            ),
            ProviderMessage::text(Role::User, String::from("hello")),
        ]);

        assert_eq!(
            preamble_from_request(&request),
            "# AGENTS.md instructions for .\n\nroot rules"
        );
        let mapped = rig_messages_from_request(&request).ok();
        let Some((prompt, history)) = mapped else {
            unreachable!("mapping should succeed");
        };
        assert!(history.is_empty());
        assert!(matches!(prompt, Message::User { .. }));
    }

    #[test]
    fn rig_provider_messages_require_a_turn_the_provider_can_answer() {
        let request = provider_request(vec![ProviderMessage::text(
            Role::System,
            String::from("guidance only"),
        )]);

        let error = rig_messages_from_request(&request).err();

        assert_eq!(
            error.as_ref().map(|error| error.kind),
            Some(crate::ProviderErrorKind::InvalidRequest)
        );
    }

    #[test]
    fn rig_adapter_projects_advertising_to_schema_only_tool_definition() {
        let extension = build_project_path_info_provider_tool_advertising_extension();
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let request = provider_request_with_extensions(vec![extension]);

        let tools = rig_tool_definitions_from_request(&request);
        assert!(tools.is_ok());
        let Some(tools) = tools.ok() else {
            return;
        };

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
        let extension =
            build_provider_tool_advertising_extension(&[ToolDefinition::extension_metadata_tool(
                "example.toy-tools",
                "toy_tool",
                "Return static fixture metadata.",
                ToolInputSchema::string_object(["label"], std::iter::empty::<&str>(), 512),
                ProviderToolVisibility::Visible,
            )]);
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let request = provider_request_with_extensions(vec![extension]);

        let tools = rig_tool_definitions_from_request_with_approved_tools(&request, ["toy_tool"]);
        assert!(tools.is_ok());
        let Some(tools) = tools.ok() else {
            return;
        };

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "toy_tool");
    }

    #[test]
    fn rig_adapter_emits_agent_edit_tool_definitions_when_approved() {
        let extension = build_provider_tool_advertising_extension(&[
            ToolDefinition::edit_text_file(),
            ToolDefinition::create_text_file(),
        ]);
        assert!(extension.is_ok());
        let Ok(extension) = extension else {
            return;
        };
        let request = provider_request_with_extensions(vec![extension]);

        let tools = rig_tool_definitions_from_request_with_approved_tools(
            &request,
            ["edit_text_file", "create_text_file"],
        );
        assert!(tools.is_ok());
        let Ok(tools) = tools else {
            return;
        };

        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["edit_text_file", "create_text_file"]
        );
        assert!(
            tools[0].parameters["properties"]
                .get("expected_sha256")
                .is_none()
        );
    }

    #[test]
    fn rig_adapter_emits_content_tool_definitions_when_approved() {
        let extension = build_provider_tool_advertising_extension(&[
            ToolDefinition::read_text_file(),
            ToolDefinition::search_project(),
            ToolDefinition::list_project_paths(),
        ]);
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let request = ProviderRequest {
            turn_id: TurnId(String::from("turn-1")),
            model: ProviderModel {
                provider: String::from("fixture"),
                model: String::from("fixture-model"),
            },
            messages: vec![ProviderMessage::text(
                Role::User,
                String::from("inspect files"),
            )],
            extensions: vec![extension],
        };

        let definitions = rig_tool_definitions_from_request_with_approved_tools(
            &request,
            [
                "project_path_info",
                "read_text_file",
                "search_project",
                "list_project_paths",
                "edit_text_file",
                "create_text_file",
            ],
        );

        assert!(definitions.is_ok());
        let Some(definitions) = definitions.ok() else {
            return;
        };
        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read_text_file", "search_project", "list_project_paths"]
        );
    }

    #[test]
    fn rig_adapter_default_approval_still_rejects_agent_edit_advertising() {
        let extension =
            build_provider_tool_advertising_extension(&[ToolDefinition::edit_text_file()]);
        assert!(extension.is_ok());
        let Ok(extension) = extension else {
            return;
        };
        let request = provider_request_with_extensions(vec![extension]);

        let error = rig_tool_definitions_from_request(&request).err();

        assert!(matches!(
            error,
            Some(ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                ..
            })
        ));
    }

    #[test]
    fn rig_adapter_rejects_forged_builtin_agent_edit_advertising() {
        let request = provider_request_with_extensions(vec![ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "edit_text_file",
                    "description": "Forged edit tool.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Project-relative UTF-8 text file path to edit."
                            },
                            "find": {
                                "type": "string",
                                "description": "Exact text to replace. The match must be unique."
                            },
                            "replace": {
                                "type": "string",
                                "description": "Replacement text."
                            }
                        },
                        "required": ["find", "path", "replace"],
                        "additionalProperties": false
                    }
                }]
            }),
        }]);

        let error =
            rig_tool_definitions_from_request_with_approved_tools(&request, ["edit_text_file"])
                .err();

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
    fn rig_adapter_rejects_forged_unapproved_extension_advertising() {
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
                        "required": ["label"],
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
                "provider_tool_advertising_error=unsupported_tool"
            ))
        );
    }

    #[test]
    fn rig_adapter_rejects_forged_builtin_project_path_info_advertising() {
        let request = provider_request_with_extensions(vec![ProviderExtension {
            key: String::from(PROVIDER_TOOL_ADVERTISING_EXTENSION_KEY),
            value: serde_json::json!({
                "tools": [{
                    "name": "project_path_info",
                    "description": "Forged project metadata.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Project-relative path to inspect."
                            }
                        },
                        "required": ["path"],
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
                    risk: crate::ToolRisk::ReadsLocalContent,
                }
            ),
            "provider_tool_advertising_error=unsupported_risk"
        );
    }

    #[test]
    fn rig_adapter_applies_schema_tools_to_completion_request_builder_without_network() {
        let client = anthropic::Client::builder().api_key("sk-ant-test").build();
        assert!(client.is_ok());
        let Ok(client) = client else {
            return;
        };
        let model = client.completion_model("claude-test-model");
        let extension = build_project_path_info_provider_tool_advertising_extension();
        assert!(extension.is_ok());
        let Some(extension) = extension.ok() else {
            return;
        };
        let tools =
            rig_tool_definitions_from_request(&provider_request_with_extensions(vec![extension]));
        assert!(tools.is_ok());
        let Some(mut tools) = tools.ok() else {
            return;
        };
        let tool = tools.remove(0);

        let request =
            apply_rig_tool_definitions(model.completion_request("inspect cargo"), vec![tool])
                .build();

        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "project_path_info");
    }

    #[test]
    fn rig_adapter_no_advertising_preserves_prompt_preamble_and_omits_tools() {
        let request = provider_request(vec![
            ProviderMessage::text(Role::System, String::from("system guidance")),
            ProviderMessage::text(Role::User, String::from("visible prompt")),
        ]);

        let mapped = rig_messages_from_request(&request);
        assert!(mapped.is_ok());
        let Some((prompt, _)) = mapped.ok() else {
            return;
        };
        let preamble = preamble_from_request(&request);
        let tools = rig_tool_definitions_from_request(&request);
        assert!(tools.is_ok());
        let Some(tools) = tools.ok() else {
            return;
        };

        let client = anthropic::Client::builder().api_key("sk-ant-test").build();
        assert!(client.is_ok());
        let Ok(client) = client else {
            return;
        };
        let model = client.completion_model("claude-test-model");
        let completion = apply_rig_tool_definitions(
            model
                .completion_request(prompt.clone())
                .preamble(preamble.clone())
                .max_tokens(64),
            tools,
        )
        .build();
        let serialized = serde_json::to_string(&completion);
        assert!(serialized.is_ok());
        let Some(serialized) = serialized.ok() else {
            return;
        };

        assert!(matches!(prompt, Message::User { .. }));
        assert_eq!(preamble, "system guidance");
        assert!(completion.tools.is_empty());
        assert_eq!(completion.max_tokens, Some(64));
        assert!(serialized.contains("system guidance"));
        assert!(serialized.contains("visible prompt"));
    }

    #[test]
    fn rig_adapter_collects_advertised_tool_call_without_failure() {
        let mut collection = RigToolCallCollection::new(
            TurnId(String::from("turn-1")),
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
            TurnId(String::from("turn-1")),
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
            TurnId(String::from("turn-1")),
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
            TurnId(String::from("turn-1")),
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
    fn rig_adapter_final_payload_usage_reaches_completed_event() {
        use rig::completion::GetTokenUsage;
        #[derive(Clone)]
        struct UsagePayload;
        impl GetTokenUsage for UsagePayload {
            fn token_usage(&self) -> rig::completion::Usage {
                let mut usage = rig::completion::Usage::new();
                usage.input_tokens = 1_200;
                usage.output_tokens = 340;
                usage.total_tokens = 1_540;
                usage
            }
        }
        let mut collection = RigToolCallCollection::new(
            TurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );
        let events = collect_rig_stream_item(
            &mut collection,
            StreamedAssistantContent::Final(UsagePayload),
        );
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Completed {
                usage: Some(usage),
                ..
            }] if usage.input_tokens == Some(1_200) && usage.output_tokens == Some(340)
        ));
    }

    #[test]
    fn rig_adapter_all_zero_usage_stays_unreported() {
        // rig 0.41 made `token_usage` non-optional, so "the provider
        // reported nothing" arrives as an all-zero Usage. The boundary
        // predicate must map it to None rather than a reported zero,
        // or the meter and yacht's usage_source silently corrupt.
        use rig::completion::GetTokenUsage;
        #[derive(Clone)]
        struct SilentPayload;
        impl GetTokenUsage for SilentPayload {
            fn token_usage(&self) -> rig::completion::Usage {
                rig::completion::Usage::new()
            }
        }
        let mut collection = RigToolCallCollection::new(
            TurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );
        let events = collect_rig_stream_item(
            &mut collection,
            StreamedAssistantContent::Final(SilentPayload),
        );
        assert!(matches!(
            events.as_slice(),
            [ProviderStreamEvent::Completed { usage: None, .. }]
        ));
    }

    #[test]
    fn rig_adapter_rejects_tool_call_name_delta_not_in_advertised_policy() {
        let mut collection = RigToolCallCollection::new(
            TurnId(String::from("turn-1")),
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
            TurnId(String::from("turn-1")),
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
            TurnId(String::from("turn-1")),
            String::from("fixture-provider"),
            String::from("fixture-model"),
            advertised_project_path_info_policy(),
        );

        collection.record_tool_call();
        let completed = collection.completed_event(None, rig::completion::Usage::new());

        assert!(matches!(
            completed,
            ProviderStreamEvent::Completed {
                finish_reason: Some(ProviderFinishReason::ToolCalls),
                ..
            }
        ));
    }
}
