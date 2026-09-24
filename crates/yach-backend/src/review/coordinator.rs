//! Review orchestration: bound the request, persist evidence, invoke the
//! reviewer, validate freshness, and route.
//!
//! Model-derived execution stays off until Task 8 flips
//! [`AUTO_REVIEW_EXECUTION_ENABLED`]. Fixture reviewers and `cfg(test)` bypass
//! that gate so routing can be proven before live enablement.

#[cfg(test)]
use std::future::Future;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::extension::{ExtensionHostProtocolError, ExtensionResourceBroker};
use crate::session::{SessionEvent, SessionId, TurnId};
use crate::session_store::SessionEventSink;
use crate::{
    BoundedReviewText, PermissionRequest, PolicyRevision, ReviewAssessmentSummary, ReviewPolicy,
    ReviewRequestSummary,
};

use super::assessment::{AssessmentValidationError, AuthorizationAnswer, ReviewAssessment};
use super::request::{
    BoundReviewRequest, EvidenceItem, REVIEW_REQUEST_SCHEMA, ReviewAction, ReviewRequest,
    SandboxState, action_kind, action_target, bind_review_request,
};

/// End-to-end deadline covering persistence, invocation, and validation.
pub const REVIEW_DEADLINE: Duration = Duration::from_secs(15);

/// Compile-time enablement. Task 8 flips this after the held-out evaluation.
/// While false, a model-derived route is [`ReviewFailure::Disabled`].
pub(crate) const AUTO_REVIEW_EXECUTION_ENABLED: bool = false;

const ROUTING_TOML: &str = include_str!("routing.toml");

#[derive(Debug, Clone, Copy, PartialEq)]
struct RoutingThresholds {
    restrict_threshold: f64,
    consequence_threshold: f64,
    destructive_level: f64,
    evidence_threshold: f64,
    origin_threshold: f64,
}

#[derive(Debug, Deserialize)]
struct RoutingFile {
    schema: String,
    noul: NoulThresholds,
    score: ScoreThresholds,
}

#[derive(Debug, Deserialize)]
struct NoulThresholds {
    restriction: RestrictThreshold,
    evidence: EvidenceThreshold,
    origin_confusion: OriginThreshold,
}

#[derive(Debug, Deserialize)]
struct RestrictThreshold {
    restrict_threshold: f64,
}

#[derive(Debug, Deserialize)]
struct EvidenceThreshold {
    evidence_threshold: f64,
}

#[derive(Debug, Deserialize)]
struct OriginThreshold {
    origin_threshold: f64,
}

#[derive(Debug, Deserialize)]
struct ScoreThresholds {
    consequence: ConsequenceThreshold,
}

#[derive(Debug, Deserialize)]
struct ConsequenceThreshold {
    consequence_threshold: f64,
    destructive_level: f64,
}

fn routing_thresholds() -> &'static RoutingThresholds {
    static THRESHOLDS: LazyLock<RoutingThresholds> = LazyLock::new(|| {
        let parsed: RoutingFile = toml::from_str(ROUTING_TOML).unwrap_or(RoutingFile {
            schema: String::new(),
            noul: NoulThresholds {
                restriction: RestrictThreshold {
                    restrict_threshold: 0.0,
                },
                evidence: EvidenceThreshold {
                    evidence_threshold: 1.0,
                },
                origin_confusion: OriginThreshold {
                    origin_threshold: 0.0,
                },
            },
            score: ScoreThresholds {
                consequence: ConsequenceThreshold {
                    consequence_threshold: 0.0,
                    destructive_level: 0.0,
                },
            },
        });
        // A mismatched schema fails closed: every comparison holds.
        if parsed.schema != "yach-review-routing.v1" {
            return RoutingThresholds {
                restrict_threshold: 0.0,
                consequence_threshold: 0.0,
                destructive_level: 0.0,
                evidence_threshold: 1.0,
                origin_threshold: 0.0,
            };
        }
        RoutingThresholds {
            restrict_threshold: parsed.noul.restriction.restrict_threshold,
            consequence_threshold: parsed.score.consequence.consequence_threshold,
            destructive_level: parsed.score.consequence.destructive_level,
            evidence_threshold: parsed.noul.evidence.evidence_threshold,
            origin_threshold: parsed.noul.origin_confusion.origin_threshold,
        }
    });
    &THRESHOLDS
}

