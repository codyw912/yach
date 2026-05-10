use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::{
    NativeEntryId, NativeJsonlSessionStore, NativeRole, NativeSessionEvent, NativeSessionEventSink,
    NativeSessionId, NativeSessionLog, NativeTurnId, NativeTurnOutcome,
};

static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_session_path(label: &str) -> PathBuf {
    let sequence = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "yach-native-session-{label}-{}-{sequence}.jsonl",
        std::process::id()
    ))
}

fn turn_events(index: usize) -> [NativeSessionEvent; 3] {
    let session_id = NativeSessionId(String::from("bench-session"));
    let turn_id = NativeTurnId(format!("turn-{index}"));
    let user_entry_id = NativeEntryId(format!("entry-{index}-user"));
    let assistant_entry_id = NativeEntryId(format!("entry-{index}-assistant"));

    [
        NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: user_entry_id.clone(),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: NativeRole::User,
            text: format!("prompt {index}"),
            provider: None,
        },
        NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: assistant_entry_id,
            parent_entry_id: Some(user_entry_id),
            turn_id: turn_id.clone(),
            role: NativeRole::Assistant,
            text: format!("response {index}"),
            provider: None,
        },
        NativeSessionEvent::TurnFinished {
            session_id,
            turn_id,
            outcome: NativeTurnOutcome::Completed,
            reason: None,
        },
    ]
}

fn session_events(turns: usize) -> Vec<NativeSessionEvent> {
    (0..turns).flat_map(turn_events).collect()
}

fn write_session_file(turns: usize) -> PathBuf {
    let path = unique_session_path(&format!("load-{turns}"));
    let store = NativeJsonlSessionStore::new(path.clone());
    store
        .append_events(&session_events(turns))
        .expect("benchmark fixture session should be writable");
    path
}

fn fixture_log(turns: usize) -> NativeSessionLog {
    NativeSessionLog {
        events: session_events(turns),
    }
}

fn native_session_append_event(c: &mut Criterion) {
    let event = turn_events(0)
        .into_iter()
        .next()
        .expect("turn fixture should include a first event");

    c.bench_function("native_session_append_event", |b| {
        b.iter_batched(
            || NativeJsonlSessionStore::new(unique_session_path("append")),
            |store| {
                store
                    .append_event(black_box(&event))
                    .expect("benchmark append should succeed");
                black_box(store.path());
            },
            BatchSize::SmallInput,
        );
    });
}

fn native_session_load_10_turns(c: &mut Criterion) {
    bench_native_session_load(c, 10, "native_session_load_10_turns");
}

fn native_session_load_100_turns(c: &mut Criterion) {
    bench_native_session_load(c, 100, "native_session_load_100_turns");
}

fn native_session_load_1000_turns(c: &mut Criterion) {
    bench_native_session_load(c, 1000, "native_session_load_1000_turns");
}

fn bench_native_session_load(c: &mut Criterion, turns: usize, name: &str) {
    let path = write_session_file(turns);
    let store = NativeJsonlSessionStore::new(path);

    c.bench_function(name, |b| {
        b.iter(|| black_box(store.load().expect("benchmark session should load")));
    });
}

fn native_session_projection_10_turns(c: &mut Criterion) {
    bench_native_session_projection(c, 10, "native_session_projection_10_turns");
}

fn native_session_projection_100_turns(c: &mut Criterion) {
    bench_native_session_projection(c, 100, "native_session_projection_100_turns");
}

fn native_session_projection_1000_turns(c: &mut Criterion) {
    bench_native_session_projection(c, 1000, "native_session_projection_1000_turns");
}

fn bench_native_session_projection(c: &mut Criterion, turns: usize, name: &str) {
    let log = fixture_log(turns);

    c.bench_function(name, |b| {
        b.iter(|| {
            black_box((
                log.next_turn_index(),
                log.last_entry_id(),
                log.transcript_messages(),
            ));
        });
    });
}

criterion_group!(
    benches,
    native_session_append_event,
    native_session_load_10_turns,
    native_session_load_100_turns,
    native_session_load_1000_turns,
    native_session_projection_10_turns,
    native_session_projection_100_turns,
    native_session_projection_1000_turns,
);
criterion_main!(benches);
