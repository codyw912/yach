//! Protocol scenarios for reviewed provider tool calls over the RPC boundary.
//!
//! These deliberately use the default provider-connections backend rather than
//! the fixture backend. Local OpenAI-compatible servers drive a denied command
//! and an extension edit that recovers from a malformed patch, exercising
//! connection creation, model discovery/activation, tool review, and
//! continuation without credentials or network access.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

use yach_proto::{
    ClientEvent, DialogResponse, HarnessOutcomeKind, ServerEvent, SubmittedSecret,
    ToolReviewDecision, ToolReviewPayload, default_ui_handshake,
};

const PROVIDER_SECRET: &str = "rpc-review-test-secret";
const MODEL_ID: &str = "rpc-review-model";
const EVENT_TIMEOUT: Duration = Duration::from_secs(20);

#[test]
fn rpc_review_deny_bash_continues_and_finishes() {
    let project = TempDir::new("rpc-review-project");
    let home = TempDir::new("rpc-review-home");
    let provider = MockOpenAiProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path());

    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    // Prompts are accepted for the backend's own session id; take it from the
    // initial state instead of inventing one.
    let session_id = child.wait_for(|event| match event {
        ServerEvent::StateUpdated(state) => state.session_id,
        _ => None,
    });

    // Provider connections intentionally become available only after the same
    // first-render marker that the TUI sends.
    child.send(&ClientEvent::FirstRenderCompleted);
    child.send(&ClientEvent::ConnectionsRequested);
    child.resolve(
        "provider-connection:root",
        DialogResponse::Selection {
            value: String::from("add"),
        },
    );
    child.resolve(
        "provider-connection:provider",
        DialogResponse::Selection {
            value: String::from("openai-compatible"),
        },
    );
    child.resolve(
        "provider-connection:label",
        DialogResponse::Text {
            value: String::from("RPC review fixture"),
        },
    );
    child.resolve(
        "provider-connection:base-url",
        DialogResponse::Text {
            value: provider.base_url(),
        },
    );
    child.resolve(
        "provider-connection:secret:create",
        DialogResponse::Secret {
            value: SubmittedSecret::new(PROVIDER_SECRET),
        },
    );

    let model = child.wait_for(|event| match event {
        // Connection-backed models arrive on the discovery snapshot.
        ServerEvent::DiscoveredModelsUpdated { models }
            if models.iter().any(|model| model.id == MODEL_ID) =>
        {
            Some(
                models
                    .into_iter()
                    .find(|model| model.id == MODEL_ID)
                    .test_unwrap(),
            )
        }
        _ => None,
    });
    let connection_id = model.connection_id.test_unwrap();
    child.send(&ClientEvent::ModelSelectedDetailed {
        provider: model.provider,
        model_id: model.id,
        request_id: 1,
        connection_id: Some(connection_id),
    });
    child.wait_for(|event| match event {
        ServerEvent::ModelChanged(target) if target.model == MODEL_ID => Some(()),
        _ => None,
    });

    child.send(&ClientEvent::PromptSubmitted {
        session_id: session_id.clone(),
        prompt: String::from("Run the command and report what happened."),
    });

    let review = child.wait_for(|event| match event {
        ServerEvent::ToolReviewRequested {
            request_id,
            tool_name,
            payload: ToolReviewPayload::Command { command },
        } => Some((request_id, tool_name, command)),
        _ => None,
    });
    assert_eq!(review.1, "bash");
    assert_eq!(review.2.command, "echo hi");
    child.send(&ClientEvent::ToolReviewDecisionSubmitted {
        request_id: review.0,
        // The command review uses review_id in the preview_id wire slot. The
        // runner's wait predicate checks both identifiers, preserving the
        // same shape used by the TUI's review submission path.
        preview_id: review.2.review_id,
        permission_decision_id: review.2.permission_decision_id,
        decision: ToolReviewDecision::Reject,
    });

    let denied = child.wait_for(|event| match event {
        ServerEvent::ToolCallFinished(result)
            if result.tool_name == "bash"
                && result.outcome_kind == Some(HarnessOutcomeKind::Denied) =>
        {
            Some(result)
        }
        _ => None,
    });
    assert!(denied.is_error, "denying bash must produce an error result");

    let mut follow_up = String::new();
    child.wait_for(|event| match event {
        ServerEvent::PromptDelta { delta, .. } => {
            follow_up.push_str(&delta);
            None
        }
        ServerEvent::PromptFinished {
            outcome: yach_proto::PromptOutcome::Completed,
            ..
        } => Some(()),
        _ => None,
    });
    assert_eq!(
        provider.post_count(),
        2,
        "denial must trigger a continuation request"
    );
    assert!(
        provider.continuation_has_tool_result(),
        "continuation request must carry the denied tool result"
    );
    // The full wire text keeps the persisted "\n\n" round join: live round
    // narrative, separator, then the continuation after the denial.
    assert_eq!(
        child.streamed_text(),
        "Let me check.\n\nthe command was denied",
        "wire-concatenated deltas must match the persisted round join"
    );
    assert!(
        follow_up.contains("the command was denied"),
        "continuation text should be emitted after denial, got {follow_up:?}"
    );
    provider.join();
}

