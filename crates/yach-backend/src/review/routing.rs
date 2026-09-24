//! Code-owned routing over reviewer signals. Order is fixed by the spec:
//! completeness, intent presence, restriction, risk, opacity, scope.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::assessment::ReviewAssessment;
use super::coordinator::{HoldReason, ReviewRoute};
use super::request::ReviewRequest;
use super::signals::ReviewSignal;
use crate::{ReviewPolicy, ReviewRestriction};

const ROUTING_TOML: &str = include_str!("routing.toml");
const ROUTING_SCHEMA: &str = "yach-review-routing.v2";

#[derive(Debug, Deserialize)]
struct RoutingFile {
    schema: String,
    signals: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Thresholds(BTreeMap<ReviewSignal, f64>);

impl Thresholds {
    fn fail_closed() -> Self {
        Self(ReviewSignal::ALL.iter().map(|s| (*s, 0.0)).collect())
    }
    fn fires(&self, assessment: &ReviewAssessment, signal: ReviewSignal) -> bool {
        assessment.signal(signal) >= self.0.get(&signal).copied().unwrap_or(0.0)
    }
}

fn thresholds_from_toml(text: &str) -> Thresholds {
    let Ok(file) = toml::from_str::<RoutingFile>(text) else {
        return Thresholds::fail_closed();
    };
    if file.schema != ROUTING_SCHEMA || file.signals.len() != ReviewSignal::ALL.len() {
        return Thresholds::fail_closed();
    }
    let mut table = BTreeMap::new();
    for signal in ReviewSignal::ALL {
        match file.signals.get(signal.id()) {
            Some(value) if value.is_finite() && (0.0..=1.0).contains(value) => {
                table.insert(signal, *value);
            }
            _ => return Thresholds::fail_closed(),
        }
    }
    Thresholds(table)
}

fn thresholds() -> &'static Thresholds {
    static THRESHOLDS: LazyLock<Thresholds> = LazyLock::new(|| thresholds_from_toml(ROUTING_TOML));
    &THRESHOLDS
}

fn hold(reason: HoldReason, assessment: &ReviewAssessment) -> ReviewRoute {
    ReviewRoute::Hold {
        reason,
        evidence_refs: assessment.evidence_refs.clone(),
    }
}

