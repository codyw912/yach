# Native Session Resume Metrics Baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make native sessions genuinely resumable across process restarts and record the first granular native-session performance evidence.

**Architecture:** Keep canonical native session state in `yach-backend` as an append-only JSONL event model plus deterministic projections. The native runner should derive turn IDs and provider transcript context from the persisted log, while metrics are persisted as explicit session events and benchmarked in `yach-bench`.

**Tech Stack:** Rust 2024, Tokio, Serde JSONL, yach-owned `yach-proto` UI/backend events, Criterion benchmarks, `just` project recipes.

---

## Scope

This plan implements the first Native MVP slice from `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`: session log/resume plus metrics baseline.

It deliberately does not add read/search tools, file edits, verification actions, branch/fork sessions, MCP, or extension runtime. Those remain separate Native MVP slices after this foundation is stable.

## File Structure

- Modify `crates/yach-backend/src/session.rs`: add native transcript projection helpers, deterministic resume ID helpers, and metric event types.
- Modify `crates/yach-backend/src/native_runner.rs`: initialize prompt turn indices from persisted sessions, build provider requests from resumed transcript context, and record runtime metric events.
- Modify `crates/yach-backend/src/lib.rs`: add backend tests for session projections and metric JSONL persistence.
- Modify `crates/yach-cli/src/main.rs`: add integration-style native runner tests that prove restart/resume does not duplicate turn IDs.
- Modify `crates/yach-bench/Cargo.toml`: add the backend crate dependency and register the native session benchmark.
- Create `crates/yach-bench/benches/native_session.rs`: benchmark session log load, rewrite, and projection costs for repeatable local evidence.
- Modify `docs/project/state.md` and `docs/project/next.md`: update active planning state after the implementation lands.

## Task 1: Native Session Resume Projections

**Files:**
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/lib.rs`

- [ ] **Step 1: Write the failing resume projection test**

Add this test in `crates/yach-backend/src/lib.rs` near the existing native session log tests:

```rust
#[test]
fn native_session_resume_projection_derives_next_ids_and_transcript() {
    let mut log = completed_text_exchange(
        NativeSessionId(String::from("default")),
        NativeEntryId(String::from("entry-0-user")),
        NativeEntryId(String::from("entry-0-assistant")),
        NativeTurnId(String::from("turn-0")),
        String::from("hello"),
        String::from("hi"),
    );
    log.push(NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("default")),
        entry_id: NativeEntryId(String::from("entry-1-user")),
        parent_entry_id: Some(NativeEntryId(String::from("entry-0-assistant"))),
        turn_id: NativeTurnId(String::from("turn-1")),
        role: NativeRole::User,
        text: String::from("continue"),
        provider: None,
    });

    assert_eq!(log.next_turn_index(), 2);
    assert_eq!(
        log.last_entry_id(),
        Some(NativeEntryId(String::from("entry-1-user")))
    );
    assert_eq!(
        log.transcript_messages(),
        vec![
            NativeTranscriptMessage {
                role: NativeRole::User,
                text: String::from("hello"),
            },
            NativeTranscriptMessage {
                role: NativeRole::Assistant,
                text: String::from("hi"),
            },
            NativeTranscriptMessage {
                role: NativeRole::User,
                text: String::from("continue"),
            },
        ]
    );
}
```

Update the `use super::{ ... }` list in the same test module to include `NativeTranscriptMessage`.

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_session_resume_projection_derives_next_ids_and_transcript -- --exact
```

Expected: FAIL because `NativeTranscriptMessage`, `next_turn_index`, `last_entry_id`, and `transcript_messages` do not exist.

- [ ] **Step 3: Implement session projection helpers**

In `crates/yach-backend/src/session.rs`, add this type after `ProviderMetadata`:

```rust
/// Provider-neutral transcript message reconstructed from native session events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeTranscriptMessage {
    pub role: NativeRole,
    pub text: String,
}
```

In the `impl NativeSessionLog` block, add these methods:

