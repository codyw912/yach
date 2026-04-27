use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use crossterm::event::{KeyCode, KeyModifiers};
use yach_bench::fixtures::{
    PayloadScale, TranscriptScale, connected_event, heavy_tool_events, large_paste_payload,
    prompt_delta_events, transcript_fixture,
};
use yach_bench::replay::{ReplayStep, replay_headless};

fn bench_idle_keypress_to_paint_headless(c: &mut Criterion) {
    c.bench_function("keypress/idle_keypress_to_paint_headless", |b| {
        b.iter(|| {
            let result = replay_headless(
                black_box(&[
                    ReplayStep::Backend(connected_event()),
                    ReplayStep::Key {
                        code: KeyCode::Char('x'),
                        modifiers: KeyModifiers::empty(),
                    },
                ]),
                100,
                30,
            );
            black_box(result.summary(None));
        });
    });
}

fn bench_active_stream_replay_headless(c: &mut Criterion) {
    let mut group = c.benchmark_group("keypress");
    for event_count in [100_usize, 1_000] {
        let mut steps = vec![ReplayStep::Backend(connected_event())];
        steps.extend(
            prompt_delta_events(event_count)
                .into_iter()
                .map(ReplayStep::Backend),
        );
        steps.push(ReplayStep::Key {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::empty(),
        });

        group.bench_with_input(
            BenchmarkId::new("active_stream_replay_headless", event_count),
            &steps,
            |b, steps| {
                b.iter(|| {
                    let result = replay_headless(black_box(steps), 100, 30);
                    black_box(result.summary(None));
                });
            },
        );
    }
    group.finish();
}

fn bench_heavy_tool_output_tail_headless(c: &mut Criterion) {
    let mut group = c.benchmark_group("replay");
    for scale in [
        PayloadScale::Small,
        PayloadScale::Medium,
        PayloadScale::Large,
    ] {
        let mut steps = vec![ReplayStep::Backend(connected_event())];
        steps.extend(
            heavy_tool_events(scale)
                .into_iter()
                .map(ReplayStep::Backend),
        );

        group.bench_with_input(
            BenchmarkId::new("heavy_tool_output_tail_headless", scale.bytes()),
            &steps,
            |b, steps| {
                b.iter(|| {
                    let result = replay_headless(black_box(steps), 100, 30);
                    black_box(result.summary(None));
                });
            },
        );
    }
    group.finish();
}

fn bench_large_multiline_paste_component(c: &mut Criterion) {
    let paste = large_paste_payload(PayloadScale::Medium);
    c.bench_function("paste/large_multiline_component", |b| {
        b.iter(|| {
            let result = replay_headless(
                black_box(&[
                    ReplayStep::Backend(connected_event()),
                    ReplayStep::PromptText(paste.clone()),
                ]),
                100,
                30,
            );
            black_box(result.prompt_text.starts_with("/not-a-command\n"));
            black_box(result.summary(None));
        });
    });
}

fn bench_huge_transcript_viewport_headless(c: &mut Criterion) {
    let mut group = c.benchmark_group("viewport");
    for scale in [TranscriptScale::Medium, TranscriptScale::Large] {
        let transcript = transcript_fixture(scale);
        let steps = vec![
            ReplayStep::Backend(connected_event()),
            ReplayStep::Transcript(transcript),
            ReplayStep::ScrollDown(20),
        ];

        group.bench_with_input(
            BenchmarkId::new("huge_transcript_scroll_headless", scale.entries()),
            &steps,
            |b, steps| {
                b.iter(|| {
                    let result = replay_headless(black_box(steps), 120, 40);
                    black_box(result.summary(None));
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_idle_keypress_to_paint_headless,
    bench_active_stream_replay_headless,
    bench_heavy_tool_output_tail_headless,
    bench_large_multiline_paste_component,
    bench_huge_transcript_viewport_headless,
);
criterion_main!(benches);
