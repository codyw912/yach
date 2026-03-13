use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

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

#[derive(Debug)]
pub enum SessionError {
    Spawn(io::Error),
    MissingStdin,
    MissingStdout,
    Io(io::Error),
    Parse(ParseError),
    Serialize(SerializeError),
    EndOfStream,
    UnexpectedEvent(ServerEvent),
    WrongMessageDirection,
}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ParseError> for SessionError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<SerializeError> for SessionError {
    fn from(error: SerializeError) -> Self {
        Self::Serialize(error)
    }
}

#[derive(Debug, Deserialize)]
struct PiRpcEnvelope {
    #[serde(default)]
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Clone)]
pub struct PiCommand {
    program: String,
    args: Vec<String>,
}

impl PiCommand {
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    #[must_use]
    pub fn stock_rpc() -> Self {
        Self::new("pi").with_arg("--mode").with_arg("rpc")
    }

    #[must_use]
    pub fn with_arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn into_command(self) -> Command {
        let mut command = Command::new(self.program);
        command.args(self.args);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::inherit());
        command
    }
}

pub struct PiRpcIo<R, W>
where
    R: Read,
    W: Write,
{
    next_message_id: u64,
    reader: BufReader<R>,
    writer: BufWriter<W>,
}

impl<R, W> PiRpcIo<R, W>
where
    R: Read,
    W: Write,
{
    #[must_use]
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            next_message_id: 1,
            reader: BufReader::new(reader),
            writer: BufWriter::new(writer),
        }
    }

    pub fn send(&mut self, message: &TransportMessage) -> Result<(), SessionError> {
        let line = serialize_client_message(message)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn read_next(&mut self) -> Result<TransportMessage, SessionError> {
        let mut line = String::new();
        let bytes_read = self.reader.read_line(&mut line)?;

        if bytes_read == 0 {
            return Err(SessionError::EndOfStream);
        }

        let message_id = self.allocate_message_id();
        parse_server_line(&line, message_id).map_err(SessionError::from)
    }

    fn allocate_message_id(&mut self) -> String {
        let message_id = format!("server-{}", self.next_message_id);
        self.next_message_id += 1;
        message_id
    }
}

pub struct PiRpcSession {
    child: Child,
    io: PiRpcIo<ChildStdout, ChildStdin>,
}

impl PiRpcSession {
    pub fn spawn(command: PiCommand) -> Result<Self, SessionError> {
        let mut child = command.into_command().spawn().map_err(SessionError::Spawn)?;
        let stdout = child.stdout.take().ok_or(SessionError::MissingStdout)?;
        let stdin = child.stdin.take().ok_or(SessionError::MissingStdin)?;

        Ok(Self {
            child,
            io: PiRpcIo::new(stdout, stdin),
        })
    }

    pub fn send(&mut self, message: &TransportMessage) -> Result<(), SessionError> {
        self.io.send(message)
    }

    pub fn read_next(&mut self) -> Result<TransportMessage, SessionError> {
        self.io.read_next()
    }

    pub fn initialize(&mut self, handshake: Handshake) -> Result<Handshake, SessionError> {
        self.io.initialize(handshake)
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, SessionError> {
        self.child.try_wait().map_err(SessionError::Io)
    }
}

impl<R, W> PiRpcIo<R, W>
where
    R: Read,
    W: Write,
{
    pub fn initialize(&mut self, handshake: Handshake) -> Result<Handshake, SessionError> {
        let initialize = TransportMessage::client(
            MessageMeta::new("client-1").with_correlation_id("initialize"),
            ClientEvent::Initialize(handshake),
        );

        self.send(&initialize)?;

        let response = self.read_next()?;
        match response.body {
            MessageBody::ServerEvent(ServerEvent::Ready { handshake }) => Ok(handshake),
            MessageBody::ServerEvent(event) => Err(SessionError::UnexpectedEvent(event)),
            MessageBody::ClientEvent(_) => Err(SessionError::WrongMessageDirection),
        }
    }
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

    Ok(serialize_client_event(event, &message.meta))
}

fn serialize_client_event(
    event: &ClientEvent,
    meta: &MessageMeta,
) -> String {
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
    line
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{
        AdapterCapabilities, ParseError, PiCommand, PiRpcIo, SerializeError, SessionError,
        negotiate_with, parse_server_line, serialize_client_message, stock_rpc_handshake,
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

        let message = parse_server_line(line, "msg-1");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

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

        let message = parse_server_line(line, "msg-2");
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

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

        let error = parse_server_line(line, "msg-3");
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, ParseError::UnsupportedMethod(String::from("unknown_call")));
    }

