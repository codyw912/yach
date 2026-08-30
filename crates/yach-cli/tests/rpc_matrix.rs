use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yach_proto::{
    Capability, ClientEvent, DialogResponse, Handshake, ModelInfo, NegotiatedCapabilities,
    PromptOutcome, ServerEvent, SubmittedSecret,
};
use yach_ui::alpha_handshake;
trait TestUnwrap {
    type Output;
    fn test_unwrap(self) -> Self::Output;
}

impl<T, E> TestUnwrap for Result<T, E> {
    type Output = T;
    fn test_unwrap(self) -> Self::Output {
        assert!(self.is_ok());
        match self {
            Ok(value) => value,
            Err(_) => unreachable!(),
        }
    }
}

impl<T> TestUnwrap for Option<T> {
    type Output = T;
    fn test_unwrap(self) -> Self::Output {
        assert!(self.is_some());
        match self {
            Some(value) => value,
            None => unreachable!(),
        }
    }
}

const READ_DEADLINE: Duration = Duration::from_secs(30);
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "yach-rpc-{prefix}-{}-{now}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).test_unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A stdout line plus its arrival time (milliseconds since spawn), so
/// transcript dumps reveal pacing (e.g. live streaming vs an end burst).
type ReaderMessage = Result<(u128, String), String>;

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<ReaderMessage>,
    last_arrival_ms: u128,
    stderr: Arc<Mutex<String>>,
    events: Vec<ServerEvent>,
    /// Frames read but not yet consumed by a `wait_for`. A wait removes only
    /// the frame it matched, so out-of-order waits still find earlier frames
    /// while repeated waits consume repeated frames one occurrence at a time.
    pending: VecDeque<ServerEvent>,
    raw_transcript: Vec<String>,
    home: TempDir,
}

impl RpcChild {
    fn spawn(backend: Option<&str>, project_root: &Path, session_path: &Path) -> Self {
        Self::spawn_with_session_path(backend, project_root, Some(session_path))
    }

    fn spawn_with_default_session(backend: Option<&str>, project_root: &Path) -> Self {
        Self::spawn_with_session_path(backend, project_root, None)
    }

    fn spawn_with_session_path(
        backend: Option<&str>,
        project_root: &Path,
        session_path: Option<&Path>,
    ) -> Self {
        let home = TempDir::new("home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_yach"));
        command
            .arg("rpc")
            .arg("--project-root")
            .arg(project_root)
            // Deterministic matrix: the background models.dev fetch must
            // never touch the network from a test child.
            .arg("--no-catalog-refresh")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("HOME", home.path());