#[test]
fn rpc_hashline_extension_recovers_from_malformed_edit_then_applies_reviewed_edit() {
    let project = TempDir::new("rpc-hashline-project");
    fs::create_dir_all(project.path().join("src")).test_unwrap();
    fs::write(project.path().join("src/lib.rs"), "alpha\nbeta\n").test_unwrap();
    let home = TempDir::new("rpc-hashline-home");
    let provider = MockHashlineProvider::start();
    let mut child = RpcChild::spawn(project.path(), home.path());

    child.send(&ClientEvent::Initialize(default_ui_handshake()));
    child.wait_for(|event| match event {
        ServerEvent::Ready { handshake }
            if handshake.protocol_version == yach_proto::PROTOCOL_VERSION =>
        {
            Some(())
        }
        _ => None,
    });
    let session_id = child.wait_for(|event| match event {
        ServerEvent::StateUpdated(state) => state.session_id,
        _ => None,
    });

    child.send(&ClientEvent::FirstRenderCompleted);
    child.send(&ClientEvent::ConnectionsRequested);
    child.resolve(
        "provider-connection:root",
        DialogResponse::Selection {
            value: String::from("add"),
        },
    );
    child.resolve(
        "provider-connection:provider",
        DialogResponse::Selection {
            value: String::from("openai-compatible"),
        },
    );
    child.resolve(
        "provider-connection:label",
        DialogResponse::Text {
            value: String::from("RPC hashline fixture"),
        },
    );
    child.resolve(
        "provider-connection:base-url",
        DialogResponse::Text {
            value: provider.base_url(),
        },
    );
    child.resolve(
        "provider-connection:secret:create",
        DialogResponse::Secret {
            value: SubmittedSecret::new(PROVIDER_SECRET),
        },
    );

    let model = child.wait_for(|event| match event {
        ServerEvent::DiscoveredModelsUpdated { models }
            if models.iter().any(|model| model.id == MODEL_ID) =>
        {
            models.into_iter().find(|model| model.id == MODEL_ID)
        }
        _ => None,
    });
    let connection_id = model.connection_id.test_unwrap();
    child.send(&ClientEvent::ModelSelectedDetailed {
        provider: model.provider,
        model_id: model.id,
        request_id: 1,
        connection_id: Some(connection_id),
    });
    child.wait_for(|event| match event {
        ServerEvent::ModelChanged(target) if target.model == MODEL_ID => Some(()),
        _ => None,
    });

    child.send(&ClientEvent::ExtensionDiagnosticSnapshotRequested {
        request_id: String::from("hashline-diagnostics"),
        selector: Some(String::from("yach.hashline")),
    });
    child.wait_for(|event| match event {
        ServerEvent::ExtensionDiagnosticSnapshotUpdated {
            request_id,
            records,
            ..
        } if request_id == "hashline-diagnostics"
            && records
                .iter()
                .any(|record| record.activation_state == "active") =>
        {
            Some(())
        }
        _ => None,
    });

    child.send(&ClientEvent::PromptSubmitted {
        session_id,
        prompt: String::from("Update the second line."),
    });
    let malformed = child.wait_for(|event| match event {
        ServerEvent::ToolCallFinished(result)
            if result.tool_name == "edit_text_file"
                && result.outcome_kind == Some(HarnessOutcomeKind::Failed) =>
        {
            Some(result)
        }
        _ => None,
    });
    assert!(malformed.is_error);
    assert_eq!(
        malformed
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.reason.as_deref()),
        Some("malformed_patch")
    );
    assert!(malformed.output.contains("malformed hashline patch"));
    assert!(
        malformed
            .output
            .contains("every PUT body row must begin with '+'")
    );
    assert_eq!(
        fs::read_to_string(project.path().join("src/lib.rs")).test_unwrap(),
        "alpha\nbeta\n"
    );

    let review = child.wait_for(|event| match event {
        ServerEvent::ToolReviewRequested {
            request_id,
            tool_name,
            payload: ToolReviewPayload::LocalEdit { preview },
        } => Some(Ok((request_id, tool_name, preview))),
        ServerEvent::PromptFinished {
            outcome, message, ..
        } => Some(Err(format!("{outcome:?}: {message:?}"))),
        _ => None,
    });
    let review = review.unwrap_or_else(|failure| {
        unreachable!(
            "hashline turn failed before review: {failure}; posts={}",
            provider.post_count()
        )
    });
    assert_eq!(review.1, "edit_text_file");
    assert_eq!(review.2.path, "src/lib.rs");
    assert_eq!(review.2.operation, "extension_edit_proposal");
    assert!(review.2.diff_summary.contains("+gamma"));
    child.send(&ClientEvent::ToolReviewDecisionSubmitted {
        request_id: review.0,
        preview_id: review.2.preview_id,
        permission_decision_id: review.2.permission_decision_id,
        decision: ToolReviewDecision::Approve,
    });

    child.wait_for(|event| match event {
        ServerEvent::PromptFinished {
            outcome: yach_proto::PromptOutcome::Completed,
            ..
        } => Some(()),
        _ => None,
    });
    assert_eq!(
        fs::read_to_string(project.path().join("src/lib.rs")).test_unwrap(),
        "alpha\ngamma\n"
    );
    assert_eq!(provider.post_count(), 4);
    assert!(provider.advertised_replaced_contracts());
    assert!(provider.saw_hashline_read_result());
    assert!(provider.saw_malformed_edit_result());
    assert!(provider.saw_applied_edit_result());
    provider.join();
}

