use serde::Deserialize;
use serde_json::{Map, Value};
use yach_proto::{
    BackendState, DialogKind, DialogOption, DialogRequest, ForkMessage, MessageMeta, ModelInfo,
    Notification, ServerEvent, SessionMessage, SessionStats, ToolResult, TransportMessage,
    WidgetState,
};

use crate::capabilities::stock_rpc_handshake;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    EmptyLine,
    InvalidJson(String),
    UnsupportedMethod(String),
    MissingField(&'static str),
}

#[derive(Debug, Deserialize)]
struct PiRpcEnvelope {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, rename = "assistantMessageEvent")]
    assistant_message_event: Option<Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

pub fn parse_server_line(
    line: &str,
    message_id: impl Into<String>,
) -> Result<TransportMessage, ParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyLine);
    }

    let envelope: PiRpcEnvelope = serde_json::from_str(trimmed)
        .map_err(|error| ParseError::InvalidJson(error.to_string()))?;

    let meta = build_message_meta(message_id.into(), &envelope);
    let event = map_server_event(&envelope)?;

    Ok(TransportMessage::server(meta, event))
}

fn build_message_meta(message_id: String, envelope: &PiRpcEnvelope) -> MessageMeta {
    let mut meta = MessageMeta::new(message_id);

    if let Some(correlation_id) = &envelope.id {
        meta = meta.with_correlation_id(correlation_id.clone());
    }

    if let Some(stream_id) = optional_string_any(
        envelope,
        &["stream_id", "streamId", "session_id", "sessionId"],
    ) {
        meta = meta.with_stream_id(stream_id);
    }

    meta
}

fn map_server_event(envelope: &PiRpcEnvelope) -> Result<ServerEvent, ParseError> {
    match event_name(envelope) {
        Some("ready" | "turn_start") => Ok(ServerEvent::Ready {
            handshake: stock_rpc_handshake(),
        }),
        Some("agent_start") => Ok(ServerEvent::StatusUpdated {
            message: String::from("agent_started"),
        }),
        Some("response") => Ok(parse_response_event(envelope)),
        Some("message_update") => parse_message_update(envelope),
        Some("message_start" | "message_end" | "turn_end" | "agent_end") => {
            Ok(ServerEvent::StatusUpdated {
                message: event_name(envelope).unwrap_or("event").to_owned(),
            })
        }
        Some("prompt_delta" | "promptDelta") => Ok(ServerEvent::PromptDelta {
            session_id: required_string_any(envelope, &["session_id", "sessionId"])?,
            delta: required_string_any(envelope, &["delta"])?,
        }),
        Some("tool_call_started" | "toolCallStarted" | "tool_execution_start") => {
            Ok(ServerEvent::ToolCallStarted {
                tool_call_id: optional_string_any(envelope, &["toolCallId", "tool_call_id"]),
                tool_name: required_string_any(envelope, &["tool_name", "toolName"])?,
                preview: value_any(envelope, &["args"]).and_then(tool_preview),
            })
        }
        Some("tool_execution_update") => Ok(ServerEvent::StatusUpdated {
            message: format!(
                "tool_update:{}",
                required_string_any(envelope, &["toolName"])?
            ),
        }),
        Some("tool_execution_end") => {
            Ok(ServerEvent::ToolCallFinished(parse_tool_result(envelope)?))
        }
        Some("status_updated" | "setStatus") => Ok(ServerEvent::StatusUpdated {
            message: required_string_any(envelope, &["message", "statusKey"])?,
        }),
        Some("session_changed" | "switchSession") => Ok(ServerEvent::SessionChanged {
            session_id: required_string_any(envelope, &["session_id", "sessionId"])?,
        }),
        Some("model_changed" | "setModel") => Ok(ServerEvent::ModelChanged {
            model: required_string_any(envelope, &["model"])?,
        }),
        Some("notify") => Ok(ServerEvent::NotificationRaised(Notification {
            level: optional_string_any(envelope, &["level"])
                .unwrap_or_else(|| String::from("info")),
            message: required_string_any(envelope, &["message"])?,
        })),
        Some("setWidget" | "widget_updated") => Ok(ServerEvent::WidgetUpdated(WidgetState {
            id: required_string_any(envelope, &["id", "widgetKey"])?,
            title: optional_string_any(envelope, &["title", "widgetKey", "id"])
                .unwrap_or_else(|| String::from("Widget")),
            body: optional_string_any(envelope, &["body", "text"]).unwrap_or_default(),
        })),
        Some("setTitle" | "title_changed") => Ok(ServerEvent::TitleChanged {
            title: required_string_any(envelope, &["title"])?,
        }),
        Some("select" | "confirm" | "input" | "editor") => Ok(ServerEvent::DialogRequested(
            parse_dialog_request(envelope)?,
        )),
        Some(other) => Ok(unknown_event_status(other)),
        None => Ok(unknown_event_status("unknown")),
    }
}