        if let Some(session_path) = session_path {
            command.arg("--session-path").arg(session_path);
        }
        // Keep the test child hermetic. In particular, a developer's saved
        // provider connection or an API key must not alter this matrix.
        for (key, _) in env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("YACH_")
                || matches!(
                    key.as_ref(),
                    "OPENAI_API_KEY"
                        | "OPENAI_BASE_URL"
                        | "ANTHROPIC_API_KEY"
                        | "ANTHROPIC_BASE_URL"
                        | "CODEX_HOME"
                )
            {
                command.env_remove(&*key);
            }
        }
        if let Some(backend) = backend {
            command.arg("--backend").arg(backend);
        }

        let mut child = command.spawn().unwrap_or_else(|error| {
            unreachable!(
                "spawn yach rpc: {error}; binary={}",
                env!("CARGO_BIN_EXE_yach")
            )
        });
        let stdin = child.stdin.take().test_unwrap();
        let stdout = child.stdout.take().test_unwrap();
        let stderr = child.stderr.take().test_unwrap();

        let (line_tx, lines) = mpsc::channel();
        let spawn_instant = std::time::Instant::now();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if line_tx
                            .send(Ok((spawn_instant.elapsed().as_millis(), line)))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = line_tx.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });

        let stderr_text = Arc::new(Mutex::new(String::new()));
        let stderr_text_writer = Arc::clone(&stderr_text);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut text = String::new();
            let _ = reader.read_to_string(&mut text);
            if let Ok(mut output) = stderr_text_writer.lock() {
                output.push_str(&text);
            }
        });

        let mut rpc = Self {
            child,
            stdin: Some(stdin),
            lines,
            stderr: stderr_text,
            events: Vec::new(),
            pending: VecDeque::new(),
            last_arrival_ms: 0,
            raw_transcript: Vec::new(),
            home,
        };
        rpc.wait_ready();
        rpc
    }

    fn send(&mut self, event: &ClientEvent) {
        let Some(stdin) = self.stdin.as_mut() else {
            unreachable!("send client event after rpc stdin closed: {event:?}");
        };
        let line = event.to_jsonl().test_unwrap();
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .unwrap_or_else(|error| unreachable!("write rpc client event {event:?}: {error}"));
    }

    fn wait_ready(&mut self) {
        self.send(&ClientEvent::Initialize(alpha_handshake()));
        self.wait_for(|event| matches!(event, ServerEvent::Ready { .. }));
    }

    fn wait_for<F>(&mut self, mut predicate: F) -> ServerEvent
    where
        F: FnMut(&ServerEvent) -> bool,
    {
        if let Some(index) = self.pending.iter().position(&mut predicate) {
            return self.pending.remove(index).test_unwrap();
        }
        loop {
            let event = self.read_event();
            if predicate(&event) {
                return event;
            }
            self.pending.push_back(event);
        }
    }

    fn read_event(&mut self) -> ServerEvent {
        let message = self
            .lines
            .recv_timeout(READ_DEADLINE)
            .unwrap_or_else(|error| {
                unreachable!(
                    "timed out waiting for rpc server event: {error}\n{}",
                    self.dump()
                )
            });
        let (arrival_ms, line) = message.unwrap_or_else(|error| {
            unreachable!("rpc stdout read failed: {error}\n{}", self.dump())
        });
        self.last_arrival_ms = arrival_ms;
        self.raw_transcript
            .push(format!("+{arrival_ms:>6}ms {line}"));
        let event = ServerEvent::from_jsonl(&line).unwrap_or_else(|error| {
            unreachable!(
                "rpc emitted non-ServerEvent JSONL: {error}\n{}",
                self.dump()
            )
        });
        self.events.push(event.clone());
        event
    }

    /// Arrival time (ms since spawn) of the most recently read frame; pacing
    /// assertions compare these across waits.
    fn last_arrival_ms(&self) -> u128 {
        self.last_arrival_ms
    }

    fn drain_ready_events(&mut self) {
        while let Ok(message) = self.lines.try_recv() {
            let (arrival_ms, line) = message.unwrap_or_else(|error| {
                unreachable!(
                    "rpc stdout read failed while draining: {error}\n{}",
                    self.dump()
                )
            });
            self.raw_transcript
                .push(format!("+{arrival_ms:>6}ms {line}"));
            let event = ServerEvent::from_jsonl(&line).unwrap_or_else(|error| {
                unreachable!(
                    "rpc emitted non-ServerEvent JSONL: {error}\n{}",
                    self.dump()
                )
            });
            self.events.push(event.clone());
            self.pending.push_back(event);
        }
    }

    fn events(&self) -> &[ServerEvent] {
        &self.events
    }

    fn dump(&self) -> String {
        let stderr = self.stderr.lock().map_or_else(
            |_| String::from("<stderr lock poisoned>"),
            |text| text.clone(),
        );
        format!(
            "rpc transcript:\n{}\nrpc stderr:\n{}",
            self.raw_transcript.join("\n"),
            stderr
        )
    }

    fn shutdown(&mut self) {
        // Closing stdin is the protocol's graceful EOF signal. Give the child
        // a little room to flush before using kill as a test cleanup fallback.
        self.stdin.take();
        for _ in 0..100 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn wait_for_dialog(child: &mut RpcChild, id: &str) -> yach_proto::DialogRequest {
    let event = child.wait_for(|event| {
        matches!(event, ServerEvent::DialogRequested(request) if request.id.as_deref() == Some(id))
    });
    let ServerEvent::DialogRequested(request) = event else {
        unreachable!("dialog predicate returned a non-dialog event");
    };
    request
}

fn wait_for_models(child: &mut RpcChild) -> Vec<ModelInfo> {
    // Connection-backed models arrive on the discovery snapshot; the curated
    // available list can legitimately stay provider-unconfigured.
    let event = child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::DiscoveredModelsUpdated { models }
                if models.iter().any(|model| model.connection_id.is_some())
        )
    });
    let ServerEvent::DiscoveredModelsUpdated { models } = event else {
        unreachable!("model predicate returned a non-model event");
    };
    models
}