/// Minimal OpenAI-compatible server. The first chat completion emits one bash
/// call; the second (which includes yach's denied tool result) emits text.
struct MockOpenAiProvider {
    base_url: String,
    posts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    continuation_has_tool_result: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockOpenAiProvider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let posts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let continuation_has_tool_result =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_posts = std::sync::Arc::clone(&posts);
        let observed_continuation = std::sync::Arc::clone(&continuation_has_tool_result);
        let worker = thread::spawn(move || {
            // Model discovery can happen once while creating the connection and
            // again after the mutation. Keep serving until the two turn posts.
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let Ok(request) = read_http_request(&mut stream) else {
                    return;
                };
                let request_line = request.lines().next().unwrap_or_default();

                if request_line.starts_with("GET ") && request_line.contains("/models") {
                    write_http_response(
                        &mut stream,
                        "application/json",
                        r#"{"object":"list","data":[{"id":"rpc-review-model","object":"model","created":0,"owned_by":"rpc-fixture"}]}"#,
                    );
                    continue;
                }
                if request_line.starts_with("POST ") && request_line.contains("/chat/completions") {
                    let post = observed_posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    if post == 2 {
                        observed_continuation.store(
                            request.contains("user_rejected") || request.contains("denied"),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                    }
                    let body = if post == 1 {
                        first_tool_call_sse()
                    } else {
                        follow_up_sse()
                    };
                    write_http_response(&mut stream, "text/event-stream", &body);
                    if post >= 2 {
                        return;
                    }
                    continue;
                }
                return;
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            posts,
            continuation_has_tool_result,
            worker: Some(worker),
        }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn post_count(&self) -> usize {
        self.posts.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn continuation_has_tool_result(&self) -> bool {
        self.continuation_has_tool_result
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn join(mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join().test_unwrap();
        }
    }
}

fn first_tool_call_sse() -> String {
    concat!(
        "data: {\"id\":\"chatcmpl-review-1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"rpc-review-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Let me check.\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-review-1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"rpc-review-model\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-review-1\",\"type\":\"function\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"echo hi\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-review-1\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"rpc-review-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_owned()
}

fn follow_up_sse() -> String {
    concat!(
        "data: {\"id\":\"chatcmpl-review-2\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"rpc-review-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"the command was denied\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-review-2\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"rpc-review-model\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    )
    .to_owned()
}

struct MockHashlineProvider {
    base_url: String,
    posts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    advertised_replaced_contracts: std::sync::Arc<std::sync::atomic::AtomicBool>,
    saw_hashline_read_result: std::sync::Arc<std::sync::atomic::AtomicBool>,
    saw_applied_edit_result: std::sync::Arc<std::sync::atomic::AtomicBool>,
    saw_malformed_edit_result: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl MockHashlineProvider {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").test_unwrap();
        let address = listener.local_addr().test_unwrap();
        let posts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let advertised_replaced_contracts =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_hashline_read_result =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_applied_edit_result =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let saw_malformed_edit_result =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_posts = std::sync::Arc::clone(&posts);
        let observed_contracts = std::sync::Arc::clone(&advertised_replaced_contracts);
        let observed_read = std::sync::Arc::clone(&saw_hashline_read_result);
        let observed_edit = std::sync::Arc::clone(&saw_applied_edit_result);
        let observed_malformed = std::sync::Arc::clone(&saw_malformed_edit_result);
        let worker = thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let Ok(request) = read_http_request(&mut stream) else {
                    return;
                };
                let request_line = request.lines().next().unwrap_or_default();
                if request_line.starts_with("GET ") && request_line.contains("/models") {
                    write_http_response(
                        &mut stream,
                        "application/json",
                        r#"{"object":"list","data":[{"id":"rpc-review-model","object":"model","created":0,"owned_by":"rpc-fixture"}]}"#,
                    );
                    continue;
                }
                if !request_line.starts_with("POST ") || !request_line.contains("/chat/completions")
                {
                    return;
                }
                let post = observed_posts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let body = match post {
                    1 => {
                        observed_contracts.store(
                            advertises_hashline_replacement_contracts(&request),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        hashline_tool_call_sse(
                            "chatcmpl-hashline-1",
                            "read_text_file",
                            &serde_json::json!({"path":"src/lib.rs"}),
                            "Reading.",
                        )
                    }
                    2 => {
                        let header = hashline_snapshot_header(&request);
                        observed_read.store(
                            header.is_some() && request.contains("1:alpha"),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        let input = format!(
                            "{}\nPUT 2.=2:\ngamma",
                            header.unwrap_or_else(|| {
                                String::from("[src/lib.rs#0000000000000000]")
                            })
                        );
                        hashline_tool_call_sse(
                            "chatcmpl-hashline-2",
                            "edit_text_file",
                            &serde_json::json!({"input":input}),
                            "Editing.",
                        )
                    }
                    3 => {
                        let header = hashline_snapshot_header(&request);
                        observed_malformed.store(
                            request.contains("malformed hashline patch")
                                && request.contains("every PUT body row must begin with '+'"),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        let input = format!(
                            "{}\nPUT 2.=2:\n+gamma",
                            header.unwrap_or_else(|| {
                                String::from("[src/lib.rs#0000000000000000]")
                            })
                        );
                        hashline_tool_call_sse(
                            "chatcmpl-hashline-3",
                            "edit_text_file",
                            &serde_json::json!({"input":input}),
                            "Correcting.",
                        )
                    }
                    _ => {
                        observed_edit.store(
                            request.contains("[applied]"),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        hashline_final_sse()
                    }
                };
                write_http_response(&mut stream, "text/event-stream", &body);
                if post >= 4 {
                    return;
                }
            }
        });
        Self {
            base_url: format!("http://{address}/v1"),
            posts,
            advertised_replaced_contracts,
            saw_hashline_read_result,
            saw_applied_edit_result,
            saw_malformed_edit_result,
            worker: Some(worker),
        }
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn post_count(&self) -> usize {
        self.posts.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn advertised_replaced_contracts(&self) -> bool {
        self.advertised_replaced_contracts
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn saw_hashline_read_result(&self) -> bool {
        self.saw_hashline_read_result
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    fn saw_malformed_edit_result(&self) -> bool {
        self.saw_malformed_edit_result
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn saw_applied_edit_result(&self) -> bool {
        self.saw_applied_edit_result
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    fn join(mut self) {
        if let Some(worker) = self.worker.take() {
            worker.join().test_unwrap();
        }
    }
}

fn hashline_snapshot_header(request: &str) -> Option<String> {
    let marker = "[src/lib.rs#";
    request.find(marker).and_then(|start| {
        request[start..]
            .find(']')
            .map(|end| request[start..=start + end].to_owned())
    })
}

fn advertises_hashline_replacement_contracts(request: &str) -> bool {
    let Some((_, body)) = request.split_once("\r\n\r\n") else {
        return false;
    };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };
    let Some(tools) = body.get("tools").and_then(serde_json::Value::as_array) else {
        return false;
    };
    let function = |name: &str| {
        tools
            .iter()
            .filter_map(|tool| tool.get("function"))
            .find(|function| function.get("name").and_then(serde_json::Value::as_str) == Some(name))
    };
    let Some(read) = function("read_text_file") else {
        return false;
    };
    let Some(edit) = function("edit_text_file") else {
        return false;
    };

    read["parameters"]["required"] == serde_json::json!(["path"])
        && edit["parameters"]["required"] == serde_json::json!(["input"])
        && edit["parameters"]["properties"]
            .as_object()
            .is_some_and(|properties| properties.len() == 1 && properties.contains_key("input"))
        && function("hashline_read").is_none()
        && function("hashline_edit").is_none()
}

fn hashline_tool_call_sse(
    completion_id: &str,
    tool_name: &str,
    arguments: &serde_json::Value,
    text: &str,
) -> String {
    let content = serde_json::json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{
            "index": 0,
            "delta": {"role":"assistant","content":text},
            "finish_reason": null
        }]
    });
    let tool_call = serde_json::json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": format!("call-{completion_id}"),
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "arguments": arguments.to_string()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finished = serde_json::json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{"index":0,"delta":{},"finish_reason":"tool_calls"}]
    });
    format!("data: {content}\n\ndata: {tool_call}\n\ndata: {finished}\n\ndata: [DONE]\n\n")
}

fn hashline_final_sse() -> String {
    let content = serde_json::json!({
        "id": "chatcmpl-hashline-4",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{
            "index": 0,
            "delta": {"role":"assistant","content":"updated"},
            "finish_reason": null
        }]
    });
    let finished = serde_json::json!({
        "id": "chatcmpl-hashline-4",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": MODEL_ID,
        "choices": [{"index":0,"delta":{},"finish_reason":"stop"}]
    });
    format!("data: {content}\n\ndata: {finished}\n\ndata: [DONE]\n\n")
}

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<String> {
    stream.set_read_timeout(Some(EVENT_TIMEOUT))?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end;
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "request ended",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = end + 4;
            break;
        }
    }
    let header = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_http_response(stream: &mut TcpStream, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).test_unwrap();
    stream.flush().test_unwrap();
}

struct RpcChild {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<String>,
    pending: VecDeque<ServerEvent>,
    transcript: Vec<String>,
}

impl RpcChild {
    fn spawn(project_root: &Path, home: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_yach"));
        command
            .args(["rpc", "--project-root"])
            .arg(project_root)
            // Deterministic test child: no background models.dev fetch.
            .arg("--no-catalog-refresh");
        for (key, _) in std::env::vars_os() {
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

        let mut child = command
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .test_unwrap();
        let stdin = child.stdin.take().test_unwrap();
        let stdout = child.stdout.take().test_unwrap();
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            events,
            pending: VecDeque::new(),
            transcript: Vec::new(),
        }
    }

