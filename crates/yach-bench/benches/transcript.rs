use criterion::{Criterion, black_box, criterion_group, criterion_main};
use yach_ui::Transcript;

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
                transcript.append_tool_call(None, black_box(&format!("tool_{i}")), Some("preview"));
                transcript.append_delta(black_box(&format!("after tool {i}")));
            }
            transcript
        });
    });
}

fn bench_entry_slice_access(c: &mut Criterion) {
    let mut transcript = Transcript::new();
    for i in 0..500 {
        transcript.append_user_message(&format!("user message {i}"));
        transcript.append_delta(&format!("assistant reply {i}"));
    }
    c.bench_function("transcript/entry_slice_access_500_entries", |b| {
        b.iter(|| {
            black_box(transcript.entries());
        });
    });
}

fn bench_compaction_count(c: &mut Criterion) {
    let mut transcript = Transcript::new();
    for i in 0..1000 {
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
    bench_entry_slice_access,
    bench_compaction_count,
);
criterion_main!(benches);
