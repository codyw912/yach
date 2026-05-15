use serde::{Deserialize, Serialize};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

static PERMISSION_DECISION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionDecisionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionCapability {
    EditTransaction,
    ShellCommand,
    NetworkAccess,
    VerificationAction,
    ExtensionTool,
    ProviderVisibleTool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativePermissionActor {
    UserLocalUi,
    Core,
    Provider,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionMode {
    Allow,
    Ask,
    Deny,
    AutoReview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativePermissionReviewer {
    None,
    User,
    AutoReview,
    Extension { extension_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionRisk {
    ReadOnly,
    WorkspaceWrite,
    ExternalWrite,
    Network,
    ProcessExecution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionTargetSummary {
    pub operation: String,
    pub resource: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionRequest {
    pub request_id: String,
    pub actor: NativePermissionActor,
    pub capability: NativePermissionCapability,
    pub target: NativePermissionTargetSummary,
    pub risk: NativePermissionRisk,
    pub requested_reviewer: Option<NativePermissionReviewer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePermissionPolicy {
    pub edit_mode: NativePermissionMode,
}

impl NativePermissionPolicy {
    #[must_use]
    pub const fn for_edit_mode(edit_mode: NativePermissionMode) -> Self {
        Self { edit_mode }
    }

    #[must_use]
    pub const fn default_local_edit() -> Self {
        Self {
            edit_mode: NativePermissionMode::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum NativePermissionDecision {
    Allowed {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    Denied {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        rationale: Option<String>,
    },
    NeedsUserReview {
        decision_id: NativePermissionDecisionId,
        reviewer: NativePermissionReviewer,
        mode: NativePermissionMode,
        reason: String,
        prompt: NativePermissionPrompt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionPrompt {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePermissionDecisionSummary {
    pub request_id: String,
    pub decision_id: NativePermissionDecisionId,
    pub actor: NativePermissionActor,
    pub capability: NativePermissionCapability,
    pub target: NativePermissionTargetSummary,
    pub risk: NativePermissionRisk,
    pub configured_mode: NativePermissionMode,
    pub reviewer: NativePermissionReviewer,
    pub outcome: NativePermissionDecisionOutcome,
    pub reason: String,
    pub rationale: Option<String>,
    pub user_override: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePermissionDecisionOutcome {
    Allowed,
    Denied,
    NeedsUserReview,
}

impl NativePermissionDecision {
    #[must_use]
    pub fn decision_id(&self) -> NativePermissionDecisionId {
        match self {
            Self::Allowed { decision_id, .. }
            | Self::Denied { decision_id, .. }
            | Self::NeedsUserReview { decision_id, .. } => decision_id.clone(),
        }
    }

    #[must_use]
    pub fn summary(
        &self,
        request: &NativePermissionRequest,
        user_override: bool,
    ) -> NativePermissionDecisionSummary {
        match self {
            Self::Allowed {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: NativePermissionDecisionOutcome::Allowed,
                reason: reason.clone(),
                rationale: sanitized_rationale(rationale.as_deref()),
                user_override,
            },
            Self::Denied {
                decision_id,
                reviewer,
                mode,
                reason,
                rationale,
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: NativePermissionDecisionOutcome::Denied,
                reason: reason.clone(),
                rationale: sanitized_rationale(rationale.as_deref()),
                user_override,
            },
            Self::NeedsUserReview {
                decision_id,
                reviewer,
                mode,
                reason,
                ..
            } => NativePermissionDecisionSummary {
                request_id: request.request_id.clone(),
                decision_id: decision_id.clone(),
                actor: request.actor.clone(),
                capability: request.capability.clone(),
                target: sanitized_target_summary(&request.target),
                risk: request.risk,
                configured_mode: *mode,
                reviewer: reviewer.clone(),
                outcome: NativePermissionDecisionOutcome::NeedsUserReview,
                reason: reason.clone(),
                rationale: None,
                user_override,
            },
        }
    }
}

pub struct NativePermissionDecisionEngine;

impl NativePermissionDecisionEngine {
    #[must_use]
    pub fn decide(
        request: &NativePermissionRequest,
        policy: &NativePermissionPolicy,
    ) -> NativePermissionDecision {
        if extension_self_approval_requested(request) {
            return NativePermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode: policy.edit_mode,
                reason: String::from("extension_self_approval_denied"),
                rationale: None,
            };
        }

        let mode = match request.capability {
            NativePermissionCapability::EditTransaction
                if request.risk == NativePermissionRisk::WorkspaceWrite =>
            {
                policy.edit_mode
            }
            NativePermissionCapability::EditTransaction => {
                return NativePermissionDecision::Denied {
                    decision_id: next_permission_decision_id(),
                    reviewer: NativePermissionReviewer::None,
                    mode: policy.edit_mode,
                    reason: String::from("permission_risk_denied"),
                    rationale: None,
                };
            }
            NativePermissionCapability::ShellCommand
            | NativePermissionCapability::NetworkAccess
            | NativePermissionCapability::VerificationAction
            | NativePermissionCapability::ExtensionTool
            | NativePermissionCapability::ProviderVisibleTool => NativePermissionMode::Deny,
        };

        match mode {
            NativePermissionMode::Allow => NativePermissionDecision::Allowed {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_allowed"),
                rationale: None,
            },
            NativePermissionMode::Ask => NativePermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::User,
                mode,
                reason: String::from("permission_mode_ask"),
                prompt: permission_prompt(request),
            },
            NativePermissionMode::Deny => NativePermissionDecision::Denied {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::None,
                mode,
                reason: String::from("permission_mode_denied"),
                rationale: None,
            },
            NativePermissionMode::AutoReview => NativePermissionDecision::NeedsUserReview {
                decision_id: next_permission_decision_id(),
                reviewer: NativePermissionReviewer::User,
                mode,
                reason: String::from("auto_review_unavailable_fallback_ask"),
                prompt: permission_prompt(request),
            },
        }
    }
}

fn sanitized_target_summary(
    target: &NativePermissionTargetSummary,
) -> NativePermissionTargetSummary {
    NativePermissionTargetSummary {
        operation: sanitized_label(&target.operation),
        resource: sanitized_resource(&target.resource),
    }
}

fn sanitized_label(label: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return String::from("<empty>");
    }
    if trimmed.chars().any(char::is_control) {
        return String::from("<redacted>");
    }
    truncate_chars(trimmed, 128)
}

fn sanitized_resource(resource: &str) -> String {
    let trimmed = resource.trim();
    if trimmed.is_empty() {
        return String::from("<empty_path>");
    }
    if trimmed.starts_with('{') || trimmed.chars().any(char::is_control) {
        return String::from("<redacted_resource>");
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return String::from("<absolute_path>");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return String::from("<path_traversal>");
    }
    if trimmed == ".yach" || trimmed.starts_with(".yach/") {
        return String::from("<metadata_path>");
    }
    truncate_chars(trimmed, 256)
}

fn sanitized_rationale(rationale: Option<&str>) -> Option<String> {
    rationale.map(|_| String::from("<redacted_rationale>"))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn permission_prompt(request: &NativePermissionRequest) -> NativePermissionPrompt {
    NativePermissionPrompt {
        title: format!("Approve {}", request.target.operation),
        body: format!(
            "{} on {}",
            request.target.operation, request.target.resource
        ),
    }
}

fn extension_self_approval_requested(request: &NativePermissionRequest) -> bool {
    match (&request.actor, &request.requested_reviewer) {
        (
            NativePermissionActor::Extension {
                extension_id: actor,
            },
            Some(NativePermissionReviewer::Extension {
                extension_id: reviewer,
            }),
        ) => actor == reviewer,
        _ => false,
    }
}

fn next_permission_decision_id() -> NativePermissionDecisionId {
    let next = PERMISSION_DECISION_COUNTER.fetch_add(1, Ordering::Relaxed);
    NativePermissionDecisionId(format!("permission-decision-{next}"))
}

#[cfg(test)]
mod tests {
    use super::{
        NativePermissionActor, NativePermissionCapability, NativePermissionDecision,
        NativePermissionDecisionEngine, NativePermissionDecisionId, NativePermissionMode,
        NativePermissionPolicy, NativePermissionRequest, NativePermissionReviewer,
        NativePermissionRisk, NativePermissionTargetSummary,
    };

    fn edit_request() -> NativePermissionRequest {
        NativePermissionRequest {
            request_id: String::from("perm-1"),
            actor: NativePermissionActor::UserLocalUi,
            capability: NativePermissionCapability::EditTransaction,
            target: NativePermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("src/lib.rs"),
            },
            risk: NativePermissionRisk::WorkspaceWrite,
            requested_reviewer: None,
        }
    }

    #[test]
    fn ask_mode_routes_edit_to_user_review() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Ask),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::NeedsUserReview {
                reviewer: NativePermissionReviewer::User,
                ..
            }
        ));
    }

    #[test]
    fn allow_mode_allows_without_reviewer() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Allowed {
                reviewer: NativePermissionReviewer::None,
                ..
            }
        ));
    }

    #[test]
    fn deny_mode_denies_before_edit_preview() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Deny),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Denied {
                reason,
                reviewer: NativePermissionReviewer::None,
                ..
            } if reason == "permission_mode_denied"
        ));
    }

    #[test]
    fn auto_review_is_represented_and_falls_back_to_user_review() {
        let decision = NativePermissionDecisionEngine::decide(
            &edit_request(),
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::AutoReview),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::NeedsUserReview {
                reviewer: NativePermissionReviewer::User,
                mode: NativePermissionMode::AutoReview,
                reason,
                ..
            } if reason == "auto_review_unavailable_fallback_ask"
        ));
    }

    #[test]
    fn extension_cannot_self_approve() {
        let request = NativePermissionRequest {
            actor: NativePermissionActor::Extension {
                extension_id: String::from("ext-a"),
            },
            requested_reviewer: Some(NativePermissionReviewer::Extension {
                extension_id: String::from("ext-a"),
            }),
            ..edit_request()
        };

        let decision = NativePermissionDecisionEngine::decide(
            &request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Denied {
                reason,
                ..
            } if reason == "extension_self_approval_denied"
        ));
    }

    #[test]
    fn edit_transaction_denies_inconsistent_risk() {
        let request = NativePermissionRequest {
            risk: NativePermissionRisk::ExternalWrite,
            ..edit_request()
        };

        let decision = NativePermissionDecisionEngine::decide(
            &request,
            &NativePermissionPolicy::for_edit_mode(NativePermissionMode::Allow),
        );

        assert!(matches!(
            decision,
            NativePermissionDecision::Denied {
                reason,
                ..
            } if reason == "permission_risk_denied"
        ));
    }

    #[test]
    fn summaries_redact_unsafe_resource_and_rationale() {
        let request = NativePermissionRequest {
            target: NativePermissionTargetSummary {
                operation: String::from("modify_text_file"),
                resource: String::from("/tmp/secret-file"),
            },
            ..edit_request()
        };
        let decision = NativePermissionDecision::Allowed {
            decision_id: NativePermissionDecisionId(String::from("permission-decision-test")),
            reviewer: NativePermissionReviewer::None,
            mode: NativePermissionMode::Allow,
            reason: String::from("permission_mode_allowed"),
            rationale: Some(String::from("raw hidden reviewer rationale")),
        };

        let summary = decision.summary(&request, false);

        assert_eq!(summary.target.resource, "<absolute_path>");
        assert_eq!(
            summary.rationale,
            Some(String::from("<redacted_rationale>"))
        );
    }
}