    fn send(&mut self, event: &ClientEvent) {
        let line = event.to_jsonl().test_unwrap();
        self.stdin.write_all(line.as_bytes()).test_unwrap();
        self.stdin.flush().test_unwrap();
    }

    /// The reducer only accepts responses for the dialog it has issued, so a
    /// resolve must first observe the matching `DialogRequested` frame.
    fn resolve(&mut self, dialog_id: &str, response: DialogResponse) {
        let owned_id = String::from(dialog_id);
        self.wait_for(move |event| match event {
            ServerEvent::DialogRequested(request) if request.id.as_deref() == Some(&owned_id) => {
                Some(())
            }
            _ => None,
        });
        self.send(&ClientEvent::DialogResolved {
            dialog_id: String::from(dialog_id),
            response,
        });
    }

    /// Every `PromptDelta` read so far, concatenated in arrival order — the
    /// text a wire consumer reconstructs for the turn.
    fn streamed_text(&self) -> String {
        self.transcript
            .iter()
            .filter_map(|line| ServerEvent::from_jsonl(line).ok())
            .filter_map(|event| match event {
                ServerEvent::PromptDelta { delta, .. } => Some(delta),
                _ => None,
            })
            .collect()
    }

    fn wait_for<T>(&mut self, mut match_event: impl FnMut(ServerEvent) -> Option<T>) -> T {
        // Scan frames earlier waits read past first: a wait removes only the
        // frame it matched, so out-of-order expectations still succeed.
        let mut index = 0;
        while index < self.pending.len() {
            if let Some(value) = match_event(self.pending[index].clone()) {
                self.pending.remove(index);
                return value;
            }
            index += 1;
        }
        let deadline = Instant::now() + EVENT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "RPC server event predicate timed out\nrpc transcript:\n{}",
                self.transcript.join("\n")
            );
            let line = match self.events.recv_timeout(remaining) {
                Ok(line) => line,
                Err(error) => unreachable!(
                    "RPC server event predicate timeout: {error}\nrpc transcript:\n{}",
                    self.transcript.join("\n")
                ),
            };
            self.transcript.push(line.clone());
            let event = ServerEvent::from_jsonl(&line).unwrap_or_else(|error| {
                unreachable!("RPC stdout was not a ServerEvent JSONL frame: {error}: {line}")
            });
            if let Some(value) = match_event(event.clone()) {
                return value;
            }
            self.pending.push_back(event);
        }
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .test_unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("yach-{label}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&path).test_unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
