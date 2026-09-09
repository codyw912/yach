use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use yach_connections::ProviderSecret;
use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

use crate::provider::{
    ProviderError, ProviderErrorKind, ProviderModel, ProviderRequest, ProviderStreamEvent,
    ProviderToolCall,
};
use crate::runner::{native_ready_handshake, ProviderRequester, RunnerConfig};
use crate::session::TurnId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script(pub Vec<Vec<ProviderStreamEvent>>);

fn model() -> ProviderModel {
    ProviderModel {
        provider: String::from("scripted"),
        model: String::from("scripted-model"),
    }
}

fn turn() -> TurnId {
    TurnId(String::from("turn-1"))
}

impl Script {
    #[must_use]
    pub fn text_only(reply: &str) -> Self {
        Self(vec![vec![
            ProviderStreamEvent::Started {
                turn_id: turn(),
                model: model(),
            },
            ProviderStreamEvent::TextDelta {
                turn_id: turn(),
                delta: reply.to_owned(),
            },
            ProviderStreamEvent::Completed {
                turn_id: turn(),
                finish_reason: None,
                usage: None,
                provider_response_id: None,
            },
        ]])
    }

    #[must_use]
    pub fn read_tool_calls(paths: &[&str], final_reply: &str) -> Self {
        let mut first = vec![ProviderStreamEvent::Started {
            turn_id: turn(),
            model: model(),
        }];
        for (index, path) in paths.iter().enumerate() {
            first.push(ProviderStreamEvent::ToolCallCompleted {
                turn_id: turn(),
                tool_call: ProviderToolCall {
                    call_id: format!("call-{}", index + 1),
                    name: String::from("read_text_file"),
                    arguments_json: serde_json::json!({ "path": path }),
                },
            });
        }
        first.push(ProviderStreamEvent::Completed {
            turn_id: turn(),
            finish_reason: None,
            usage: None,
            provider_response_id: None,
        });
        let Self(mut rounds) = Self::text_only(final_reply);
        rounds.insert(0, first);
        Self(rounds)
    }
}

#[derive(Clone)]
pub struct ScriptedProvider {
    responses: Arc<Mutex<VecDeque<Vec<ProviderStreamEvent>>>>,
    requests: Arc<AtomicUsize>,
    pub(crate) trace: Option<yach_trace::TraceSink>,
}

impl ScriptedProvider {
    #[must_use]
    pub fn new(script: Script) -> Self {
        Self {
            responses: Arc::new(Mutex::new(script.0.into())),
            requests: Arc::new(AtomicUsize::new(0)),
            trace: None,
        }
    }

    #[must_use]
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

impl ProviderRequester for ScriptedProvider {
    fn request(
        &mut self,
        request: ProviderRequest,
    ) -> BoxFuture<'_, Result<Vec<ProviderStreamEvent>, ProviderError>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let turn_id = request.turn_id.0.clone();
        if let Some(trace) = &self.trace {
            trace.mark(
                yach_trace::TraceScope::Turn(&turn_id),
                "provider_first_event",
            );
        }
        let response = match self.responses.lock() {
            Ok(mut queue) => queue.pop_front().ok_or_else(|| ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                message: String::from("scripted provider exhausted"),
                redacted_debug: None,
                metadata: crate::ProviderErrorMetadata::default(),
            }),
            Err(_) => Err(ProviderError {
                kind: ProviderErrorKind::InvalidRequest,
                message: String::from("scripted provider lock poisoned"),
                redacted_debug: None,
                metadata: crate::ProviderErrorMetadata::default(),
            }),
        };
        if let Some(trace) = &self.trace {
            trace.mark(yach_trace::TraceScope::Turn(&turn_id), "provider_stream_end");
        }
        Box::pin(async move { response })
    }
}

