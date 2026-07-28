use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::edit_harness::{
    EditHarness, edit_apply_evidence_summary, edit_prepared_evidence_summary,
};
use crate::{
    EditEngine, EditError, EditEvidenceOutcome, EditHunk, EditOperation, EditPolicy,
    EditTransactionRequest, ResourceRoot, SessionEvent, SessionId, SessionLog, TurnId,
    edit_error_label,
};

static PROFILE_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EditProfileScenario {
    CreateSmallTextFile,
    ModifySingleHunkSmallFile,
    ModifyMultiHunkMediumFile,
    ValidationFailurePathTraversal,
    ApplyFailureHashChanged,
}

const ALL_NATIVE_EDIT_PROFILE_SCENARIOS: [EditProfileScenario; 5] = [
    EditProfileScenario::CreateSmallTextFile,
    EditProfileScenario::ModifySingleHunkSmallFile,
    EditProfileScenario::ModifyMultiHunkMediumFile,
    EditProfileScenario::ValidationFailurePathTraversal,
    EditProfileScenario::ApplyFailureHashChanged,
];

impl EditProfileScenario {
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &ALL_NATIVE_EDIT_PROFILE_SCENARIOS
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::CreateSmallTextFile => "create_small_text_file",
            Self::ModifySingleHunkSmallFile => "modify_single_hunk_small_file",
            Self::ModifyMultiHunkMediumFile => "modify_multi_hunk_medium_file",
            Self::ValidationFailurePathTraversal => "validation_failure_path_traversal",
            Self::ApplyFailureHashChanged => "apply_failure_hash_changed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EditProfilePhase {
    Preview,
    PreparedEvidenceSummary,
    Apply,
    FinishedEvidenceSummary,
    SessionAppendEvents,
    EndToEndHarnessSuccess,
    EndToEndHarnessValidationFailure,
    EndToEndHarnessApplyFailure,
}

