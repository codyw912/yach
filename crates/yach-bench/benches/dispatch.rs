use criterion::{Criterion, black_box, criterion_group, criterion_main};
use yach_adapter_pi_rpc::{DispatchAction, dispatch_event};
use yach_proto::ServerEvent;

fn bench_dispatch_delta(c: &mut Criterion) {
    let event = ServerEvent::PromptDelta {
        session_id: String::from("sess-1"),
        delta: String::from("token"),
    };
    c.bench_function("dispatch/prompt_delta", |b| {
        b.iter(|| {
            let action = dispatch_event(black_box(event.clone()));
            black_box(action);
        });
    });
}

fn bench_dispatch_status(c: &mut Criterion) {
    let event = ServerEvent::StatusUpdated {
        message: String::from("connected"),
    };
    c.bench_function("dispatch/status_updated", |b| {
        b.iter(|| {
            let action = dispatch_event(black_box(event.clone()));
            black_box(action);
        });
    });
}

fn bench_dispatch_tool_call(c: &mut Criterion) {
    let event = ServerEvent::ToolCallStarted {
        tool_name: String::from("Read"),
    };
    c.bench_function("dispatch/tool_call_started", |b| {
        b.iter(|| {
            let action = dispatch_event(black_box(event.clone()));
            black_box(action);
        });
    });
}

fn bench_dispatch_session_changed(c: &mut Criterion) {
    let event = ServerEvent::SessionChanged {
        session_id: String::from("sess-2"),
    };
    c.bench_function("dispatch/session_changed", |b| {
        b.iter(|| {
            let action = dispatch_event(black_box(event.clone()));
            black_box(action);
        });
    });
}

fn bench_dispatch_ready(c: &mut Criterion) {
    let handshake = yach_proto::Handshake::new("test", vec![]);
    let event = ServerEvent::Ready { handshake };
    c.bench_function("dispatch/ready", |b| {
        b.iter(|| {
            let action = dispatch_event(black_box(event.clone()));
            black_box(action);
        });
    });
}

fn bench_dispatch_stream_of_1000(c: &mut Criterion) {
    let events: Vec<ServerEvent> = (0..1000)
        .map(|i| {
            if i % 5 == 0 {
                ServerEvent::StatusUpdated {
                    message: format!("status_{i}"),
                }
            } else {
                ServerEvent::PromptDelta {
                    session_id: String::from("sess-1"),
                    delta: format!("token_{i} "),
                }
            }
        })
        .collect();
    c.bench_function("dispatch/stream_of_1000", |b| {
        b.iter(|| {
            for event in &events {
                let action = dispatch_event(black_box(event.clone()));
                black_box(action);
            }
        });
    });
}

fn bench_dispatch_action_match(c: &mut Criterion) {
    let action = DispatchAction::AppendDelta(String::from("hello"));
    c.bench_function("dispatch/action_pattern_match", |b| {
        b.iter(|| match black_box(&action) {
            DispatchAction::AppendDelta(s) => black_box(s.len()),
            _ => 0,
        });
    });
}

criterion_group!(
    benches,
    bench_dispatch_delta,
    bench_dispatch_status,
    bench_dispatch_tool_call,
    bench_dispatch_session_changed,
    bench_dispatch_ready,
    bench_dispatch_stream_of_1000,
    bench_dispatch_action_match,
);
criterion_main!(benches);