pub struct ScriptedTurnConfig {
    pub project_root: PathBuf,
    pub session_path: PathBuf,
    pub script: Script,
    pub prompt: String,
    pub trace: Option<yach_trace::TraceSink>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedTurnProfile {
    pub wall: Duration,
    pub requests: usize,
    pub events_appended: usize,
}

pub fn run_scripted_turn(config: ScriptedTurnConfig) -> Result<ScriptedTurnProfile, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let mut provider = ScriptedProvider::new(config.script);
        let requests = Arc::clone(&provider.requests);
        provider.trace = config.trace.clone();
        let start = Instant::now();
        let handle = tokio::spawn(crate::runner::run_native_loop_with_provider_requester(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: config.session_path.clone(),
                project_root: Some(config.project_root),
                provider: Some(scripted_provider_config()),
                startup_model_override: None,
                provider_setup_error: None,
                extension_package_roots: Vec::new(),
                extension_package_root_loader: None,
                trace: config.trace,
                catalog_refresh: None,
                model_discovery: None,
                provider_connections: None,
            },
            provider,
        ));
        client_tx
            .send(ClientEvent::Initialize(native_ready_handshake(true)))
            .map_err(|_| String::from("runner closed before initialize"))?;
        client_tx
            .send(ClientEvent::PromptSubmitted {
                session_id: String::from("default"),
                prompt: config.prompt,
            })
            .map_err(|_| String::from("runner closed before prompt"))?;
        loop {
            match backend_rx.recv().await {
                Some(BackendEvent::Server(ServerEvent::PromptFinished { .. })) => break,
                Some(_) => {}
                None => return Err(String::from("runner exited before prompt finished")),
            }
        }
        let wall = start.elapsed();
        drop(client_tx);
        handle.await.map_err(|error| error.to_string())?;
        let contents = std::fs::read_to_string(&config.session_path)
            .map_err(|error| error.to_string())?;
        Ok(ScriptedTurnProfile {
            wall,
            requests: requests.load(Ordering::SeqCst),
            events_appended: contents.lines().count(),
        })
    })
}

pub(crate) fn scripted_provider_config() -> crate::ProviderConfig {
    // Mirrors the test fixture at runner.rs `provider_test_config`; the
    // adapter is never contacted because the requester is scripted.
    crate::ProviderConfig {
        adapter: Arc::new(crate::rig_adapter::RigProviderAdapterConfig {
            provider: crate::rig_adapter::RigProviderConfig::Anthropic {
                api_key: ProviderSecret::new(String::from("scripted")),
                base_url: None,
            },
            timeout: Duration::from_secs(30),
            max_tokens: 1000,
            context_window: 200_000,
            max_tokens_param: crate::rig_adapter::MaxTokensParam::default(),
            error_dialect: crate::DialectSelection::Missing,
        }),
        model: String::from("scripted-model"),
        connection_id: None,
        connection_key: None,
        connection_display: None,
        test_delay_ms: None,
        catalog_models: Vec::new().into(),
        responses_compact: Some(true),
    }
}

#[cfg(test)]
mod tests {
    use super::{run_scripted_turn, Script, ScriptedTurnConfig};
    use crate::runner::run_native_loop_with_scripted_provider;
    use tokio::sync::mpsc;
    use yach_proto::{BackendEvent, ClientEvent, ServerEvent};

    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root(name: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "yach-bench-loop-{name}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(root.join("src"));
        let _ = std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n");
        root
    }

