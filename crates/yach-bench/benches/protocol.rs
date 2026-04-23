use criterion::{Criterion, black_box, criterion_group, criterion_main};
use yach_proto::{ServerEvent, TransportMessage};

fn generate_sample_delta_lines(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"type":"prompt_delta","session_id":"sess-1","delta":"token_{i} "}}"#))
        .collect()
}

fn generate_sample_status_lines(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"type":"status_updated","message":"status_update_{i}"}}"#))
        .collect()
}

fn generate_sample_tool_call_lines(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"type":"tool_call_started","tool_name":"Read_{i}"}}"#))
        .collect()
}

fn bench_parse_delta(c: &mut Criterion) {
    let lines = generate_sample_delta_lines(1000);
    c.bench_function("parse/1000_delta_lines", |b| {
        b.iter(|| {
            for line in &lines {
                let _ = TransportMessage::from_jsonl(black_box(line));
            }
        });
    });
}

fn bench_parse_status(c: &mut Criterion) {
    let lines = generate_sample_status_lines(1000);
    c.bench_function("parse/1000_status_lines", |b| {
        b.iter(|| {
            for line in &lines {
                let _ = TransportMessage::from_jsonl(black_box(line));
            }
        });
    });
}

fn bench_parse_tool_calls(c: &mut Criterion) {
    let lines = generate_sample_tool_call_lines(1000);
    c.bench_function("parse/1000_tool_call_lines", |b| {
        b.iter(|| {
            for line in &lines {
                let _ = TransportMessage::from_jsonl(black_box(line));
            }
        });
    });
}

fn bench_parse_mixed_stream(c: &mut Criterion) {
    let mut lines = Vec::new();
    for i in 0..500 {
        lines.push(format!(
            r#"{{"type":"prompt_delta","session_id":"sess-1","delta":"token_{i} "}}"#
        ));
        if i % 10 == 0 {
            lines.push(format!(
                r#"{{"type":"status_updated","message":"status_{i}"}}"#
            ));
        }
        if i % 50 == 0 {
            lines.push(format!(
                r#"{{"type":"tool_call_started","tool_name":"tool_{i}"}}"#
            ));
        }
    }
    c.bench_function("parse/500_mixed_stream", |b| {
        b.iter(|| {
            for line in &lines {
                let _ = TransportMessage::from_jsonl(black_box(line));
            }
        });
    });
}

fn bench_parse_single_delta(c: &mut Criterion) {
    let line = r#"{"type":"prompt_delta","session_id":"sess-1","delta":"hello world"}"#;
    c.bench_function("parse/single_delta", |b| {
        b.iter(|| {
            let _ = TransportMessage::from_jsonl(black_box(line));
        });
    });
}

fn bench_parse_server_event(c: &mut Criterion) {
    let line = r#"{"type":"prompt_delta","session_id":"sess-1","delta":"hello world"}"#;
    c.bench_function("parse/server_event_from_json", |b| {
        b.iter(|| {
            let _ = ServerEvent::from_jsonl(black_box(line));
        });
    });
}

criterion_group!(
    benches,
    bench_parse_delta,
    bench_parse_status,
    bench_parse_tool_calls,
    bench_parse_mixed_stream,
    bench_parse_single_delta,
    bench_parse_server_event,
);
criterion_main!(benches);