```rust
    #[must_use]
    pub fn next_turn_index(&self) -> u64 {
        self.events
            .iter()
            .filter_map(native_event_turn_id)
            .filter_map(native_turn_index)
            .max()
            .map_or(0, |index| index.saturating_add(1))
    }

    #[must_use]
    pub fn last_entry_id(&self) -> Option<NativeEntryId> {
        self.events.iter().rev().find_map(|event| match event {
            NativeSessionEvent::EntryAppended { entry_id, .. } => Some(entry_id.clone()),
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. } => None,
        })
    }

    #[must_use]
    pub fn transcript_messages(&self) -> Vec<NativeTranscriptMessage> {
        self.events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EntryAppended { role, text, .. } => {
                    Some(NativeTranscriptMessage {
                        role: *role,
                        text: text.clone(),
                    })
                }
                NativeSessionEvent::ToolRequestRecorded { .. }
                | NativeSessionEvent::ToolExecutionFinished { .. }
                | NativeSessionEvent::TurnFinished { .. } => None,
            })
            .collect()
    }
```

Add these private helpers below the `impl NativeSessionLog` block:

```rust
fn native_event_turn_id(event: &NativeSessionEvent) -> Option<&NativeTurnId> {
    match event {
        NativeSessionEvent::EntryAppended { turn_id, .. }
        | NativeSessionEvent::ToolRequestRecorded { turn_id, .. }
        | NativeSessionEvent::ToolExecutionFinished { turn_id, .. }
        | NativeSessionEvent::TurnFinished { turn_id, .. } => Some(turn_id),
    }
}

fn native_turn_index(turn_id: &NativeTurnId) -> Option<u64> {
    turn_id.0.strip_prefix("turn-")?.parse().ok()
}
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
just dev cargo test -p yach-backend native_session_resume_projection_derives_next_ids_and_transcript -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs
git commit -m "feat: add native session resume projections"
```

## Task 2: Native Session Metric Events

**Files:**
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/lib.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write the failing metric JSONL test**

Add this test in `crates/yach-backend/src/lib.rs` near `native_session_log_preserves_provider_metadata_jsonl`:

```rust
#[test]
fn native_session_log_preserves_metric_records_jsonl() {
    let path = temp_log_path("native-session-log-metrics");
    let mut log = NativeSessionLog::default();
    log.record_duration_metric(
        NativeSessionId(String::from("default")),
        Some(NativeTurnId(String::from("turn-7"))),
        "session_log_load",
        std::time::Duration::from_millis(12),
        vec![NativeMetricAttribute {
            key: String::from("source"),
            value: String::from("resume"),
        }],
    );

    assert!(log.write_to_file(&path).is_ok());
    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    let loaded = NativeSessionLog::load_from_file(&path).ok();
    assert!(std::fs::remove_file(path).is_ok());

    assert!(persisted.contains("metric_recorded"));
    assert!(persisted.contains("session_log_load"));
    assert_eq!(loaded, Some(log));
}
```

Update the `use super::{ ... }` list to include `NativeMetricAttribute`.

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_session_log_preserves_metric_records_jsonl -- --exact
```

Expected: FAIL because `NativeMetricAttribute`, the metric event variant, and `record_duration_metric` do not exist.

- [ ] **Step 3: Implement metric event types and helper**

In `crates/yach-backend/src/session.rs`, add these types after `NativeTranscriptMessage`:

```rust
/// String-valued metric attribute persisted without provider secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeMetricAttribute {
    pub key: String,
    pub value: String,
}

/// Granular duration metric persisted in the native session log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeDurationMetric {
    pub name: String,
    pub duration_ms: u64,
    pub attributes: Vec<NativeMetricAttribute>,
}
```

Add this variant to `NativeSessionEvent` after `TurnFinished`:

```rust
    MetricRecorded {
        session_id: NativeSessionId,
        turn_id: Option<NativeTurnId>,
        metric: NativeDurationMetric,
    },