impl EditProfilePhase {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::PreparedEvidenceSummary => "prepared_evidence_summary",
            Self::Apply => "apply",
            Self::FinishedEvidenceSummary => "finished_evidence_summary",
            Self::SessionAppendEvents => "session_append_events",
            Self::EndToEndHarnessSuccess => "end_to_end_harness_success",
            Self::EndToEndHarnessValidationFailure => "end_to_end_harness_validation_failure",
            Self::EndToEndHarnessApplyFailure => "end_to_end_harness_apply_failure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditProfileOutcome {
    Completed,
    ExpectedValidationFailure,
    ExpectedApplyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditProfilePhaseDuration {
    pub phase: EditProfilePhase,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditProfileSample {
    pub scenario: EditProfileScenario,
    pub outcome: EditProfileOutcome,
    pub phases: Vec<EditProfilePhaseDuration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditProfileError {
    pub scenario: EditProfileScenario,
    pub message: String,
}

pub struct EditProfileRunner;

impl EditProfileRunner {
    pub fn sample_scenario(
        scenario: EditProfileScenario,
    ) -> Result<EditProfileSample, EditProfileError> {
        ProfileFixture::new(scenario)
            .and_then(|mut fixture| fixture.sample())
            .map_err(|message| EditProfileError { scenario, message })
    }
}

struct ProfileFixture {
    scenario: EditProfileScenario,
    root_path: PathBuf,
    root: ResourceRoot,
}

impl ProfileFixture {
    fn new(scenario: EditProfileScenario) -> Result<Self, String> {
        let sequence = PROFILE_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root_path = std::env::temp_dir().join(format!(
            "yach-native-edit-profile-{}-{}-{timestamp_nanos}-{sequence}",
            std::process::id(),
            scenario.label()
        ));
        if root_path.exists() {
            fs::remove_dir_all(&root_path).map_err(|error| error.to_string())?;
        }
        fs::create_dir_all(root_path.join("src")).map_err(|error| error.to_string())?;
        let root = ResourceRoot::project(&root_path)
            .map_err(|_| String::from("failed to create profile resource root"))?;
        let fixture = Self {
            scenario,
            root_path,
            root,
        };
        fixture.seed()?;
        Ok(fixture)
    }

    fn seed(&self) -> Result<(), String> {
        match self.scenario {
            EditProfileScenario::CreateSmallTextFile
            | EditProfileScenario::ValidationFailurePathTraversal => Ok(()),
            EditProfileScenario::ModifySingleHunkSmallFile
            | EditProfileScenario::ApplyFailureHashChanged => {
                self.write("src/edit.rs", "fn demo() {\n    old_profile_text();\n}\n")
            }
            EditProfileScenario::ModifyMultiHunkMediumFile => {
                let mut content = String::new();
                for index in 0..128 {
                    writeln!(&mut content, "line {index:03}: keep")
                        .map_err(|error| error.to_string())?;
                }
                content.push_str("line 128: alpha_profile_text\n");
                content.push_str("line 129: beta_profile_text\n");
                self.write("src/edit.rs", &content)
            }
        }
    }

    fn sample(&mut self) -> Result<EditProfileSample, String> {
        match self.scenario {
            EditProfileScenario::CreateSmallTextFile
            | EditProfileScenario::ModifySingleHunkSmallFile
            | EditProfileScenario::ModifyMultiHunkMediumFile => self.sample_success(),
            EditProfileScenario::ValidationFailurePathTraversal => self.sample_validation_failure(),
            EditProfileScenario::ApplyFailureHashChanged => self.sample_apply_failure(),
        }
    }

    fn sample_success(&mut self) -> Result<EditProfileSample, String> {
        let mut phases = Vec::new();
        let request = self.request()?;
        let policy = EditPolicy::conservative();

        let started = Instant::now();
        let preview = EditEngine::preview(&self.root, request, &policy)
            .map_err(|error| format!("preview failed: {}", edit_error_label(&error)))?;
        phases.push(phase(EditProfilePhase::Preview, started.elapsed()));

        let started = Instant::now();
        let prepared_summary = edit_prepared_evidence_summary(&preview);
        phases.push(phase(
            EditProfilePhase::PreparedEvidenceSummary,
            started.elapsed(),
        ));

        let started = Instant::now();
        let result = EditEngine::apply(&self.root, preview, &policy)
            .map_err(|error| format!("apply failed: {}", edit_error_label(&error)))?;
        phases.push(phase(EditProfilePhase::Apply, started.elapsed()));

        let started = Instant::now();
        let finished_summary = edit_apply_evidence_summary(&result);
        phases.push(phase(
            EditProfilePhase::FinishedEvidenceSummary,
            started.elapsed(),
        ));

        let started = Instant::now();
        let mut log = SessionLog::default();
        log.push(SessionEvent::EditTransactionPrepared {
            session_id: SessionId(String::from("profile-session")),
            turn_id: TurnId(String::from("turn-profile")),
            tool_request_id: None,
            transaction_id: result.transaction_id.clone(),
            summary: prepared_summary,
        });
        log.push(SessionEvent::EditTransactionFinished {
            session_id: SessionId(String::from("profile-session")),
            turn_id: TurnId(String::from("turn-profile")),
            tool_request_id: None,
            transaction_id: Some(result.transaction_id),
            outcome: EditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(finished_summary),
        });
        phases.push(phase(
            EditProfilePhase::SessionAppendEvents,
            started.elapsed(),
        ));

        let mut harness_fixture = ProfileFixture::new(self.scenario)?;
        phases.extend(harness_fixture.sample_harness_success()?);

        Ok(EditProfileSample {
            scenario: self.scenario,
            outcome: EditProfileOutcome::Completed,
            phases,
        })
    }

    fn sample_harness_success(&mut self) -> Result<Vec<EditProfilePhaseDuration>, String> {
        let started = Instant::now();
        let mut log = SessionLog::default();
        EditHarness::preview_and_apply(
            &self.root,
            self.request()?,
            EditPolicy::conservative(),
            &mut log,
            profile_context(),
        )
        .map_err(|error| format!("harness failed: {}", edit_error_label(&error)))?;
        Ok(vec![phase(
            EditProfilePhase::EndToEndHarnessSuccess,
            started.elapsed(),
        )])
    }

    fn sample_validation_failure(&mut self) -> Result<EditProfileSample, String> {
        let mut log = SessionLog::default();
        let started = Instant::now();
        let result = EditHarness::preview_and_apply(
            &self.root,
            self.request()?,
            EditPolicy::conservative(),
            &mut log,
            profile_context(),
        );
        let elapsed = started.elapsed();

        match result {
            Err(EditError::PathTraversal { .. }) => Ok(EditProfileSample {
                scenario: self.scenario,
                outcome: EditProfileOutcome::ExpectedValidationFailure,
                phases: vec![phase(
                    EditProfilePhase::EndToEndHarnessValidationFailure,
                    elapsed,
                )],
            }),
            Ok(_) => Err(String::from("expected path traversal, got success")),
            Err(error) => Err(format!(
                "expected path traversal, got error: {}",
                edit_error_label(&error)
            )),
        }
    }

    fn sample_apply_failure(&mut self) -> Result<EditProfileSample, String> {
        let policy = EditPolicy::conservative();
        let request = self.request()?;
        let preview = EditEngine::preview(&self.root, request, &policy)
            .map_err(|error| format!("preview failed: {}", edit_error_label(&error)))?;
        self.write(
            "src/edit.rs",
            "fn demo() {\n    changed_between_preview_and_apply();\n}\n",
        )?;
        let started = Instant::now();
        let direct_apply = EditEngine::apply(&self.root, preview, &policy);
        let apply_elapsed = started.elapsed();
        match direct_apply {
            Err(EditError::HashMismatch { .. }) => {}
            Ok(_) => return Err(String::from("expected hash mismatch, got success")),
            Err(error) => {
                return Err(format!(
                    "expected hash mismatch, got error: {}",
                    edit_error_label(&error)
                ));
            }
        }

        let harness_fixture = ProfileFixture::new(self.scenario)?;
        let mut log = SessionLog::default();
        let preview_policy = EditPolicy::conservative();
        let apply_policy = EditPolicy {
            allow_modify: false,
            ..EditPolicy::conservative()
        };
        let started = Instant::now();
        let harness_result = EditHarness::preview_and_apply_with_apply_policy(
            &harness_fixture.root,
            harness_fixture.request()?,
            preview_policy,
            apply_policy,
            &mut log,
            profile_context(),
        );
        let harness_elapsed = started.elapsed();
        match harness_result {
            Err(EditError::ModifyDisabled) => {}
            Ok(_) => return Err(String::from("expected modify disabled, got success")),
            Err(error) => {
                return Err(format!(
                    "expected modify disabled, got error: {}",
                    edit_error_label(&error)
                ));
            }
        }

        Ok(EditProfileSample {
            scenario: self.scenario,
            outcome: EditProfileOutcome::ExpectedApplyFailure,
            phases: vec![
                phase(EditProfilePhase::Apply, apply_elapsed),
                phase(
                    EditProfilePhase::EndToEndHarnessApplyFailure,
                    harness_elapsed,
                ),
            ],
        })
    }

    fn request(&self) -> Result<EditTransactionRequest, String> {
        let operation = match self.scenario {
            EditProfileScenario::CreateSmallTextFile => EditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("created profile body\n"),
            },
            EditProfileScenario::ModifySingleHunkSmallFile
            | EditProfileScenario::ApplyFailureHashChanged => EditOperation::ModifyTextFile {
                path: String::from("src/edit.rs"),
                expected_sha256: self.sha256("src/edit.rs")?,
                hunks: vec![EditHunk {
                    find: String::from("old_profile_text"),
                    replace: String::from("replacement_profile_text"),
                }],
            },
            EditProfileScenario::ModifyMultiHunkMediumFile => EditOperation::ModifyTextFile {
                path: String::from("src/edit.rs"),
                expected_sha256: self.sha256("src/edit.rs")?,
                hunks: vec![
                    EditHunk {
                        find: String::from("alpha_profile_text"),
                        replace: String::from("replacement_profile_text_alpha"),
                    },
                    EditHunk {
                        find: String::from("beta_profile_text"),
                        replace: String::from("replacement_profile_text_beta"),
                    },
                ],
            },
            EditProfileScenario::ValidationFailurePathTraversal => EditOperation::CreateTextFile {
                path: String::from("../outside.rs"),
                content: String::from("secret profile payload\n"),
            },
        };
        Ok(EditTransactionRequest {
            operations: vec![operation],
        })
    }

    fn write(&self, relative_path: &str, content: &str) -> Result<(), String> {
        let path = self.root_path.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::write(path, content).map_err(|error| error.to_string())
    }

    fn sha256(&self, relative_path: &str) -> Result<String, String> {
        let bytes =
            fs::read(self.root_path.join(relative_path)).map_err(|error| error.to_string())?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

impl Drop for ProfileFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root_path);
    }
}

fn profile_context() -> crate::edit_harness::EditHarnessContext {
    crate::edit_harness::EditHarnessContext {
        session_id: SessionId(String::from("profile-session")),
        turn_id: TurnId(String::from("turn-profile")),
        tool_request_id: None,
    }
}

fn phase(phase: EditProfilePhase, duration: Duration) -> EditProfilePhaseDuration {
    EditProfilePhaseDuration { phase, duration }
}

#[cfg(test)]
mod tests {
    use super::{EditProfileOutcome, EditProfilePhase, EditProfileRunner, EditProfileScenario};