fn assert_session_log(path: &Path, required_fragments: &[&str]) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| unreachable!("read session log {}: {error}", path.display()));
    assert!(
        !text.trim().is_empty(),
        "session log is empty: {}",
        path.display()
    );
    for fragment in required_fragments {
        assert!(
            text.contains(fragment),
            "session log {} lacks {fragment:?}",
            path.display()
        );
    }
}

/// Drive the full first-connection choreography over the wire: first-render
/// marker, `/connect` dialog flow against the local OpenAI-compatible
/// fixture, discovery, and explicit model activation. Returns the activated
/// connection model and its connection id.
fn create_and_activate_connection(child: &mut RpcChild, base_url: &str) -> (ModelInfo, String) {
    child.send(&ClientEvent::FirstRenderCompleted);
    child.send(&ClientEvent::ConnectionsRequested);
    wait_for_dialog(child, "provider-connection:root");

    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:root"),
        response: DialogResponse::Selection {
            value: String::from("add"),
        },
    });
    wait_for_dialog(child, "provider-connection:provider");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:provider"),
        response: DialogResponse::Selection {
            value: String::from("openai-compatible"),
        },
    });
    wait_for_dialog(child, "provider-connection:label");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:label"),
        response: DialogResponse::Text {
            value: String::from("matrix fixture"),
        },
    });
    wait_for_dialog(child, "provider-connection:base-url");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:base-url"),
        response: DialogResponse::Text {
            value: base_url.to_owned(),
        },
    });
    wait_for_dialog(child, "provider-connection:secret:create");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:secret:create"),
        response: DialogResponse::Secret {
            value: SubmittedSecret::new("matrix-fixture-key"),
        },
    });

    // A successful create refreshes the model catalog and then lists the
    // connections again. Wait on protocol identities rather than request
    // counts: discovery can legitimately retry or refresh more than once.
    wait_for_dialog(child, "provider-connection:root");
    child.send(&ClientEvent::AvailableModelsRequested);
    let models = wait_for_models(child);
    let model = models
        .iter()
        .find(|model| model.connection_id.is_some())
        .cloned()
        .unwrap_or_else(|| unreachable!("connection model missing from catalog\n{}", child.dump()));
    let Some(connection_id) = model.connection_id.clone() else {
        unreachable!("catalog connection model lacks its connection id");
    };
    child.send(&ClientEvent::ModelActivationRequested {
        target: yach_proto::ModelTarget {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
            connection_id: connection_id.clone(),
            connection_key: None,
        },
        intent: yach_proto::ModelActivationIntent::SessionOnly,
        request_id: 7,
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::ModelActivationFinished(result)
                if result.session_activated
                    && result.target.model_id == model.id
                    && result.target.connection_id == connection_id
        )
    });
    (model, connection_id)
}