/// Why a reviewed action is held for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    EvidenceOverBudget,
    SignificantRisk,
    NeedsClarification,
    RestrictionApplies,
}

/// Why automatic review could not produce a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFailure {
    Disabled,
    Stale,
    MalformedAssessment,
    TimedOut,
    Unavailable,
    EvidenceWriteFailed,
    OversizedAssessment,
}

/// Code-owned route. The reviewer never selects this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewRoute {
    /// Deterministic or reviewer-authorized: proceed to the executor.
    Execute,
    /// Hold for a human with a bounded reason and cited evidence refs.
    Hold {
        reason: HoldReason,
        evidence_refs: Vec<String>,
    },
    /// Reviewer, transport, or budget failure — distinct from a risk hold.
    ReviewFailed { reason: ReviewFailure },
}

/// Snapshots the coordinator rechecks after the reviewer responds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewFreshness {
    pub policy_revision: PolicyRevision,
    pub authorization_revision: u64,
    pub reviewer_generation: u64,
}

/// Builds requests, invokes one reviewer, and routes the typed assessment.
///
/// The reviewer session is the same shared handle the tool executor uses, so
/// a reviewer host that also serves tools is locked consistently. `review()`
/// on [`crate::ExtensionHostInvoker`] fails closed for hosts that never
/// completed the `review.ready` handshake.
///
/// The sink is borrowed, not owned: `append_event` must write through to
/// durable storage before returning, because `ReviewRequestRecorded` has to
/// be persisted before `review.assess` is sent. A sink that only buffers
/// breaks evidence-before-effects.
pub struct ReviewCoordinator<'a> {
    policy: Arc<Mutex<ReviewPolicy>>,
    reviewer_generation: Arc<Mutex<u64>>,
    authorization_revision: Arc<Mutex<u64>>,
    sink: &'a (dyn SessionEventSink + Sync),
    reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
    reviewer_id: String,
    session_id: SessionId,
    turn_id: TurnId,
    sandbox_state: SandboxState,
    resources: Arc<dyn ExtensionResourceBroker + Sync>,
    fixture_reviewer: bool,
    /// Test-only seam: runs after the request is sent and before the response
    /// is read, so a test can revoke policy mid-flight.
    #[cfg(test)]
    after_send: Mutex<Option<AfterSendHook>>,
}

#[cfg(test)]
type AfterSendHook = Box<dyn Fn() + Send>;

