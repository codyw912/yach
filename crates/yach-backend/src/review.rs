//! Intent-aware review: durable user restrictions and bounded decision records.
//!
//! Design: `docs/project/specs/2026-09-21-intent-aware-auto-review-design.md`.

mod assessment;
mod coordinator;
mod policy;
mod request;
mod signals;

pub use assessment::ReviewAssessment;
pub use coordinator::{
    HoldReason, ReviewCoordinator, ReviewFailure, ReviewFreshness, ReviewRoute,
    bump_authorization_revision, route_assessment,
};
pub use request::{
    BoundReviewRequest, EvidenceItem, OmissionMarker, REVIEW_REQUEST_SCHEMA, ReviewAction,
    ReviewEditOperation, ReviewRequest, SandboxState, USER_MESSAGE_BUDGET_BYTES,
    UserMessageEvidence, bind_review_request, user_message_evidence,
};
pub use signals::ReviewSignal;

pub use policy::{
    ActionClass, PolicyRevision, RestrictionMatcher, ReviewPolicy, ReviewPolicyError,
    ReviewPolicyStore, ReviewRestriction,
};

/// Where a user changed durable review policy. The event records the surface,
/// not a raw client payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicySurface {
    Tui,
    Rpc,
    Headless,
    Cli,
}

/// Maximum persisted size of one review-event string. Larger input is
/// truncated on a char boundary; control characters are redacted entirely.
pub const REVIEW_TEXT_MAX_BYTES: usize = 4096;

/// Review-event text that cannot carry an unbounded or control-character payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedReviewText(String);

impl BoundedReviewText {
    #[must_use]
    pub fn new(value: &str) -> Self {
        Self(bound_review_text(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl serde::Serialize for BoundedReviewText {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&bound_review_text(&self.0))
    }
}

impl<'de> serde::Deserialize<'de> for BoundedReviewText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self::new(&raw))
    }
}

fn bound_review_text(value: &str) -> String {
    let escaped: String = value
        .chars()
        .map(|ch| if ch.is_control() { '\u{FFFD}' } else { ch })
        .collect();
    if escaped.len() <= REVIEW_TEXT_MAX_BYTES {
        return escaped;
    }
    let mut end = REVIEW_TEXT_MAX_BYTES;
    while !escaped.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    escaped[..end].to_owned()
}

/// Bounded review-request evidence. Never carries command environment values,
/// secrets, or a full remote payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewRequestSummary {
    pub request_id: BoundedReviewText,
    pub action_kind: BoundedReviewText,
    pub target: BoundedReviewText,
    pub policy_revision: PolicyRevision,
    pub authorization_revision: u64,
}

/// Bounded reviewer assessment. Reason and model are identifiers, not a
/// generated rationale or raw provider body.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewAssessmentSummary {
    pub reviewer_id: BoundedReviewText,
    pub reason: BoundedReviewText,
    pub model: BoundedReviewText,
}

/// When a recorded exact-action grant stops being usable. The record is
/// evidence; the grant itself is not a standing permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GrantExpiry {
    OneAction,
    UnixSecs { secs: u64 },
}