    #[test]
    fn text_only_turn_appends_user_assistant_and_finished() {
        let root = temp_root("text");
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: root.clone(),
            session_path: root.join("session.jsonl"),
            script: Script::text_only("ok"),
            prompt: String::from("hello"),
            trace: None,
        });
        let contents = std::fs::read_to_string(root.join("session.jsonl"));
        let _ = std::fs::remove_dir_all(&root);
        assert!(profile.is_ok(), "scripted text turn failed: {profile:?}");
        let Ok(profile) = profile else {
            return;
        };
        assert!(contents.is_ok(), "read session log: {contents:?}");
        let Ok(contents) = contents else {
            return;
        };
        assert_eq!(profile.requests, 1);
        assert!(contents.contains("\"assistant\""));
        assert!(contents.contains("turn_finished"));
    }

    #[test]
    fn four_read_calls_produce_four_tool_results_and_two_requests() {
        let root = temp_root("tools");
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: root.clone(),
            session_path: root.join("session.jsonl"),
            script: Script::read_tool_calls(&["src/lib.rs"; 4], "done"),
            prompt: String::from("read it"),
            trace: None,
        });
        let contents = std::fs::read_to_string(root.join("session.jsonl"));
        let _ = std::fs::remove_dir_all(&root);
        assert!(profile.is_ok(), "scripted tool turn failed: {profile:?}");
        let Ok(profile) = profile else {
            return;
        };
        assert!(contents.is_ok(), "read session log: {contents:?}");
        let Ok(contents) = contents else {
            return;
        };
        assert_eq!(profile.requests, 2);
        assert_eq!(contents.matches("tool_execution_finished").count(), 4);
    }

    #[test]
    fn unused_script_rounds_are_not_counted() {
        let root = temp_root("unused");
        let Script(mut rounds) = Script::text_only("ok");
        let Script(extra) = Script::text_only("unused");
        rounds.extend(extra);
        let profile = run_scripted_turn(ScriptedTurnConfig {
            project_root: root.clone(),
            session_path: root.join("session.jsonl"),
            script: Script(rounds),
            prompt: String::from("hello"),
            trace: None,
        });
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            profile.is_ok(),
            "scripted unused-round turn failed: {profile:?}"
        );
        let Ok(profile) = profile else {
            return;
        };
        assert_eq!(profile.requests, 1);
    }

    #[test]
    fn script_round_trips_through_json() {
        let script = Script::read_tool_calls(&["a.rs"], "x");
        let json = serde_json::to_string(&script);
        assert!(json.is_ok(), "serialize script: {json:?}");
        let Ok(json) = json else {
            return;
        };
        let back: Result<Script, _> = serde_json::from_str(&json);
        assert!(back.is_ok(), "deserialize script: {back:?}");
        let Ok(back) = back else {
            return;
        };
        assert_eq!(back, script);
    }

    #[test]
    fn two_prompts_consume_script_in_order() {
        let root = temp_root("two-prompts");
        let session_path = root.join("session.jsonl");
        let Script(mut rounds) = Script::text_only("one");
        let Script(second) = Script::text_only("two");
        rounds.extend(second);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok(), "tokio runtime: {runtime:?}");
        let Ok(runtime) = runtime else {
            let _ = std::fs::remove_dir_all(&root);
            return;
        };
        let result = runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(run_native_loop_with_scripted_provider(
                client_rx,
                backend_tx,
                crate::runner::RunnerConfig {
                    session_path: session_path.clone(),
                    project_root: Some(root.clone()),
                    provider: Some(super::scripted_provider_config()),
                    startup_model_override: None,
                    provider_setup_error: None,
                    extension_package_roots: Vec::new(),
                    extension_package_root_loader: None,
                    trace: None,
                    catalog_refresh: None,
                    model_discovery: None,
                    provider_connections: None,
                },
                Script(rounds),
            ));
            client_tx
                .send(ClientEvent::Initialize(super::native_ready_handshake(true)))
                .map_err(|_| String::from("runner closed before initialize"))?;
            let mut finished = 0;
            for prompt in ["first", "second"] {
                client_tx
                    .send(ClientEvent::PromptSubmitted {
                        session_id: String::from("default"),
                        prompt: String::from(prompt),
                    })
                    .map_err(|_| format!("runner closed before prompt {prompt}"))?;
                loop {
                    match backend_rx.recv().await {
                        Some(BackendEvent::Server(ServerEvent::PromptFinished { .. })) => {
                            finished += 1;
                            break;
                        }
                        Some(_) => {}
                        None => {
                            return Err(String::from("runner exited before prompt finished"));
                        }
                    }
                }
            }
            drop(client_tx);
            handle
                .await
                .map_err(|error| error.to_string())?;
            Ok(finished)
        });
        let contents = std::fs::read_to_string(&session_path);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            result.is_ok(),
            "two scripted prompts failed: {result:?}"
        );
        let Ok(finished) = result else {
            return;
        };
        assert_eq!(finished, 2);
        assert!(contents.is_ok(), "read session log: {contents:?}");
        let Ok(contents) = contents else {
            return;
        };
        assert!(contents.contains("one"), "first reply missing: {contents}");
        assert!(contents.contains("two"), "second reply missing: {contents}");
        let requests = contents.matches("turn_finished").count();
        assert_eq!(requests, 2);
    }
}