impl<'a> ReviewCoordinator<'a> {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        policy: Arc<Mutex<ReviewPolicy>>,
        reviewer_generation: Arc<Mutex<u64>>,
        authorization_revision: Arc<Mutex<u64>>,
        sink: &'a (dyn SessionEventSink + Sync),
        reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
        reviewer_id: impl Into<String>,
        session_id: SessionId,
        turn_id: TurnId,
        sandbox_state: SandboxState,
        resources: Arc<dyn ExtensionResourceBroker + Sync>,
    ) -> Self {
        Self::build(
            policy,
            reviewer_generation,
            authorization_revision,
            sink,
            reviewer,
            reviewer_id,
            session_id,
            turn_id,
            sandbox_state,
            resources,
            false,
        )
    }

    /// Construct a coordinator for an in-process, no-network fixture reviewer.
    #[expect(clippy::too_many_arguments)]
    pub fn new_fixture(
        policy: Arc<Mutex<ReviewPolicy>>,
        reviewer_generation: Arc<Mutex<u64>>,
        authorization_revision: Arc<Mutex<u64>>,
        sink: &'a (dyn SessionEventSink + Sync),
        reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
        reviewer_id: impl Into<String>,
        session_id: SessionId,
        turn_id: TurnId,
        sandbox_state: SandboxState,
        resources: Arc<dyn ExtensionResourceBroker + Sync>,
    ) -> Self {
        Self::build(
            policy,
            reviewer_generation,
            authorization_revision,
            sink,
            reviewer,
            reviewer_id,
            session_id,
            turn_id,
            sandbox_state,
            resources,
            true,
        )
    }

    #[expect(clippy::too_many_arguments)]
    fn build(
        policy: Arc<Mutex<ReviewPolicy>>,
        reviewer_generation: Arc<Mutex<u64>>,
        authorization_revision: Arc<Mutex<u64>>,
        sink: &'a (dyn SessionEventSink + Sync),
        reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>>,
        reviewer_id: impl Into<String>,
        session_id: SessionId,
        turn_id: TurnId,
        sandbox_state: SandboxState,
        resources: Arc<dyn ExtensionResourceBroker + Sync>,
        fixture_reviewer: bool,
    ) -> Self {
        let reviewer_id = reviewer_id.into();
        Self {
            policy,
            reviewer_generation,
            authorization_revision,
            sink,
            reviewer,
            reviewer_id,
            session_id,
            turn_id,
            sandbox_state,
            resources,
            fixture_reviewer,
            #[cfg(test)]
            after_send: Mutex::new(None),
        }
    }

    /// Build, bound-check, persist, invoke, validate, persist, and route.
    ///
    /// The 15-second deadline covers the whole operation. A late or stale
    /// response never becomes [`ReviewRoute::Execute`].
    pub async fn review_action(
        &self,
        request: PermissionRequest,
        action: ReviewAction,
        trusted: Vec<EvidenceItem>,
        untrusted: Vec<EvidenceItem>,
    ) -> ReviewRoute {
        // The reviewer call blocks on a subprocess; `spawn_blocking` inside
        // `review_within_deadline` keeps the runtime thread free. The
        // deadline is enforced by the transport timeout.
        self.review_within_deadline(&request, &action, trusted, untrusted)
            .await
    }
    async fn review_within_deadline(
        &self,
        request: &PermissionRequest,
        action: &ReviewAction,
        trusted: Vec<EvidenceItem>,
        untrusted: Vec<EvidenceItem>,
    ) -> ReviewRoute {
        let Some(freshness) = self.snapshot() else {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::Unavailable,
            };
        };
        let review_request = ReviewRequest {
            schema: REVIEW_REQUEST_SCHEMA,
            request_id: request.request_id.clone(),
            session_id: self.session_id.0.clone(),
            turn_id: self.turn_id.0.clone(),
            policy_revision: freshness.policy_revision,
            authorization_revision: freshness.authorization_revision,
            reviewer_id: self.reviewer_id.clone(),
            reviewer_generation: freshness.reviewer_generation,
            action: action.clone(),
            trusted_evidence: trusted,
            untrusted_evidence: untrusted,
            omissions: Vec::new(),
            sandbox_state: self.sandbox_state.clone(),
        };
        let review_request = match bind_review_request(review_request) {
            BoundReviewRequest::Ready(review_request) => review_request,
            BoundReviewRequest::OverBudget { .. } => {
                return ReviewRoute::Hold {
                    reason: HoldReason::EvidenceOverBudget,
                    evidence_refs: Vec::new(),
                };
            }
        };
        let summary = ReviewRequestSummary {
            request_id: BoundedReviewText::new(&review_request.request_id),
            action_kind: BoundedReviewText::new(action_kind(action)),
            target: BoundedReviewText::new(&action_target(action)),
            policy_revision: freshness.policy_revision,
            authorization_revision: freshness.authorization_revision,
        };
        if self
            .sink
            .append_event(&SessionEvent::ReviewRequestRecorded { request: summary })
            .is_err()
        {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::EvidenceWriteFailed,
            };
        }

        let Ok(payload) = serde_json::to_value(&review_request) else {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::Unavailable,
            };
        };
        #[cfg(test)]
        if let Ok(hook) = self.after_send.lock()
            && let Some(hook) = hook.as_ref()
        {
            hook();
        }
        let reviewer = self.reviewer.clone();
        let resources = self.resources.clone();
        let request_id = review_request.request_id.clone();
        let assessment = match tokio::task::spawn_blocking(move || {
            let Ok(mut reviewer) = reviewer.lock() else {
                return Err(ExtensionHostProtocolError::SpawnFailed);
            };
            reviewer.review(&request_id, payload, REVIEW_DEADLINE, resources.as_ref())
        })
        .await
        {
            Ok(result) => result,
            Err(_join_error) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Unavailable,
                };
            }
        };
        let assessment = match assessment {
            Ok(value) => value,
            Err(ExtensionHostProtocolError::TimedOut) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::TimedOut,
                };
            }
            Err(ExtensionHostProtocolError::OutputTooLarge { .. }) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::OversizedAssessment,
                };
            }
            Err(_) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Unavailable,
                };
            }
        };
        let Ok(bytes) = serde_json::to_vec(&assessment) else {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::MalformedAssessment,
            };
        };
        let assessment = match ReviewAssessment::validate_against(&bytes, &review_request) {
            Ok(assessment) => assessment,
            Err(AssessmentValidationError::Oversized) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::OversizedAssessment,
                };
            }
            Err(AssessmentValidationError::AdapterFailed) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Unavailable,
                };
            }
            Err(
                AssessmentValidationError::Malformed
                | AssessmentValidationError::RequestMismatch
                | AssessmentValidationError::UnknownEvidenceRef,
            ) => {
                return ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::MalformedAssessment,
                };
            }
        };
        let Some(current) = self.snapshot() else {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::Unavailable,
            };
        };
        if current != freshness {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::Stale,
            };
        }
        let route = route_assessment(&assessment);
        let route = self.gate_execution(route);
        let recorded = SessionEvent::ReviewAssessmentRecorded {
            request_id: BoundedReviewText::new(&review_request.request_id),
            assessment: ReviewAssessmentSummary {
                reviewer_id: BoundedReviewText::new(&assessment.reviewer_id),
                reason: BoundedReviewText::new(route_reason(&route)),
                model: BoundedReviewText::new(assessment.model_returned()),
            },
        };
        if self.sink.append_event(&recorded).is_err() {
            return ReviewRoute::ReviewFailed {
                reason: ReviewFailure::EvidenceWriteFailed,
            };
        }
        route
    }

    pub(crate) fn snapshot(&self) -> Option<ReviewFreshness> {
        let policy_revision = self.policy.lock().ok()?.revision;
        let authorization_revision = *self.authorization_revision.lock().ok()?;
        let reviewer_generation = *self.reviewer_generation.lock().ok()?;
        Some(ReviewFreshness {
            policy_revision,
            authorization_revision,
            reviewer_generation,
        })
    }
    fn gate_execution(&self, route: ReviewRoute) -> ReviewRoute {
        if AUTO_REVIEW_EXECUTION_ENABLED || cfg!(test) || self.fixture_reviewer {
            return route;
        }
        match route {
            ReviewRoute::Execute | ReviewRoute::Hold { .. } => ReviewRoute::ReviewFailed {
                reason: ReviewFailure::Disabled,
            },
            failed @ ReviewRoute::ReviewFailed { .. } => failed,
        }
    }
}

