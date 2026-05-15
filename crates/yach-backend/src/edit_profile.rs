use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::edit_harness::{
    NativeEditHarness, native_edit_apply_evidence_summary, native_edit_prepared_evidence_summary,
};
use crate::{
    NativeEditEngine, NativeEditError, NativeEditEvidenceOutcome, NativeEditHunk,
    NativeEditOperation, NativeEditPolicy, NativeEditTransactionRequest, NativeResourceRoot,
    NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeTurnId, native_edit_error_label,
};

static PROFILE_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeEditProfileScenario {
    CreateSmallTextFile,
    ModifySingleHunkSmallFile,
    ModifyMultiHunkMediumFile,
    ValidationFailurePathTraversal,
    ApplyFailureHashChanged,
}

const ALL_NATIVE_EDIT_PROFILE_SCENARIOS: [NativeEditProfileScenario; 5] = [
    NativeEditProfileScenario::CreateSmallTextFile,
    NativeEditProfileScenario::ModifySingleHunkSmallFile,
    NativeEditProfileScenario::ModifyMultiHunkMediumFile,
    NativeEditProfileScenario::ValidationFailurePathTraversal,
    NativeEditProfileScenario::ApplyFailureHashChanged,
];

impl NativeEditProfileScenario {
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
pub enum NativeEditProfilePhase {
    Preview,
    PreparedEvidenceSummary,
    Apply,
    FinishedEvidenceSummary,
    SessionAppendEvents,
    EndToEndHarnessSuccess,
    EndToEndHarnessValidationFailure,
    EndToEndHarnessApplyFailure,
}

impl NativeEditProfilePhase {
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
pub enum NativeEditProfileOutcome {
    Completed,
    ExpectedValidationFailure,
    ExpectedApplyFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditProfilePhaseDuration {
    pub phase: NativeEditProfilePhase,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditProfileSample {
    pub scenario: NativeEditProfileScenario,
    pub outcome: NativeEditProfileOutcome,
    pub phases: Vec<NativeEditProfilePhaseDuration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeEditProfileError {
    pub scenario: NativeEditProfileScenario,
    pub message: String,
}

pub struct NativeEditProfileRunner;

impl NativeEditProfileRunner {
    pub fn sample_scenario(
        scenario: NativeEditProfileScenario,
    ) -> Result<NativeEditProfileSample, NativeEditProfileError> {
        ProfileFixture::new(scenario)
            .and_then(|mut fixture| fixture.sample())
            .map_err(|message| NativeEditProfileError { scenario, message })
    }
}

struct ProfileFixture {
    scenario: NativeEditProfileScenario,
    root_path: PathBuf,
    root: NativeResourceRoot,
}

impl ProfileFixture {
    fn new(scenario: NativeEditProfileScenario) -> Result<Self, String> {
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
        let root = NativeResourceRoot::project(&root_path)
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
            NativeEditProfileScenario::CreateSmallTextFile
            | NativeEditProfileScenario::ValidationFailurePathTraversal => Ok(()),
            NativeEditProfileScenario::ModifySingleHunkSmallFile
            | NativeEditProfileScenario::ApplyFailureHashChanged => {
                self.write("src/edit.rs", "fn demo() {\n    old_profile_text();\n}\n")
            }
            NativeEditProfileScenario::ModifyMultiHunkMediumFile => {
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

    fn sample(&mut self) -> Result<NativeEditProfileSample, String> {
        match self.scenario {
            NativeEditProfileScenario::CreateSmallTextFile
            | NativeEditProfileScenario::ModifySingleHunkSmallFile
            | NativeEditProfileScenario::ModifyMultiHunkMediumFile => self.sample_success(),
            NativeEditProfileScenario::ValidationFailurePathTraversal => {
                self.sample_validation_failure()
            }
            NativeEditProfileScenario::ApplyFailureHashChanged => self.sample_apply_failure(),
        }
    }

    fn sample_success(&mut self) -> Result<NativeEditProfileSample, String> {
        let mut phases = Vec::new();
        let request = self.request()?;
        let policy = NativeEditPolicy::conservative();

        let started = Instant::now();
        let preview = NativeEditEngine::preview(&self.root, request, &policy)
            .map_err(|error| format!("preview failed: {}", native_edit_error_label(&error)))?;
        phases.push(phase(NativeEditProfilePhase::Preview, started.elapsed()));

        let started = Instant::now();
        let prepared_summary = native_edit_prepared_evidence_summary(&preview);
        phases.push(phase(
            NativeEditProfilePhase::PreparedEvidenceSummary,
            started.elapsed(),
        ));

        let started = Instant::now();
        let result = NativeEditEngine::apply(&self.root, preview, &policy)
            .map_err(|error| format!("apply failed: {}", native_edit_error_label(&error)))?;
        phases.push(phase(NativeEditProfilePhase::Apply, started.elapsed()));

        let started = Instant::now();
        let finished_summary = native_edit_apply_evidence_summary(&result);
        phases.push(phase(
            NativeEditProfilePhase::FinishedEvidenceSummary,
            started.elapsed(),
        ));

        let started = Instant::now();
        let mut log = NativeSessionLog::default();
        log.push(NativeSessionEvent::EditTransactionPrepared {
            session_id: NativeSessionId(String::from("profile-session")),
            turn_id: NativeTurnId(String::from("turn-profile")),
            tool_request_id: None,
            transaction_id: result.transaction_id.clone(),
            summary: prepared_summary,
        });
        log.push(NativeSessionEvent::EditTransactionFinished {
            session_id: NativeSessionId(String::from("profile-session")),
            turn_id: NativeTurnId(String::from("turn-profile")),
            tool_request_id: None,
            transaction_id: Some(result.transaction_id),
            outcome: NativeEditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(finished_summary),
        });
        phases.push(phase(
            NativeEditProfilePhase::SessionAppendEvents,
            started.elapsed(),
        ));

        let mut harness_fixture = ProfileFixture::new(self.scenario)?;
        phases.extend(harness_fixture.sample_harness_success()?);

        Ok(NativeEditProfileSample {
            scenario: self.scenario,
            outcome: NativeEditProfileOutcome::Completed,
            phases,
        })
    }

    fn sample_harness_success(&mut self) -> Result<Vec<NativeEditProfilePhaseDuration>, String> {
        let started = Instant::now();
        let mut log = NativeSessionLog::default();
        NativeEditHarness::preview_and_apply(
            &self.root,
            self.request()?,
            NativeEditPolicy::conservative(),
            &mut log,
            profile_context(),
        )
        .map_err(|error| format!("harness failed: {}", native_edit_error_label(&error)))?;
        Ok(vec![phase(
            NativeEditProfilePhase::EndToEndHarnessSuccess,
            started.elapsed(),
        )])
    }

    fn sample_validation_failure(&mut self) -> Result<NativeEditProfileSample, String> {
        let mut log = NativeSessionLog::default();
        let started = Instant::now();
        let result = NativeEditHarness::preview_and_apply(
            &self.root,
            self.request()?,
            NativeEditPolicy::conservative(),
            &mut log,
            profile_context(),
        );
        let elapsed = started.elapsed();

        match result {
            Err(NativeEditError::PathTraversal { .. }) => Ok(NativeEditProfileSample {
                scenario: self.scenario,
                outcome: NativeEditProfileOutcome::ExpectedValidationFailure,
                phases: vec![phase(
                    NativeEditProfilePhase::EndToEndHarnessValidationFailure,
                    elapsed,
                )],
            }),
            Ok(_) => Err(String::from("expected path traversal, got success")),
            Err(error) => Err(format!(
                "expected path traversal, got error: {}",
                native_edit_error_label(&error)
            )),
        }
    }

    fn sample_apply_failure(&mut self) -> Result<NativeEditProfileSample, String> {
        let policy = NativeEditPolicy::conservative();
        let request = self.request()?;
        let preview = NativeEditEngine::preview(&self.root, request, &policy)
            .map_err(|error| format!("preview failed: {}", native_edit_error_label(&error)))?;
        self.write(
            "src/edit.rs",
            "fn demo() {\n    changed_between_preview_and_apply();\n}\n",
        )?;
        let started = Instant::now();
        let direct_apply = NativeEditEngine::apply(&self.root, preview, &policy);
        let apply_elapsed = started.elapsed();
        match direct_apply {
            Err(NativeEditError::HashMismatch { .. }) => {}
            Ok(_) => return Err(String::from("expected hash mismatch, got success")),
            Err(error) => {
                return Err(format!(
                    "expected hash mismatch, got error: {}",
                    native_edit_error_label(&error)
                ));
            }
        }

        let harness_fixture = ProfileFixture::new(self.scenario)?;
        let mut log = NativeSessionLog::default();
        let preview_policy = NativeEditPolicy::conservative();
        let apply_policy = NativeEditPolicy {
            allow_modify: false,
            ..NativeEditPolicy::conservative()
        };
        let started = Instant::now();
        let harness_result = NativeEditHarness::preview_and_apply_with_apply_policy(
            &harness_fixture.root,
            harness_fixture.request()?,
            preview_policy,
            apply_policy,
            &mut log,
            profile_context(),
        );
        let harness_elapsed = started.elapsed();
        match harness_result {
            Err(NativeEditError::ModifyDisabled) => {}
            Ok(_) => return Err(String::from("expected modify disabled, got success")),
            Err(error) => {
                return Err(format!(
                    "expected modify disabled, got error: {}",
                    native_edit_error_label(&error)
                ));
            }
        }

        Ok(NativeEditProfileSample {
            scenario: self.scenario,
            outcome: NativeEditProfileOutcome::ExpectedApplyFailure,
            phases: vec![
                phase(NativeEditProfilePhase::Apply, apply_elapsed),
                phase(
                    NativeEditProfilePhase::EndToEndHarnessApplyFailure,
                    harness_elapsed,
                ),
            ],
        })
    }

    fn request(&self) -> Result<NativeEditTransactionRequest, String> {
        let operation = match self.scenario {
            NativeEditProfileScenario::CreateSmallTextFile => NativeEditOperation::CreateTextFile {
                path: String::from("src/new.rs"),
                content: String::from("created profile body\n"),
            },
            NativeEditProfileScenario::ModifySingleHunkSmallFile
            | NativeEditProfileScenario::ApplyFailureHashChanged => {
                NativeEditOperation::ModifyTextFile {
                    path: String::from("src/edit.rs"),
                    expected_sha256: self.sha256("src/edit.rs")?,
                    hunks: vec![NativeEditHunk {
                        find: String::from("old_profile_text"),
                        replace: String::from("replacement_profile_text"),
                    }],
                }
            }
            NativeEditProfileScenario::ModifyMultiHunkMediumFile => {
                NativeEditOperation::ModifyTextFile {
                    path: String::from("src/edit.rs"),
                    expected_sha256: self.sha256("src/edit.rs")?,
                    hunks: vec![
                        NativeEditHunk {
                            find: String::from("alpha_profile_text"),
                            replace: String::from("replacement_profile_text_alpha"),
                        },
                        NativeEditHunk {
                            find: String::from("beta_profile_text"),
                            replace: String::from("replacement_profile_text_beta"),
                        },
                    ],
                }
            }
            NativeEditProfileScenario::ValidationFailurePathTraversal => {
                NativeEditOperation::CreateTextFile {
                    path: String::from("../outside.rs"),
                    content: String::from("secret profile payload\n"),
                }
            }
        };
        Ok(NativeEditTransactionRequest {
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

fn profile_context() -> crate::edit_harness::NativeEditHarnessContext {
    crate::edit_harness::NativeEditHarnessContext {
        session_id: NativeSessionId(String::from("profile-session")),
        turn_id: NativeTurnId(String::from("turn-profile")),
        tool_request_id: None,
    }
}

fn phase(phase: NativeEditProfilePhase, duration: Duration) -> NativeEditProfilePhaseDuration {
    NativeEditProfilePhaseDuration { phase, duration }
}

#[cfg(test)]
mod tests {
    use super::{
        NativeEditProfileOutcome, NativeEditProfilePhase, NativeEditProfileRunner,
        NativeEditProfileScenario,
    };

    #[test]
    fn native_edit_profile_runner_samples_all_scenarios() {
        for scenario in NativeEditProfileScenario::all() {
            let result = NativeEditProfileRunner::sample_scenario(*scenario);
            assert!(result.is_ok(), "scenario failed: {}", scenario.label());
            let Ok(sample) = result else {
                return;
            };

            assert_eq!(sample.scenario, *scenario);
            assert!(!sample.phases.is_empty());
        }
    }

    #[test]
    fn native_edit_profile_runner_reports_expected_failure_outcomes() {
        let validation = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::ValidationFailurePathTraversal,
        );
        assert!(validation.is_ok(), "validation scenario failed");
        let Ok(validation) = validation else {
            return;
        };
        let apply_failure = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::ApplyFailureHashChanged,
        );
        assert!(apply_failure.is_ok(), "apply failure scenario failed");
        let Ok(apply_failure) = apply_failure else {
            return;
        };

        assert_eq!(
            validation.outcome,
            NativeEditProfileOutcome::ExpectedValidationFailure
        );
        assert_eq!(
            apply_failure.outcome,
            NativeEditProfileOutcome::ExpectedApplyFailure
        );
    }

    #[test]
    fn native_edit_profile_runner_does_not_expose_fixture_bodies() {
        let sample = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::CreateSmallTextFile,
        );
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
    fn native_edit_profile_runner_records_expected_phases() {
        let sample = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::CreateSmallTextFile,
        );
        assert!(sample.is_ok(), "create scenario failed");
        let Ok(sample) = sample else {
            return;
        };
        let phases = sample
            .phases
            .iter()
            .map(|phase| phase.phase)
            .collect::<Vec<_>>();

        assert!(phases.contains(&NativeEditProfilePhase::Preview));
        assert!(phases.contains(&NativeEditProfilePhase::PreparedEvidenceSummary));
        assert!(phases.contains(&NativeEditProfilePhase::Apply));
        assert!(phases.contains(&NativeEditProfilePhase::FinishedEvidenceSummary));
        assert!(phases.contains(&NativeEditProfilePhase::SessionAppendEvents));
        assert!(phases.contains(&NativeEditProfilePhase::EndToEndHarnessSuccess));
    }
}
