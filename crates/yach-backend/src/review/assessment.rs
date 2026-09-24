//! Typed reviewer assessment. Core rejects malformed, stale, or out-of-range
//! answers before any route is computed.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::request::ReviewRequest;

pub const REVIEW_ASSESSMENT_SCHEMA: &str = "yach.review-assessment.v2";
pub const REVIEW_ASSESSMENT_MAX_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationAnswer {
    ExactAuthorized,
    SubstantiveAuthorized,
    Insufficient,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AssessmentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Assessment as emitted by a reviewer adapter (`yach.review-assessment.v2`).
///
/// `model` is the concrete model the adapter recorded. An adapter error is a
/// failed review, not an authorization.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewAssessment {
    pub schema: String,
    pub request_id: String,
    pub reviewer_id: String,
    pub model: Option<String>,
    pub authorization: String,
    pub signals: BTreeMap<String, f64>,
    #[serde(default)]
    pub confidence: BTreeMap<String, f64>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub adapter_error: Option<String>,
    pub usage: AssessmentUsage,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssessmentValidationError {
    Oversized,
    Malformed,
    RequestMismatch,
    UnknownEvidenceRef,
    AdapterFailed,
}

impl ReviewAssessment {
    /// Deserialize and check the contract against the request that was sent.
    pub fn validate_against(
        bytes: &[u8],
        request: &ReviewRequest,
    ) -> Result<Self, AssessmentValidationError> {
        if bytes.len() > REVIEW_ASSESSMENT_MAX_BYTES {
            return Err(AssessmentValidationError::Oversized);
        }
        let assessment: Self =
            serde_json::from_slice(bytes).map_err(|_| AssessmentValidationError::Malformed)?;
        if assessment.schema != REVIEW_ASSESSMENT_SCHEMA
            || assessment.request_id != request.request_id
            || assessment.reviewer_id != request.reviewer_id
        {
            return Err(AssessmentValidationError::RequestMismatch);
        }
        if assessment.model.as_deref().is_none_or(str::is_empty) {
            return Err(AssessmentValidationError::Malformed);
        }
        if assessment.adapter_error.is_some() {
            return Err(AssessmentValidationError::AdapterFailed);
        }
        assessment.authorization_answer()?;
        if assessment.signals.len() != super::signals::ReviewSignal::ALL.len()
            || super::signals::ReviewSignal::ALL.iter().any(|signal| {
                assessment
                    .signals
                    .get(signal.id())
                    .is_none_or(|value| !unit_interval(*value))
            })
        {
            return Err(AssessmentValidationError::Malformed);
        }
        if assessment
            .confidence
            .values()
            .any(|value| !unit_interval(*value))
        {
            return Err(AssessmentValidationError::Malformed);
        }
        let known = request
            .trusted_evidence
            .iter()
            .chain(request.untrusted_evidence.iter())
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>();
        if assessment
            .evidence_refs
            .iter()
            .any(|reference| !known.contains(&reference.as_str()))
        {
            return Err(AssessmentValidationError::UnknownEvidenceRef);
        }
        Ok(assessment)
    }

    /// Validated signal value; `1.0` when missing so an unvalidated map fails
    /// closed.
    #[must_use]
    pub fn signal(&self, signal: super::signals::ReviewSignal) -> f64 {
        self.signals.get(signal.id()).copied().unwrap_or(1.0)
    }
    pub fn authorization_answer(&self) -> Result<AuthorizationAnswer, AssessmentValidationError> {
        match self.authorization.as_str() {
            "exact_authorized" => Ok(AuthorizationAnswer::ExactAuthorized),
            "substantive_authorized" => Ok(AuthorizationAnswer::SubstantiveAuthorized),
            "insufficient" => Ok(AuthorizationAnswer::Insufficient),
            "ambiguous" => Ok(AuthorizationAnswer::Ambiguous),
            _ => Err(AssessmentValidationError::Malformed),
        }
    }

    #[must_use]
    pub fn model_returned(&self) -> &str {
        self.model.as_deref().unwrap_or("")
    }
}

fn unit_interval(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AssessmentValidationError, REVIEW_ASSESSMENT_MAX_BYTES, ReviewAssessment};
    use crate::PolicyRevision;
    use crate::review::request::{
        EvidenceItem, REVIEW_REQUEST_SCHEMA, ReviewAction, ReviewRequest, SandboxState,
    };

    fn request() -> ReviewRequest {
        ReviewRequest {
            schema: REVIEW_REQUEST_SCHEMA,
            request_id: String::from("req-1"),
            session_id: String::from("session-1"),
            turn_id: String::from("turn-1"),
            policy_revision: PolicyRevision(1),
            authorization_revision: 1,
            reviewer_id: String::from("jev-typesafe"),
            reviewer_generation: 1,
            action: ReviewAction::ShellCommand {
                command: String::from("cargo test"),
                cwd: String::from("/work"),
                timeout_ms: 1_000,
                env_keys: Vec::new(),
            },
            trusted_evidence: vec![EvidenceItem {
                id: String::from("user-1"),
                source: String::from("user"),
                kind: String::from("message"),
                excerpt: String::from("run tests"),
                truncated: false,
            }],
            untrusted_evidence: Vec::new(),
            omissions: Vec::new(),
            sandbox_state: SandboxState::None,
        }
    }

    fn body() -> serde_json::Value {
        let signals: serde_json::Map<_, _> = crate::review::ReviewSignal::ALL
            .iter()
            .map(|signal| (signal.id().to_owned(), json!(0.05)))
            .collect();
        json!({
            "schema": "yach.review-assessment.v2",
            "request_id": "req-1",
            "reviewer_id": "jev-typesafe",
            "model": "jev-1.13.0",
            "authorization": "substantive_authorized",
            "signals": signals,
            "confidence": {"authorization": 0.8},
            "evidence_refs": ["user-1"],
            "adapter_error": null,
            "usage": {"input_tokens": 1, "output_tokens": 1},
            "duration_ms": 3
        })
    }

    #[test]
    fn missing_signal_is_malformed() {
        let mut value = body();
        let removed = value["signals"]
            .as_object_mut()
            .map(|s| s.remove("scope_conflict"));
        assert!(removed.is_some());
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Malformed)
        ));
    }

    #[test]
    fn extra_signal_is_malformed() {
        let mut value = body();
        value["signals"]["evidence_sufficient"] = json!(0.9);
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Malformed)
        ));
    }

    #[test]
    fn out_of_range_signal_is_malformed() {
        for bad in [json!(1.01), json!(-0.1), json!("NaN")] {
            let mut value = body();
            value["signals"]["publish"] = bad;
            let bytes = serde_json::to_vec(&value).unwrap_or_default();
            assert!(matches!(
                ReviewAssessment::validate_against(&bytes, &request()),
                Err(AssessmentValidationError::Malformed)
            ));
        }
    }

    #[test]
    fn valid_v2_assessment_exposes_typed_signals() {
        let mut value = body();
        value["signals"]["publish"] = json!(0.93);
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        let parsed = ReviewAssessment::validate_against(&bytes, &request());
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        assert!((parsed.signal(crate::review::ReviewSignal::Publish) - 0.93).abs() < f64::EPSILON);
    }

    #[test]
    fn non_finite_probability_is_malformed() {
        let mut value = body();
        value["confidence"]["authorization"] = json!("NaN");
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Malformed)
        ));
    }

    #[test]
    fn empty_model_is_malformed() {
        let mut value = body();
        value["model"] = json!("");
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Malformed)
        ));
    }

    #[test]
    fn mismatched_request_id_is_rejected() {
        let mut value = body();
        value["request_id"] = json!("other");
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::RequestMismatch)
        ));
    }

    #[test]
    fn oversized_assessment_bytes_are_rejected() {
        let bytes = vec![b'x'; REVIEW_ASSESSMENT_MAX_BYTES + 1];
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Oversized)
        ));
    }

    #[test]
    fn unknown_field_is_rejected() {
        let mut value = body();
        value["rationale"] = json!("looks fine");
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        assert!(matches!(
            ReviewAssessment::validate_against(&bytes, &request()),
            Err(AssessmentValidationError::Malformed)
        ));
    }
}
