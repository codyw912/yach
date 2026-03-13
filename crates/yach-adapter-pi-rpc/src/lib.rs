use serde::Deserialize;
use serde_json::{Value, json};
use yach_proto::{
    Capability, ClientEvent, Handshake, MessageBody, MessageMeta, NegotiatedCapabilities,
    ServerEvent, TransportMessage, default_rpc_handshake,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapabilities {
    pub prompt_streaming: bool,
    pub dialogs: bool,
    pub widgets: bool,
}

impl AdapterCapabilities {
    #[must_use]
    pub const fn stock_rpc() -> Self {
        Self {
            prompt_streaming: true,
            dialogs: true,
            widgets: true,
        }
    }

    #[must_use]
    pub const fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::PromptStreaming => self.prompt_streaming,
            Capability::Dialogs => self.dialogs,
            Capability::Widgets => self.widgets,
            Capability::Notifications | Capability::StatusEntries | Capability::SessionForking => true,
            Capability::ThemeLoading | Capability::RichUi => false,
        }
    }
}

#[must_use]
pub fn stock_rpc_handshake() -> Handshake {
    default_rpc_handshake()
}

#[must_use]
pub fn negotiate_with(ui: &Handshake) -> NegotiatedCapabilities {
    NegotiatedCapabilities::from_handshakes(ui, &stock_rpc_handshake())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    EmptyLine,
    InvalidJson(String),
    UnsupportedMethod(String),
    MissingField(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializeError {
    WrongDirection,
    WrongBodyType,
}

#[derive(Debug, Deserialize)]
struct PiRpcEnvelope {
    #[serde(default)]
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

pub fn parse_server_line(
    line: &str,
    message_id: impl Into<String>,
) -> Result<TransportMessage, ParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyLine);
    }

    let envelope: PiRpcEnvelope =
        serde_json::from_str(trimmed).map_err(|error| ParseError::InvalidJson(error.to_string()))?;

    let meta = build_message_meta(message_id.into(), &envelope);
    let event = map_server_event(&envelope)?;

    Ok(TransportMessage::server(meta, event))
}

fn build_message_meta(message_id: String, envelope: &PiRpcEnvelope) -> MessageMeta {
    let mut meta = MessageMeta::new(message_id);

    if let Some(correlation_id) = &envelope.id {
        meta = meta.with_correlation_id(correlation_id.clone());
    }

    if let Some(stream_id) = envelope
        .params
        .get("stream_id")
        .and_then(Value::as_str)
        .or_else(|| envelope.params.get("session_id").and_then(Value::as_str))
    {
        meta = meta.with_stream_id(stream_id.to_owned());
    }

    meta
}

fn map_server_event(envelope: &PiRpcEnvelope) -> Result<ServerEvent, ParseError> {
    match envelope.method.as_str() {
        "ready" => Ok(ServerEvent::Ready {
            handshake: stock_rpc_handshake(),
        }),
        "prompt_delta" | "promptDelta" => Ok(ServerEvent::PromptDelta {
            session_id: required_string(&envelope.params, "session_id")?,
            delta: required_string(&envelope.params, "delta")?,
        }),
        "tool_call_started" | "toolCallStarted" => Ok(ServerEvent::ToolCallStarted {
            tool_name: required_string(&envelope.params, "tool_name")?,
        }),
        "status_updated" | "setStatus" => Ok(ServerEvent::StatusUpdated {
            message: required_string(&envelope.params, "message")?,
        }),
        _ => Err(ParseError::UnsupportedMethod(envelope.method.clone())),
    }
}

