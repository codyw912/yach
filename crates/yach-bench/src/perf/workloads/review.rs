use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use yach_backend::{
    DenyExtensionResources, EvidenceItem, ExtensionHostInvocation, ExtensionHostInvoker,
    ExtensionHostProtocolError, ExtensionResourceBroker, PermissionActor, PermissionCapability,
    PermissionRequest, PermissionReviewer, PermissionRisk, PermissionTargetSummary, PolicyRevision,
    ReviewAction, ReviewCoordinator, ReviewPolicy, SandboxState, SessionEvent, SessionEventSink,
    SessionId, TurnId,
};

use crate::perf::registry::{Measured, RunCtx, Workload};
use crate::perf::schema::{Class, Isolation};

struct NoopSink;

impl SessionEventSink for NoopSink {
    fn append_event(&self, _event: &SessionEvent) -> std::io::Result<()> {
        Ok(())
    }
}

struct FixtureReviewer;

impl ExtensionHostInvoker for FixtureReviewer {
    fn invoke(
        &mut self,
        _request_id: &str,
        _tool_name: &str,
        _arguments: Value,
        _timeout: Duration,
        _resources: &dyn ExtensionResourceBroker,
    ) -> Result<ExtensionHostInvocation, ExtensionHostProtocolError> {
        Err(ExtensionHostProtocolError::UnsupportedProtocol)
    }

    fn review(
        &mut self,
        request_id: &str,
        request: Value,
        _timeout: Duration,
        _resources: &dyn ExtensionResourceBroker,
    ) -> Result<Value, ExtensionHostProtocolError> {
        let evidence_refs = request
            .get("trusted_evidence")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("id").and_then(Value::as_str))
            .take(1)
            .collect::<Vec<_>>();
        Ok(json!({
            "schema": "yach.review-assessment.v1",
            "request_id": request_id,
            "reviewer_id": "fixture",
            "model": "fixture-v1",
            "authorization": "exact_authorized",
            "restriction_applies": 0.0,
            "consequence": 0.1,
            "evidence_sufficient": 1.0,
            "origin_confusion": 0.0,
            "confidence": {"authorization": 1.0},
            "evidence_refs": evidence_refs,
            "adapter_error": null,
            "usage": {"input_tokens": 0, "output_tokens": 0},
            "duration_ms": 0
        }))
    }
}

fn run_review_route(ctx: &RunCtx, with_reviewer: bool) -> Result<Measured, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())?;
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        let generation = Arc::new(Mutex::new(1_u64));
        let reviewer: Arc<Mutex<Box<dyn ExtensionHostInvoker>>> =
            Arc::new(Mutex::new(Box::new(FixtureReviewer)));
        let sink = NoopSink;
        let coordinator = ReviewCoordinator::new_fixture(
            Arc::new(Mutex::new(ReviewPolicy::empty())),
            generation,
            Arc::new(Mutex::new(1)),
            &sink,
            reviewer,
            "fixture",
            SessionId(String::from("perf")),
            TurnId(String::from("turn-1")),
            SandboxState::None,
            Arc::new(DenyExtensionResources),
        );
        let request = PermissionRequest {
            request_id: String::from("perf-request"),
            actor: PermissionActor::Provider,
            capability: PermissionCapability::ShellCommand,
            target: PermissionTargetSummary {
                operation: String::from("perf"),
                resource: String::from("perf"),
            },
            risk: PermissionRisk::ProcessExecution,
            requested_reviewer: Some(PermissionReviewer::AutoReview),
            command: Some(String::from("true")),
        };
        let action = ReviewAction::ShellCommand {
            command: String::from("true"),
            cwd: String::from("/tmp"),
            timeout_ms: 5000,
            env_keys: vec![String::from("PATH")],
        };
        let trusted = vec![EvidenceItem {
            id: String::from("cmd"),
            source: String::from("permission_request"),
            kind: String::from("shell_command"),
            excerpt: String::from("true"),
            truncated: false,
        }];
        let start = std::time::Instant::now();
        let route = if with_reviewer {
            runtime.block_on(coordinator.review_action(request, action, trusted, Vec::new()))
        } else {
            // Deterministic path: build the request, bind it, validate a
            // canned assessment, and route it — the full code-owned path
            // without the reviewer subprocess.
            let review_request = yach_backend::ReviewRequest {
                schema: yach_backend::REVIEW_REQUEST_SCHEMA,
                request_id: request.request_id.clone(),
                session_id: String::from("perf"),
                turn_id: String::from("turn-1"),
                policy_revision: PolicyRevision(1),
                authorization_revision: 1,
                reviewer_id: String::from("fixture"),
                reviewer_generation: 1,
                action: action.clone(),
                trusted_evidence: trusted.clone(),
                untrusted_evidence: Vec::new(),
                omissions: Vec::new(),
                sandbox_state: SandboxState::None,
            };
            let bound = yach_backend::bind_review_request(review_request);
            let assessment_bytes = serde_json::to_vec(&json!({
                "schema": "yach.review-assessment.v1",
                "request_id": "perf-request",
                "reviewer_id": "fixture",
                "model": "fixture-v1",
                "authorization": "exact_authorized",
                "restriction_applies": 0.0,
                "consequence": 0.1,
                "evidence_sufficient": 1.0,
                "origin_confusion": 0.0,
                "confidence": {"authorization": 1.0},
                "evidence_refs": ["cmd"],
                "adapter_error": null,
                "usage": {"input_tokens": 0, "output_tokens": 0},
                "duration_ms": 0
            }))
            .map_err(|error| error.to_string())?;
            let assessment = match &bound {
                yach_backend::BoundReviewRequest::Ready(request) => {
                    yach_backend::ReviewAssessment::validate_against(
                        &assessment_bytes,
                        request.as_ref(),
                    )
                    .map_err(|error| format!("{error:?}"))?
                }
                yach_backend::BoundReviewRequest::OverBudget { .. } => {
                    return Err(String::from("request over budget"));
                }
            };
            yach_backend::route_assessment(&assessment)
        };
        let _ = route;
        samples.push(start.elapsed());
    }
    Ok(Measured::Latency {
        samples,
        alloc: None,
    })
}

pub static REVIEW: [Workload; 2] = [
    Workload {
        id: "review/route/deterministic",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| run_review_route(ctx, false),
        emit_alloc: false,
    },
    Workload {
        id: "review/route/fixture_assess",
        class: Class::Latency,
        isolation: Isolation::InProcessSerial,
        requires: &[],
        bin: None,
        run: |ctx| run_review_route(ctx, true),
        emit_alloc: false,
    },
];
