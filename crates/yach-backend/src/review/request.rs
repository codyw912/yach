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

/// `EvidenceItem::source` for trusted user messages; shared by routing and
/// the bench so the intent-presence check and builders cannot drift.
pub const USER_EVIDENCE_SOURCE: &str = "user";

/// Byte budget for trusted user-message evidence in one review request.
pub const USER_MESSAGE_BUDGET_BYTES: usize = 16 * 1024;

/// Trusted user messages fitted into the budget, or a signal that the
/// message that triggered this review cannot be represented.
#[derive(Debug, Clone, PartialEq)]
pub enum UserMessageEvidence {
    Ready {
        items: Vec<EvidenceItem>,
        omissions: Vec<OmissionMarker>,
    },
    IssuingTurnOverBudget,
}

/// Collect durable user messages for a review request. The issuing turn's
/// message leads, then older messages newest-first while they fit whole.
#[must_use]
pub fn user_message_evidence(
    log: &crate::SessionLog,
    issuing_turn: &crate::TurnId,
) -> UserMessageEvidence {
    let messages = log.user_messages_newest_first();
    let evidence = |entry: &crate::EntryId, text: &str| EvidenceItem {
        id: format!("user:{}", entry.0),
        source: String::from(USER_EVIDENCE_SOURCE),
        kind: String::from("message"),
        excerpt: text.to_owned(),
        truncated: false,
    };
    let mut items = Vec::new();
    let mut omissions = Vec::new();
    let mut used = 0_usize;
    let mut issuing_included: Option<&crate::EntryId> = None;
    for (entry, turn, text) in &messages {
        if *turn != issuing_turn {
            continue;
        }
        if issuing_included.is_none() {
            if text.len() > USER_MESSAGE_BUDGET_BYTES {
                return UserMessageEvidence::IssuingTurnOverBudget;
            }
            used = text.len();
            issuing_included = Some(*entry);
            items.push(evidence(entry, text));
            continue;
        }
        // A second issuing-turn user entry is never cited; mark it so the
        // reviewer sees it existed instead of silently dropping it.
        omissions.push(OmissionMarker::Unavailable {
            id: format!("user:{}", entry.0),
        });
    }
    for (entry, turn, text) in &messages {
        if *turn == issuing_turn {
            continue;
        }
        if used.saturating_add(text.len()) > USER_MESSAGE_BUDGET_BYTES {
            omissions.push(OmissionMarker::Unavailable {
                id: format!("user:{}", entry.0),
            });
            continue;
        }
        used += text.len();
        items.push(evidence(entry, text));
    }
    UserMessageEvidence::Ready { items, omissions }
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
        REVIEW_REQUEST_SCHEMA, ReviewAction, ReviewRequest, SandboxState,
        USER_MESSAGE_BUDGET_BYTES, UserMessageEvidence, bind_review_request, user_message_evidence,
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

    fn user_entry(entry: &str, turn: &str, text: &str) -> crate::SessionEvent {
        crate::SessionEvent::EntryAppended {
            session_id: crate::SessionId(String::from("s")),
            entry_id: crate::EntryId(entry.to_owned()),
            parent_entry_id: None,
            turn_id: crate::TurnId(turn.to_owned()),
            role: crate::Role::User,
            text: text.to_owned(),
            provider: None,
        }
    }

    #[test]
    fn issuing_turn_message_is_first_then_older_newest_first() {
        let mut log = crate::SessionLog::default();
        log.push(user_entry("e1", "turn-1", "first"));
        log.push(user_entry("e2", "turn-2", "second"));
        log.push(user_entry("e3", "turn-3", "third"));
        let UserMessageEvidence::Ready { items, omissions } =
            user_message_evidence(&log, &crate::TurnId(String::from("turn-2")))
        else {
            unreachable!("fits the budget")
        };
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["user:e2", "user:e3", "user:e1"]);
        assert!(omissions.is_empty());
        assert!(
            items
                .iter()
                .all(|item| !item.truncated && item.source == "user")
        );
    }

    #[test]
    fn older_messages_drop_whole_with_unavailable_markers() {
        let mut log = crate::SessionLog::default();
        log.push(user_entry("old", "turn-1", &"o".repeat(12 * 1024)));
        log.push(user_entry("mid", "turn-2", &"m".repeat(6 * 1024)));
        log.push(user_entry("now", "turn-3", &"n".repeat(6 * 1024)));
        let UserMessageEvidence::Ready { items, omissions } =
            user_message_evidence(&log, &crate::TurnId(String::from("turn-3")))
        else {
            unreachable!("issuing turn fits")
        };
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["user:now", "user:mid"]);
        assert!(
            omissions
                .iter()
                .any(|m| matches!(m, OmissionMarker::Unavailable { id } if id == "user:old"))
        );
    }

    #[test]
    fn oversized_issuing_turn_message_is_over_budget() {
        let mut log = crate::SessionLog::default();
        log.push(user_entry(
            "now",
            "turn-1",
            &"n".repeat(USER_MESSAGE_BUDGET_BYTES + 1),
        ));
        assert!(matches!(
            user_message_evidence(&log, &crate::TurnId(String::from("turn-1"))),
            UserMessageEvidence::IssuingTurnOverBudget
        ));
    }

    #[test]
    fn a_large_message_does_not_block_smaller_older_ones() {
        let mut log = crate::SessionLog::default();
        log.push(user_entry("tiny", "turn-1", "keep the tests green"));
        log.push(user_entry("big", "turn-2", &"b".repeat(16 * 1024)));
        log.push(user_entry("now", "turn-3", "run them"));
        let UserMessageEvidence::Ready { items, omissions } =
            user_message_evidence(&log, &crate::TurnId(String::from("turn-3")))
        else {
            unreachable!()
        };
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["user:now", "user:tiny"]);
        assert!(
            omissions
                .iter()
                .any(|m| matches!(m, OmissionMarker::Unavailable { id } if id == "user:big"))
        );
    }

    #[test]
    fn extra_issuing_turn_user_entries_get_unavailable_markers() {
        let mut log = crate::SessionLog::default();
        log.push(user_entry("older", "turn-1", "earlier turn"));
        log.push(user_entry("first", "turn-2", "first in issuing turn"));
        log.push(user_entry("newest", "turn-2", "newest in issuing turn"));
        let UserMessageEvidence::Ready { items, omissions } =
            user_message_evidence(&log, &crate::TurnId(String::from("turn-2")))
        else {
            unreachable!("fits the budget")
        };
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["user:newest", "user:older"]);
        assert!(
            omissions
                .iter()
                .any(|m| matches!(m, OmissionMarker::Unavailable { id } if id == "user:first")),
            "skipped issuing-turn entry must be marked: {omissions:?}"
        );
    }
}
