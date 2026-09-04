# Native Edit Benchmark And Profiling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add benchmark-only native edit profiling with Criterion coverage and a granular `yach-bench native-edit-profile-report` command.

**Architecture:** Add a `bench` feature to `yach-backend` that exposes a narrow `edit_profile` module for deterministic native edit profiling scenarios. `yach-bench` opts into that feature, adds a `native_edit` Criterion bench target, and adds a report mode that renders p50/p95/p99/max phase summaries. The implementation does not make `NativeEditEngine::apply` public, does not register mutation tools, and does not add production tracing.

**Tech Stack:** Rust 2024, `yach-backend`, `yach-bench`, Criterion, existing `LatencySummary`, `just dev cargo ...`.

---

## Source Spec

- `docs/project/specs/2026-05-15-native-edit-benchmark-trace-design.md`

## File Structure

Implementation files:

- Modify `crates/yach-backend/Cargo.toml`: add a disabled-by-default `bench` feature.
- Create `crates/yach-backend/src/edit_profile.rs`: feature-gated benchmark-facing edit profile runner, scenarios, phases, sample outcomes, errors, and tests.
- Modify `crates/yach-backend/src/lib.rs`: feature-gate and publicly expose only the `edit_profile` module when `bench` is enabled.
- Modify `crates/yach-bench/Cargo.toml`: add the `native_edit` Criterion bench target and enable the backend `bench` feature for `yach-bench`.
- Create `crates/yach-bench/benches/native_edit.rs`: Criterion microbenchmarks for preview, evidence summary, apply, session append, and end-to-end harness paths.
- Modify `crates/yach-bench/src/main.rs`: add `native-edit-profile-report --samples N`, render edit phase summaries, and add focused unit tests for labels/privacy.
- Create `docs/benchmarks/native-edit-profile-2026-05-15.md`: record the first local report output and methodology.
- Modify `docs/benchmarks/README.md`: index the new report command and report file.
- Modify `docs/project/state.md` and `docs/project/next.md`: update only after implementation changes the current status and recommended next move.

No production CLI/TUI edit command, provider-visible tool schema, native tool registration, extension mutation, approval UI, or production tracing file is part of this plan.

## Task 1: Backend Bench Feature And Profile Runner

**Files:**
- Modify: `crates/yach-backend/Cargo.toml`
- Modify: `crates/yach-backend/src/lib.rs`
- Create: `crates/yach-backend/src/edit_profile.rs`

- [ ] **Step 1: Add the disabled-by-default backend bench feature**

In `crates/yach-backend/Cargo.toml`, insert this section before `[dependencies]`:

```toml
[features]
default = []
bench = []
```

- [ ] **Step 2: Wire a feature-gated edit profile module**

In `crates/yach-backend/src/lib.rs`, add this after `mod edit_harness;`:

```rust
#[cfg(feature = "bench")]
pub mod edit_profile;
```

Do not add a normal public re-export for `NativeEditHarness`, `NativeEditEngine::apply`, `native_edit_prepared_evidence_summary`, or `native_edit_apply_evidence_summary`.

- [ ] **Step 3: Create the failing profile runner tests**

Create `crates/yach-backend/src/edit_profile.rs` with these tests first. The test module intentionally references the API the task will implement.

```rust
#[cfg(test)]
mod tests {
    use super::{
        NativeEditProfileOutcome, NativeEditProfilePhase, NativeEditProfileRunner,
        NativeEditProfileScenario,
    };

    #[test]
    fn native_edit_profile_runner_samples_all_scenarios() {
        for scenario in NativeEditProfileScenario::all() {
            let sample = NativeEditProfileRunner::sample_scenario(*scenario).unwrap();

            assert_eq!(sample.scenario, *scenario);
            assert!(!sample.phases.is_empty());
        }
    }

    #[test]
    fn native_edit_profile_runner_reports_expected_failure_outcomes() {
        let validation = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::ValidationFailurePathTraversal,
        )
        .unwrap();
        let apply_failure = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::ApplyFailureHashChanged,
        )
        .unwrap();

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
        )
        .unwrap();
        let debug = format!("{sample:?}");

        assert!(!debug.contains("created profile body"));
        assert!(!debug.contains("secret profile payload"));
        assert!(!debug.contains("replacement_profile_text"));
    }

    #[test]
    fn native_edit_profile_runner_records_expected_phases() {
        let sample = NativeEditProfileRunner::sample_scenario(
            NativeEditProfileScenario::CreateSmallTextFile,
        )
        .unwrap();
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
```

