use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use yach_backend::{
    NativeEntryId, NativeJsonlSessionStore, NativeRole, NativeSessionEvent, NativeSessionEventSink,
    NativeSessionId, NativeSessionLog, NativeToolOutcome, NativeToolPayloadSummary,
    NativeToolPermissionState, NativeToolRequestId, NativeTurnId, NativeTurnOutcome,
};

static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempSessionFile {
    directory: PathBuf,
    path: PathBuf,
}

impl TempSessionFile {
    fn new(label: &str) -> Option<Self> {
        let sequence = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "yach-native-session-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).ok()?;
        let path = directory.join("session.jsonl");

        Some(Self { directory, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempSessionFile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn turn_events(index: usize) -> [NativeSessionEvent; 5] {
    let session_id = NativeSessionId(String::from("bench-session"));
    let turn_id = NativeTurnId(format!("turn-{index}"));
    let user_entry_id = NativeEntryId(format!("entry-{index}-user"));
    let assistant_entry_id = NativeEntryId(format!("entry-{index}-assistant"));
    let tool_request_id = NativeToolRequestId(format!("tool-request-{index}"));
    // Representative persisted tool payloads (session tool payload
    // persistence design): bounded argument JSON plus a ~1KiB result body.
    let result_content = format!(
        "{{\"path\":\"src/file-{index}.rs\",\"content\":\"{}\",\"truncated\":false}}",
        "line of persisted file content\\n".repeat(32)
    );

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
        NativeSessionEvent::ToolRequestRecorded {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id: tool_request_id.clone(),
            tool_name: String::from("read_text_file"),
            provider_call_id: Some(format!("call-{index}")),
            validation: Ok(()),
            permission: NativeToolPermissionState::Allowed,
            argument_summary: NativeToolPayloadSummary {
                summary: String::from("tool payload redacted"),
                byte_count: 32,
                redacted: true,
                truncated: false,
            },
            argument_content: Some(format!("{{\"path\":\"src/file-{index}.rs\"}}")),
        },
        NativeSessionEvent::ToolExecutionFinished {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            tool_request_id,
            outcome: NativeToolOutcome::Completed,
            reason: None,
            result_summary: Some(NativeToolPayloadSummary {
                summary: String::from("read_text_file result redacted"),
                byte_count: result_content.len(),
                redacted: true,
                truncated: false,
            }),
            result_content: Some(result_content),
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

fn write_session_file(turns: usize) -> Option<TempSessionFile> {
    let fixture = TempSessionFile::new(&format!("load-{turns}"))?;
    let store = NativeJsonlSessionStore::new(fixture.path().to_path_buf());
    store.append_events(&session_events(turns)).ok()?;
    Some(fixture)
}

fn fixture_log(turns: usize) -> NativeSessionLog {
    NativeSessionLog {
        events: session_events(turns),
    }
}

fn native_session_append_event(c: &mut Criterion) {
    let event = NativeSessionEvent::TurnFinished {
        session_id: NativeSessionId(String::from("bench-session")),
        turn_id: NativeTurnId(String::from("turn-append")),
        outcome: NativeTurnOutcome::Completed,
        reason: None,
    };

    c.bench_function("native_session_append_event", |b| {
        b.iter_batched(
            || TempSessionFile::new("append"),
            |fixture| {
                let Some(fixture) = fixture else {
                    black_box(false);
                    return;
                };
                let store = NativeJsonlSessionStore::new(fixture.path().to_path_buf());
                let appended = store.append_event(black_box(&event)).is_ok();
                black_box(appended);
                black_box(store.path().to_path_buf());
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
    let Some(fixture) = write_session_file(turns) else {
        return;
    };
    let store = NativeJsonlSessionStore::new(fixture.path().to_path_buf());

    c.bench_function(name, |b| {
        b.iter(|| black_box(store.load().unwrap_or_default()));
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