#[test]
fn rpc_capability_drift_is_explicit() {
    let workspace = TempDir::new("capabilities");
    let session_path = workspace.path().join("capabilities.jsonl");
    let mut child = RpcChild::spawn(Some("fixture"), workspace.path(), &session_path);
    // Give the startup batch time to finish, then request one more event so
    // any duplicated initial batch would be captured before asserting.
    child.send(&ClientEvent::SessionStatsRequested);
    child.wait_for(|event| matches!(event, ServerEvent::SessionStatsUpdated(_)));
    let ready_frames: Vec<_> = child
        .events()
        .iter()
        .filter_map(|event| match event {
            ServerEvent::Ready { handshake } => Some(handshake.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ready_frames.len(),
        1,
        "exactly one Ready frame per connection\n{}",
        child.dump()
    );

    let expected = NegotiatedCapabilities::from_handshakes(
        &alpha_handshake(),
        &Handshake::new(
            "yach-native",
            vec![
                Capability::PromptStreaming,
                Capability::PromptCancellation,
                Capability::StatusEntries,
                Capability::Notifications,
                Capability::LocalEdit,
                Capability::ExtensionLifecycle,
                Capability::FirstRenderEvents,
                Capability::StructuredReviewRows,
                Capability::ApprovalModes,
                Capability::ModelState,
                Capability::PromptAttemptReset,
            ],
        ),
    )
    .ready_handshake();
    assert_eq!(
        ready_frames[0],
        expected,
        "Ready handshake capability drift\n{}",
        child.dump()
    );
}

#[test]
fn default_rpc_sessions_live_in_project_keyed_user_state() {
    let workspace = TempDir::new("user-state-session");
    let mut child = RpcChild::spawn_with_default_session(Some("fixture"), workspace.path());
    let state = child.wait_for(|event| matches!(event, ServerEvent::StateUpdated(_)));
    let ServerEvent::StateUpdated(state) = state else {
        unreachable!("state predicate returned a different event");
    };
    let session_id = state.session_id.test_unwrap();
    child.send(&ClientEvent::PromptSubmitted {
        session_id: session_id.clone(),
        prompt: String::from("persist outside the project"),
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::PromptFinished {
                session_id: finished_id,
                outcome: PromptOutcome::Completed,
                ..
            } if finished_id == &session_id
        )
    });
    child.shutdown();

    assert!(!workspace.path().join(".yach").exists());
    let sessions_root = child.home.path().join(".yach/sessions");
    let project_dirs = fs::read_dir(&sessions_root)
        .test_unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(project_dirs.len(), 1);
    assert!(
        project_dirs[0]
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("yach-rpc-user-state-session-"))
    );
    assert!(
        project_dirs[0]
            .join(format!("{session_id}.jsonl"))
            .is_file()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        for directory in [
            child.home.path().join(".yach"),
            sessions_root,
            project_dirs[0].clone(),
        ] {
            let mode = fs::metadata(directory).test_unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let file_mode = fs::metadata(project_dirs[0].join(format!("{session_id}.jsonl")))
            .test_unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
    }
}

#[test]
fn rpc_approval_mode_change_is_correlated_persisted_and_auditable() {
    let workspace = TempDir::new("approval-mode");
    let mut child = RpcChild::spawn_with_default_session(Some("fixture"), workspace.path());
    child.send(&ClientEvent::ApprovalModeSelected {
        request_id: 41,
        mode: yach_proto::ApprovalMode::AcceptEdits,
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::ApprovalModeChanged {
                request_id: 41,
                mode: yach_proto::ApprovalMode::AcceptEdits,
            }
        )
    });
    child.shutdown();

    let permission_files = fs::read_dir(child.home.path().join(".yach/permissions"))
        .test_unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(permission_files.len(), 1);
    assert!(
        fs::read_to_string(&permission_files[0])
            .test_unwrap()
            .contains("\"mode\":\"accept-edits\"")
    );

    let session_root = child.home.path().join(".yach/sessions");
    let session_file = fs::read_dir(session_root)
        .test_unwrap()
        .filter_map(Result::ok)
        .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten())
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .test_unwrap();
    assert!(
        fs::read_to_string(session_file)
            .test_unwrap()
            .contains("\"type\":\"approval_mode_changed\"")
    );
}

#[test]
fn rpc_full_access_is_correlated_auditable_and_not_persisted() {
    let workspace = TempDir::new("full-access-mode");
    let mut child = RpcChild::spawn_with_default_session(Some("fixture"), workspace.path());
    child.send(&ClientEvent::ApprovalModeSelected {
        request_id: 42,
        mode: yach_proto::ApprovalMode::FullAccess,
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::ApprovalModeChanged {
                request_id: 42,
                mode: yach_proto::ApprovalMode::FullAccess,
            }
        )
    });
    child.shutdown();

    let permissions = child.home.path().join(".yach/permissions");
    assert!(!permissions.exists() || fs::read_dir(permissions).test_unwrap().next().is_none());
    let session_file = fs::read_dir(child.home.path().join(".yach/sessions"))
        .test_unwrap()
        .filter_map(Result::ok)
        .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten())
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .test_unwrap();
    let session = fs::read_to_string(session_file).test_unwrap();
    assert!(session.contains("\"type\":\"approval_mode_changed\""));
    assert!(session.contains("\"mode\":\"full-access\""));
}
#[test]
fn rpc_provider_cancel_interrupts_midstream() {
    let workspace = TempDir::new("provider-cancel");
    let session_path = workspace.path().join("provider-cancel.jsonl");
    let fixture = MockOpenAiServer::new();
    let mut child = RpcChild::spawn(None, workspace.path(), &session_path);

    let (_model, _connection_id) = create_and_activate_connection(&mut child, &fixture.base_url());

    child.send(&ClientEvent::PromptSubmitted {
        session_id: String::from("provider-cancel"),
        prompt: String::from("stream slowly so cancellation lands mid-turn"),
    });
    // Live token streaming (design 2026-08-18): the first delta arrives while
    // the fixture stream is still running, so cancelling here is genuinely
    // mid-stream.
    child.wait_for(|event| matches!(event, ServerEvent::PromptDelta { .. }));
    child.send(&ClientEvent::PromptCancelled {
        session_id: String::from("provider-cancel"),
    });

    let finished = child.wait_for(|event| matches!(event, ServerEvent::PromptFinished { .. }));
    let ServerEvent::PromptFinished { outcome, .. } = finished else {
        unreachable!("finish predicate returned a non-terminal event");
    };
    assert!(
        matches!(outcome, PromptOutcome::Cancelled),
        "mid-stream cancel must end the turn cancelled, got {outcome:?}\n{}",
        child.dump()
    );

    // Nothing streams after the terminal frame.
    thread::sleep(Duration::from_millis(600));
    child.drain_ready_events();
    let terminal_index = child
        .events()
        .iter()
        .rposition(|event| matches!(event, ServerEvent::PromptFinished { .. }))
        .unwrap_or(0);
    assert!(
        !child.events()[terminal_index + 1..]
            .iter()
            .any(|event| matches!(event, ServerEvent::PromptDelta { .. })),
        "deltas arrived after the cancelled terminal frame\n{}",
        child.dump()
    );
}

