use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

use yach_proto::{ClientEvent, Handshake, MessageBody, MessageMeta, ServerEvent, TransportMessage};

use crate::capabilities::stock_rpc_handshake;
use crate::parse::{ParseError, parse_server_line};
use crate::serialize::{SerializeError, serialize_client_message};

#[derive(Debug)]
pub enum SessionError {
    Spawn(io::Error),
    MissingStdin,
    MissingStdout,
    Io(io::Error),
    Parse(ParseError),
    Serialize(SerializeError),
    EndOfStream,
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

#[derive(Debug, Clone)]
pub struct PiCommand {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
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
        command.stderr(Stdio::piped());
        command
    }
}

pub struct PiRpcReader<R>
where
    R: Read,
{
    next_message_id: u64,
    pub(crate) reader: BufReader<R>,
}

impl<R> PiRpcReader<R>
where
    R: Read,
{
    #[must_use]
    pub fn new(reader: R) -> Self {
        Self {
            next_message_id: 1,
            reader: BufReader::new(reader),
        }
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

pub struct PiRpcWriter<W>
where
    W: Write,
{
    next_message_id: u64,
    next_request_id: u64,
    pub(crate) writer: BufWriter<W>,
}

impl<W> PiRpcWriter<W>
where
    W: Write,
{
    #[must_use]
    pub fn new(writer: W) -> Self {
        Self {
            next_message_id: 1,
            next_request_id: 1,
            writer: BufWriter::new(writer),
        }
    }

    pub fn send(&mut self, message: &TransportMessage) -> Result<(), SessionError> {
        let line = serialize_client_message(message)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn send_event(&mut self, event: ClientEvent) -> Result<(), SessionError> {
        let message = TransportMessage::client(MessageMeta::new(self.allocate_message_id()), event);
        self.send(&message)
    }

    pub fn send_command_json(&mut self, command_json: &str) -> Result<(), SessionError> {
        self.writer.write_all(command_json.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn send_rpc_command(
        &mut self,
        command_type: &str,
        data_fields: &[(&str, &str)],
    ) -> Result<String, SessionError> {
        let request_id = format!("req_{}", self.next_request_id);
        self.next_request_id += 1;

        let mut payload = serde_json::Map::new();
        payload.insert(
            String::from("id"),
            serde_json::Value::String(request_id.clone()),
        );
        payload.insert(
            String::from("type"),
            serde_json::Value::String(String::from(command_type)),
        );
        for (key, value) in data_fields {
            payload.insert(
                String::from(*key),
                serde_json::Value::String(String::from(*value)),
            );
        }

        let mut line = serde_json::Value::Object(payload).to_string();
        line.push('\n');
        self.send_command_json(&line)?;
        Ok(request_id)
    }

    pub fn submit_prompt(&mut self, session_id: &str, prompt: &str) -> Result<(), SessionError> {
        self.send_event(ClientEvent::PromptSubmitted {
            session_id: String::from(session_id),
            prompt: String::from(prompt),
        })
    }

    fn allocate_message_id(&mut self) -> String {
        let message_id = format!("client-{}", self.next_message_id);
        self.next_message_id += 1;
        message_id
    }
}

pub struct PiRpcIo<R, W>
where
    R: Read,
    W: Write,
{
    pub(crate) reader: PiRpcReader<R>,
    pub(crate) writer: PiRpcWriter<W>,
}

impl<R, W> PiRpcIo<R, W>
where
    R: Read,
    W: Write,
{
    #[must_use]
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: PiRpcReader::new(reader),
            writer: PiRpcWriter::new(writer),
        }
    }

    pub fn send(&mut self, message: &TransportMessage) -> Result<(), SessionError> {
        self.writer.send(message)
    }

    pub fn read_next(&mut self) -> Result<TransportMessage, SessionError> {
        self.reader.read_next()
    }

    pub fn initialize(&mut self, handshake: Handshake) -> Result<Handshake, SessionError> {
        let initialize = TransportMessage::client(
            MessageMeta::new("client-1").with_correlation_id("initialize"),
            ClientEvent::Initialize(handshake),
        );

        self.send(&initialize)?;

        loop {
            let response = self.read_next()?;
            match response.body {
                MessageBody::ServerEvent(ServerEvent::StatusUpdated { message })
                    if message == "get_state" =>
                {
                    return Ok(stock_rpc_handshake());
                }
                MessageBody::ServerEvent(ServerEvent::StateUpdated(_)) => {
                    return Ok(stock_rpc_handshake());
                }
                MessageBody::ServerEvent(ServerEvent::Ready { handshake }) => return Ok(handshake),
                MessageBody::ServerEvent(_) => {}
                MessageBody::ClientEvent(_) => return Err(SessionError::WrongMessageDirection),
            }
        }
    }

    pub fn send_command_json(&mut self, command_json: &str) -> Result<(), SessionError> {
        self.writer.send_command_json(command_json)
    }

    pub fn send_rpc_command(
        &mut self,
        command_type: &str,
        data_fields: &[(&str, &str)],
    ) -> Result<String, SessionError> {
        self.writer.send_rpc_command(command_type, data_fields)
    }

    pub fn submit_prompt(&mut self, session_id: &str, prompt: &str) -> Result<(), SessionError> {
        self.writer.submit_prompt(session_id, prompt)
    }

    pub fn into_split(self) -> (PiRpcReader<R>, PiRpcWriter<W>) {
        (self.reader, self.writer)
    }
}

pub struct PiRpcSession {
    child: Child,
    io: PiRpcIo<ChildStdout, ChildStdin>,
}

impl PiRpcSession {
    pub fn spawn(command: PiCommand) -> Result<Self, SessionError> {
        let mut child = command
            .into_command()
            .spawn()
            .map_err(SessionError::Spawn)?;
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

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, SessionError> {
        self.child.try_wait().map_err(SessionError::Io)
    }

    pub fn send_command_json(&mut self, command_json: &str) -> Result<(), SessionError> {
        self.io.send_command_json(command_json)
    }

    pub fn send_rpc_command(
        &mut self,
        command_type: &str,
        data_fields: &[(&str, &str)],
    ) -> Result<String, SessionError> {
        self.io.send_rpc_command(command_type, data_fields)
    }

    pub fn submit_prompt(&mut self, session_id: &str, prompt: &str) -> Result<(), SessionError> {
        self.io.submit_prompt(session_id, prompt)
    }

    pub fn into_split(self) -> (Child, PiRpcReader<ChildStdout>, PiRpcWriter<ChildStdin>) {
        let (reader, writer) = self.io.into_split();
        (self.child, reader, writer)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{PiCommand, PiRpcIo, PiRpcReader, PiRpcWriter, SessionError};
    use crate::capabilities::stock_rpc_handshake;
    use yach_proto::{
        ClientEvent, MessageBody, MessageMeta, TransportMessage, default_ui_handshake,
    };

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

        let writer = io.writer.writer.into_inner();
        assert!(writer.is_ok());
        let Ok(writer) = writer else {
            return;
        };
        let written = String::from_utf8(writer);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };

        assert!(written.contains("\"type\":\"switch_session\""));
        assert!(written.contains("\"sessionId\":\"sess-8\""));
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
        assert_eq!(
            command.args,
            vec![String::from("--mode"), String::from("rpc")]
        );
    }

    #[test]
    fn initialize_sends_handshake_and_waits_for_ready() {
        let reader = Cursor::new(
            b"{\"type\":\"response\",\"command\":\"initialize\",\"success\":true}\n{\"type\":\"agent_start\"}\n{\"type\":\"turn_start\"}\n".to_vec(),
        );
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let ready_handshake = io.initialize(default_ui_handshake());
        assert!(ready_handshake.is_ok());
        let Ok(ready_handshake) = ready_handshake else {
            return;
        };

        assert_eq!(ready_handshake, stock_rpc_handshake());

        let writer = io.writer.writer.into_inner();
        assert!(writer.is_ok());
        let Ok(writer) = writer else {
            return;
        };
        let written = String::from_utf8(writer);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };

        assert!(written.contains("\"type\":\"get_state\""));
        assert!(written.contains("\"agent_name\":\"yach-ui\""));
    }

    #[test]
    fn initialize_tolerates_startup_noise() {
        let reader = Cursor::new(
            b"{\"type\":\"response\",\"success\":false,\"error\":\"Unknown command: undefined\"}\n{\"type\":\"response\",\"command\":\"get_state\",\"success\":true}\n".to_vec(),
        );
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let result = io.initialize(default_ui_handshake());
        assert!(result.is_ok());
    }

    #[test]
    fn initialize_rejects_client_events() {
        let reader = Cursor::new(
            b"{\"type\":\"prompt_submitted\",\"session_id\":\"x\",\"prompt\":\"hi\"}\n".to_vec(),
        );
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let error = io.initialize(default_ui_handshake());
        assert!(error.is_err());
    }

    #[test]
    fn writer_can_send_events_without_transport_wrapping_in_callers() {
        let mut writer = PiRpcWriter::new(Vec::<u8>::new());

        let sent = writer.send_event(ClientEvent::ModelSelectedDetailed {
            provider: String::from("openai"),
            model_id: String::from("gpt-5"),
        });
        assert!(sent.is_ok());

        let writer = writer.writer.into_inner();
        assert!(writer.is_ok());
        let Ok(writer) = writer else {
            return;
        };
        let written = String::from_utf8(writer);
        assert!(written.is_ok());
        let Ok(written) = written else {
            return;
        };

        assert!(written.contains("\"type\":\"set_model\""));
    }

    #[test]
    fn io_can_split_reader_and_writer() {
        let io = PiRpcIo::new(Cursor::new(Vec::<u8>::new()), Vec::<u8>::new());
        let (_reader, _writer): (PiRpcReader<_>, PiRpcWriter<_>) = io.into_split();
    }
}