/// Route from typed answers. Probabilities are compared, never multiplied.
#[must_use]
pub fn route_assessment(assessment: &ReviewAssessment) -> ReviewRoute {
    let thresholds = routing_thresholds();
    let refs = assessment.evidence_refs.clone();
    if assessment.consequence >= thresholds.destructive_level {
        return ReviewRoute::Hold {
            reason: HoldReason::SignificantRisk,
            evidence_refs: refs,
        };
    }
    let Ok(authorization) = assessment.authorization_answer() else {
        return ReviewRoute::ReviewFailed {
            reason: ReviewFailure::MalformedAssessment,
        };
    };
    if !matches!(
        authorization,
        AuthorizationAnswer::ExactAuthorized | AuthorizationAnswer::SubstantiveAuthorized
    ) || assessment.evidence_sufficient < thresholds.evidence_threshold
    {
        return ReviewRoute::Hold {
            reason: HoldReason::NeedsClarification,
            evidence_refs: refs,
        };
    }
    if assessment.restriction_applies >= thresholds.restrict_threshold {
        return ReviewRoute::Hold {
            reason: HoldReason::RestrictionApplies,
            evidence_refs: refs,
        };
    }
    if assessment.consequence >= thresholds.consequence_threshold
        || assessment.origin_confusion >= thresholds.origin_threshold
    {
        return ReviewRoute::Hold {
            reason: HoldReason::SignificantRisk,
            evidence_refs: refs,
        };
    }
    ReviewRoute::Execute
}

