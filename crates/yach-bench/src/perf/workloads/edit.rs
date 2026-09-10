use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use yach_backend::edit_profile::{
    EditProfilePhase, EditProfileRunner, EditProfileSample, EditProfileScenario,
};

use crate::perf::registry::{Measured, RunCtx, Workload};
use crate::perf::schema::{Class, Isolation};

macro_rules! edit_workload {
    ($id:literal, $scenario:expr, $phase:expr) => {
        Workload {
            id: $id,
            class: Class::Latency,
            isolation: Isolation::InProcessSerial,
            requires: &[],
            bin: None,
            run: |ctx| run_edit_phase(ctx, $scenario, $phase),
            emit_alloc: false,
        }
    };
}

pub static EDIT: [Workload; 21] = [
    edit_workload!(
        "native_edit/create_small_text_file/preview",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::Preview
    ),
    edit_workload!(
        "native_edit/create_small_text_file/prepared_evidence_summary",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::PreparedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/create_small_text_file/apply",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::Apply
    ),
    edit_workload!(
        "native_edit/create_small_text_file/finished_evidence_summary",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::FinishedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/create_small_text_file/session_append_events",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::SessionAppendEvents
    ),
    edit_workload!(
        "native_edit/create_small_text_file/end_to_end_harness_success",
        EditProfileScenario::CreateSmallTextFile,
        EditProfilePhase::EndToEndHarnessSuccess
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/preview",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::Preview
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/prepared_evidence_summary",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::PreparedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/apply",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::Apply
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/finished_evidence_summary",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::FinishedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/session_append_events",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::SessionAppendEvents
    ),
    edit_workload!(
        "native_edit/modify_single_hunk_small_file/end_to_end_harness_success",
        EditProfileScenario::ModifySingleHunkSmallFile,
        EditProfilePhase::EndToEndHarnessSuccess
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/preview",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::Preview
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/prepared_evidence_summary",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::PreparedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/apply",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::Apply
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/finished_evidence_summary",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::FinishedEvidenceSummary
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/session_append_events",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::SessionAppendEvents
    ),
    edit_workload!(
        "native_edit/modify_multi_hunk_medium_file/end_to_end_harness_success",
        EditProfileScenario::ModifyMultiHunkMediumFile,
        EditProfilePhase::EndToEndHarnessSuccess
    ),
    edit_workload!(
        "native_edit/validation_failure_path_traversal/end_to_end_harness_validation_failure",
        EditProfileScenario::ValidationFailurePathTraversal,
        EditProfilePhase::EndToEndHarnessValidationFailure
    ),
    edit_workload!(
        "native_edit/apply_failure_hash_changed/apply",
        EditProfileScenario::ApplyFailureHashChanged,
        EditProfilePhase::Apply
    ),
    edit_workload!(
        "native_edit/apply_failure_hash_changed/end_to_end_harness_apply_failure",
        EditProfileScenario::ApplyFailureHashChanged,
        EditProfilePhase::EndToEndHarnessApplyFailure
    ),
];

type EditCacheKey = (usize, &'static str);
type EditCache = HashMap<EditCacheKey, Result<Vec<EditProfileSample>, String>>;

static EDIT_CACHE: LazyLock<Mutex<EditCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock_edit_cache() -> std::sync::MutexGuard<'static, EditCache> {
    match EDIT_CACHE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn cached_scenario(
    ctx: &RunCtx,
    scenario: EditProfileScenario,
) -> Result<Vec<EditProfileSample>, String> {
    let key = (ctx.samples, scenario.label());
    {
        let cache = lock_edit_cache();
        if let Some(existing) = cache.get(&key) {
            return existing.clone();
        }
    }
    let computed = collect_scenario_samples(ctx, scenario);
    let mut cache = lock_edit_cache();
    cache.entry(key).or_insert_with(|| computed.clone());
    computed
}

fn collect_scenario_samples(
    ctx: &RunCtx,
    scenario: EditProfileScenario,
) -> Result<Vec<EditProfileSample>, String> {
    let mut samples = Vec::with_capacity(ctx.samples);
    for _ in 0..ctx.samples {
        let sample = EditProfileRunner::sample_scenario(scenario)
            .map_err(|error| format!("{}: {}", error.scenario.label(), error.message))?;
        samples.push(sample);
    }
    Ok(samples)
}

fn run_edit_phase(
    ctx: &RunCtx,
    scenario: EditProfileScenario,
    phase: EditProfilePhase,
) -> Result<Measured, String> {
    let samples = cached_scenario(ctx, scenario)?;
    let mut durations = Vec::with_capacity(samples.len());
    for sample in &samples {
        let Some(duration) = sample
            .phases
            .iter()
            .find(|entry| entry.phase == phase)
            .map(|entry| entry.duration)
        else {
            return Err(format!(
                "missing phase {} in {}",
                phase.label(),
                scenario.label()
            ));
        };
        durations.push(duration);
    }
    Ok(Measured::Latency {
        samples: durations,
        alloc: None,
    })
}

#[cfg(test)]
mod tests {
    use super::EDIT;
    use crate::perf::registry::{Measured, RunCtx};

    #[test]
    fn create_small_text_file_preview_does_not_emit_file_bodies() -> Result<(), String> {
        let ctx = RunCtx {
            samples: 1,
            yach_bin: None,
            yach_bench_yach_bin: None,
            yach_bench_bin: None,
            filter: None,
        };
        let workload = EDIT
            .iter()
            .find(|workload| workload.id == "native_edit/create_small_text_file/preview")
            .ok_or_else(|| String::from("missing native_edit/create_small_text_file/preview"))?;
        match (workload.run)(&ctx) {
            Ok(Measured::Latency { samples, .. }) => {
                if samples.len() != 1 {
                    return Err(format!("expected 1 sample, got {}", samples.len()));
                }
            }
            Ok(_) => return Err(String::from("expected Latency from native edit preview")),
            Err(error) => {
                if error.contains("created profile body")
                    || error.contains("secret profile payload")
                    || error.contains("replacement_profile_text")
                {
                    return Err(format!("error leaked file body: {error}"));
                }
                return Err(format!("native edit preview failed: {error}"));
            }
        }
        for workload in &EDIT {
            if workload.id.contains("created profile body") {
                return Err(format!("id leaked file body: {}", workload.id));
            }
        }
        Ok(())
    }
}
