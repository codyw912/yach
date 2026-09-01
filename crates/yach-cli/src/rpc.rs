//! Stdio JSONL protocol server (`yach rpc`).
//!
//! The transport is deliberately thin: protocol events remain the only
//! client/backend seam, while diagnostics and argument errors stay on stderr.

use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;
use yach_backend::{
    BackendMetadata, ModelDiscoveryFuture, ProviderConfig, project_session_log_dir,
    run_native_loop_with_negotiated_capabilities, session_log_path_in, start_backend_session,
};
use yach_proto::{BackendEvent, ClientEvent, Handshake, NegotiatedCapabilities, ServerEvent};

use super::{
    ModelOverrideLayers, NativeTuiBackendSetup, RunnerConfigInput, catalog_refresh,
    fresh_session_id, native_backend_handshake, provider_connection_timeout,
    provider_test_delay_ms, rig_provider_adapter_config_from_env_with_model_override,
    runner_config, unconfigured_launch_setup_error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RpcBackend {
    Fixture,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RpcOptions {
    pub project_root: Option<PathBuf>,
    pub session_path: Option<PathBuf>,
    pub session_id: Option<String>,
    pub backend: RpcBackend,
    /// `--no-catalog-refresh` disables the background models.dev fetch so
    /// deterministic clients (CI, the invariant matrix) never touch the
    /// network.
    pub catalog_refresh: bool,
}

pub(crate) fn parse_rpc_args(args: &[String]) -> Result<RpcOptions, String> {
    let mut project_root = None;
    let mut session_path = None;
    let mut session_id = None;
    let mut backend = RpcBackend::System;
    let mut catalog_refresh = true;
    let mut index = 0;
    while index < args.len() {
        let value_of = |flag: &str| {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match args[index].as_str() {
            "--project-root" => {
                project_root = Some(PathBuf::from(value_of("--project-root")?));
                index += 2;
            }
            "--session-path" => {
                session_path = Some(PathBuf::from(value_of("--session-path")?));
                index += 2;
            }
            "--session" => {
                let raw = value_of("--session")?;
                if raw.is_empty() || raw.contains(['/', '\\']) || raw.contains("..") {
                    return Err(format!("--session must be a plain session id, got '{raw}'"));
                }
                session_id = Some(raw);
                index += 2;
            }
            "--backend" => {
                let raw = value_of("--backend")?;
                backend = match raw.as_str() {
                    "fixture" => RpcBackend::Fixture,
                    "system" => RpcBackend::System,
                    other => return Err(format!("unknown rpc backend '{other}'")),
                };
                index += 2;
            }
            "--no-catalog-refresh" => {
                catalog_refresh = false;
                index += 1;
            }
            other => return Err(format!("unknown 'rpc' flag '{other}'")),
        }
    }
    if session_id.is_some() && session_path.is_some() {
        return Err(String::from(
            "--session and --session-path are mutually exclusive",
        ));
    }
    Ok(RpcOptions {
        project_root,
        session_path,
        session_id,
        backend,
        catalog_refresh,
    })
}

fn resolved_project_root(options: &RpcOptions) -> Option<PathBuf> {
    options
        .project_root
        .clone()
        .or_else(|| std::env::current_dir().ok())
}

fn resolved_session_path(
    options: &RpcOptions,
    project_root: Option<&PathBuf>,
) -> io::Result<PathBuf> {
    if let Some(path) = options.session_path.clone() {
        return Ok(path);
    }
    let project_root =
        project_root.ok_or_else(|| io::Error::other("unable to resolve the current project"))?;
    let session_dir = project_session_log_dir(project_root)?;
    let session_id = options.session_id.clone().unwrap_or_else(fresh_session_id);
    Ok(session_log_path_in(&session_dir, &session_id))
}

#[derive(Debug)]
pub(crate) enum ClientInput {
    Event(ClientEvent),
    Invalid(String),
}

/// Parse stdin independently of the async runtime. Every malformed line is
/// represented as a recoverable input item; a malformed line never closes the
/// channel or terminates the child.
pub(crate) fn pump_stdin<R: BufRead>(reader: R, tx: &mpsc::UnboundedSender<ClientInput>) {
    for line in reader.lines() {
        match line {
            Ok(line) => match ClientEvent::from_jsonl(&line) {
                Ok(event) => {
                    if tx.send(ClientInput::Event(event)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    if tx.send(ClientInput::Invalid(error.to_string())).is_err() {
                        break;
                    }
                }
            },
            Err(error) => {
                let _ = tx.send(ClientInput::Invalid(error.to_string()));
                break;
            }
        }
    }
}

/// Emit one server event, flushing at the JSONL frame boundary. Connected and
/// disconnected plumbing events are intentionally handled by the caller and
/// never serialized on the wire.
pub(crate) fn write_server_event<W: Write>(writer: &mut W, event: &ServerEvent) -> io::Result<()> {
    let line = event
        .to_jsonl()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

fn write_invalid_line<W: Write>(writer: &mut W, error: &str) -> io::Result<()> {
    write_server_event(
        writer,
        &ServerEvent::StatusUpdated {
            message: format!("rpc: invalid client event: {error}"),
        },
    )
}

/// Read through recoverable input until the protocol handshake arrives.
/// Non-initialize events are rejected on the wire but do not terminate the
/// child, allowing clients to correct an early framing mistake.
pub(crate) fn read_initialize<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> io::Result<Option<Handshake>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match ClientEvent::from_jsonl(&line) {
            Ok(ClientEvent::Initialize(handshake)) => return Ok(Some(handshake)),
            Ok(_) => write_invalid_line(writer, "expected initialize event")?,
            Err(error) => write_invalid_line(writer, &error.to_string())?,
        }
    }
}
async fn drain_backend_events<W: Write>(
    backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    writer: &mut W,
) -> io::Result<()> {
    while let Some(event) = backend_rx.recv().await {
        match event {
            BackendEvent::Server(event) => write_server_event(writer, &event)?,
            BackendEvent::Connected { .. } => {}
            BackendEvent::Disconnected { .. } => break,
        }
    }
    Ok(())
}

/// Run the protocol pumps after a backend session has been started.
pub(crate) async fn run_pumps<R: BufRead + Send + 'static>(
    reader: R,
    client_tx: mpsc::UnboundedSender<ClientEvent>,
    mut backend_rx: mpsc::UnboundedReceiver<BackendEvent>,
) -> io::Result<()> {
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || pump_stdin(reader, &input_tx));
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut client_tx = Some(client_tx);

    loop {
        tokio::select! {
            input = input_rx.recv() => match input {
                Some(ClientInput::Event(event)) => {
                    if client_tx.as_ref().is_none_or(|tx| tx.send(event).is_err()) {
                        client_tx = None;
                    }
                }
                Some(ClientInput::Invalid(error)) => write_invalid_line(&mut writer, &error)?,
                None => {
                    // Dropping the sender is the graceful EOF signal. The
                    // backend cancels any live turn and exits once it observes
                    // its client channel close.
                    drop(client_tx.take());
                    drain_backend_events(&mut backend_rx, &mut writer).await?;
                    break;
                }
            },
            backend = backend_rx.recv() => match backend {
                Some(BackendEvent::Server(event)) => write_server_event(&mut writer, &event)?,
                Some(BackendEvent::Connected { .. }) => {}
                Some(BackendEvent::Disconnected { .. }) | None => break,
            }
        }
    }
    writer.flush()
}

async fn run_rpc(options: RpcOptions) -> io::Result<()> {
    let project_root = resolved_project_root(&options);
    let session_path = resolved_session_path(&options, project_root.as_ref())?;
    let mut reader = BufReader::new(io::stdin());
    let stdout = io::stdout();
    let mut startup_writer = stdout.lock();
    let Some(client_handshake) = read_initialize(&mut reader, &mut startup_writer)? else {
        startup_writer.flush()?;
        return Ok(());
    };
    startup_writer.flush()?;
    drop(startup_writer);
    let layers = ModelOverrideLayers::load_for_project(project_root.as_deref());
    let (setup, catalog_refresh) = match options.backend {
        RpcBackend::Fixture => (NativeTuiBackendSetup::Fixture, None),
        RpcBackend::System => {
            let refresh = options
                .catalog_refresh
                .then(|| catalog_refresh::spawn_refresh_status(layers.fetched.clone()));
            match rig_provider_adapter_config_from_env_with_model_override(None, &layers) {
                Ok(resolved) => (
                    NativeTuiBackendSetup::Configured {
                        adapter: Arc::new(resolved.adapter),
                        model: resolved.model,
                        responses_compact: resolved
                            .profile
                            .responses_compact
                            .map(|capability| capability.value),
                    },
                    refresh,
                ),
                Err(error) => (
                    NativeTuiBackendSetup::Unconfigured(unconfigured_launch_setup_error(
                        &error,
                        super::provider_connections::has_stored_connections(),
                    )),
                    refresh,
                ),
            }
        }
    };
    let environment = match &setup {
        NativeTuiBackendSetup::Configured { adapter, .. } => {
            Some(super::provider_connections::EnvironmentConnection::from_runtime_adapter(adapter))
        }
        NativeTuiBackendSetup::Fixture | NativeTuiBackendSetup::Unconfigured(_) => None,
    };
    let runtime_timeout = match &setup {
        NativeTuiBackendSetup::Configured { adapter, .. } => adapter.timeout,
        NativeTuiBackendSetup::Fixture | NativeTuiBackendSetup::Unconfigured(_) => {
            provider_connection_timeout()
        }
    };
    let provider_connections = match options.backend {
        RpcBackend::Fixture => None,
        RpcBackend::System => super::provider_connections::CliProviderConnectionRuntime::system(
            layers.clone(),
            environment,
            runtime_timeout,
            provider_test_delay_ms(),
        )
        .map(|runtime| Arc::new(runtime) as Arc<dyn yach_backend::ProviderConnectionRuntime>),
    };
    let backend_handshake = native_backend_handshake(&setup, provider_connections.is_some());
    let negotiated = NegotiatedCapabilities::from_handshakes(&client_handshake, &backend_handshake);
    let backend_session = start_backend_session(BackendMetadata::native(), negotiated.clone());
    // Initialize is consumed for negotiation only and deliberately NOT
    // forwarded: `run_native_loop` already emits the full initial batch
    // (Ready, state, status, models) once at startup, so forwarding would
    // duplicate every frame of it on the wire.
    let (provider, provider_setup_error, model_discovery) = match setup {
        NativeTuiBackendSetup::Fixture => (None, None, None),
        NativeTuiBackendSetup::Unconfigured(error) => (None, error, None),
        NativeTuiBackendSetup::Configured {
            adapter,
            model,
            responses_compact,
        } => (
            Some(ProviderConfig {
                model,
                connection_id: provider_connections
                    .as_ref()
                    .map(|_| yach_connections::ConnectionId::environment()),
                connection_key: None,
                connection_display: provider_connections
                    .as_ref()
                    .map(|_| String::from("Environment")),
                test_delay_ms: provider_test_delay_ms(),
                adapter,
                responses_compact,
                catalog_models: Vec::new().into(),
            }),
            None,
            None::<ModelDiscoveryFuture>,
        ),
    };
    let backend_config = runner_config(RunnerConfigInput {
        session_path,
        project_root,
        provider,
        provider_setup_error,
        startup_trace: None,
        catalog_refresh,
        model_discovery,
        provider_connections,
    });
    let backend_handle = tokio::spawn(run_native_loop_with_negotiated_capabilities(
        backend_session.endpoints.client_rx,
        backend_session.endpoints.backend_tx,
        backend_config,
        negotiated,
    ));
    let result = run_pumps(
        reader,
        backend_session.channels.client_tx,
        backend_session.channels.backend_rx,
    )
    .await;
    let _ = backend_handle.await;
    result
}

pub(crate) fn run_rpc_command(options: RpcOptions) -> u8 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "error=failed to create tokio runtime: {error}"
            );
            return 2;
        }
    };
    match runtime.block_on(run_rpc(options)) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(io::stderr(), "error=rpc transport: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    use std::io::Cursor;

    #[test]
    fn rpc_args_default_to_system_backend() {
        let options = parse_rpc_args(&[]).test_unwrap();
        assert_eq!(options.backend, RpcBackend::System);
        assert!(options.project_root.is_none());
    }

    #[test]
    fn rpc_args_parse_paths_session_and_fixture() {
        let options = parse_rpc_args(&[
            "--project-root".into(),
            "/tmp/project".into(),
            "--session".into(),
            "session-1".into(),
            "--backend".into(),
            "fixture".into(),
        ])
        .test_unwrap();
        assert_eq!(options.project_root, Some(PathBuf::from("/tmp/project")));
        assert_eq!(options.session_id.as_deref(), Some("session-1"));
        assert_eq!(options.backend, RpcBackend::Fixture);
    }

    #[test]
    fn rpc_args_reject_unknown_and_conflicting_flags() {
        assert!(parse_rpc_args(&["--nope".into()]).is_err());
        assert!(
            parse_rpc_args(&[
                "--session".into(),
                "one".into(),
                "--session-path".into(),
                "two".into(),
            ])
            .is_err()
        );
    }

    #[test]
    fn malformed_input_emits_error_and_continues() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        pump_stdin(
            Cursor::new("not-json\n{\"type\":\"first_render_completed\"}\n"),
            &tx,
        );
        drop(tx);
        assert!(matches!(rx.blocking_recv(), Some(ClientInput::Invalid(_))));
        assert!(matches!(
            rx.blocking_recv(),
            Some(ClientInput::Event(ClientEvent::FirstRenderCompleted))
        ));
        assert!(rx.blocking_recv().is_none());
    }

    #[test]
    fn stdout_frames_are_jsonl_and_pure() {
        let mut output = Vec::new();
        write_server_event(
            &mut output,
            &ServerEvent::StatusUpdated {
                message: "ok".into(),
            },
        )
        .test_unwrap();
        let text = String::from_utf8(output).test_unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(ServerEvent::from_jsonl(text.trim_end()).is_ok());
        assert!(!text.contains("Connected"));
    }

    #[test]
    fn initialize_reader_skips_bad_and_non_initialize_lines() {
        let handshake = Handshake::new("client", Vec::new());
        let initialize = ClientEvent::Initialize(handshake.clone())
            .to_jsonl()
            .test_unwrap();
        let first_render = ClientEvent::FirstRenderCompleted.to_jsonl().test_unwrap();
        let input = format!("bad\n{first_render}{initialize}");
        let mut reader = Cursor::new(input);
        let mut output = Vec::new();
        assert_eq!(
            read_initialize(&mut reader, &mut output).test_unwrap(),
            Some(handshake)
        );
        let frames = String::from_utf8(output).test_unwrap();
        assert_eq!(
            frames.lines().count(),
            2,
            "one error frame per rejected line"
        );
    }
}
