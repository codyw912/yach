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
    pub(crate) reader: BufReader<R>,
    pub(crate) writer: BufWriter<W>,
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
                    if message == "get_state" => return Ok(stock_rpc_handshake()),
                MessageBody::ServerEvent(ServerEvent::Ready { handshake }) => return Ok(handshake),
                MessageBody::ServerEvent(ServerEvent::StatusUpdated { message })
                    if message == "initialize" || message == "agent_started" => {}
                MessageBody::ServerEvent(event) => return Err(SessionError::UnexpectedEvent(event)),
                MessageBody::ClientEvent(_) => return Err(SessionError::WrongMessageDirection),
            }
        }
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
    next_request_id: u64,
}

impl PiRpcSession {
    pub fn spawn(command: PiCommand) -> Result<Self, SessionError> {
        let mut child = command.into_command().spawn().map_err(SessionError::Spawn)?;
        let stdout = child.stdout.take().ok_or(SessionError::MissingStdout)?;
        let stdin = child.stdin.take().ok_or(SessionError::MissingStdin)?;

        Ok(Self {
            child,
            io: PiRpcIo::new(stdout, stdin),
            next_request_id: 1,
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
        self.io.writer.write_all(command_json.as_bytes())?;
        self.io.writer.flush()?;
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
        payload.insert(String::from("id"), serde_json::Value::String(request_id.clone()));
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
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{PiCommand, PiRpcIo, SessionError};
    use crate::capabilities::stock_rpc_handshake;
    use yach_proto::{ClientEvent, MessageBody, MessageMeta, TransportMessage, default_ui_handshake};

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
        assert_eq!(command.args, vec![String::from("--mode"), String::from("rpc")]);
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

        assert!(written.contains("\"type\":\"get_state\""));
        assert!(written.contains("\"agent_name\":\"yach-ui\""));
    }

    #[test]
    fn initialize_rejects_non_ready_events() {
        let reader = Cursor::new(
            b"{\"type\":\"response\",\"success\":false,\"error\":\"Unknown command: undefined\"}\n".to_vec(),
        );
        let writer = Vec::<u8>::new();
        let mut io = PiRpcIo::new(reader, writer);

        let error = io.initialize(default_ui_handshake());
        assert!(error.is_err());
        let Err(error) = error else {
            return;
        };

        match error {
            SessionError::UnexpectedEvent(yach_proto::ServerEvent::StatusUpdated { message }) => {
                assert_eq!(message, "Unknown command: undefined");
            }
            other => assert!(matches!(other, SessionError::UnexpectedEvent(_))),
        }
    }
}
