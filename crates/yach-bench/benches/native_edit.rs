use std::io::{self, Write as _};
use std::process;
use std::time::Duration;

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::edit_profile::{
    EditProfilePhase, EditProfileRunner, EditProfileSample, EditProfileScenario,
};

const EDIT_PROFILE_PHASES: [EditProfilePhase; 8] = [
    EditProfilePhase::Preview,
    EditProfilePhase::PreparedEvidenceSummary,
    EditProfilePhase::Apply,
    EditProfilePhase::FinishedEvidenceSummary,
    EditProfilePhase::SessionAppendEvents,
    EditProfilePhase::EndToEndHarnessSuccess,
    EditProfilePhase::EndToEndHarnessValidationFailure,
    EditProfilePhase::EndToEndHarnessApplyFailure,
];

fn bench_native_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit");

    for scenario in EditProfileScenario::all() {
        let scenario = *scenario;

        group.bench_function(format!("{}/sample", scenario.label()), move |b| {
            b.iter_batched(
                || scenario,
                |scenario| black_box(sample_or_abort(scenario)),
                BatchSize::SmallInput,
            );
        });

        for phase in EDIT_PROFILE_PHASES {
            if profile_phase_duration(scenario, phase).is_none() {
                continue;
            }

            group.bench_function(
                format!("{}/{}", scenario.label(), phase.label()),
                move |b| {
                    b.iter_custom(|iterations| {
                        let mut total = Duration::ZERO;
                        for _ in 0..iterations {
                            match profile_phase_duration(scenario, phase) {
                                Some(duration) => total += duration,
                                None => abort_missing_phase(scenario, phase),
                            }
                        }
                        total
                    });
                },
            );
        }
    }

    group.finish();
}

fn profile_phase_duration(
    scenario: EditProfileScenario,
    phase: EditProfilePhase,
) -> Option<Duration> {
    sample_or_abort(scenario)
        .phases
        .into_iter()
        .find(|phase_duration| phase_duration.phase == phase)
        .map(|phase_duration| phase_duration.duration)
}

fn sample_or_abort(scenario: EditProfileScenario) -> EditProfileSample {
    match EditProfileRunner::sample_scenario(scenario) {
        Ok(sample) => sample,
        Err(error) => {
            let _ = writeln!(
                io::stderr().lock(),
                "native edit profile sample failed: scenario={}, error={}",
                error.scenario.label(),
                error.message
            );
            process::abort();
        }
    }
}

fn abort_missing_phase(scenario: EditProfileScenario, phase: EditProfilePhase) -> ! {
    let _ = writeln!(
        io::stderr().lock(),
        "native edit profile phase missing: scenario={}, phase={}",
        scenario.label(),
        phase.label()
    );
    process::abort();
}

criterion_group!(benches, bench_native_edit);
criterion_main!(benches);