fn required_string(params: &Value, field_name: &'static str) -> Result<String, ParseError> {
    params
        .get(field_name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(ParseError::MissingField(field_name))
}

pub fn serialize_client_message(message: &TransportMessage) -> Result<String, SerializeError> {
    if message.direction != yach_proto::MessageDirection::ClientToAdapter {
        return Err(SerializeError::WrongDirection);
    }

    let MessageBody::ClientEvent(event) = &message.body else {
        return Err(SerializeError::WrongBodyType);
    };

    serialize_client_event(event, &message.meta)
}

fn serialize_client_event(
    event: &ClientEvent,
    meta: &MessageMeta,
) -> Result<String, SerializeError> {
    let id = meta.correlation_id.clone().unwrap_or_else(|| meta.message_id.clone());

    let envelope = match event {
        ClientEvent::Initialize(handshake) => json!({
            "id": id,
            "method": "initialize",
            "params": {
                "protocol_version": handshake.protocol_version,
                "agent_name": handshake.agent_name,
                "capabilities": handshake.capabilities,
            }
        }),
        ClientEvent::PromptSubmitted { session_id, prompt } => json!({
            "id": id,
            "method": "prompt",
            "params": {
                "session_id": session_id,
                "prompt": prompt,
                "stream_id": meta.stream_id,
            }
        }),
        ClientEvent::SessionSelected { session_id } => json!({
            "id": id,
            "method": "select_session",
            "params": {
                "session_id": session_id,
            }
        }),
    };

    let mut line = envelope.to_string();
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::{
        AdapterCapabilities, ParseError, SerializeError, negotiate_with, parse_server_line,
        serialize_client_message, stock_rpc_handshake,
    };
    use yach_proto::{
        Capability, ClientEvent, MessageBody, MessageMeta, TransportMessage, default_ui_handshake,
    };

    #[test]
    fn stock_rpc_supports_phase_one_basics() {
        let capabilities = AdapterCapabilities::stock_rpc();

        assert!(capabilities.prompt_streaming);
        assert!(capabilities.dialogs);
        assert!(capabilities.widgets);
    }

    #[test]
    fn stock_rpc_matches_proto_handshake() {
        let capabilities = AdapterCapabilities::stock_rpc();
        let handshake = stock_rpc_handshake();

        assert!(capabilities.supports(Capability::PromptStreaming));
        assert!(handshake.supports(Capability::PromptStreaming));
        assert_eq!(capabilities.supports(Capability::ThemeLoading), handshake.supports(Capability::ThemeLoading));
    }

    #[test]
    fn negotiation_matches_ui_intersection() {
        let negotiation = negotiate_with(&default_ui_handshake());

        assert!(negotiation.supports(Capability::Widgets));
        assert!(!negotiation.supports(Capability::ThemeLoading));
    }

    #[test]
    fn parser_maps_prompt_delta_lines_into_transport_messages() {
        let line =
            r#"{"id":"req-1","method":"prompt_delta","params":{"session_id":"sess-1","delta":"hello","stream_id":"stream-9"}}"#;

        let message = parse_server_line(line, "msg-1").expect("parser should accept prompt deltas");

        assert_eq!(message.meta.message_id, "msg-1");
        assert_eq!(message.meta.correlation_id.as_deref(), Some("req-1"));
        assert_eq!(message.meta.stream_id.as_deref(), Some("stream-9"));
        assert_eq!(
            message.body,
            MessageBody::ServerEvent(yach_proto::ServerEvent::PromptDelta {
                session_id: String::from("sess-1"),
                delta: String::from("hello"),
            })
        );
    }

    #[test]
    fn parser_maps_status_aliases() {
        let line = r#"{"method":"setStatus","params":{"message":"syncing"}}"#;

        let message = parse_server_line(line, "msg-2").expect("parser should accept setStatus");

        assert_eq!(
            message.body,
            MessageBody::ServerEvent(yach_proto::ServerEvent::StatusUpdated {
                message: String::from("syncing"),
            })
        );
    }

    #[test]
    fn parser_rejects_unknown_methods() {
        let line = r#"{"method":"unknown_call","params":{}}"#;

        let error = parse_server_line(line, "msg-3").expect_err("parser should reject unknown methods");

        assert_eq!(error, ParseError::UnsupportedMethod(String::from("unknown_call")));
    }

    #[test]
    fn parser_rejects_missing_required_fields() {
        let line = r#"{"method":"prompt_delta","params":{"delta":"hello"}}"#;

        let error = parse_server_line(line, "msg-4").expect_err("parser should require session id");

        assert_eq!(error, ParseError::MissingField("session_id"));
    }

    #[test]
    fn serializer_maps_prompt_messages_into_rpc_lines() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-5")
                .with_correlation_id("req-5")
                .with_stream_id("stream-5"),
            ClientEvent::PromptSubmitted {
                session_id: String::from("sess-5"),
                prompt: String::from("hello from yach"),
            },
        );

        let line = serialize_client_message(&message).expect("serializer should accept client prompts");

        assert!(line.ends_with('\n'));
        assert!(line.contains("\"method\":\"prompt\""));
        assert!(line.contains("\"id\":\"req-5\""));
        assert!(line.contains("\"stream_id\":\"stream-5\""));
    }

    #[test]
    fn serializer_maps_initialize_messages_into_rpc_lines() {
        let message = TransportMessage::client(
            MessageMeta::new("msg-6"),
            ClientEvent::Initialize(default_ui_handshake()),
        );

        let line = serialize_client_message(&message).expect("serializer should accept initialize messages");

        assert!(line.contains("\"method\":\"initialize\""));
        assert!(line.contains("\"agent_name\":\"yach-ui\""));
        assert!(line.contains("\"capabilities\""));
    }

    #[test]
    fn serializer_rejects_server_messages() {
        let message = TransportMessage::server(
            MessageMeta::new("msg-7"),
            yach_proto::ServerEvent::StatusUpdated {
                message: String::from("ready"),
            },
        );

        let error = serialize_client_message(&message)
            .expect_err("serializer should reject adapter-to-client messages");

        assert_eq!(error, SerializeError::WrongDirection);
    }
}
