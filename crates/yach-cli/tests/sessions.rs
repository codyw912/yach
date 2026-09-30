//! `yach sessions list` and `yach sessions show` against fixture JSONL logs.
//! Helpers are copied from `presets.rs` — integration test files do not share
//! modules in this crate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

trait TestUnwrap {
    type Output;
    fn test_unwrap(self) -> Self::Output;
}

impl<T, E> TestUnwrap for Result<T, E> {
    type Output = T;
    fn test_unwrap(self) -> Self::Output {
        assert!(self.is_ok());
        match self {
            Ok(value) => value,
            Err(_) => unreachable!(),
        }
    }
}

impl<T> TestUnwrap for Option<T> {
    type Output = T;
    fn test_unwrap(self) -> Self::Output {
        assert!(self.is_some());
        match self {
            Some(value) => value,
            None => unreachable!(),
        }
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .test_unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("yach-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).test_unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn sessions_command(home: &Path, sessions: &Path, project: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_yach"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("YACH_") {
            command.env_remove(&key);
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("YACH_SESSION_DIR", sessions)
        .current_dir(project);
    command
}

fn run(home: &Path, sessions: &Path, project: &Path, args: &[&str]) -> Output {
    let mut command = sessions_command(home, sessions, project);
    command.args(args);
    command.output().test_unwrap()
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).test_unwrap()
}

struct Fixture {
    home: TempDir,
    project: TempDir,
    sessions: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            home: TempDir::new("sessions-home"),
            project: TempDir::new("sessions-project"),
            sessions: TempDir::new("sessions-logs"),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        run(
            self.home.path(),
            self.sessions.path(),
            self.project.path(),
            args,
        )
    }
}

fn write_session(dir: &Path, id: &str, body: &str, mtime: SystemTime) {
    let path = dir.join(format!("{id}.jsonl"));
    fs::write(&path, body).test_unwrap();
    let file = fs::File::options().write(true).open(&path).test_unwrap();
    file.set_modified(mtime).test_unwrap();
}

/// Newest session: failed attempt then success, a timed tool, usage, completed.
fn newest_session() -> String {
    [
        r#"{"at_ms":1700000001000,"type":"entry_appended","session_id":"s-new","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"fix the failing test\nmore detail","provider":null}"#,
        r#"{"at_ms":1700000001800,"type":"provider_attempt_finished","session_id":"s-new","turn_id":"turn-0","purpose":"turn","attempt_sequence":1,"retry_index":0,"outcome":"failed","error_kind":"provider_internal","error_variant":"provider","status_code":503,"next_delay_ms":1000,"started_at_ms":1700000001000,"duration_ms":812,"provider_request_id":"gw-2","model":"gpt-fixture"}"#,
        r#"{"at_ms":1700000004000,"type":"provider_attempt_finished","session_id":"s-new","turn_id":"turn-0","purpose":"turn","attempt_sequence":2,"retry_index":1,"outcome":"succeeded","started_at_ms":1700000002830,"duration_ms":2210,"first_event_ms":410,"provider_request_id":"gw-3","model":"gpt-fixture"}"#,
        r#"{"at_ms":1700000004100,"type":"tool_request_recorded","session_id":"s-new","turn_id":"turn-0","tool_request_id":"tool-1","tool_name":"read_file","provider_call_id":"call-1","validation":{"Ok":null},"permission":"allowed","argument_summary":{"summary":"path=src/lib.rs","byte_count":16,"redacted":false,"truncated":false}}"#,
        r#"{"at_ms":1700000004200,"type":"tool_execution_finished","session_id":"s-new","turn_id":"turn-0","tool_request_id":"tool-1","outcome":"completed","reason":null,"result_summary":null,"started_at_ms":1700000005050,"duration_ms":3}"#,
        r#"{"at_ms":1700000005000,"type":"entry_appended","session_id":"s-new","entry_id":"e-assistant","parent_entry_id":"e-user","turn_id":"turn-0","role":"assistant","text":"fixed","provider":{"provider":"openai-compatible","model":"gpt-fixture","response_id":"resp-1","usage":{"input_tokens":1234,"output_tokens":210,"total_tokens":1444}}}"#,
        r#"{"at_ms":1700000005100,"type":"turn_finished","session_id":"s-new","turn_id":"turn-0","outcome":"completed","reason":null}"#,
    ]
    .join("\n")
        + "\n"
}

/// Older session: one failed turn, so list order and last_outcome are distinct.
fn older_session() -> String {
    [
        r#"{"at_ms":1600000000000,"type":"entry_appended","session_id":"s-old","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"older prompt","provider":null}"#,
        r#"{"at_ms":1600000001000,"type":"turn_finished","session_id":"s-old","turn_id":"turn-0","outcome":"failed","reason":"provider"}"#,
    ]
    .join("\n")
        + "\n"
}

/// Pre-change log: no `at_ms`, no tool or attempt timing.
fn prechange_session() -> String {
    [
        r#"{"type":"entry_appended","session_id":"s-legacy","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"legacy prompt","provider":null}"#,
        r#"{"type":"tool_request_recorded","session_id":"s-legacy","turn_id":"turn-0","tool_request_id":"tool-legacy","tool_name":"bash","validation":{"Ok":null},"permission":"allowed","argument_summary":{"summary":"command=true","byte_count":12,"redacted":false,"truncated":false}}"#,
        r#"{"type":"tool_execution_finished","session_id":"s-legacy","turn_id":"turn-0","tool_request_id":"tool-legacy","outcome":"completed","reason":null,"result_summary":null}"#,
        r#"{"type":"turn_finished","session_id":"s-legacy","turn_id":"turn-0","outcome":"completed","reason":null}"#,
    ]
    .join("\n")
        + "\n"
}

/// Attempt line is written before an earlier-started tool finish line.
fn reordered_session() -> String {
    [
        r#"{"at_ms":1800000000000,"type":"entry_appended","session_id":"s-order","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"reorder","provider":null}"#,
        r#"{"at_ms":1800000000500,"type":"tool_request_recorded","session_id":"s-order","turn_id":"turn-0","tool_request_id":"tool-early","tool_name":"read_file","validation":{"Ok":null},"permission":"allowed","argument_summary":{"summary":"path=early.rs","byte_count":13,"redacted":false,"truncated":false}}"#,
        r#"{"at_ms":1800000003000,"type":"provider_attempt_finished","session_id":"s-order","turn_id":"turn-0","purpose":"turn","attempt_sequence":2,"retry_index":0,"outcome":"succeeded","started_at_ms":1800000002000,"duration_ms":100,"model":"gpt-fixture"}"#,
        r#"{"at_ms":1800000003100,"type":"tool_execution_finished","session_id":"s-order","turn_id":"turn-0","tool_request_id":"tool-early","outcome":"completed","reason":null,"result_summary":null,"started_at_ms":1800000001000,"duration_ms":40}"#,
        r#"{"at_ms":1800000003200,"type":"turn_finished","session_id":"s-order","turn_id":"turn-0","outcome":"completed","reason":null}"#,
    ]
    .join("\n")
        + "\n"
}

fn fixture_pair() -> Fixture {
    let fixture = Fixture::new();
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    write_session(fixture.sessions.path(), "s-old", &older_session(), base);
    write_session(
        fixture.sessions.path(),
        "s-new",
        &newest_session(),
        base + Duration::from_secs(10),
    );
    fixture
}

#[test]
fn sessions_list_json_is_newest_first_with_turns_and_last_outcome() {
    let fixture = fixture_pair();
    let output = fixture.run(&["sessions", "list", "--json"]);
    assert!(
        output.status.success(),
        "list failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "json list must be one compact line: {text}");
    let value: serde_json::Value = serde_json::from_str(lines[0]).test_unwrap();
    let sessions = value.as_array().test_unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["id"], "s-new");
    assert_eq!(sessions[0]["turns"], 1);
    assert_eq!(sessions[0]["last_outcome"], "completed");
    assert_eq!(sessions[1]["id"], "s-old");
    assert_eq!(sessions[1]["turns"], 1);
    assert_eq!(sessions[1]["last_outcome"], "failed");
}

#[test]
fn sessions_show_latest_json_keeps_attempt_order_and_tool_duration() {
    let fixture = fixture_pair();
    let output = fixture.run(&["sessions", "show", "latest", "--json"]);
    assert!(
        output.status.success(),
        "show latest failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "json show must be one compact line: {text}");
    let value: serde_json::Value = serde_json::from_str(lines[0]).test_unwrap();
    assert_eq!(value["id"], "s-new");
    let turns = value["turns"].as_array().test_unwrap();
    assert_eq!(turns.len(), 1);
    let items = turns[0]["items"].as_array().test_unwrap();
    let attempts: Vec<&serde_json::Value> = items
        .iter()
        .filter(|item| item["kind"] == "provider_attempt")
        .collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["attempt"]["outcome"], "failed");
    assert_eq!(attempts[0]["attempt"]["provider_request_id"], "gw-2");
    assert_eq!(attempts[1]["attempt"]["outcome"], "succeeded");
    assert_eq!(attempts[1]["attempt"]["provider_request_id"], "gw-3");
    let tool = items
        .iter()
        .find(|item| item["kind"] == "tool")
        .test_unwrap();
    assert_eq!(tool["duration_ms"], 3);
    assert_eq!(tool["tool_name"], "read_file");
}

#[test]
fn sessions_show_text_includes_tool_attempt_request_and_outcome() {
    let fixture = fixture_pair();
    let output = fixture.run(&["sessions", "show", "s-new"]);
    assert!(
        output.status.success(),
        "show text failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    assert!(text.contains("read_file"), "{text}");
    assert!(text.contains("attempt 1 turn failed"), "{text}");
    assert!(
        !text.contains("retry 0"),
        "a first try must stay terse: {text}"
    );
    assert!(text.contains("attempt 2 retry 1 turn succeeded"), "{text}");
    assert!(text.contains("gw-2"), "{text}");
    assert!(text.contains("completed"), "{text}");
}

#[test]
fn prechange_session_renders_without_offsets_and_exits_zero() {
    let fixture = Fixture::new();
    write_session(
        fixture.sessions.path(),
        "s-legacy",
        &prechange_session(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000),
    );
    let output = fixture.run(&["sessions", "show", "s-legacy"]);
    assert!(
        output.status.success(),
        "pre-change show failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    assert!(
        !text.contains('+'),
        "pre-change render must not show offsets: {text}"
    );
    assert!(text.contains("bash"), "{text}");
    assert!(text.contains("completed"), "{text}");
}

#[test]
fn sessions_show_missing_id_exits_one_with_error_line() {
    let fixture = fixture_pair();
    let output = fixture.run(&["sessions", "show", "missing-id"]);
    assert_eq!(output.status.code(), Some(1));
    let text = stdout_text(&output);
    assert!(
        text.lines().any(|line| line.starts_with("error=")),
        "{text}"
    );
}

#[test]
fn sessions_bogus_subcommand_exits_two_with_usage() {
    let fixture = fixture_pair();
    let output = fixture.run(&["sessions", "bogus"]);
    assert_eq!(output.status.code(), Some(2));
    let text = stdout_text(&output);
    assert!(
        text.lines().any(|line| line.starts_with("error=")),
        "{text}"
    );
    assert!(text.contains("usage: yach"), "{text}");
    assert!(text.contains("sessions"), "{text}");
}

#[test]
fn sessions_show_sorts_items_by_measured_start_not_file_order() {
    let fixture = Fixture::new();
    write_session(
        fixture.sessions.path(),
        "s-order",
        &reordered_session(),
        SystemTime::UNIX_EPOCH + Duration::from_hours(500_000),
    );
    let output = fixture.run(&["sessions", "show", "s-order", "--json"]);
    assert!(
        output.status.success(),
        "reorder show failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    let value: serde_json::Value = serde_json::from_str(text.trim()).test_unwrap();
    let items = value["turns"][0]["items"].as_array().test_unwrap();
    let kinds: Vec<&str> = items
        .iter()
        .filter_map(|item| item["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        ["tool", "provider_attempt"],
        "earlier-started tool must precede the later attempt: {text}"
    );
    assert_eq!(items[0]["tool_request_id"], "tool-early");
    assert_eq!(items[1]["attempt"]["attempt_sequence"], 2);
    assert_eq!(
        value["turns"][0]["started_at_ms"], 1_800_000_000_000_u64,
        "turn start is the earliest event timing, including the user entry: {text}"
    );
    assert_eq!(
        items[0]["offset_ms"], 1_000,
        "tool that starts 1000ms after the user entry must show +1000ms: {text}"
    );
    assert_eq!(items[1]["offset_ms"], 2_000, "{text}");
}

/// A turn whose only events are stamped entries still has a start time.
fn stamped_entry_only_session() -> String {
    [
        r#"{"at_ms":1900000000000,"type":"entry_appended","session_id":"s-stamp","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"stamped only","provider":null}"#,
        r#"{"at_ms":1900000000500,"type":"turn_finished","session_id":"s-stamp","turn_id":"turn-0","outcome":"completed","reason":null}"#,
    ]
    .join("\n")
        + "\n"
}

#[test]
fn stamped_entries_still_set_the_turn_start() {
    let fixture = Fixture::new();
    write_session(
        fixture.sessions.path(),
        "s-stamp",
        &stamped_entry_only_session(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000),
    );
    let output = fixture.run(&["sessions", "show", "s-stamp", "--json"]);
    assert!(
        output.status.success(),
        "stamped show failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    let value: serde_json::Value = serde_json::from_str(text.trim()).test_unwrap();
    assert_eq!(
        value["turns"][0]["started_at_ms"], 1_900_000_000_000_u64,
        "{text}"
    );
}

/// An attempt with `started_at_ms` 0 falls back to the line's `at_ms`.
fn zero_start_attempt_session() -> String {
    [
        r#"{"at_ms":2000000000000,"type":"entry_appended","session_id":"s-zero","entry_id":"e-user","parent_entry_id":null,"turn_id":"turn-0","role":"user","text":"zero start","provider":null}"#,
        r#"{"at_ms":2000000001500,"type":"provider_attempt_finished","session_id":"s-zero","turn_id":"turn-0","purpose":"turn","attempt_sequence":1,"retry_index":0,"outcome":"succeeded","started_at_ms":0,"duration_ms":12,"model":"gpt-fixture"}"#,
        r#"{"at_ms":2000000001600,"type":"turn_finished","session_id":"s-zero","turn_id":"turn-0","outcome":"completed","reason":null}"#,
    ]
    .join("\n")
        + "\n"
}

#[test]
fn attempt_with_zero_start_uses_line_stamp() {
    let fixture = Fixture::new();
    write_session(
        fixture.sessions.path(),
        "s-zero",
        &zero_start_attempt_session(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000),
    );
    let output = fixture.run(&["sessions", "show", "s-zero", "--json"]);
    assert!(
        output.status.success(),
        "zero-start show failed: {}",
        stdout_text(&output)
    );
    let text = stdout_text(&output);
    let value: serde_json::Value = serde_json::from_str(text.trim()).test_unwrap();
    let items = value["turns"][0]["items"].as_array().test_unwrap();
    assert_eq!(items[0]["offset_ms"], 1_500, "{text}");
}

#[test]
fn sub_millisecond_mtime_orders_list_and_latest() {
    let fixture = Fixture::new();
    let older = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let newer = older + Duration::from_nanos(500);
    write_session(
        fixture.sessions.path(),
        "s-earlier",
        &older_session(),
        older,
    );
    write_session(fixture.sessions.path(), "s-later", &newest_session(), newer);
    let listed = fixture.run(&["sessions", "list", "--json"]);
    assert!(listed.status.success(), "{}", stdout_text(&listed));
    let list_text = stdout_text(&listed);
    let list_value: serde_json::Value = serde_json::from_str(list_text.trim()).test_unwrap();
    let sessions = list_value.as_array().test_unwrap();
    assert_eq!(sessions[0]["id"], "s-later", "{list_text}");
    assert_eq!(sessions[1]["id"], "s-earlier", "{list_text}");
    let shown = fixture.run(&["sessions", "show", "latest", "--json"]);
    assert!(shown.status.success(), "{}", stdout_text(&shown));
    let show_text = stdout_text(&shown);
    let show_value: serde_json::Value = serde_json::from_str(show_text.trim()).test_unwrap();
    assert_eq!(show_value["id"], "s-later", "{show_text}");
}

#[test]
fn sessions_list_reports_malformed_line_warning_and_exits_zero() {
    let fixture = Fixture::new();
    let body = [
        r#"{"at_ms":1600000000000,"type":"turn_finished","session_id":"s-warn","turn_id":"turn-0","outcome":"completed","reason":null}"#,
        "{not-json",
    ]
    .join("\n");
    write_session(
        fixture.sessions.path(),
        "s-warn",
        &body,
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000),
    );
    let listed = fixture.run(&["sessions", "list", "--json"]);
    assert!(
        listed.status.success(),
        "malformed list must still exit 0: {}",
        stdout_text(&listed)
    );
    let list_text = stdout_text(&listed);
    let value: serde_json::Value = serde_json::from_str(list_text.trim()).test_unwrap();
    let sessions = value.as_array().test_unwrap();
    assert_eq!(sessions[0]["id"], "s-warn");
    assert_eq!(sessions[0]["warnings"], 1, "{list_text}");
    assert_eq!(
        sessions[0]["turns"], 1,
        "valid lines still count: {list_text}"
    );
    let text = fixture.run(&["sessions", "list"]);
    assert!(text.status.success(), "{}", stdout_text(&text));
    let rendered = stdout_text(&text);
    assert!(
        rendered.contains("warnings=1"),
        "text list must show the warning count: {rendered}"
    );
}