    #[test]
    fn parser_rejects_missing_required_fields() {
        let line = r#"{"method":"prompt_delta","params":{"delta":"hello"}}"#;

        let error = parse_server_line(line, "msg-4");
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

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

        let line = serialize_client_message(&message);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

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

        let line = serialize_client_message(&message);
        assert!(line.is_ok());
        let Ok(line) = line else {
            return;
        };

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

        let error = serialize_client_message(&message);
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert_eq!(error, SerializeError::WrongDirection);
    }

    #[test]
    fn io_wrapper_sends_serialized_messages() {
        let reader = Cursor::new(Vec::<u8>::new());
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);
        let message = TransportMessage::client(
            MessageMeta::new("msg-8").with_correlation_id("req-8"),
            ClientEvent::SessionSelected {
                session_id: String::from("sess-8"),
            },
        );

        let sent = io.send(&message);
        assert!(sent.is_ok());
        if sent.is_err() {
            return;
        }

        let writer = io.writer.into_inner();
        assert!(writer.is_ok());
        let Ok(writer) = writer else {
            return;
        };
        let written = String::from_utf8(writer);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };

        assert!(written.contains("\"method\":\"select_session\""));
        assert!(written.contains("\"id\":\"req-8\""));
    }

    #[test]
    fn io_wrapper_reads_and_parses_server_messages() {
        let reader = Cursor::new(b"{\"method\":\"ready\",\"params\":{}}\n".to_vec());
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let message = io.read_next();
        assert!(message.is_ok());
        let Ok(message) = message else {
            return;
        };

        assert_eq!(message.meta.message_id, "server-1");
        assert_eq!(
            message.body,
            MessageBody::ServerEvent(yach_proto::ServerEvent::Ready {
                handshake: stock_rpc_handshake(),
            })
        );
    }

    #[test]
    fn io_wrapper_reports_end_of_stream() {
        let reader = Cursor::new(Vec::<u8>::new());
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let error = io.read_next();
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        assert!(matches!(error, SessionError::EndOfStream));
    }

    #[test]
    fn stock_command_targets_pi_rpc_mode() {
        let command = PiCommand::stock_rpc();

        assert_eq!(command.program, "pi");
        assert_eq!(command.args, vec![String::from("--mode"), String::from("rpc")]);
    }

    #[test]
    fn initialize_sends_handshake_and_waits_for_ready() {
        let reader = Cursor::new(b"{\"method\":\"ready\",\"params\":{}}\n".to_vec());
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let ready_handshake = io.initialize(default_ui_handshake());
        assert!(ready_handshake.is_ok());
        let Ok(ready_handshake) = ready_handshake else {
            return;
        };

        assert_eq!(ready_handshake, stock_rpc_handshake());

        let writer = io.writer.into_inner();
        assert!(writer.is_ok());
        let Ok(writer) = writer else {
            return;
        };
        let written = String::from_utf8(writer);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };

        assert!(written.contains("\"method\":\"initialize\""));
        assert!(written.contains("\"agent_name\":\"yach-ui\""));
    }

    #[test]
    fn initialize_rejects_non_ready_events() {
        let reader = Cursor::new(b"{\"method\":\"setStatus\",\"params\":{\"message\":\"warming up\"}}\n".to_vec());
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let error = io.initialize(default_ui_handshake());
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        match error {
            SessionError::UnexpectedEvent(yach_proto::ServerEvent::StatusUpdated { message }) => {
                assert_eq!(message, "warming up");
            }
            other => assert!(matches!(other, SessionError::UnexpectedEvent(_))),
        }
    }
}