#[test]
fn rpc_provider_deltas_stream_live_and_exactly_once() {
    let workspace = TempDir::new("provider-stream");
    let session_path = workspace.path().join("provider-stream.jsonl");
    let fixture = MockOpenAiServer::new();
    let mut child = RpcChild::spawn(None, workspace.path(), &session_path);

    let (_model, _connection_id) = create_and_activate_connection(&mut child, &fixture.base_url());

    child.send(&ClientEvent::PromptSubmitted {
        session_id: String::from("provider-stream"),
        prompt: String::from("stream the whole response"),
    });
    child.wait_for(|event| matches!(event, ServerEvent::PromptDelta { .. }));
    let first_delta_at = child.last_arrival_ms();
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::PromptFinished {
                outcome: PromptOutcome::Completed,
                ..
            }
        )
    });
    let finished_at = child.last_arrival_ms();

    // Pacing: the fixture streams 12 chunks over ~3s; a post-hoc burst would
    // put the first delta within milliseconds of the terminal frame.
    assert!(
        finished_at.saturating_sub(first_delta_at) >= 1_500,
        "deltas did not stream live: first at +{first_delta_at}ms, finished at +{finished_at}ms\n{}",
        child.dump()
    );

    // Exactly-once: burst suppression must not re-send streamed text.
    let streamed: String = child
        .events()
        .iter()
        .filter_map(|event| match event {
            ServerEvent::PromptDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    for index in 0..12_u32 {
        let marker = format!("chunk-{index} ");
        assert_eq!(
            streamed.matches(&marker).count(),
            1,
            "chunk marker {marker:?} must appear exactly once\n{}",
            child.dump()
        );
    }
}

#[test]
fn rpc_fixture_resume_rehydrates_user_and_assistant_rows() {
    let workspace = TempDir::new("resume");
    let session_path = workspace.path().join("resume.jsonl");
    let session_id = "resume";
    let prompt = "persist this turn for resume parity";

    {
        let mut first = RpcChild::spawn(Some("fixture"), workspace.path(), &session_path);
        first.send(&ClientEvent::PromptSubmitted {
            session_id: session_id.to_owned(),
            prompt: prompt.to_owned(),
        });
        let finished = first.wait_for(|event| {
            matches!(
                event,
                ServerEvent::PromptFinished { session_id: id, outcome: PromptOutcome::Completed, .. }
                    if id == session_id
            )
        });
        assert!(matches!(finished, ServerEvent::PromptFinished { .. }));
        first.shutdown();
    }
    assert_session_log(&session_path, &[prompt, "fixture response"]);

    let mut resumed = RpcChild::spawn(Some("fixture"), workspace.path(), &session_path);
    resumed.send(&ClientEvent::SessionMessagesRequested);
    let event =
        resumed.wait_for(|event| matches!(event, ServerEvent::SessionMessagesUpdated { .. }));
    let ServerEvent::SessionMessagesUpdated { messages } = event else {
        unreachable!("session messages predicate returned another event");
    };
    assert!(
        messages
            .iter()
            .any(|message| message.role == "user" && message.text == prompt),
        "resumed transcript lacks user row: {messages:?}\n{}",
        resumed.dump()
    );
    assert!(
        messages
            .iter()
            .any(|message| message.role == "assistant" && message.text.contains("fixture response")),
        "resumed transcript lacks assistant row: {messages:?}\n{}",
        resumed.dump()
    );
}

#[test]
fn rpc_default_backend_remove_last_connection_is_honest() {
    let workspace = TempDir::new("connections");
    let session_path = workspace.path().join("connections.jsonl");
    let fixture = MockOpenAiServer::new();
    let mut child = RpcChild::spawn(None, workspace.path(), &session_path);

    let (model, connection_id) = create_and_activate_connection(&mut child, &fixture.base_url());
    let _ = model;

    child.send(&ClientEvent::ConnectionsRequested);
    wait_for_dialog(&mut child, "provider-connection:root");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:root"),
        response: DialogResponse::Selection {
            value: connection_id.clone(),
        },
    });
    wait_for_dialog(&mut child, "provider-connection:actions");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:actions"),
        response: DialogResponse::Selection {
            value: String::from("remove"),
        },
    });
    wait_for_dialog(&mut child, "provider-connection:remove");
    child.send(&ClientEvent::DialogResolved {
        dialog_id: String::from("provider-connection:remove"),
        response: DialogResponse::Confirmed { accepted: true },
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::StatusUpdated { message }
                if message == "connection removed"
        )
    });
    child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::ModelSelectionRequired {
                reason: yach_proto::ModelTargetResolutionReason::ConnectionMissing,
                ..
            }
        )
    });
    child.send(&ClientEvent::PromptSubmitted {
        session_id: String::from("connections"),
        prompt: String::from("this must fail without a provider"),
    });
    let failed = child.wait_for(|event| {
        matches!(
            event,
            ServerEvent::PromptFinished {
                session_id,
                outcome: PromptOutcome::Failed,
                message: Some(message),
            } if session_id == "connections" && message.contains("provider")
        )
    });
    assert!(matches!(failed, ServerEvent::PromptFinished { .. }));
    assert_session_log(&session_path, &["this must fail without a provider"]);
}