/// Route a validated assessment. The reviewer proposes signals; only this
/// code decides. Order: completeness, intent presence, restriction, risk,
/// opacity, scope, execute.
#[must_use]
pub fn route_assessment(
    request: &ReviewRequest,
    assessment: &ReviewAssessment,
    policy: &ReviewPolicy,
) -> ReviewRoute {
    let thresholds = thresholds();
    if request.trusted_evidence.iter().any(|item| item.truncated) {
        return hold(HoldReason::NeedsClarification, assessment);
    }
    if !request
        .trusted_evidence
        .iter()
        .any(|item| item.source == "user")
    {
        return hold(HoldReason::NeedsClarification, assessment);
    }
    // Strongest restriction across all firing class signals: the first
    // HumanPerforms found wins, else the first AskFirst.
    let mut restriction: Option<crate::ReviewRestriction> = None;
    for signal in ReviewSignal::ALL {
        let Some(class) = signal.policy_class() else {
            continue;
        };
        if !thresholds.fires(assessment, signal) {
            continue;
        }
        let Some(candidate) = crate::restriction_for_class(policy, class) else {
            continue;
        };
        let replace = match &restriction {
            None => true,
            Some(ReviewRestriction::AskFirst { .. })
                if matches!(candidate, ReviewRestriction::HumanPerforms { .. }) =>
            {
                true
            }
            Some(_) => false,
        };
        if replace {
            restriction = Some(candidate.clone());
        }
    }
    if let Some(restriction) = restriction {
        return hold(HoldReason::RestrictionApplies { restriction }, assessment);
    }
    if ReviewSignal::RISK
        .iter()
        .any(|signal| thresholds.fires(assessment, *signal))
    {
        return hold(HoldReason::SignificantRisk, assessment);
    }
    if thresholds.fires(assessment, ReviewSignal::OpaqueEffect)
        || thresholds.fires(assessment, ReviewSignal::ScopeConflict)
    {
        return hold(HoldReason::NeedsClarification, assessment);
    }
    ReviewRoute::Execute
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::review::request::ReviewAction;
    use crate::review::{
        EvidenceItem, PolicyRevision, RestrictionMatcher, ReviewPolicy, ReviewRestriction,
        SandboxState,
    };
    use crate::{ActionClass, ReviewSignal};

    fn assessment(overrides: &[(ReviewSignal, f64)]) -> ReviewAssessment {
        let mut signals = serde_json::Map::new();
        for signal in ReviewSignal::ALL {
            signals.insert(signal.id().to_owned(), json!(0.05));
        }
        for (signal, value) in overrides {
            signals.insert(signal.id().to_owned(), json!(value));
        }
        let parsed = serde_json::from_value(json!({
            "schema": "yach.review-assessment.v2", "request_id": "req-1",
            "reviewer_id": "jev-typesafe", "model": "m", "authorization": "insufficient",
            "signals": signals, "confidence": {}, "evidence_refs": [],
            "adapter_error": null, "usage": {"input_tokens": 0, "output_tokens": 0},
            "duration_ms": 0
        }));
        let Ok(parsed) = parsed else {
            unreachable!("fixture parses")
        };
        parsed
    }

    fn user(truncated: bool) -> EvidenceItem {
        EvidenceItem {
            id: String::from("user:e1"),
            source: String::from("user"),
            kind: String::from("message"),
            excerpt: String::from("run the tests"),
            truncated,
        }
    }

    fn command() -> EvidenceItem {
        EvidenceItem {
            id: String::from("command"),
            source: String::from("permission_request"),
            kind: String::from("shell_command"),
            excerpt: String::from("cargo test"),
            truncated: false,
        }
    }

    fn request(trusted: Vec<EvidenceItem>, untrusted: Vec<EvidenceItem>) -> ReviewRequest {
        ReviewRequest {
            schema: crate::review::REVIEW_REQUEST_SCHEMA,
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
                timeout_ms: 5_000,
                env_keys: vec![String::from("PATH")],
            },
            trusted_evidence: trusted,
            untrusted_evidence: untrusted,
            omissions: Vec::new(),
            sandbox_state: SandboxState::None,
        }
    }

    fn ask_first(class: ActionClass, note: &str) -> ReviewRestriction {
        ReviewRestriction::AskFirst {
            matcher: RestrictionMatcher::ActionClass { class },
            note: String::from(note),
        }
    }

    fn human_performs(class: ActionClass, note: &str) -> ReviewRestriction {
        ReviewRestriction::HumanPerforms {
            matcher: RestrictionMatcher::ActionClass { class },
            note: String::from(note),
        }
    }

    fn policy(global: Vec<ReviewRestriction>, project: Vec<ReviewRestriction>) -> ReviewPolicy {
        ReviewPolicy {
            revision: PolicyRevision(1),
            global,
            project,
        }
    }

    fn route(
        trusted: Vec<EvidenceItem>,
        untrusted: Vec<EvidenceItem>,
        overrides: &[(ReviewSignal, f64)],
        policy: &ReviewPolicy,
    ) -> ReviewRoute {
        route_assessment(&request(trusted, untrusted), &assessment(overrides), policy)
    }

    #[test]
    fn clean_signals_with_user_intent_execute() {
        let route = route(
            vec![user(false), command()],
            Vec::new(),
            &[],
            &ReviewPolicy::empty(),
        );
        assert_eq!(route, ReviewRoute::Execute);
    }

    #[test]
    fn truncated_trusted_item_holds_before_any_signal() {
        let route = route(
            vec![user(true)],
            Vec::new(),
            &ReviewSignal::ALL
                .iter()
                .map(|s| (*s, 0.99))
                .collect::<Vec<_>>(),
            &ReviewPolicy::empty(),
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::NeedsClarification,
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn truncated_untrusted_item_alone_does_not_hold() {
        let route = route(
            vec![user(false)],
            vec![EvidenceItem {
                truncated: true,
                ..command()
            }],
            &[],
            &ReviewPolicy::empty(),
        );
        assert_eq!(route, ReviewRoute::Execute);
    }

    #[test]
    fn missing_user_message_holds_before_any_signal() {
        let route = route(vec![command()], Vec::new(), &[], &ReviewPolicy::empty());
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::NeedsClarification,
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn class_signal_with_matching_restriction_holds_as_restriction() {
        let restriction = ask_first(ActionClass::ExternalPublish, "confirm publish");
        let policy = policy(vec![restriction.clone()], Vec::new());
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Publish, 0.9)],
            &policy,
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::RestrictionApplies { restriction },
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn delete_signal_maps_to_destructive_delete_restriction() {
        let restriction = ask_first(ActionClass::DestructiveDelete, "confirm delete");
        let policy = policy(vec![restriction.clone()], Vec::new());
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Delete, 0.9)],
            &policy,
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::RestrictionApplies { restriction },
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn delete_signal_without_restriction_executes() {
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Delete, 0.9)],
            &ReviewPolicy::empty(),
        );
        assert_eq!(route, ReviewRoute::Execute);
    }

    #[test]
    fn global_human_performs_outranks_project_ask_first() {
        let global = human_performs(ActionClass::PersistentInstall, "I install");
        let project = ask_first(ActionClass::PersistentInstall, "project asks");
        let policy = policy(vec![global.clone()], vec![project]);
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Install, 0.9)],
            &policy,
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::RestrictionApplies {
                    restriction: global
                },
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn restriction_outranks_risk() {
        let restriction = ask_first(ActionClass::ExternalPublish, "confirm publish");
        let policy = policy(vec![restriction.clone()], Vec::new());
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Publish, 0.9), (ReviewSignal::Privilege, 0.9)],
            &policy,
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::RestrictionApplies { restriction },
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn each_risk_signal_holds_without_restriction() {
        for signal in ReviewSignal::RISK {
            let route = route(
                vec![user(false)],
                Vec::new(),
                &[(signal, 0.9)],
                &ReviewPolicy::empty(),
            );
            assert_eq!(
                route,
                ReviewRoute::Hold {
                    reason: HoldReason::SignificantRisk,
                    evidence_refs: Vec::new(),
                },
                "signal {} must hold as risk",
                signal.id()
            );
        }
    }

    #[test]
    fn opaque_effect_asks_for_clarification() {
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::OpaqueEffect, 0.9)],
            &ReviewPolicy::empty(),
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::NeedsClarification,
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn scope_conflict_asks_for_clarification() {
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::ScopeConflict, 0.9)],
            &ReviewPolicy::empty(),
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::NeedsClarification,
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn risk_outranks_opacity_and_scope() {
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[
                (ReviewSignal::RemoteCode, 0.9),
                (ReviewSignal::OpaqueEffect, 0.9),
                (ReviewSignal::ScopeConflict, 0.9),
            ],
            &ReviewPolicy::empty(),
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::SignificantRisk,
                evidence_refs: Vec::new(),
            }
        );
    }

    #[test]
    fn threshold_is_inclusive() {
        let at = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Publish, 0.5)],
            &ReviewPolicy::empty(),
        );
        assert_eq!(
            at,
            ReviewRoute::Hold {
                reason: HoldReason::SignificantRisk,
                evidence_refs: Vec::new(),
            }
        );
        let below = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Publish, 0.4999)],
            &ReviewPolicy::empty(),
        );
        assert_eq!(below, ReviewRoute::Execute);
    }

    #[test]
    fn mismatched_schema_fails_closed() {
        let thresholds = thresholds_from_toml("schema = \"other\"\n[signals]\n");
        for signal in ReviewSignal::ALL {
            assert_eq!(thresholds.0.get(&signal), Some(&0.0));
        }
    }

    #[test]
    fn human_performs_on_any_firing_class_wins_over_ask_first_on_another() {
        let human = human_performs(ActionClass::PersistentInstall, "I install");
        let ask = ask_first(ActionClass::ExternalPublish, "confirm publish");
        let policy = policy(vec![ask], vec![human.clone()]);
        let route = route(
            vec![user(false)],
            Vec::new(),
            &[(ReviewSignal::Install, 0.9), (ReviewSignal::Publish, 0.9)],
            &policy,
        );
        assert_eq!(
            route,
            ReviewRoute::Hold {
                reason: HoldReason::RestrictionApplies { restriction: human },
                evidence_refs: Vec::new(),
            }
        );
    }
}