    #[test]
    fn edit_profile_runner_samples_all_scenarios() {
        for scenario in EditProfileScenario::all() {
            let result = EditProfileRunner::sample_scenario(*scenario);
            assert!(result.is_ok(), "scenario failed: {}", scenario.label());
            let Ok(sample) = result else {
                return;
            };

            assert_eq!(sample.scenario, *scenario);
            assert!(!sample.phases.is_empty());
        }
    }

    #[test]
    fn edit_profile_runner_reports_expected_failure_outcomes() {
        let validation =
            EditProfileRunner::sample_scenario(EditProfileScenario::ValidationFailurePathTraversal);
        assert!(validation.is_ok(), "validation scenario failed");
        let Ok(validation) = validation else {
            return;
        };
        let apply_failure =
            EditProfileRunner::sample_scenario(EditProfileScenario::ApplyFailureHashChanged);
        assert!(apply_failure.is_ok(), "apply failure scenario failed");
        let Ok(apply_failure) = apply_failure else {
            return;
        };

        assert_eq!(
            validation.outcome,
            EditProfileOutcome::ExpectedValidationFailure
        );
        assert_eq!(
            apply_failure.outcome,
            EditProfileOutcome::ExpectedApplyFailure
        );
    }

    #[test]
    fn edit_profile_runner_does_not_expose_fixture_bodies() {
        let sample = EditProfileRunner::sample_scenario(EditProfileScenario::CreateSmallTextFile);
        assert!(sample.is_ok(), "create scenario failed");
        let Ok(sample) = sample else {
            return;
        };
        let debug = format!("{sample:?}");

        assert!(!debug.contains("created profile body"));
        assert!(!debug.contains("secret profile payload"));
        assert!(!debug.contains("replacement_profile_text"));
    }

    #[test]
    fn edit_profile_runner_records_expected_phases() {
        let sample = EditProfileRunner::sample_scenario(EditProfileScenario::CreateSmallTextFile);
        assert!(sample.is_ok(), "create scenario failed");
        let Ok(sample) = sample else {
            return;
        };
        let phases = sample
            .phases
            .iter()
            .map(|phase| phase.phase)
            .collect::<Vec<_>>();

        assert!(phases.contains(&EditProfilePhase::Preview));
        assert!(phases.contains(&EditProfilePhase::PreparedEvidenceSummary));
        assert!(phases.contains(&EditProfilePhase::Apply));
        assert!(phases.contains(&EditProfilePhase::FinishedEvidenceSummary));
        assert!(phases.contains(&EditProfilePhase::SessionAppendEvents));
        assert!(phases.contains(&EditProfilePhase::EndToEndHarnessSuccess));
    }
}