struct MockOpenAiServer {
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockOpenAiServer {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        listener.set_nonblocking(true).test_unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !stop_thread.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => handle_openai_request(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.address)
    }
}

impl Drop for MockOpenAiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle_openai_request(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let Ok(count) = stream.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..count]);
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() > 128 * 1024 {
            return;
        }
    };
    let header = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let content_length = header
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        })
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while request.len() < header_end.saturating_add(content_length) {
        let Ok(count) = stream.read(&mut buffer) else {
            return;
        };
        if count == 0 {
            return;
        }
        request.extend_from_slice(&buffer[..count]);
    }

    let request_line = header.lines().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();
    let body = &request[header_end..header_end.saturating_add(content_length)];
    let (status, content_type, response_body) = if request_line.starts_with("GET /v1/models") {
        (
            "200 OK",
            "application/json",
            br#"{"object":"list","data":[{"id":"matrix-model","object":"model","created":0,"owned_by":"matrix"}]}"#.to_vec(),
        )
    } else if request_line.starts_with("POST /v1/chat/completions") {
        let stream_requested = String::from_utf8_lossy(body).contains("\"stream\":true");
        if stream_requested {
            // Stream slowly enough that mid-stream cancellation and pacing
            // assertions genuinely interleave with live deltas.
            let header =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            if stream.write_all(header.as_bytes()).is_err() {
                return;
            }
            for index in 0..12_u32 {
                let chunk = format!(
                    "data: {{\"id\":\"matrix-response\",\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"content\":\"chunk-{index} \"}},\"finish_reason\":null}}]}}\n\n"
                );
                if stream
                    .write_all(chunk.as_bytes())
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    // The client cancelled and closed the socket: expected.
                    return;
                }
                thread::sleep(Duration::from_millis(250));
            }
            let _ = stream.write_all(
                b"data: {\"id\":\"matrix-response\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            );
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        (
            "200 OK",
            "application/json",
            br#"{"id":"matrix-response","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"matrix response"},"finish_reason":"stop"}]}"#.to_vec(),
        )
    } else {
        (
            "404 Not Found",
            "application/json",
            format!("{{\"error\":{{\"message\":\"unknown fixture path {path}\"}}}}").into_bytes(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(&response_body);
    let _ = stream.shutdown(Shutdown::Both);
}