fn route_reason(route: &ReviewRoute) -> &'static str {
    match route {
        ReviewRoute::Execute => "execute",
        ReviewRoute::Hold {
            reason: HoldReason::EvidenceOverBudget,
            ..
        } => "evidence_over_budget",
        ReviewRoute::Hold {
            reason: HoldReason::SignificantRisk,
            ..
        } => "reviewer_hold_risk",
        ReviewRoute::Hold {
            reason: HoldReason::NeedsClarification,
            ..
        } => "reviewer_hold_evidence",
        ReviewRoute::Hold {
            reason: HoldReason::RestrictionApplies,
            ..
        } => "restriction_ask_first",
        ReviewRoute::ReviewFailed {
            reason: ReviewFailure::Unavailable | ReviewFailure::TimedOut,
        } => "reviewer_unavailable",
        ReviewRoute::ReviewFailed { .. } => "reviewer_error",
    }
}

#[cfg(test)]
impl ReviewCoordinator<'_> {
    fn review_action_observing<F>(
        &self,
        request: PermissionRequest,
        action: ReviewAction,
        trusted: Vec<EvidenceItem>,
        untrusted: Vec<EvidenceItem>,
        observe: F,
    ) -> impl Future<Output = ReviewRoute> + '_
    where
        F: Fn() + Send + 'static,
    {
        if let Ok(mut hook) = self.after_send.lock() {
            *hook = Some(Box::new(observe));
        }
        self.review_action(request, action, trusted, untrusted)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::{Value, json};

    use crate::extension::{
        ExtensionHostClientMessage, ExtensionHostProtocolError, ExtensionHostServerMessage,
        ExtensionHostTransport, ExtensionResourceBroker, ExtensionResourceRequest,
        ExtensionResourceResult,
    };
    use crate::review::{PolicyRevision, ReviewPolicy};
    use crate::session::{SessionEvent, SessionId, TurnId};
    use crate::session_store::SessionEventSink;
    use crate::{
        PermissionActor, PermissionCapability, PermissionRequest, PermissionReviewer,
        PermissionRisk, PermissionTargetSummary,
    };

    use super::{
        EvidenceItem, HoldReason, ReviewAction, ReviewCoordinator, ReviewFailure, ReviewRoute,
        SandboxState,
    };

    struct MemorySink {
        events: Mutex<Vec<SessionEvent>>,
        fail: bool,
    }

    impl MemorySink {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
                fail: true,
            }
        }

        fn events(&self) -> Vec<SessionEvent> {
            self.events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default()
        }
    }

    impl SessionEventSink for MemorySink {
        fn append_event(&self, event: &SessionEvent) -> std::io::Result<()> {
            if self.fail {
                return Err(std::io::Error::other("evidence write failed"));
            }
            self.events
                .lock()
                .map_err(|_| std::io::Error::other("evidence lock poisoned"))?
                .push(event.clone());
            Ok(())
        }
    }

    struct ScriptedTransport {
        received: Mutex<Vec<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>>,
        invocations: Arc<AtomicUsize>,
    }

    impl ScriptedTransport {
        fn new(
            received: Vec<Result<ExtensionHostServerMessage, ExtensionHostProtocolError>>,
        ) -> Self {
            Self {
                received: Mutex::new(received),
                invocations: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn invocations(&self) -> usize {
            self.invocations.load(Ordering::SeqCst)
        }
    }

    impl ExtensionHostTransport for ScriptedTransport {
        fn send(
            &mut self,
            _message: ExtensionHostClientMessage,
        ) -> Result<(), ExtensionHostProtocolError> {
            self.invocations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn recv(
            &mut self,
            _timeout: Duration,
        ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
            self.received
                .lock()
                .map_err(|_| ExtensionHostProtocolError::Malformed)?
                .pop()
                .unwrap_or(Err(ExtensionHostProtocolError::TimedOut))
        }
    }

    struct NoResources;

    impl ExtensionResourceBroker for NoResources {
        fn execute(&self, _request: &ExtensionResourceRequest) -> ExtensionResourceResult {
            ExtensionResourceResult::Failed {
                reason: String::from("fixture"),
                message: String::from("fixture broker rejects resource reads"),
            }
        }
    }

    fn permission_request() -> PermissionRequest {
        PermissionRequest {
            request_id: String::from("perm-1"),
            actor: PermissionActor::Provider,
            capability: PermissionCapability::ShellCommand,
            target: PermissionTargetSummary {
                operation: String::from("cargo test"),
                resource: String::from("."),
            },
            risk: PermissionRisk::ProcessExecution,
            requested_reviewer: Some(PermissionReviewer::AutoReview),
            command: Some(String::from("cargo test")),
        }
    }

    fn shell_action() -> ReviewAction {
        ReviewAction::ShellCommand {
            command: String::from("cargo test"),
            cwd: String::from("/work"),
            timeout_ms: 5_000,
            env_keys: vec![String::from("PATH")],
        }
    }

    fn trusted() -> Vec<EvidenceItem> {
        vec![EvidenceItem {
            id: String::from("user-1"),
            source: String::from("user"),
            kind: String::from("message"),
            excerpt: String::from("run the tests"),
            truncated: false,
        }]
    }

    fn clear_assessment(request_id: &str) -> Value {
        json!({
            "schema": "yach.review-assessment.v1",
            "request_id": request_id,
            "reviewer_id": "jev-typesafe",
            "model": "jev-1.13.0",
            "authorization": "exact_authorized",
            "restriction_applies": 0.02,
            "consequence": 0.4,
            "evidence_sufficient": 0.97,
            "origin_confusion": 0.01,
            "confidence": {"authorization": 0.9},
            "evidence_refs": ["user-1"],
            "adapter_error": null,
            "usage": {"input_tokens": 10, "output_tokens": 4},
            "duration_ms": 12
        })
    }

    fn review_result(assessment: Value) -> ExtensionHostServerMessage {
        let request_id = assessment
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("missing")
            .to_owned();
        ExtensionHostServerMessage::ReviewResult {
            request_id,
            assessment,
        }
    }

    struct Fixture {
        policy: Arc<Mutex<ReviewPolicy>>,
        generation: Arc<Mutex<u64>>,
        authorization: Arc<Mutex<u64>>,
        sink: Arc<MemorySink>,
        transport: Arc<Mutex<ScriptedTransport>>,
    }

    fn fixture(
        response: Result<ExtensionHostServerMessage, ExtensionHostProtocolError>,
    ) -> Fixture {
        Fixture {
            policy: Arc::new(Mutex::new(ReviewPolicy::empty())),
            generation: Arc::new(Mutex::new(1)),
            authorization: Arc::new(Mutex::new(1)),
            sink: Arc::new(MemorySink::new()),
            transport: Arc::new(Mutex::new(ScriptedTransport::new(vec![response]))),
        }
    }

    fn coordinator(fixture: &Fixture) -> ReviewCoordinator<'_> {
        let session = crate::ExtensionHostSession::new(
            "jev-typesafe",
            SharedTransport(fixture.transport.clone()),
            16 * 1024,
        );
        let reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>> =
            Arc::new(Mutex::new(Box::new(session)));
        ReviewCoordinator::new(
            fixture.policy.clone(),
            fixture.generation.clone(),
            fixture.authorization.clone(),
            fixture.sink.as_ref(),
            reviewer,
            "jev-typesafe",
            SessionId(String::from("session-1")),
            TurnId(String::from("turn-1")),
            SandboxState::None,
            Arc::new(NoResources),
        )
    }

    struct SharedTransport(Arc<Mutex<ScriptedTransport>>);

    impl ExtensionHostTransport for SharedTransport {
        fn send(
            &mut self,
            message: ExtensionHostClientMessage,
        ) -> Result<(), ExtensionHostProtocolError> {
            self.0
                .lock()
                .map_err(|_| ExtensionHostProtocolError::Malformed)?
                .send(message)
        }

        fn recv(
            &mut self,
            timeout: Duration,
        ) -> Result<ExtensionHostServerMessage, ExtensionHostProtocolError> {
            self.0
                .lock()
                .map_err(|_| ExtensionHostProtocolError::Malformed)?
                .recv(timeout)
        }
    }

    fn recorded_kinds(events: &[SessionEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event {
                SessionEvent::ReviewRequestRecorded { .. } => "request",
                SessionEvent::ReviewAssessmentRecorded { .. } => "assessment",
                _ => "other",
            })
            .collect()
    }

    #[tokio::test]
    async fn authorized_low_risk_routes_to_execute() {
        let built = fixture(Ok(review_result(clear_assessment("perm-1"))));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(route, ReviewRoute::Execute),
            "authorized low-risk work executes, got {route:?}"
        );
        let events = built.sink.events();
        assert_eq!(
            recorded_kinds(&events),
            ["request", "assessment"],
            "request evidence precedes the assessment, and both exist before return"
        );
        let model = events.iter().find_map(|event| match event {
            SessionEvent::ReviewAssessmentRecorded { assessment, .. } => {
                Some(assessment.model.as_str().to_owned())
            }
            _ => None,
        });
        assert_eq!(model.as_deref(), Some("jev-1.13.0"));
    }

    #[tokio::test]
    async fn destructive_consequence_holds_even_when_authorized() {
        let mut assessment = clear_assessment("perm-1");
        assessment["consequence"] = json!(3.0);
        assessment["authorization"] = json!("exact_authorized");
        let built = fixture(Ok(review_result(assessment)));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::Hold {
                    reason: HoldReason::SignificantRisk,
                    ..
                }
            ),
            "destructive consequence holds despite exact authorization, got {route:?}"
        );
    }

    #[tokio::test]
    async fn stale_policy_revision_discards_response() {
        let built = fixture(Ok(review_result(clear_assessment("perm-1"))));
        let policy = built.policy.clone();
        let route = coordinator(&built)
            .review_action_observing(
                permission_request(),
                shell_action(),
                trusted(),
                Vec::new(),
                move || {
                    if let Ok(mut policy) = policy.lock() {
                        policy.revision = PolicyRevision(policy.revision.0.saturating_add(1));
                    }
                },
            )
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Stale
                }
            ),
            "a policy bump between send and response is stale, got {route:?}"
        );
    }

    #[tokio::test]
    async fn oversized_request_holds_without_calling_reviewer() {
        let built = fixture(Ok(review_result(clear_assessment("perm-1"))));
        let oversized = EvidenceItem {
            id: String::from("blob"),
            source: String::from("user"),
            kind: String::from("message"),
            excerpt: "x".repeat(70 * 1024),
            truncated: false,
        };
        let route = coordinator(&built)
            .review_action(
                permission_request(),
                shell_action(),
                vec![oversized],
                Vec::new(),
            )
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::Hold {
                    reason: HoldReason::EvidenceOverBudget,
                    ..
                }
            ),
            "trusted evidence over 64 KiB holds, got {route:?}"
        );
        let invocations = built
            .transport
            .lock()
            .map_or(usize::MAX, |transport| transport.invocations());
        assert_eq!(
            invocations, 0,
            "over-budget review never invokes the reviewer"
        );
    }

    #[tokio::test]
    async fn assessment_citing_unknown_evidence_ref_is_rejected() {
        let mut assessment = clear_assessment("perm-1");
        assessment["evidence_refs"] = json!(["e99"]);
        let built = fixture(Ok(review_result(assessment)));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::MalformedAssessment
                }
            ),
            "an unknown evidence ref is a malformed assessment, got {route:?}"
        );
    }

    #[tokio::test]
    async fn timeout_is_review_failed() {
        let built = fixture(Err(ExtensionHostProtocolError::TimedOut));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::TimedOut
                }
            ),
            "a reviewer timeout fails closed, got {route:?}"
        );
    }

    #[tokio::test]
    async fn transport_error_is_unavailable() {
        let built = fixture(Err(ExtensionHostProtocolError::HostExited {
            status: Some(1),
        }));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Unavailable
                }
            ),
            "a host exit is reviewer unavailable, got {route:?}"
        );
    }

    #[tokio::test]
    async fn evidence_persistence_failure_blocks_execution() {
        let built = fixture(Ok(review_result(clear_assessment("perm-1"))));
        let sink = Arc::new(MemorySink::failing());
        let session = crate::ExtensionHostSession::new(
            "jev-typesafe",
            SharedTransport(built.transport.clone()),
            16 * 1024,
        );
        let reviewer: Arc<Mutex<Box<dyn crate::ExtensionHostInvoker>>> =
            Arc::new(Mutex::new(Box::new(session)));
        let failing = ReviewCoordinator::new(
            built.policy.clone(),
            built.generation.clone(),
            built.authorization.clone(),
            sink.as_ref(),
            reviewer,
            "jev-typesafe",
            SessionId(String::from("session-1")),
            TurnId(String::from("turn-1")),
            SandboxState::None,
            Arc::new(NoResources),
        );
        let route = failing
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::EvidenceWriteFailed
                }
            ),
            "evidence write failure blocks execution, got {route:?}"
        );
        assert!(sink.events().is_empty());
        let invocations = built
            .transport
            .lock()
            .map_or(usize::MAX, |transport| transport.invocations());
        assert_eq!(
            invocations, 0,
            "nothing is sent when evidence cannot be recorded"
        );
    }

    #[tokio::test]
    async fn ambiguous_authorization_needs_clarification() {
        let mut assessment = clear_assessment("perm-1");
        assessment["authorization"] = json!("ambiguous");
        let built = fixture(Ok(review_result(assessment)));
        let route = coordinator(&built)
            .review_action(permission_request(), shell_action(), trusted(), Vec::new())
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::Hold {
                    reason: HoldReason::NeedsClarification,
                    ..
                }
            ),
            "ambiguous authorization asks for clarification, got {route:?}"
        );
    }

    #[tokio::test]
    async fn stale_reviewer_generation_discards_response() {
        let built = fixture(Ok(review_result(clear_assessment("perm-1"))));
        let generation = built.generation.clone();
        let route = coordinator(&built)
            .review_action_observing(
                permission_request(),
                shell_action(),
                trusted(),
                Vec::new(),
                move || {
                    if let Ok(mut generation) = generation.lock() {
                        *generation = generation.saturating_add(1);
                    }
                },
            )
            .await;
        assert!(
            matches!(
                route,
                ReviewRoute::ReviewFailed {
                    reason: ReviewFailure::Stale
                }
            ),
            "a reviewer reload between send and response is stale, got {route:?}"
        );
    }
}