- [ ] **Step 4: Run the feature-gated backend tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-backend --features bench native_edit_profile_runner_ -- --nocapture
```

Expected: compile failure for missing `NativeEditProfileRunner`, `NativeEditProfileScenario`, `NativeEditProfileOutcome`, and `NativeEditProfilePhase`.

- [ ] **Step 5: Add the profile runner API and fixtures**

Add the implementation above the test module in `crates/yach-backend/src/edit_profile.rs`:

```rust
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::edit_harness::{
    NativeEditHarness, native_edit_apply_evidence_summary, native_edit_prepared_evidence_summary,
};
use crate::{
    NativeEditEngine, NativeEditError, NativeEditHunk, NativeEditOperation, NativeEditPolicy,
    NativeEditTransactionRequest, NativeResourceRoot, NativeSessionEvent, NativeSessionId,
    NativeSessionLog, NativeTurnId,
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
        let root_path = std::env::temp_dir().join(format!(
            "yach-native-edit-profile-{}-{}-{sequence}",
            std::process::id(),
            scenario.label()
        ));
        fs::create_dir_all(root_path.join("src")).map_err(|error| error.to_string())?;
        let root = NativeResourceRoot::project(&root_path).map_err(|error| format!("{error:?}"))?;
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
                    content.push_str(&format!("line {index:03}: keep\n"));
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
            .map_err(|error| format!("{error:?}"))?;
        phases.push(phase(NativeEditProfilePhase::Preview, started.elapsed()));

        let started = Instant::now();
        let prepared_summary = native_edit_prepared_evidence_summary(&preview);
        phases.push(phase(
            NativeEditProfilePhase::PreparedEvidenceSummary,
            started.elapsed(),
        ));

        let started = Instant::now();
        let result = NativeEditEngine::apply(&self.root, preview, &policy)
            .map_err(|error| format!("{error:?}"))?;
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
            outcome: crate::NativeEditEvidenceOutcome::Completed,
            reason: None,
            summary: Some(finished_summary),
        });
        phases.push(phase(
            NativeEditProfilePhase::SessionAppendEvents,
            started.elapsed(),
        ));

        let mut harness_fixture = ProfileFixture::new(self.scenario)?;
        let end_to_end = harness_fixture.sample_harness_success()?;
        phases.extend(end_to_end);

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
        .map_err(|error| format!("{error:?}"))?;
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
            other => Err(format!("expected path traversal, got {other:?}")),
        }
    }

    fn sample_apply_failure(&mut self) -> Result<NativeEditProfileSample, String> {
        let policy = NativeEditPolicy::conservative();
        let request = self.request()?;
        let preview = NativeEditEngine::preview(&self.root, request, &policy)
            .map_err(|error| format!("{error:?}"))?;
        self.write("src/edit.rs", "fn demo() {\n    changed_between_preview_and_apply();\n}\n")?;
        let started = Instant::now();
        let direct_apply = NativeEditEngine::apply(&self.root, preview, &policy);
        let apply_elapsed = started.elapsed();
        if !matches!(direct_apply, Err(NativeEditError::HashMismatch { .. })) {
            return Err(format!("expected direct hash mismatch, got {direct_apply:?}"));
        }

        let mut harness_fixture = ProfileFixture::new(self.scenario)?;
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
        if !matches!(harness_result, Err(NativeEditError::ModifyDisabled)) {
            return Err(format!(
                "expected harness modify disabled, got {harness_result:?}"
            ));
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
        let bytes = fs::read(self.root_path.join(relative_path)).map_err(|error| error.to_string())?;
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

fn phase(
    phase: NativeEditProfilePhase,
    duration: Duration,
) -> NativeEditProfilePhaseDuration {
    NativeEditProfilePhaseDuration { phase, duration }
}
```

The direct `apply` phase in `ApplyFailureHashChanged` uses a deterministic
hash-mismatch by changing the file after preview. The end-to-end harness
apply-failure phase in that same scenario uses a denied apply policy, because
`NativeEditHarness::preview_and_apply_with_apply_policy` intentionally has no
hook for mutating the target between preview and apply. Both failures occur
after a prepared transaction exists and exercise the finished-failed evidence
path.

- [ ] **Step 6: Run focused backend profile tests**

Run:

```bash
just dev cargo test -p yach-backend --features bench native_edit_profile_runner_ -- --nocapture
```

Expected: all four `native_edit_profile_runner_...` tests pass.

- [ ] **Step 7: Run backend tests with and without the bench feature**

Run:

```bash
just dev cargo test -p yach-backend --features bench
just dev cargo test -p yach-backend
```

Expected: both test suites pass. The non-feature test proves normal backend builds do not require the benchmark-only module.

## Task 2: Native Edit Criterion Benchmarks

**Files:**
- Modify: `crates/yach-bench/Cargo.toml`
- Create: `crates/yach-bench/benches/native_edit.rs`

- [ ] **Step 1: Add the native edit bench target and enable backend bench feature**

In `crates/yach-bench/Cargo.toml`, add the bench target after `native_static_context`:

```toml
[[bench]]
name = "native_edit"
harness = false
```

Change the `yach-backend` dependency to:

```toml
yach-backend = { path = "../yach-backend", features = ["bench"] }
```

- [ ] **Step 2: Add the native edit Criterion bench**

Create `crates/yach-bench/benches/native_edit.rs`:

```rust
use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::edit_profile::{
    NativeEditProfilePhase, NativeEditProfileRunner, NativeEditProfileScenario,
};

fn bench_native_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_edit");
    let phases = [
        NativeEditProfilePhase::Preview,
        NativeEditProfilePhase::PreparedEvidenceSummary,
        NativeEditProfilePhase::Apply,
        NativeEditProfilePhase::FinishedEvidenceSummary,
        NativeEditProfilePhase::SessionAppendEvents,
        NativeEditProfilePhase::EndToEndHarnessSuccess,
        NativeEditProfilePhase::EndToEndHarnessValidationFailure,
        NativeEditProfilePhase::EndToEndHarnessApplyFailure,
    ];

    for scenario in NativeEditProfileScenario::all() {
        group.bench_function(format!("{}/sample", scenario.label()), |b| {
            b.iter_batched(
                || *scenario,
                |scenario| {
                    black_box(
                        NativeEditProfileRunner::sample_scenario(black_box(scenario))
                            .expect("native edit profile sample should succeed"),
                    );
                },
                BatchSize::SmallInput,
            );
        });

        for phase in phases {
            if profile_phase_duration(*scenario, phase).is_some() {
                group.bench_function(format!("{}/{}", scenario.label(), phase.label()), |b| {
                    b.iter_custom(|iterations| {
                        let mut total = std::time::Duration::ZERO;
                        for _ in 0..iterations {
                            total += profile_phase_duration(black_box(*scenario), black_box(phase))
                                .expect("phase should be present for scenario");
                        }
                        total
                    });
                });
            }
        }
    }

    group.finish();
}

fn profile_phase_duration(
    scenario: NativeEditProfileScenario,
    phase: NativeEditProfilePhase,
) -> Option<std::time::Duration> {
    NativeEditProfileRunner::sample_scenario(scenario)
        .ok()?
        .phases
        .into_iter()
        .find(|duration| duration.phase == phase)
        .map(|duration| duration.duration)
}

criterion_group!(benches, bench_native_edit);
criterion_main!(benches);
```

- [ ] **Step 3: Run the Criterion target in compile-test mode**

Run:

```bash
just dev cargo bench -p yach-bench --bench native_edit -- --test
```

Expected: the Criterion bench binary compiles and reports 0 failed tests.

- [ ] **Step 4: Run yach-bench tests to catch dependency feature issues**

Run:

```bash
just dev cargo test -p yach-bench
```

Expected: existing yach-bench unit tests pass with `yach-backend/bench` enabled.

## Task 3: Native Edit Profile Report Command

**Files:**
- Modify: `crates/yach-bench/src/main.rs`

- [ ] **Step 1: Add failing unit tests for the report output**

In the existing `#[cfg(test)] mod tests` in `crates/yach-bench/src/main.rs`, add:

```rust
    #[test]
    fn native_edit_profile_report_emits_expected_workloads() {
        let lines = native_edit_profile_report_lines(1);
        let joined = lines.join("\n");

        assert!(joined.contains("samples_requested=1"));
        assert!(joined.contains("samples_collected=1"));
        assert!(joined.contains("workload=native_edit/create_small_text_file/preview"));
        assert!(joined.contains(
            "workload=native_edit/create_small_text_file/end_to_end_harness_success"
        ));
        assert!(joined.contains(
            "workload=native_edit/validation_failure_path_traversal/end_to_end_harness_validation_failure"
        ));
        assert!(joined.contains(
            "workload=native_edit/apply_failure_hash_changed/end_to_end_harness_apply_failure"
        ));
    }

    #[test]
    fn native_edit_profile_report_does_not_emit_file_bodies() {
        let lines = native_edit_profile_report_lines(1);
        let joined = lines.join("\n");

        assert!(!joined.contains("created profile body"));
        assert!(!joined.contains("secret profile payload"));
        assert!(!joined.contains("replacement_profile_text"));
    }
```

- [ ] **Step 2: Run the focused yach-bench tests and verify they fail**

Run:

```bash
just dev cargo test -p yach-bench native_edit_profile_report_ -- --nocapture
```

Expected: compile failure for missing `native_edit_profile_report_lines`.

- [ ] **Step 3: Import edit profile types**

Near the existing imports at the top of `crates/yach-bench/src/main.rs`, add:

```rust
use yach_backend::edit_profile::{
    NativeEditProfilePhase, NativeEditProfileRunner, NativeEditProfileScenario,
};
```

- [ ] **Step 4: Add the report command dispatch and usage text**

In the `match args.first().map(String::as_str)` in `main`, add this arm before `_ => usage_lines()`:

```rust
        Some("native-edit-profile-report") => native_edit_profile_report_lines(sample_count(&args)),
```

Update `usage_lines()` so the string includes `native-edit-profile-report`:

```rust
        "usage: yach-bench headless-report|terminal-report|terminal-keypress-report|terminal-active-stream-report|terminal-stream-backlog-report|terminal-async-backlog-report|terminal-async-backlog-stress-report|terminal-heavy-output-report|terminal-transcript-scroll-report|terminal-transcript-scroll-stress-report|pi-transcript-fixture|pi-clean-startup-report|yach-cli-startup-report|yach-tui-startup-report|yach-tui-startup-profile-report|yach-tui-startup-profile-with-inactive-extension-report|yach-tui-ready-startup-report|native-edit-profile-report [--samples N]",
```

- [ ] **Step 5: Add report collection helpers**

Add these helpers near the other report-line functions in `crates/yach-bench/src/main.rs`:

```rust
fn native_edit_profile_report_lines(samples: usize) -> Vec<String> {
    let mut collected = 0_usize;
    let mut errors = Vec::new();
    let mut phase_samples: BTreeMap<String, Vec<Duration>> = BTreeMap::new();

    for _ in 0..samples {
        let mut sample_failed = false;
        for scenario in NativeEditProfileScenario::all() {
            match NativeEditProfileRunner::sample_scenario(*scenario) {
                Ok(sample) => {
                    for phase in sample.phases {
                        let label = native_edit_phase_label(sample.scenario, phase.phase);
                        phase_samples.entry(label).or_default().push(phase.duration);
                    }
                }
                Err(error) => {
                    sample_failed = true;
                    errors.push(format!("{}: {}", error.scenario.label(), error.message));
                }
            }
        }
        if !sample_failed {
            collected += 1;
        }
    }

    let mut lines = vec![format!("samples_requested={samples}")];
    lines.push(format!("samples_collected={collected}"));
    if !errors.is_empty() {
        lines.push(format!("errors={}", errors.len()));
        if let Some(first_error) = errors.first() {
            lines.push(format!("first_error={first_error}"));
        }
    }

    for (label, samples) in phase_samples {
        lines.push(render_summary(
            &label,
            &LatencySummary::from_samples(None, &samples),
        ));
    }

    lines
}

fn native_edit_phase_label(
    scenario: NativeEditProfileScenario,
    phase: NativeEditProfilePhase,
) -> String {
    format!("native_edit/{}/{}", scenario.label(), phase.label())
}
```

- [ ] **Step 6: Run focused report tests**

Run:

```bash
just dev cargo test -p yach-bench native_edit_profile_report_ -- --nocapture
```

Expected: both `native_edit_profile_report_...` tests pass.

- [ ] **Step 7: Run the report command in debug mode**

Run:

```bash
just dev cargo run -p yach-bench -- native-edit-profile-report --samples 1
```

Expected: output includes `samples_requested=1`, `samples_collected=1`, and `workload=native_edit/...` lines for all scenarios. It must not include synthetic fixture bodies.

## Task 4: Benchmark Report And Documentation

**Files:**
- Create: `docs/benchmarks/native-edit-profile-2026-05-15.md`
- Modify: `docs/benchmarks/README.md`
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run release profile samples**

Run:

```bash
just dev cargo run -p yach-bench --release -- native-edit-profile-report --samples 5
```

Expected: command succeeds and prints p50/p95/p99/max summaries. Save the output for the next step.

- [ ] **Step 2: Add the benchmark report**

Create `docs/benchmarks/native-edit-profile-2026-05-15.md` with this shape. In the `Results` section, include a `text` fenced block containing the complete stdout from Step 1.

```markdown
# Native Edit Profile - 2026-05-15

## Summary

This report records the first native edit profiling baseline after preview,
guarded apply, redacted evidence, and the backend-local harness were merged.

The command profiles deterministic synthetic edit scenarios. It is not a Pi
comparison and does not exercise CLI/TUI/provider edit UX.

## Environment

- Date: 2026-05-15
- Branch: native-edit-profile-impl
- Build/profile mode: release `yach-bench`
- Machine: local macOS development machine

## Command

```bash
just dev cargo run -p yach-bench --release -- native-edit-profile-report --samples 5
```

## Results

## Interpretation

The profile separates native edit preview, apply, redacted evidence summary,
session append, and end-to-end harness paths. Treat these numbers as a local
baseline for future edit UX work, not as product latency claims or external
harness comparisons.
```

- [ ] **Step 3: Update benchmark README**

In `docs/benchmarks/README.md`, add the command bullet near the existing `yach-bench` command list:

```markdown
- `cargo run -p yach-bench --release -- native-edit-profile-report --samples N` — native edit profile sampler for preview, apply, evidence summary, session append, and end-to-end harness phases. Uses synthetic local fixtures and does not expose edit UX or provider-visible mutation.
```

In the `Current reports` list, add:

```markdown
- `native-edit-profile-2026-05-15.md` — first local native edit preview/apply/evidence/session-append profiling baseline. Synthetic edit fixtures only; not a Pi comparison or user-facing edit latency claim.
```

- [ ] **Step 4: Update active project docs**

In `docs/project/state.md`, update the native edit posture to mention that native edit profiling now has Criterion and report-mode coverage.

In `docs/project/next.md`, change the recommended next move away from edit profiling implementation. Recommended wording:

```markdown
Recommended next move: design local CLI/TUI edit access on top of the native edit transaction and evidence boundary.

Why: native edit preview, guarded apply, redacted evidence, backend-local harnessing, and profiling are now in place. The next Native MVP gap is deciding how users initiate, review, approve, and inspect local edits without exposing provider-visible mutation prematurely.
```

Keep provider-advertised edit tools, extension-owned mutation, hidden built-ins, delete/rename, shell/process tools, and network tools in the "Not Ready Without a New Spec" section.

## Task 5: Final Verification And Commit

**Files:**
- Modify: `crates/yach-backend/Cargo.toml`
- Create: `crates/yach-backend/src/edit_profile.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-bench/Cargo.toml`
- Create: `crates/yach-bench/benches/native_edit.rs`
- Modify: `crates/yach-bench/src/main.rs`
- Create: `docs/benchmarks/native-edit-profile-2026-05-15.md`
- Modify: `docs/benchmarks/README.md`
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Run formatting**

Run:

```bash
just dev cargo fmt
just dev cargo fmt --check
```

Expected: formatting check passes.

- [ ] **Step 2: Run focused tests and smoke checks**

Run:

```bash
just dev cargo test -p yach-backend --features bench native_edit_profile_runner_ -- --nocapture
just dev cargo test -p yach-bench native_edit_profile_report_ -- --nocapture
just dev cargo run -p yach-bench -- native-edit-profile-report --samples 1
```

Expected: focused tests pass and the report command emits non-empty native edit workload summaries.

- [ ] **Step 3: Run package tests and Criterion compile check**

Run:

```bash
just dev cargo test -p yach-backend --features bench
just dev cargo test -p yach-backend
just dev cargo test -p yach-bench
just dev cargo bench -p yach-bench --bench native_edit -- --test
```

Expected: all commands pass.

- [ ] **Step 4: Run final release report and lint checks**

Run:

```bash
just dev cargo run -p yach-bench --release -- native-edit-profile-report --samples 5
just dev cargo clippy -p yach-backend --features bench --lib -- -D warnings
just dev cargo clippy -p yach-bench --all-targets -- -D warnings
git diff --check
```

Expected: release report succeeds, clippy passes with warnings denied, and whitespace check passes.

- [ ] **Step 5: Commit**

Run:

```bash
git add crates/yach-backend/Cargo.toml crates/yach-backend/src/edit_profile.rs crates/yach-backend/src/lib.rs crates/yach-bench/Cargo.toml crates/yach-bench/benches/native_edit.rs crates/yach-bench/src/main.rs docs/benchmarks/native-edit-profile-2026-05-15.md docs/benchmarks/README.md docs/project/state.md docs/project/next.md
PREK_ALLOW_NO_CONFIG=1 git -c commit.gpgsign=false commit -m "Profile native edit transactions"
```

Expected: one implementation commit.

## Follow-Up Slices

- Use the profile results to decide whether any edit preview/apply/evidence optimization is warranted.
- Design local CLI/TUI edit access and approval UX.
- Design hidden built-in edit tools only after local user-facing semantics are clear.
- Design provider-visible edit/write tools separately.
- Design extension-owned mutation capabilities separately.
