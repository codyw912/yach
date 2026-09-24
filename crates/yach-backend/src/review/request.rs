//! Immutable bounded review requests. Core serializes once and refuses to
//! treat a silently trimmed request as complete.

use serde::{Deserialize, Serialize};

use super::PolicyRevision;

pub const REVIEW_REQUEST_SCHEMA: &str = "yach.review-request.v2";
pub const REVIEW_REQUEST_MAX_BYTES: usize = 64 * 1024;

/// Exact action under review. Environment values never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewAction {
    ShellCommand {
        command: String,
        cwd: String,
        timeout_ms: u64,
        env_keys: Vec<String>,
    },
    EditTransaction {
        operations: Vec<ReviewEditOperation>,
        preconditions: Vec<String>,
    },
    ExtensionProposal {
        extension_id: String,
        operations: Vec<ReviewEditOperation>,
    },
}

/// Bounded edit surface copied into a review request. Not an executable plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewEditOperation {
    ModifyTextFile {
        path: String,
        expected_sha256: String,
    },
    ReplaceTextFile {
        path: String,
        expected_sha256: String,
    },
    CreateTextFile {
        path: String,
    },
}

/// One cited piece of evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub excerpt: String,
    /// True exactly when excerpt is not the complete source value.
    pub truncated: bool,
}

/// Why an evidence item is absent from the request the reviewer sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OmissionMarker {
    DroppedUntrusted { id: String },
    Redacted { id: String },
    Unavailable { id: String },
}

/// Actual sandbox restrictions, or an explicit statement that there are none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SandboxState {
    None,
    Declared { restrictions: Vec<String> },
}

/// Immutable review request; serialized once and bound to the action.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewRequest {
    pub schema: &'static str,
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub policy_revision: PolicyRevision,
    pub authorization_revision: u64,
    pub reviewer_id: String,
    pub reviewer_generation: u64,
    pub action: ReviewAction,
    pub trusted_evidence: Vec<EvidenceItem>,
    pub untrusted_evidence: Vec<EvidenceItem>,
    pub omissions: Vec<OmissionMarker>,
    pub sandbox_state: SandboxState,
}

/// Outcome of fitting a request into the 64 KiB contract.
#[derive(Debug, Clone, PartialEq)]
pub enum BoundReviewRequest {
    Ready(Box<ReviewRequest>),
    /// A trusted item would have to be dropped, or the request is still over
    /// budget after dropping untrusted evidence. The reviewer is not called.
    OverBudget {
        omissions: Vec<OmissionMarker>,
    },
}

/// Build a request and enforce the serialized bound.
///
/// Serializes once. On overflow, retries once after marking every untrusted
/// item omitted. Holds if that retry is still over, or if dropping a trusted
/// item would be required to fit.
pub fn bind_review_request(mut request: ReviewRequest) -> BoundReviewRequest {
    if serialized_len(&request).is_some_and(|len| len <= REVIEW_REQUEST_MAX_BYTES) {
        return BoundReviewRequest::Ready(Box::new(request));
    }
    if request.untrusted_evidence.is_empty() {
        return BoundReviewRequest::OverBudget {
            omissions: request.omissions,
        };
    }
    let dropped = request
        .untrusted_evidence
        .iter()
        .map(|item| OmissionMarker::DroppedUntrusted {
            id: item.id.clone(),
        })
        .collect::<Vec<_>>();
    request.omissions.extend(dropped);
    request.untrusted_evidence.clear();
    if serialized_len(&request).is_some_and(|len| len <= REVIEW_REQUEST_MAX_BYTES) {
        BoundReviewRequest::Ready(Box::new(request))
    } else {
        BoundReviewRequest::OverBudget {
            omissions: request.omissions,
        }
    }
}

fn serialized_len(request: &ReviewRequest) -> Option<usize> {
    serde_json::to_vec(request).ok().map(|bytes| bytes.len())
}

#[must_use]
pub fn action_kind(action: &ReviewAction) -> &'static str {
    match action {
        ReviewAction::ShellCommand { .. } => "shell_command",
        ReviewAction::EditTransaction { .. } => "edit_transaction",
        ReviewAction::ExtensionProposal { .. } => "extension_proposal",
    }
}

#[must_use]
pub fn action_target(action: &ReviewAction) -> String {
    match action {
        ReviewAction::ShellCommand { command, .. } => command.clone(),
        ReviewAction::EditTransaction { operations, .. }
        | ReviewAction::ExtensionProposal { operations, .. } => operations
            .first()
            .map(edit_operation_path)
            .unwrap_or_default(),
    }
}

fn edit_operation_path(operation: &ReviewEditOperation) -> String {
    match operation {
        ReviewEditOperation::ModifyTextFile { path, .. }
        | ReviewEditOperation::ReplaceTextFile { path, .. }
        | ReviewEditOperation::CreateTextFile { path } => path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundReviewRequest, EvidenceItem, OmissionMarker, REVIEW_REQUEST_MAX_BYTES,
        REVIEW_REQUEST_SCHEMA, ReviewAction, ReviewRequest, SandboxState, bind_review_request,
    };
    use crate::PolicyRevision;

    fn request(trusted: Vec<EvidenceItem>, untrusted: Vec<EvidenceItem>) -> ReviewRequest {
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
                env_keys: vec![String::from("PATH")],
            },
            trusted_evidence: trusted,
            untrusted_evidence: untrusted,
            omissions: Vec::new(),
            sandbox_state: SandboxState::None,
        }
    }

    fn item(id: &str, bytes: usize) -> EvidenceItem {
        EvidenceItem {
            id: id.to_owned(),
            source: String::from("repo"),
            kind: String::from("file"),
            excerpt: "x".repeat(bytes),
            truncated: false,
        }
    }

    #[test]
    fn evidence_serializes_truncated_not_bounded() {
        let value = serde_json::to_value(item("user", 4));
        assert!(value.is_ok());
        let Ok(value) = value else { return };
        assert_eq!(value.get("truncated"), Some(&serde_json::json!(false)));
        assert!(value.get("bounded").is_none());
    }

    #[test]
    fn oversized_untrusted_evidence_is_omitted_once_and_kept_when_it_fits() {
        let bound = bind_review_request(request(
            vec![item("user", 64)],
            vec![item("tool", REVIEW_REQUEST_MAX_BYTES)],
        ));
        assert!(matches!(bound, BoundReviewRequest::Ready(_)));
        let BoundReviewRequest::Ready(ready) = bound else {
            return;
        };
        assert!(ready.untrusted_evidence.is_empty());
        assert!(ready.omissions.iter().any(|marker| matches!(
            marker,
            OmissionMarker::DroppedUntrusted { id } if id == "tool"
        )));
        assert_eq!(ready.trusted_evidence.len(), 1);
    }

    #[test]
    fn trusted_overflow_holds_without_dropping_the_item() {
        let bound = bind_review_request(request(vec![item("user", 70 * 1024)], Vec::new()));
        assert!(matches!(bound, BoundReviewRequest::OverBudget { .. }));
    }
}