fn unknown_event_status(event: &str) -> ServerEvent {
    let severity = if looks_like_core_event(event) {
        "unknown_core_event"
    } else {
        "unknown_event"
    };
    ServerEvent::StatusUpdated {
        message: format!("{severity}:{event}"),
    }
}

fn looks_like_core_event(event: &str) -> bool {
    event.starts_with("turn_")
        || event.starts_with("agent_")
        || event.starts_with("message_")
        || event.starts_with("tool_")
        || event.starts_with("dialog_")
        || event.starts_with("session_")
}

fn event_name(envelope: &PiRpcEnvelope) -> Option<&str> {
    match envelope.r#type.as_deref() {
        Some("extension_ui_request") => envelope.method.as_deref().or(envelope.r#type.as_deref()),
        Some(other) => Some(other),
        None => envelope.method.as_deref(),
    }
}

fn parse_response_event(envelope: &PiRpcEnvelope) -> ServerEvent {
    if envelope.success == Some(false) {
        return ServerEvent::StatusUpdated {
            message: envelope
                .error
                .clone()
                .unwrap_or_else(|| String::from("rpc_error")),
        };
    }

    if envelope.command.as_deref() == Some("get_state")
        && let Some(state) = parse_backend_state(envelope)
    {
        return ServerEvent::StateUpdated(state);
    }

    if envelope.command.as_deref() == Some("get_available_models")
        && let Some(models) = parse_available_models(envelope)
    {
        return ServerEvent::AvailableModelsUpdated { models };
    }

    if envelope.command.as_deref() == Some("set_model")
        && let Some(model) = parse_response_model(envelope)
    {
        return ServerEvent::ModelChanged { model: model.name };
    }

    if envelope.command.as_deref() == Some("get_fork_messages")
        && let Some(messages) = parse_fork_messages(envelope)
    {
        return ServerEvent::ForkMessagesUpdated { messages };
    }

    if envelope.command.as_deref() == Some("get_messages")
        && let Some(messages) = parse_session_messages(envelope)
    {
        return ServerEvent::SessionMessagesUpdated { messages };
    }

    if envelope.command.as_deref() == Some("get_session_stats")
        && let Some(stats) = parse_session_stats(envelope)
    {
        return ServerEvent::SessionStatsUpdated(stats);
    }

    let message = match envelope.command.as_deref() {
        Some("clone") => String::from("session cloned"),
        Some("fork") => String::from("session forked"),
        Some(command) => command.to_owned(),
        None => String::from("response"),
    };

    ServerEvent::StatusUpdated { message }
}

