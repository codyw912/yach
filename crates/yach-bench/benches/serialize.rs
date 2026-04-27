use criterion::{Criterion, black_box, criterion_group, criterion_main};
use yach_adapter_pi_rpc::serialize_client_message;
use yach_proto::{ClientEvent, MessageMeta, TransportMessage, default_ui_handshake};

fn bench_serialize_initialize(c: &mut Criterion) {
    let message = TransportMessage::client(
        MessageMeta::new("init-1"),
        ClientEvent::Initialize(default_ui_handshake()),
    );
    c.bench_function("serialize/initialize", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

fn bench_serialize_prompt(c: &mut Criterion) {
    let message = TransportMessage::client(
        MessageMeta::new("prompt-1"),
        ClientEvent::PromptSubmitted {
            session_id: String::from("sess-1"),
            prompt: String::from("Write a function that sorts a list of integers"),
        },
    );
    c.bench_function("serialize/prompt_short", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

fn bench_serialize_long_prompt(c: &mut Criterion) {
    let prompt = "Write a function".repeat(100);
    let message = TransportMessage::client(
        MessageMeta::new("prompt-2"),
        ClientEvent::PromptSubmitted {
            session_id: String::from("sess-1"),
            prompt,
        },
    );
    c.bench_function("serialize/prompt_long", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

fn bench_serialize_model_selected(c: &mut Criterion) {
    let message = TransportMessage::client(
        MessageMeta::new("model-1"),
        ClientEvent::ModelSelectedDetailed {
            provider: String::from("anthropic"),
            model_id: String::from("claude-sonnet-4-20250514"),
        },
    );
    c.bench_function("serialize/model_selected", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

fn bench_serialize_session_fork(c: &mut Criterion) {
    let message = TransportMessage::client(
        MessageMeta::new("fork-1"),
        ClientEvent::SessionForkRequested {
            session_id: String::from("sess-abc123"),
        },
    );
    c.bench_function("serialize/session_fork", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

fn bench_serialize_thinking_level(c: &mut Criterion) {
    let message = TransportMessage::client(
        MessageMeta::new("thinking-1"),
        ClientEvent::ThinkingLevelSelected {
            level: String::from("high"),
        },
    );
    c.bench_function("serialize/thinking_level", |b| {
        b.iter(|| {
            let _ = black_box(serialize_client_message(black_box(&message)));
        });
    });
}

criterion_group!(
    benches,
    bench_serialize_initialize,
    bench_serialize_prompt,
    bench_serialize_long_prompt,
    bench_serialize_model_selected,
    bench_serialize_session_fork,
    bench_serialize_thinking_level,
);
criterion_main!(benches);