```

Add this method to `impl NativeSessionLog`:

```rust
    pub fn record_duration_metric(
        &mut self,
        session_id: NativeSessionId,
        turn_id: Option<NativeTurnId>,
        name: impl Into<String>,
        duration: std::time::Duration,
        attributes: Vec<NativeMetricAttribute>,
    ) {
        self.push(NativeSessionEvent::MetricRecorded {
            session_id,
            turn_id,
            metric: NativeDurationMetric {
                name: name.into(),
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                attributes,
            },
        });
    }
```

Update existing exhaustive `match NativeSessionEvent` expressions in `session.rs` and `crates/yach-backend/src/native_runner.rs` so `MetricRecorded` is ignored by transcript, messages, stats, and first-message projections:

```rust
            NativeSessionEvent::ToolRequestRecorded { .. }
            | NativeSessionEvent::ToolExecutionFinished { .. }
            | NativeSessionEvent::TurnFinished { .. }
            | NativeSessionEvent::MetricRecorded { .. } => None,
```

Update `native_event_turn_id` so metrics with a turn ID participate in `next_turn_index`:

```rust
        NativeSessionEvent::MetricRecorded { turn_id, .. } => turn_id.as_ref(),
```

- [ ] **Step 4: Run backend tests for native session logs**

Run:

```bash
just dev cargo test -p yach-backend native_session_log
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/session.rs crates/yach-backend/src/lib.rs crates/yach-backend/src/native_runner.rs
git commit -m "feat: persist native session metrics"
```

## Task 3: Runner Turn IDs Resume From Existing Logs

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Write the failing restart/resume runner test**

Update the backend imports in `crates/yach-cli/src/main.rs` tests from:

```rust
use yach_backend::{NativeDogfoodRunnerConfig, run_native_dogfood_loop};
```

to:

```rust
use yach_backend::{
    NativeDogfoodRunnerConfig, NativeEntryId, NativeRole, NativeSessionEvent, NativeSessionId,
    NativeSessionLog, NativeTurnId, completed_text_exchange, run_native_dogfood_loop,
};
```

Add this test near `native_dogfood_loop_streams_and_persists_prompt`:

```rust
#[test]
fn native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids() {
    let runtime = tokio::runtime::Runtime::new();
    assert!(runtime.is_ok());
    let Some(runtime) = runtime.ok() else {
        return;
    };

    runtime.block_on(async {
        let path = temp_native_log_path();
        let seed = completed_text_exchange(
            NativeSessionId(String::from("default")),
            NativeEntryId(String::from("entry-0-user")),
            NativeEntryId(String::from("entry-0-assistant")),
            NativeTurnId(String::from("turn-0")),
            String::from("first"),
            String::from("native dogfood fixture response: first"),
        );
        assert!(seed.write_to_file(&path).is_ok());

        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(run_native_dogfood_loop(
            client_rx,
            backend_tx,
            NativeDogfoodRunnerConfig {
                session_path: path.clone(),
                provider: None,
            },
        ));

        assert!(
            client_tx
                .send(ClientEvent::PromptSubmitted {
                    session_id: String::from("default"),
                    prompt: String::from("second"),
                })
                .is_ok()
        );

        for _ in 0..64 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), backend_rx.recv())
                .await;
            let Ok(Some(BackendEvent::Server(ServerEvent::StatusUpdated { message }))) = event
            else {
                continue;
            };
            if message.starts_with("turn_end") {
                break;
            }
        }

        handle.abort();
        let loaded = NativeSessionLog::load_from_file(&path).ok();
        let _ = std::fs::remove_file(path);
        let Some(loaded) = loaded else {
            panic!("native session log should reload after resumed prompt");
        };

        let user_turns = loaded
            .events
            .iter()
            .filter_map(|event| match event {
                NativeSessionEvent::EntryAppended {
                    turn_id,
                    role: NativeRole::User,
                    ..
                } => Some(turn_id.0.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(user_turns, vec!["turn-0", "turn-1"]);
    });
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-cli native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids -- --exact
```

Expected: FAIL because the runner starts `turn_index` at `0` for every process.

- [ ] **Step 3: Initialize runner turn index from the persisted log**

In `crates/yach-backend/src/native_runner.rs`, change the start of `run_native_dogfood_loop` from:

```rust
    let mut turn_index = 0_u64;
    let mut active_prompt: Option<JoinHandle<()>> = None;
    let provider = config.provider;
    let session_path = config.session_path;
```

to:

```rust
    let provider = config.provider;
    let session_path = config.session_path;
    let mut turn_index = load_native_log_or_default(&session_path).next_turn_index();
    let mut active_prompt: Option<JoinHandle<()>> = None;
```

- [ ] **Step 4: Run the focused test and verify it passes**

Run:

```bash
just dev cargo test -p yach-cli native_dogfood_loop_resumes_existing_session_without_duplicate_turn_ids -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-cli/src/main.rs
git commit -m "feat: resume native runner turn ids"
```

## Task 4: Provider Requests Use Resumed Transcript Context

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`

- [ ] **Step 1: Write the failing provider transcript helper test**

In `crates/yach-backend/src/native_runner.rs`, replace the current test module import:

```rust
use super::{NativeFixtureOutcome, native_fixture_outcome, native_response_chunks};
```

with:

```rust
use super::{
    NativeFixtureOutcome, native_fixture_outcome, native_provider_messages_from_log,
    native_response_chunks,
};
use crate::{
    NativeEntryId, NativeRole, NativeSessionId, NativeTurnId, completed_text_exchange,
};
```

Add this test in the same test module:

```rust
#[test]
fn native_provider_messages_include_resumed_transcript() {
    let mut log = completed_text_exchange(
        NativeSessionId(String::from("default")),
        NativeEntryId(String::from("entry-0-user")),
        NativeEntryId(String::from("entry-0-assistant")),
        NativeTurnId(String::from("turn-0")),
        String::from("first"),
        String::from("answer"),
    );
    log.push(crate::NativeSessionEvent::EntryAppended {
        session_id: NativeSessionId(String::from("default")),
        entry_id: NativeEntryId(String::from("entry-1-user")),
        parent_entry_id: Some(NativeEntryId(String::from("entry-0-assistant"))),
        turn_id: NativeTurnId(String::from("turn-1")),
        role: NativeRole::User,
        text: String::from("second"),
        provider: None,
    });

    let messages = native_provider_messages_from_log(&log);

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, NativeRole::User);
    assert_eq!(messages[0].content, "first");
    assert_eq!(messages[1].role, NativeRole::Assistant);
    assert_eq!(messages[1].content, "answer");
    assert_eq!(messages[2].role, NativeRole::User);
    assert_eq!(messages[2].content, "second");
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --exact
```

Expected: FAIL because `native_provider_messages_from_log` does not exist.

- [ ] **Step 3: Implement provider transcript conversion**

In `crates/yach-backend/src/native_runner.rs`, add this helper near `handle_native_provider_prompt`:

```rust
fn native_provider_messages_from_log(log: &NativeSessionLog) -> Vec<ProviderMessage> {
    log.transcript_messages()
        .into_iter()
        .map(|message| ProviderMessage {
            role: message.role,
            content: message.text,
        })
        .collect()
}
```

In `handle_native_provider_prompt`, change:

```rust
        messages: vec![ProviderMessage {
            role: NativeRole::User,
            content: prompt.to_owned(),
        }],
```

to:

```rust
        messages: native_provider_messages_from_log(log),
```

Remove the now-unused `prompt` parameter from `handle_native_provider_prompt` and its call site:

```rust
async fn handle_native_provider_prompt(
    tx: &mpsc::UnboundedSender<BackendEvent>,
    session_path: &Path,
    provider: NativeProviderDogfoodConfig,
    log: &mut NativeSessionLog,
    ids: NativeProviderTurnRefs,
) {
```

and:

```rust
        handle_native_provider_prompt(
            &tx,
            &session_path,
            provider,
            &mut log,
            NativeProviderTurnRefs {
                turn: turn_id,
                user_entry: user_entry_id,
                assistant_entry: assistant_entry_id,
            },
        )
```

- [ ] **Step 4: Run provider and backend tests**

Run:

```bash
just dev cargo test -p yach-backend native_provider_messages_include_resumed_transcript -- --exact
just dev cargo test -p yach-backend provider_request_keeps_common_shape_provider_free -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs
git commit -m "feat: resume native provider transcript context"
```

## Task 5: Runtime Metric Baseline Events

**Files:**
- Modify: `crates/yach-backend/src/native_runner.rs`
- Modify: `crates/yach-cli/src/main.rs`

- [ ] **Step 1: Write the failing runner metrics test**

Add this test in `crates/yach-cli/src/main.rs` near the native dogfood loop tests:

```rust
#[test]
fn native_dogfood_loop_persists_prompt_runtime_metrics() {
    let persisted = run_native_fixture_prompt("metrics please");

    assert!(persisted.contains("metric_recorded"));
    assert!(persisted.contains("session_log_load"));
    assert!(persisted.contains("native_prompt_total"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
just dev cargo test -p yach-cli native_dogfood_loop_persists_prompt_runtime_metrics -- --exact
```

Expected: FAIL because native prompt handling does not record metric events.

- [ ] **Step 3: Record load and prompt duration metrics**

In `crates/yach-backend/src/native_runner.rs`, update imports:

```rust
use std::time::{Duration, Instant, UNIX_EPOCH};
```

and include `NativeMetricAttribute` in the backend session imports at the top:

```rust
    NativeEntryId, NativeMetricAttribute, NativeRole, NativeSessionEvent, NativeSessionId,
```

At the start of `handle_native_prompt`, add:

```rust
    let prompt_started = Instant::now();
```

Replace:

```rust
    let mut log = load_native_log_or_default(&session_path);
```

with:

```rust
    let load_started = Instant::now();
    let mut log = load_native_log_or_default(&session_path);
    log.record_duration_metric(
        NativeSessionId(String::from("default")),
        Some(turn_id.clone()),
        "session_log_load",
        load_started.elapsed(),
        vec![NativeMetricAttribute {
            key: String::from("path"),
            value: session_path.to_string_lossy().into_owned(),
        }],
    );
```

Before the final fixture `log.write_to_file(&session_path)` call, add:

```rust
    log.record_duration_metric(
        NativeSessionId(String::from("default")),
        Some(turn_id.clone()),
        "native_prompt_total",
        prompt_started.elapsed(),
        vec![NativeMetricAttribute {
            key: String::from("mode"),
            value: String::from("fixture"),
        }],
    );
```

For provider mode, add a `prompt_started: Instant` field to `NativeProviderTurnRefs`:

```rust
#[derive(Debug, Clone)]
struct NativeProviderTurnRefs {
    turn: NativeTurnId,
    user_entry: NativeEntryId,
    assistant_entry: NativeEntryId,
    prompt_started: Instant,
}
```

Set it at the call site:

```rust
                prompt_started,
```

Before each provider `finish_native_prompt(...)` call, ensure `log` contains the same `native_prompt_total` event with `mode=provider`. Add this helper near `finish_native_prompt`:

```rust
fn record_native_prompt_total(
    log: &mut NativeSessionLog,
    turn_id: &NativeTurnId,
    prompt_started: Instant,
    mode: &str,
) {
    log.record_duration_metric(
        NativeSessionId(String::from("default")),
        Some(turn_id.clone()),
        "native_prompt_total",
        prompt_started.elapsed(),
        vec![NativeMetricAttribute {
            key: String::from("mode"),
            value: mode.to_owned(),
        }],
    );
}
```

Use it in fixture and provider paths instead of duplicating the event construction:

```rust
record_native_prompt_total(&mut log, &turn_id, prompt_started, "fixture");
```

and:

```rust
record_native_prompt_total(log, &ids.turn, ids.prompt_started, "provider");
```

- [ ] **Step 4: Run the focused test and native runner tests**

Run:

```bash
just dev cargo test -p yach-cli native_dogfood_loop_persists_prompt_runtime_metrics -- --exact
just dev cargo test -p yach-cli native_dogfood_loop
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-backend/src/native_runner.rs crates/yach-cli/src/main.rs
git commit -m "feat: record native prompt metrics"
```

## Task 6: Native Session Benchmark Baseline

**Files:**
- Modify: `crates/yach-bench/Cargo.toml`
- Create: `crates/yach-bench/benches/native_session.rs`

- [ ] **Step 1: Add the native session benchmark target**

Modify `crates/yach-bench/Cargo.toml` by adding:

```toml
[[bench]]
name = "native_session"
harness = false
```

and add this dependency:

```toml
yach-backend = { path = "../yach-backend" }
```

- [ ] **Step 2: Create the benchmark file**

Create `crates/yach-bench/benches/native_session.rs`:

```rust
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use yach_backend::{
    NativeEntryId, NativeSessionEvent, NativeSessionId, NativeSessionLog, NativeTurnId,
    NativeTurnOutcome,
};

fn native_session_log_with_turns(turns: u64) -> NativeSessionLog {
    let session_id = NativeSessionId(String::from("bench"));
    let mut log = NativeSessionLog::default();
    for index in 0..turns {
        let turn_id = NativeTurnId(format!("turn-{index}"));
        let user_entry_id = NativeEntryId(format!("entry-{index}-user"));
        let assistant_entry_id = NativeEntryId(format!("entry-{index}-assistant"));
        log.push(NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: user_entry_id.clone(),
            parent_entry_id: None,
            turn_id: turn_id.clone(),
            role: yach_backend::NativeRole::User,
            text: format!("prompt {index}"),
            provider: None,
        });
        log.push(NativeSessionEvent::EntryAppended {
            session_id: session_id.clone(),
            entry_id: assistant_entry_id,
            parent_entry_id: Some(user_entry_id),
            turn_id: turn_id.clone(),
            role: yach_backend::NativeRole::Assistant,
            text: format!("response {index}"),
            provider: None,
        });
        log.push(NativeSessionEvent::TurnFinished {
            session_id: session_id.clone(),
            turn_id,
            outcome: NativeTurnOutcome::Completed,
            reason: None,
        });
    }
    log
}

fn temp_native_session_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "yach-native-session-bench-{label}-{}-{nanos}.jsonl",
        std::process::id()
    ))
}

fn bench_native_session_load(c: &mut Criterion) {
    for turns in [10_u64, 100, 1_000] {
        let path = temp_native_session_path(&format!("load-{turns}"));
        let log = native_session_log_with_turns(turns);
        log.write_to_file(&path)
            .expect("benchmark fixture should write");
        c.bench_function(&format!("native_session_load_{turns}_turns"), |b| {
            b.iter(|| NativeSessionLog::load_from_file(&path).expect("log should load"))
        });
        let _ = std::fs::remove_file(path);
    }
}

fn bench_native_session_rewrite(c: &mut Criterion) {
    for turns in [10_u64, 100, 1_000] {
        let log = native_session_log_with_turns(turns);
        c.bench_function(&format!("native_session_rewrite_{turns}_turns"), |b| {
            b.iter(|| {
                let path = temp_native_session_path(&format!("rewrite-{turns}"));
                log.write_to_file(&path).expect("log should write");
                let _ = std::fs::remove_file(path);
            })
        });
    }
}

fn bench_native_session_projection(c: &mut Criterion) {
    for turns in [10_u64, 100, 1_000] {
        let log = native_session_log_with_turns(turns);
        c.bench_function(&format!("native_session_projection_{turns}_turns"), |b| {
            b.iter(|| {
                let next_turn = log.next_turn_index();
                let last_entry = log.last_entry_id();
                let transcript = log.transcript_messages();
                (next_turn, last_entry, transcript.len())
            })
        });
    }
}

criterion_group!(
    benches,
    bench_native_session_load,
    bench_native_session_rewrite,
    bench_native_session_projection
);
criterion_main!(benches);
```

- [ ] **Step 3: Run the benchmark compile check**

Run:

```bash
just dev cargo test -p yach-bench --bench native_session --no-run
```

Expected: PASS and the benchmark binary compiles.

- [ ] **Step 4: Run a short local benchmark sample**

Run:

```bash
just dev cargo bench -p yach-bench --bench native_session -- --sample-size 10
```

Expected: PASS and Criterion prints timings for `native_session_load_*`, `native_session_rewrite_*`, and `native_session_projection_*`.

- [ ] **Step 5: Commit**

```bash
git add crates/yach-bench/Cargo.toml crates/yach-bench/benches/native_session.rs
git commit -m "bench: add native session baseline"
```

## Task 7: Project Planning Docs and Full Verification

**Files:**
- Modify: `docs/project/state.md`
- Modify: `docs/project/next.md`

- [ ] **Step 1: Update active project state**

In `docs/project/state.md`, update `Last updated` to the implementation date and replace the native backend posture bullet with:

```markdown
- Native backend work is in progress behind explicit opt-in boundaries. Pi remains the default backend for now, but Native MVP work is framed around yach-owned backend primitives rather than Pi compatibility.
```

Add this bullet to `Currently Relevant Records`:

```markdown
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-resume-metrics-baseline.md`
```

- [ ] **Step 2: Update next work**

In `docs/project/next.md`, update `Last updated` to the implementation date and replace `Recommended Next Move` with:

```markdown
Resume Native MVP implementation with read/search/context tools.

Recommended first slice: add native read-only project inspection through policy-governed project roots, path metadata, text reads, and search packaging before file edits.

Why: session resume and baseline native-session metrics are in place, so the next blocker for real dogfooding is safe project understanding before mutation.
```

Update relevant sources to include:

```markdown
- `docs/superpowers/specs/2026-05-09-native-mvp-definition-design.md`
- `docs/superpowers/plans/2026-05-10-native-session-resume-metrics-baseline.md`
- `docs/plans/2026-05-05-006-plan-first-non-fixture-native-tool.md`
```

- [ ] **Step 3: Run formatting, tests, lint, and benchmark compile check**

Run:

```bash
just fmt
just test
just lint
just dev cargo test -p yach-bench --bench native_session --no-run
```

Expected: all commands pass.

- [ ] **Step 4: Run one short benchmark sample and save the key numbers in the final implementation notes**

Run:

```bash
just dev cargo bench -p yach-bench --bench native_session -- --sample-size 10
```

Expected: PASS with Criterion output. In the implementation final answer, include the measured command and the benchmark names that ran. Do not claim a performance win from this baseline alone.

- [ ] **Step 5: Commit**

```bash
git add docs/project/state.md docs/project/next.md
git commit -m "docs: update native mvp next work"
```

## Final Verification

Before marking the implementation complete, use `superpowers:verification-before-completion` and run:

```bash
git status --short --branch
just fmt
just test
just lint
just dev cargo test -p yach-bench --bench native_session --no-run
just dev cargo bench -p yach-bench --bench native_session -- --sample-size 10
```

Expected:

- `git status --short --branch` shows only intentional branch state.
- Formatting, tests, lint, and benchmark compile check pass.
- The short benchmark sample completes and prints native session load, rewrite, and projection timings.

## Self-Review

- Spec coverage: This plan covers the Native MVP session log/resume and metrics/benchmarking foundation. It does not implement read/search/context, autonomous tools, edits, verification actions, or extensions; those are separate required MVP slices.
- Placeholder scan: The plan contains concrete files, tests, commands, and code snippets for every implementation step.
- Type consistency: New session types are `NativeTranscriptMessage`, `NativeMetricAttribute`, and `NativeDurationMetric`; later tasks reference those exact names.