fn parse_backend_state(envelope: &PiRpcEnvelope) -> Option<BackendState> {
    let data = envelope.extra.get("data")?;
    let model = data.get("model");

    Some(BackendState {
        model_id: model
            .and_then(|model| model.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        model_name: model
            .and_then(|model| model.get("name"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        model_provider: model
            .and_then(|model| model.get("provider"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        session_id: data
            .get("sessionId")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        session_file: data
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        thinking_level: data
            .get("thinkingLevel")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        is_streaming: data
            .get("isStreaming")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        is_compacting: data
            .get("isCompacting")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        message_count: data.get("messageCount").and_then(Value::as_u64),
        pending_message_count: data.get("pendingMessageCount").and_then(Value::as_u64),
    })
}

fn parse_available_models(envelope: &PiRpcEnvelope) -> Option<Vec<ModelInfo>> {
    let models = envelope
        .extra
        .get("data")?
        .get("models")?
        .as_array()?
        .iter()
        .filter_map(parse_model_info)
        .collect::<Vec<_>>();
    Some(models)
}

fn parse_response_model(envelope: &PiRpcEnvelope) -> Option<ModelInfo> {
    let data = envelope.extra.get("data")?;
    data.get("model")
        .and_then(parse_model_info)
        .or_else(|| parse_model_info(data))
}

fn parse_fork_messages(envelope: &PiRpcEnvelope) -> Option<Vec<ForkMessage>> {
    let messages = envelope
        .extra
        .get("data")?
        .get("messages")?
        .as_array()?
        .iter()
        .filter_map(|message| {
            Some(ForkMessage {
                entry_id: message.get("entryId")?.as_str()?.to_owned(),
                text: message.get("text")?.as_str()?.to_owned(),
            })
        })
        .collect();
    Some(messages)
}

fn parse_session_messages(envelope: &PiRpcEnvelope) -> Option<Vec<SessionMessage>> {
    let messages = envelope
        .extra
        .get("data")?
        .get("messages")?
        .as_array()?
        .iter()
        .map(parse_session_message)
        .collect();
    Some(messages)
}

fn parse_session_message(value: &Value) -> SessionMessage {
    let role = value
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let text = value
        .get("content")
        .map(extract_message_text)
        .filter(|text| !text.is_empty())
        .or_else(|| {
            value
                .get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();
    let entry_id = value
        .get("entryId")
        .or_else(|| value.get("entry_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    SessionMessage {
        role,
        text,
        entry_id,
    }
}

fn parse_session_stats(envelope: &PiRpcEnvelope) -> Option<SessionStats> {
    let data = envelope.extra.get("data")?;
    Some(SessionStats {
        message_count: optional_u64_any(data, &["messageCount", "messages", "message_count"]),
        user_message_count: optional_u64_any(data, &["userMessageCount", "userMessages"]),
        assistant_message_count: optional_u64_any(
            data,
            &["assistantMessageCount", "assistantMessages"],
        ),
        tool_message_count: optional_u64_any(data, &["toolMessageCount", "toolMessages"]),
        total_tokens: optional_u64_any(data, &["totalTokens", "tokens"]),
    })
}

fn parse_model_info(value: &Value) -> Option<ModelInfo> {
    let raw_id = value.get("id")?.as_str()?;
    let explicit_provider = value.get("provider").and_then(Value::as_str);
    let (provider, id) = match explicit_provider {
        Some(provider) => {
            let id = raw_id
                .strip_prefix(&format!("{provider}/"))
                .unwrap_or(raw_id)
                .to_owned();
            (provider.to_owned(), id)
        }
        None => raw_id.split_once('/').map_or_else(
            || (String::from("unknown"), raw_id.to_owned()),
            |(provider, id)| (provider.to_owned(), id.to_owned()),
        ),
    };

    Some(ModelInfo {
        id,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(raw_id)
            .to_owned(),
        provider,
    })
}

fn parse_tool_result(envelope: &PiRpcEnvelope) -> Result<ToolResult, ParseError> {
    let tool_name = required_string_any(envelope, &["toolName"])?;
    let result = value_any(envelope, &["result"]);

    Ok(ToolResult {
        tool_call_id: optional_string_any(envelope, &["toolCallId", "tool_call_id"]),
        tool_name,
        output: result.map(extract_text_content).unwrap_or_default(),
        is_error: result
            .and_then(|result| result.get("isError"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn tool_preview(args: &Value) -> Option<String> {
    args.get("command")
        .and_then(Value::as_str)
        .or_else(|| args.get("path").and_then(Value::as_str))
        .map(|value| truncate_chars(value, 96))
        .or_else(|| {
            if args.is_null() {
                None
            } else {
                Some(truncate_chars(&args.to_string(), 96))
            }
        })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut result: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

fn extract_text_content(value: &Value) -> String {
    value
        .get("content")
        .map(extract_message_text)
        .unwrap_or_default()
}

fn extract_message_text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }

    value
        .as_array()
        .map(|content| {
            content
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn parse_message_update(envelope: &PiRpcEnvelope) -> Result<ServerEvent, ParseError> {
    let Some(assistant_event) = envelope.assistant_message_event.as_ref() else {
        return Err(ParseError::MissingField("assistantMessageEvent"));
    };

    let Some(event_type) = assistant_event.get("type").and_then(Value::as_str) else {
        return Err(ParseError::MissingField("assistantMessageEvent.type"));
    };

    match event_type {
        "text_delta" => Ok(ServerEvent::PromptDelta {
            session_id: String::from("active"),
            delta: assistant_event
                .get("delta")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_default(),
        }),
        _ => Ok(ServerEvent::StatusUpdated {
            message: format!("assistant_event:{event_type}"),
        }),
    }
}

fn parse_dialog_request(envelope: &PiRpcEnvelope) -> Result<DialogRequest, ParseError> {
    let title = optional_string_any(envelope, &["title"]);
    let prompt = optional_string_any(envelope, &["prompt", "message"]);

    let kind = match event_name(envelope) {
        Some("select") => DialogKind::Select {
            options: dialog_options(envelope),
        },
        Some("confirm") => DialogKind::Confirm,
        Some("input") => DialogKind::Input {
            default: optional_string_any(envelope, &["default"]),
        },
        Some("editor") => DialogKind::Editor {
            initial_text: optional_string_any(envelope, &["text", "initial_text", "initialText"]),
        },
        Some(other) => return Err(ParseError::UnsupportedMethod(String::from(other))),
        None => return Err(ParseError::UnsupportedMethod(String::from("unknown"))),
    };

    Ok(DialogRequest {
        id: envelope.id.clone(),
        title,
        prompt,
        kind,
    })
}

fn dialog_options(envelope: &PiRpcEnvelope) -> Vec<DialogOption> {
    value_any(envelope, &["options"])
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let label = option.get("label").and_then(Value::as_str)?;
                    let value = option.get("value").and_then(Value::as_str).unwrap_or(label);

                    Some(DialogOption {
                        label: String::from(label),
                        value: String::from(value),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn value_any<'a>(envelope: &'a PiRpcEnvelope, field_names: &[&str]) -> Option<&'a Value> {
    field_names.iter().find_map(|field_name| {
        envelope
            .params
            .get(*field_name)
            .or_else(|| envelope.extra.get(*field_name))
    })
}

fn required_string_any(
    envelope: &PiRpcEnvelope,
    field_names: &[&'static str],
) -> Result<String, ParseError> {
    value_any(envelope, field_names)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(ParseError::MissingField(field_names[0]))
}

fn optional_string_any(envelope: &PiRpcEnvelope, field_names: &[&str]) -> Option<String> {
    value_any(envelope, field_names)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn optional_u64_any(value: &Value, field_names: &[&str]) -> Option<u64> {
    field_names
        .iter()
        .find_map(|field_name| value.get(*field_name).and_then(Value::as_u64))
}

#[cfg(test)]
mod tests {
    use super::{ParseError, parse_server_line};
    use yach_proto::{
        DialogKind, ForkMessage, MessageBody, ModelInfo, Notification, ServerEvent, SessionMessage,
        SessionStats, WidgetState,
    };

    #[test]
    fn parser_maps_prompt_delta_lines_into_transport_messages() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"hello"}}"#;

        let message = parse_server_line(line, "msg-1");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(message.meta.message_id, "msg-1");
        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::PromptDelta {
                session_id: String::from("active"),
                delta: String::from("hello"),
            })
        );
    }

    #[test]
    fn parser_maps_status_aliases() {
        let line = r#"{"type":"response","command":"prompt","success":true}"#;

        let message = parse_server_line(line, "msg-2");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("prompt"),
            })
        );
    }

    #[test]
    fn parser_maps_get_state_response() {
        let line = r#"{"type":"response","command":"get_state","success":true,"data":{"model":{"id":"gpt-5.4","name":"GPT-5.4","provider":"openai"},"thinkingLevel":"high","isStreaming":false,"isCompacting":true,"sessionFile":"/tmp/session.jsonl","sessionId":"sess-1","messageCount":7,"pendingMessageCount":2}}"#;

        let message = parse_server_line(line, "msg-state");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        let MessageBody::ServerEvent(ServerEvent::StateUpdated(state)) = message.body else {
            unreachable!();
        };
        assert_eq!(state.model_id.as_deref(), Some("gpt-5.4"));
        assert_eq!(state.model_name.as_deref(), Some("GPT-5.4"));
        assert_eq!(state.model_provider.as_deref(), Some("openai"));
        assert_eq!(state.session_id.as_deref(), Some("sess-1"));
        assert_eq!(state.session_file.as_deref(), Some("/tmp/session.jsonl"));
        assert_eq!(state.thinking_level.as_deref(), Some("high"));
        assert!(state.is_compacting);
        assert!(!state.is_streaming);
        assert_eq!(state.message_count, Some(7));
        assert_eq!(state.pending_message_count, Some(2));
    }

    #[test]
    fn parser_tolerates_message_lifecycle_events() {
        let message = parse_server_line(
            r#"{"type":"message_start","message":{"role":"user","content":[]}}"#,
            "msg-lifecycle",
        );
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("message_start"),
            })
        );
    }

    #[test]
    fn parser_maps_tool_execution_events() {
        let start = parse_server_line(
            r#"{"type":"tool_execution_start","toolCallId":"call-1","toolName":"bash","args":{"command":"pwd"}}"#,
            "msg-tool-start",
        );
        assert!(start.is_ok());
        let Ok(start) = start else {
            return;
        };
        assert_eq!(
            start.body,
            MessageBody::ServerEvent(ServerEvent::ToolCallStarted {
                tool_call_id: Some(String::from("call-1")),
                tool_name: String::from("bash"),
                preview: Some(String::from("pwd")),
            })
        );

        let end = parse_server_line(
            r#"{"type":"tool_execution_end","toolCallId":"call-1","toolName":"bash","result":{"content":[{"type":"text","text":"/tmp\n"}],"isError":false}}"#,
            "msg-tool-end",
        );
        assert!(end.is_ok());
        let Ok(end) = end else {
            return;
        };
        let MessageBody::ServerEvent(ServerEvent::ToolCallFinished(result)) = end.body else {
            unreachable!();
        };
        assert_eq!(result.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(result.tool_name, "bash");
        assert_eq!(result.output, "/tmp\n");
        assert!(!result.is_error);
    }

    #[test]
    fn parser_maps_available_models_response() {
        let line = r#"{"type":"response","command":"get_available_models","success":true,"data":{"models":[{"id":"claude-sonnet-4-20250514","name":"Claude Sonnet 4","provider":"anthropic"},{"id":"gpt-5","name":"GPT-5","provider":"openai"}]}}"#;

        let message = parse_server_line(line, "msg-models");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::AvailableModelsUpdated {
                models: vec![
                    ModelInfo {
                        id: String::from("claude-sonnet-4-20250514"),
                        name: String::from("Claude Sonnet 4"),
                        provider: String::from("anthropic"),
                    },
                    ModelInfo {
                        id: String::from("gpt-5"),
                        name: String::from("GPT-5"),
                        provider: String::from("openai"),
                    },
                ],
            })
        );
    }

    #[test]
    fn parser_maps_session_data_responses() {
        let fork_line = r#"{"type":"response","command":"get_fork_messages","success":true,"data":{"messages":[{"entryId":"entry-1","text":"First prompt"},{"entryId":"entry-2","text":"Second prompt"}]}}"#;
        let fork_message = parse_server_line(fork_line, "msg-fork-messages");
        assert!(fork_message.is_ok());
        let Ok(fork_message) = fork_message else {
            return;
        };
        assert_eq!(
            fork_message.body,
            MessageBody::ServerEvent(ServerEvent::ForkMessagesUpdated {
                messages: vec![
                    ForkMessage {
                        entry_id: String::from("entry-1"),
                        text: String::from("First prompt"),
                    },
                    ForkMessage {
                        entry_id: String::from("entry-2"),
                        text: String::from("Second prompt"),
                    },
                ],
            })
        );

        let messages_line = r#"{"type":"response","command":"get_messages","success":true,"data":{"messages":[{"id":"entry-1","role":"user","content":"hello"},{"role":"assistant","content":[{"type":"text","text":"hi"}]}]}}"#;
        let messages = parse_server_line(messages_line, "msg-session-messages");
        assert!(messages.is_ok());
        let Ok(messages) = messages else {
            return;
        };
        assert_eq!(
            messages.body,
            MessageBody::ServerEvent(ServerEvent::SessionMessagesUpdated {
                messages: vec![
                    SessionMessage {
                        role: String::from("user"),
                        text: String::from("hello"),
                        entry_id: Some(String::from("entry-1")),
                    },
                    SessionMessage {
                        role: String::from("assistant"),
                        text: String::from("hi"),
                        entry_id: None,
                    },
                ],
            })
        );

        let stats_line = r#"{"type":"response","command":"get_session_stats","success":true,"data":{"messageCount":3,"userMessageCount":1,"assistantMessageCount":1,"toolMessageCount":1,"totalTokens":42}}"#;
        let stats = parse_server_line(stats_line, "msg-stats");
        assert!(stats.is_ok());
        let Ok(stats) = stats else {
            return;
        };
        assert_eq!(
            stats.body,
            MessageBody::ServerEvent(ServerEvent::SessionStatsUpdated(SessionStats {
                message_count: Some(3),
                user_message_count: Some(1),
                assistant_message_count: Some(1),
                tool_message_count: Some(1),
                total_tokens: Some(42),
            }))
        );
    }

    #[test]
    fn parser_strips_duplicate_provider_prefix_from_model_id() {
        let line = r#"{"type":"response","command":"get_available_models","success":true,"data":{"models":[{"id":"anthropic/claude-sonnet-4-20250514","name":"Claude Sonnet 4","provider":"anthropic"}]}}"#;

        let message = parse_server_line(line, "msg-models-prefixed");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::AvailableModelsUpdated {
                models: vec![ModelInfo {
                    id: String::from("claude-sonnet-4-20250514"),
                    name: String::from("Claude Sonnet 4"),
                    provider: String::from("anthropic"),
                }],
            })
        );
    }

    #[test]
    fn parser_derives_model_provider_from_slash_id() {
        let line = r#"{"type":"response","command":"get_available_models","success":true,"data":{"models":[{"id":"anthropic/claude-sonnet-4-20250514","name":"Claude Sonnet 4"}]}}"#;

        let message = parse_server_line(line, "msg-models-legacy");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::AvailableModelsUpdated {
                models: vec![ModelInfo {
                    id: String::from("claude-sonnet-4-20250514"),
                    name: String::from("Claude Sonnet 4"),
                    provider: String::from("anthropic"),
                }],
            })
        );
    }

    #[test]
    fn parser_maps_set_model_response_to_model_changed() {
        let line = r#"{"type":"response","command":"set_model","success":true,"data":{"model":{"id":"gpt-5","name":"GPT-5","provider":"openai"}}}"#;

        let message = parse_server_line(line, "msg-set-model");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::ModelChanged {
                model: String::from("GPT-5"),
            })
        );
    }

    #[test]
    fn parser_maps_bare_set_model_response_to_model_changed() {
        let line = r#"{"type":"response","command":"set_model","success":true,"data":{"id":"gpt-5","name":"GPT-5","provider":"openai"}}"#;

        let message = parse_server_line(line, "msg-set-model-bare");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::ModelChanged {
                model: String::from("GPT-5"),
            })
        );
    }

    #[test]
    fn parser_maps_response_errors_to_status_updates() {
        let line = r#"{"type":"response","success":false,"error":"Unknown command: undefined"}"#;

        let message = parse_server_line(line, "msg-response-error");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("Unknown command: undefined"),
            })
        );
    }

    #[test]
    fn parser_labels_clone_responses_as_session_cloned() {
        let line =
            r#"{"type":"response","command":"clone","success":true,"data":{"cancelled":false}}"#;

        let message = parse_server_line(line, "msg-clone-response");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("session cloned"),
            })
        );
    }

    #[test]
    fn parser_maps_dialog_requests() {
        let line = r#"{"id":"dlg-1","method":"select","params":{"title":"Pick one","options":[{"label":"Alpha","value":"a"}]}}"#;

        let message = parse_server_line(line, "msg-3");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        let MessageBody::ServerEvent(ServerEvent::DialogRequested(dialog)) = message.body else {
            unreachable!();
        };
        assert_eq!(dialog.id.as_deref(), Some("dlg-1"));
        assert_eq!(dialog.title.as_deref(), Some("Pick one"));
        let DialogKind::Select { options } = dialog.kind else {
            unreachable!();
        };
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].value, "a");
    }

    #[test]
    fn parser_maps_session_and_model_events() {
        let session = parse_server_line(
            r#"{"method":"switchSession","params":{"session_id":"sess-2"}}"#,
            "msg-4",
        );
        assert!(session.is_ok());
        let Ok(session) = session else {
            return;
        };
        assert_eq!(
            session.body,
            MessageBody::ServerEvent(ServerEvent::SessionChanged {
                session_id: String::from("sess-2"),
            })
        );

        let model = parse_server_line(
            r#"{"method":"setModel","params":{"model":"gpt-5"}}"#,
            "msg-5",
        );
        assert!(model.is_ok());
        let Ok(model) = model else {
            return;
        };
        assert_eq!(
            model.body,
            MessageBody::ServerEvent(ServerEvent::ModelChanged {
                model: String::from("gpt-5"),
            })
        );
    }

    #[test]
    fn parser_maps_notification_widget_and_title_events() {
        let notification = parse_server_line(
            r#"{"method":"notify","params":{"level":"warn","message":"heads up"}}"#,
            "msg-8",
        );
        assert!(notification.is_ok());
        let Ok(notification) = notification else {
            return;
        };
        assert_eq!(
            notification.body,
            MessageBody::ServerEvent(ServerEvent::NotificationRaised(Notification {
                level: String::from("warn"),
                message: String::from("heads up"),
            }))
        );

        let widget = parse_server_line(
            r#"{"method":"setWidget","params":{"id":"tool-1","title":"Bash","body":"running"}}"#,
            "msg-9",
        );
        assert!(widget.is_ok());
        let Ok(widget) = widget else {
            return;
        };
        assert_eq!(
            widget.body,
            MessageBody::ServerEvent(ServerEvent::WidgetUpdated(WidgetState {
                id: String::from("tool-1"),
                title: String::from("Bash"),
                body: String::from("running"),
            }))
        );

        let title = parse_server_line(
            r#"{"method":"setTitle","params":{"title":"yach"}}"#,
            "msg-10",
        );
        assert!(title.is_ok());
        let Ok(title) = title else {
            return;
        };
        assert_eq!(
            title.body,
            MessageBody::ServerEvent(ServerEvent::TitleChanged {
                title: String::from("yach"),
            })
        );

        let extension_widget = parse_server_line(
            r#"{"type":"extension_ui_request","id":"widget-1","method":"setWidget","widgetKey":"review"}"#,
            "msg-11",
        );
        assert!(extension_widget.is_ok());
        let Ok(extension_widget) = extension_widget else {
            return;
        };
        assert_eq!(
            extension_widget.body,
            MessageBody::ServerEvent(ServerEvent::WidgetUpdated(WidgetState {
                id: String::from("review"),
                title: String::from("review"),
                body: String::new(),
            }))
        );

        let extension_status = parse_server_line(
            r#"{"type":"extension_ui_request","id":"status-1","method":"setStatus","statusKey":"session-control"}"#,
            "msg-12",
        );
        assert!(extension_status.is_ok());
        let Ok(extension_status) = extension_status else {
            return;
        };
        assert_eq!(
            extension_status.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("session-control"),
            })
        );
    }

    #[test]
    fn parser_maps_unknown_methods_to_status_updates() {
        let message = parse_server_line(r#"{"method":"unknown_call","params":{}}"#, "msg-6");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("unknown_event:unknown_call")
            })
        );
    }

    #[test]
    fn parser_marks_unknown_core_like_methods_as_degraded() {
        let message = parse_server_line(r#"{"method":"turn_weird","params":{}}"#, "msg-6b");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(ServerEvent::StatusUpdated {
                message: String::from("unknown_core_event:turn_weird")
            })
        );
    }

    #[test]
    fn parser_rejects_missing_required_fields() {
        let error = parse_server_line(
            r#"{"method":"prompt_delta","params":{"delta":"hello"}}"#,
            "msg-7",
        );
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, ParseError::MissingField("session_id"));
    }
}
