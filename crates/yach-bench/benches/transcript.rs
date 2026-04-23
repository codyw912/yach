use criterion::{Criterion, black_box, criterion_group, criterion_main};
use yach_adapter_pi_rpc::Transcript;

fn bench_append_deltas(c: &mut Criterion) {
    c.bench_function("transcript/1000_append_deltas", |b| {
        b.iter(|| {
            let mut transcript = Transcript::new();
            for i in 0..1000 {
                transcript.append_delta(black_box(&format!("token_{i} ")));
            }
            transcript
        });
    });
}

fn bench_append_large_deltas(c: &mut Criterion) {
    let big_delta = "x".repeat(10_000);
    c.bench_function("transcript/100_large_deltas", |b| {
        b.iter(|| {
            let mut transcript = Transcript::new();
            for _ in 0..100 {
                transcript.append_delta(black_box(&big_delta));
            }
            transcript
        });
    });
}

fn bench_alternating_entries(c: &mut Criterion) {
    c.bench_function("transcript/500_alternating_entries", |b| {
        b.iter(|| {
            let mut transcript = Transcript::new();
            for i in 0..500 {
                if i % 2 == 0 {
                    transcript.append_user_message(black_box(&format!("user message {i}")));
                } else {
                    transcript.append_delta(black_box(&format!("assistant reply {i}")));
                }
            }
            transcript
        });
    });
}

fn bench_tool_call_entries(c: &mut Criterion) {
    c.bench_function("transcript/200_tool_call_entries", |b| {
        b.iter(|| {
            let mut transcript = Transcript::new();
            for i in 0..200 {
                transcript.append_user_message("do something");
                transcript.append_tool_call(black_box(&format!("tool_{i}")));
                transcript.append_tool_result(
                    black_box(&format!("tool_{i}")),
                    black_box(&format!("result for tool {i}")),
                );
                transcript.append_delta(black_box(&format!("after tool {i}")));
            }
            transcript
        });
    });
}

fn bench_compaction_entries(c: &mut Criterion) {
    c.bench_function("transcript/1000_with_compactions", |b| {
        b.iter(|| {
            let mut transcript = Transcript::new();
            for i in 0..1000 {
                if i % 100 == 0 && i > 0 {
                    transcript.append_compaction();
                }
                transcript.append_delta(black_box(&format!("token_{i} ")));
            }
            transcript
        });
    });
}

fn bench_content_render(c: &mut Criterion) {
    let mut transcript = Transcript::new();
    for i in 0..500 {
        transcript.append_user_message(&format!("user message {i}"));
        transcript.append_delta(&format!("assistant reply {i}"));
    }
    c.bench_function("transcript/content_render_500_entries", |b| {
        b.iter(|| {
            black_box(transcript.content());
        });
    });
}

fn bench_compaction_count(c: &mut Criterion) {
    let mut transcript = Transcript::new();
    for i in 0..1000 {
        if i % 100 == 0 && i > 0 {
            transcript.append_compaction();
        }
        transcript.append_delta(&format!("token_{i} "));
    }
    c.bench_function("transcript/compaction_count_1000", |b| {
        b.iter(|| {
            black_box(transcript.compaction_count());
        });
    });
}

criterion_group!(
    benches,
    bench_append_deltas,
    bench_append_large_deltas,
    bench_alternating_entries,
    bench_tool_call_entries,
    bench_compaction_entries,
    bench_content_render,
    bench_compaction_count,
);
criterion_main!(benches);
