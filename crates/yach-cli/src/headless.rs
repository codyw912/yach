//! Headless driver (`yach run`): a second, non-interactive client over the
//! same `ClientEvent`/`BackendEvent` channels the TUI uses. Streams
//! progress to stderr, emits exactly one outcome JSON document on stdout
//! (or to a file), and maps outcomes to stable exit codes.
//!
//! Design: `docs/superpowers/specs/2026-07-26-headless-driver-design.md`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use yach_backend::{
    ExtensionPackageRoot, ExtensionPackageRootLoader, ProviderConfig, RunnerConfig, SessionEvent,
    SessionLog, estimate_current_context_tokens, fresh_session_id, run_native_loop,
};
use yach_proto::{
    BackendEvent, ClientEvent, LocalEditDecision, PromptOutcome, ServerEvent, ToolReviewPayload,
};

pub(crate) const EXIT_COMPLETED: u8 = 0;
pub(crate) const EXIT_TURN_FAILED: u8 = 1;
pub(crate) const EXIT_SETUP_ERROR: u8 = 2;
pub(crate) const EXIT_APPROVAL_REQUIRED: u8 = 3;
pub(crate) const EXIT_TIMEOUT: u8 = 4;

const DEFAULT_TURN_TIMEOUT_SECS: u64 = 600;
/// After cancelling a turn (timeout or approval refusal), wait this long
/// for the backend to acknowledge with `PromptFinished` before moving on.
const CANCEL_DRAIN: Duration = Duration::from_secs(5);
const OUTCOME_SCHEMA: &str = "yach-run-outcome/1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunOptions {
    pub prompts: Vec<String>,
    pub project_root: Option<PathBuf>,
    pub session_path: Option<PathBuf>,
    /// Session id continuing (or naming) a session under the project's
    /// `.yach/sessions/`; the log is loaded if it exists, so
    /// repeated invocations with the same id form one long-running
    /// headless session.
    pub session_id: Option<String>,
    /// Overrides the env-derived model (yacht substitutes `{model}` here).
    pub model: Option<String>,
    pub full_auto: bool,
    pub turn_timeout: Duration,
    /// `None` writes the outcome document to stdout.
    pub outcome_path: Option<PathBuf>,
    pub quiet: bool,
}

pub(crate) fn parse_run_args(args: &[String]) -> Result<RunOptions, String> {
    let mut prompt = None;
    let mut script = None;
    let mut project_root = None;
    let mut session_path = None;
    let mut session_id = None;
    let mut model = None;
    let mut full_auto = false;
    let mut turn_timeout_secs = DEFAULT_TURN_TIMEOUT_SECS;
    let mut outcome_path = None;
    let mut quiet = false;

    let mut index = 0;
    let value_of = |flag: &str, args: &[String], index: usize| -> Result<String, String> {
        args.get(index + 1)
            .cloned()
            .ok_or_else(|| format!("{flag} requires a value"))
    };
    while index < args.len() {
        match args[index].as_str() {
            "--prompt" => {
                prompt = Some(value_of("--prompt", args, index)?);
                index += 2;
            }
            "--script" => {
                script = Some(PathBuf::from(value_of("--script", args, index)?));
                index += 2;
            }
            "--project-root" => {
                project_root = Some(PathBuf::from(value_of("--project-root", args, index)?));
                index += 2;
            }
            "--session-path" => {
                session_path = Some(PathBuf::from(value_of("--session-path", args, index)?));
                index += 2;
            }
            "--session" => {
                let raw = value_of("--session", args, index)?;
                if raw.contains(['/', '\\']) || raw.contains("..") || raw.is_empty() {
                    return Err(format!("--session must be a plain session id, got '{raw}'"));
                }
                session_id = Some(raw);
                index += 2;
            }
            "--model" => {
                model = Some(value_of("--model", args, index)?);
                index += 2;
            }
            "--full-auto" => {
                full_auto = true;
                index += 1;
            }
            "--turn-timeout-secs" => {
                let raw = value_of("--turn-timeout-secs", args, index)?;
                turn_timeout_secs = raw
                    .parse::<u64>()
                    .ok()
                    .filter(|secs| *secs > 0)
                    .ok_or_else(|| {
                        format!("--turn-timeout-secs must be a positive integer, got '{raw}'")
                    })?;
                index += 2;
            }
            "--outcome" => {
                let raw = value_of("--outcome", args, index)?;
                outcome_path = (raw != "-").then(|| PathBuf::from(raw));
                index += 2;
            }
            "--quiet" => {
                quiet = true;
                index += 1;
            }
            other => return Err(format!("unknown 'run' flag '{other}'")),
        }
    }

    if session_id.is_some() && session_path.is_some() {
        return Err(String::from(
            "--session and --session-path are mutually exclusive",
        ));
    }
    let prompts = match (prompt, script) {
        (Some(_), Some(_)) => {
            return Err(String::from("--prompt and --script are mutually exclusive"));
        }
        (None, None) => {
            return Err(String::from("'run' requires --prompt or --script"));
        }
        (Some(prompt), None) => vec![prompt],
        (None, Some(script)) => parse_script_file(&script)?,
    };

    Ok(RunOptions {
        prompts,
        project_root,
        session_path,
        session_id,
        model,
        full_auto,
        turn_timeout: Duration::from_secs(turn_timeout_secs),
        outcome_path,
        quiet,
    })
}

/// Script files are JSON Lines, one `{"prompt": "..."}` object per turn.
fn parse_script_file(path: &std::path::Path) -> Result<Vec<String>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read script {}: {error}", path.display()))?;
    let mut prompts = Vec::new();
    for (line_index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
            format!(
                "script {} line {}: invalid JSON: {error}",
                path.display(),
                line_index + 1
            )
        })?;
        let prompt = value
            .get("prompt")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                format!(
                    "script {} line {}: expected an object with a string 'prompt' field",
                    path.display(),
                    line_index + 1
                )
            })?;
        prompts.push(String::from(prompt));
    }
    if prompts.is_empty() {
        return Err(format!("script {} contains no turns", path.display()));
    }
    Ok(prompts)
}

/// Streaming progress goes to stderr by contract (stdout carries exactly
/// the outcome JSON); routed through `Write` rather than the `eprint!`
/// macros the crate lints against.
fn stream_line(quiet: bool, line: &str) {
    if quiet {
        return;
    }
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "{line}");
}

fn stream_chunk(quiet: bool, chunk: &str) {
    if quiet {
        return;
    }
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{chunk}");
    let _ = stderr.flush();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnRunOutcome {
    Completed,
    Failed,
    Timeout,
    ApprovalRequired,
    Skipped,
}

impl TurnRunOutcome {
    /// Per-turn outcome label; approval refusal is a failure at turn
    /// granularity (the overall outcome carries `approval_required`).
    fn turn_label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed | Self::ApprovalRequired => "failed",
            Self::Timeout => "timeout",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug)]
struct TurnRun {
    prompt: String,
    outcome: TurnRunOutcome,
    failure_reason: Option<String>,
    response: String,
    duration_ms: u64,
}

/// Runs the headless session end to end; returns the process exit code.
/// Setup errors are handled by the caller before this point — from here on
/// an outcome document is always emitted.
pub(crate) fn run_headless_command(
    options: &RunOptions,
    provider: ProviderConfig,
    extension_package_roots: Vec<ExtensionPackageRoot>,
    extension_package_root_loader: Option<ExtensionPackageRootLoader>,
) -> u8 {
    let project_root = options
        .project_root
        .clone()
        .or_else(|| std::env::current_dir().ok());
    let session_path = options.session_path.clone().unwrap_or_else(|| {
        let base = project_root.clone().unwrap_or_else(|| PathBuf::from("."));
        // --session <id> names (and on rerun continues) a session under
        // the project; otherwise every invocation gets a fresh one.
        let session_file = options.session_id.clone().unwrap_or_else(fresh_session_id);
        base.join(".yach")
            .join("sessions")
            .join(format!("{session_file}.jsonl"))
    });
    if let Some(parent) = session_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        stream_line(false, "error=failed to create tokio runtime");
        return EXIT_SETUP_ERROR;
    };

    let resolved_model = provider.model.clone();
    let started = Instant::now();
    let turns = runtime.block_on(async {
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
        let backend_handle = tokio::spawn(run_native_loop(
            client_rx,
            backend_tx,
            RunnerConfig {
                session_path: session_path.clone(),
                project_root: project_root.clone(),
                provider: Some(provider),
                provider_setup_error: None,
                extension_package_roots,
                extension_package_root_loader,
                startup_trace: None,
            },
        ));
        let turns = drive_turns(&client_tx, &mut backend_rx, options).await;
        // Closing the client channel ends the loop; awaiting it flushes
        // pending session events to disk before the log is read back.
        drop(client_tx);
        let _ = backend_handle.await;
        turns
    });

    let log = SessionLog::load_from_file(&session_path).ok();
    let outcome_json = build_outcome_document(
        &turns,
        log.as_ref(),
        &resolved_model,
        &session_path,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    // stdout is line-oriented machine output (consumers read the final
    // non-empty line), so the document is compact there; the --outcome
    // file is a human-inspectable artifact and stays pretty-printed.
    let written = if let Some(path) = options.outcome_path.as_ref() {
        std::fs::write(path, format!("{outcome_json:#}\n"))
            .map_err(|error| format!("failed to write outcome to {}: {error}", path.display()))
    } else {
        let mut stdout = std::io::stdout();
        writeln!(stdout, "{outcome_json}")
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("failed to write outcome to stdout: {error}"))
    };
    if let Err(message) = written {
        stream_line(false, &format!("error={message}"));
        return EXIT_SETUP_ERROR;
    }

    match overall_outcome(&turns) {
        TurnRunOutcome::Completed => EXIT_COMPLETED,
        TurnRunOutcome::ApprovalRequired => EXIT_APPROVAL_REQUIRED,
        TurnRunOutcome::Timeout => EXIT_TIMEOUT,
        TurnRunOutcome::Failed | TurnRunOutcome::Skipped => EXIT_TURN_FAILED,
    }
}

/// Sum provider-reported usage across the log's assistant entries.
/// Returns (input, output, reported) where `reported` is false when no
/// entry carried usage (the integers are then honest zeros, flagged in
/// evidence extras rather than silently estimated).
fn sum_log_usage(log: Option<&SessionLog>) -> (u64, u64, bool) {
    let mut input: u64 = 0;
    let mut output: u64 = 0;
    let mut reported = false;
    for event in log.map(|log| log.events.as_slice()).unwrap_or_default() {
        if let SessionEvent::EntryAppended {
            provider: Some(metadata),
            ..
        } = event
            && let Some(usage) = metadata.usage
        {
            reported = true;
            input = input.saturating_add(usage.input_tokens.unwrap_or(0));
            output = output.saturating_add(usage.output_tokens.unwrap_or(0));
        }
    }
    (input, output, reported)
}

/// The stopping turn's outcome, or `Completed` when every turn completed.
fn overall_outcome(turns: &[TurnRun]) -> TurnRunOutcome {
    turns
        .iter()
        .map(|turn| turn.outcome)
        .find(|outcome| !matches!(outcome, TurnRunOutcome::Completed | TurnRunOutcome::Skipped))
        .unwrap_or(TurnRunOutcome::Completed)
}

async fn drive_turns(
    client_tx: &mpsc::UnboundedSender<ClientEvent>,
    backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    options: &RunOptions,
) -> Vec<TurnRun> {
    let mut turns = Vec::new();
    let mut stopped = false;
    for prompt in &options.prompts {
        if stopped {
            turns.push(TurnRun {
                prompt: prompt.clone(),
                outcome: TurnRunOutcome::Skipped,
                failure_reason: None,
                response: String::new(),
                duration_ms: 0,
            });
            continue;
        }
        let turn = drive_one_turn(client_tx, backend_rx, prompt, options).await;
        stopped = !matches!(turn.outcome, TurnRunOutcome::Completed);
        turns.push(turn);
    }
    turns
}

async fn drive_one_turn(
    client_tx: &mpsc::UnboundedSender<ClientEvent>,
    backend_rx: &mut mpsc::UnboundedReceiver<BackendEvent>,
    prompt: &str,
    options: &RunOptions,
) -> TurnRun {
    let started = Instant::now();
    let deadline = started + options.turn_timeout;
    let mut turn = TurnRun {
        prompt: String::from(prompt),
        outcome: TurnRunOutcome::Failed,
        failure_reason: None,
        response: String::new(),
        duration_ms: 0,
    };
    if client_tx
        .send(ClientEvent::PromptSubmitted {
            session_id: String::from("default"),
            prompt: String::from(prompt),
        })
        .is_err()
    {
        turn.failure_reason = Some(String::from("backend channel closed before prompt"));
        turn.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        return turn;
    }

    let mut cancel_sent_for: Option<TurnRunOutcome> = None;
    loop {
        let now = Instant::now();
        let remaining = if let Some(pending) = cancel_sent_for {
            // Already cancelling: give the backend a bounded window to
            // acknowledge, then stop waiting.
            let drain_deadline = deadline + CANCEL_DRAIN;
            if now >= drain_deadline {
                turn.outcome = pending;
                break;
            }
            drain_deadline - now
        } else if now >= deadline {
            let _ = client_tx.send(ClientEvent::PromptCancelled {
                session_id: String::from("default"),
            });
            turn.failure_reason = Some(format!(
                "turn exceeded --turn-timeout-secs {}",
                options.turn_timeout.as_secs()
            ));
            cancel_sent_for = Some(TurnRunOutcome::Timeout);
            continue;
        } else {
            deadline - now
        };

        let Ok(received) = tokio::time::timeout(remaining, backend_rx.recv()).await else {
            continue;
        };
        let Some(event) = received else {
            turn.outcome = cancel_sent_for.unwrap_or(TurnRunOutcome::Failed);
            turn.failure_reason
                .get_or_insert_with(|| String::from("backend channel closed mid-turn"));
            break;
        };
        let BackendEvent::Server(event) = event else {
            continue;
        };
        match event {
            ServerEvent::PromptDelta { delta, .. } => {
                turn.response.push_str(&delta);
                stream_chunk(options.quiet, &delta);
            }
            ServerEvent::StatusUpdated { message } => {
                stream_line(options.quiet, &format!("status: {message}"));
            }
            ServerEvent::ToolCallStarted { tool_name, .. } => {
                stream_line(options.quiet, &format!("tool: {tool_name}"));
            }
            ServerEvent::ToolReviewRequested {
                request_id,
                tool_name,
                payload,
            } => {
                if options.full_auto {
                    let (preview_id, permission_decision_id) = match payload {
                        ToolReviewPayload::LocalEdit { preview } => {
                            (preview.preview_id, preview.permission_decision_id)
                        }
                        ToolReviewPayload::Command { command } => {
                            (command.review_id, command.permission_decision_id)
                        }
                    };
                    stream_line(
                        options.quiet,
                        &format!("review: auto-approving {tool_name} (--full-auto)"),
                    );
                    let _ = client_tx.send(ClientEvent::ToolReviewDecisionSubmitted {
                        request_id,
                        preview_id,
                        permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    });
                } else if cancel_sent_for.is_none() {
                    turn.failure_reason = Some(format!(
                        "approval required for tool '{tool_name}' (run with --full-auto)"
                    ));
                    let _ = client_tx.send(ClientEvent::PromptCancelled {
                        session_id: String::from("default"),
                    });
                    cancel_sent_for = Some(TurnRunOutcome::ApprovalRequired);
                }
            }
            ServerEvent::PromptFinished {
                outcome, message, ..
            } => {
                turn.outcome = cancel_sent_for.unwrap_or(match outcome {
                    PromptOutcome::Completed => TurnRunOutcome::Completed,
                    PromptOutcome::Failed | PromptOutcome::Cancelled => TurnRunOutcome::Failed,
                });
                if turn.failure_reason.is_none()
                    && !matches!(turn.outcome, TurnRunOutcome::Completed)
                {
                    turn.failure_reason = message;
                }
                break;
            }
            _ => {}
        }
    }
    stream_line(options.quiet, "");
    turn.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    turn
}

/// Per-turn enrichment read back from the session log — the log is
/// authoritative for tool activity, compactions, and failure reasons.
#[derive(Debug, Default)]
struct TurnLogFacts {
    tool_calls: BTreeMap<String, u64>,
    compactions: u64,
    finish_reason: Option<String>,
}

fn turn_facts_from_log(log: &SessionLog, executed_turns: usize) -> Vec<TurnLogFacts> {
    let mut order: Vec<&str> = Vec::new();
    let mut facts: BTreeMap<&str, TurnLogFacts> = BTreeMap::new();
    for event in &log.events {
        let turn_id = match event {
            SessionEvent::EntryAppended { turn_id, .. }
            | SessionEvent::ToolRequestRecorded { turn_id, .. }
            | SessionEvent::TurnFinished { turn_id, .. }
            | SessionEvent::CompactionCheckpoint { turn_id, .. } => turn_id.0.as_str(),
            _ => continue,
        };
        if !facts.contains_key(turn_id) {
            order.push(turn_id);
            facts.insert(turn_id, TurnLogFacts::default());
        }
        let Some(entry) = facts.get_mut(turn_id) else {
            continue;
        };
        match event {
            SessionEvent::ToolRequestRecorded { tool_name, .. } => {
                *entry.tool_calls.entry(tool_name.clone()).or_insert(0) += 1;
            }
            SessionEvent::CompactionCheckpoint { .. } => entry.compactions += 1,
            SessionEvent::TurnFinished { reason, .. } => {
                entry.finish_reason.clone_from(reason);
            }
            _ => {}
        }
    }
    // The driver's turns are the newest N in the log (a caller-supplied
    // --session-path may hold earlier history).
    let skip = order.len().saturating_sub(executed_turns);
    order
        .into_iter()
        .skip(skip)
        .filter_map(|turn_id| facts.remove(turn_id))
        .collect()
}

fn build_outcome_document(
    turns: &[TurnRun],
    log: Option<&SessionLog>,
    resolved_model: &str,
    session_path: &std::path::Path,
    duration_ms: u64,
) -> serde_json::Value {
    let executed = turns
        .iter()
        .filter(|turn| !matches!(turn.outcome, TurnRunOutcome::Skipped))
        .count();
    let mut log_facts = log
        .map(|log| turn_facts_from_log(log, executed))
        .unwrap_or_default();
    // Pad so zip below stays aligned if the log is missing turns (e.g. a
    // turn cancelled before any event was persisted).
    while log_facts.len() < executed {
        log_facts.insert(0, TurnLogFacts::default());
    }

    let mut facts_iter = log_facts.into_iter();
    let turn_documents: Vec<serde_json::Value> = turns
        .iter()
        .map(|turn| {
            let facts = if matches!(turn.outcome, TurnRunOutcome::Skipped) {
                TurnLogFacts::default()
            } else {
                facts_iter.next().unwrap_or_default()
            };
            let failure_reason = turn.failure_reason.clone().or(facts.finish_reason);
            serde_json::json!({
                "prompt": turn.prompt,
                "outcome": turn.outcome.turn_label(),
                "failure_reason": failure_reason,
                "tool_calls": facts
                    .tool_calls
                    .iter()
                    .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
                    .collect::<Vec<_>>(),
                "compactions": facts.compactions,
                "duration_ms": turn.duration_ms,
            })
        })
        .collect();

    let response = turns
        .iter()
        .rfind(|turn| !matches!(turn.outcome, TurnRunOutcome::Skipped))
        .map(|turn| turn.response.clone())
        .unwrap_or_default();
    // Provider-reported token sums across the session's requests. Always
    // present so dotted-path consumers never dangle: honest zeros with
    // `reported: false` when the provider reported nothing — never
    // estimated.
    let (input_tokens, output_tokens, usage_reported) = sum_log_usage(log);
    // Session-level tool totals, aggregated from the per-turn documents.
    let mut tool_totals: BTreeMap<String, u64> = BTreeMap::new();
    for turn in &turn_documents {
        for call in turn["tool_calls"].as_array().unwrap_or(&Vec::new()) {
            if let (Some(name), Some(count)) = (call["name"].as_str(), call["count"].as_u64()) {
                *tool_totals.entry(String::from(name)).or_insert(0) += count;
            }
        }
    }
    let overall = match overall_outcome(turns) {
        TurnRunOutcome::Completed => "completed",
        TurnRunOutcome::ApprovalRequired => "approval_required",
        TurnRunOutcome::Timeout => "timeout",
        TurnRunOutcome::Failed | TurnRunOutcome::Skipped => "failed",
    };

    serde_json::json!({
        "schema": OUTCOME_SCHEMA,
        "outcome": overall,
        "response": response,
        "model": resolved_model,
        "turns": turn_documents,
        "tool_calls": tool_totals
            .iter()
            .map(|(name, count)| serde_json::json!({ "name": name, "count": count }))
            .collect::<Vec<_>>(),
        "tokens": log.map(|log| {
            serde_json::json!({
                "context_estimate": estimate_current_context_tokens(log),
                "provenance": "estimated",
            })
        }),
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "reported": usage_reported,
        },
        "session_path": session_path.to_string_lossy(),
        "duration_ms": duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| String::from(*arg)).collect()
    }

    #[test]
    fn parse_run_args_requires_a_prompt_source() {
        let missing = parse_run_args(&[]);
        assert!(missing.is_err());
        let both = parse_run_args(&args(&["--prompt", "hi", "--script", "s.jsonl"]));
        assert!(both.is_err());
    }

    #[test]
    fn parse_run_args_reads_flags() {
        let parsed = parse_run_args(&args(&[
            "--prompt",
            "do the thing",
            "--project-root",
            "/tmp/fixture",
            "--full-auto",
            "--turn-timeout-secs",
            "30",
            "--outcome",
            "-",
            "--quiet",
        ]));
        assert_eq!(
            parsed,
            Ok(RunOptions {
                prompts: vec![String::from("do the thing")],
                project_root: Some(PathBuf::from("/tmp/fixture")),
                session_path: None,
                session_id: None,
                model: None,
                full_auto: true,
                turn_timeout: Duration::from_secs(30),
                outcome_path: None,
                quiet: true,
            })
        );
    }

    #[test]
    fn parse_run_args_session_id_is_validated_and_exclusive() {
        let parsed = parse_run_args(&args(&["--prompt", "hi", "--session", "nightly-refactor"]));
        assert_eq!(
            parsed.map(|options| options.session_id),
            Ok(Some(String::from("nightly-refactor")))
        );
        // Path-shaped ids are rejected; ids name files under the project's
        // session directory.
        assert!(parse_run_args(&args(&["--prompt", "hi", "--session", "a/b"])).is_err());
        assert!(parse_run_args(&args(&["--prompt", "hi", "--session", ".."])).is_err());
        assert!(
            parse_run_args(&args(&[
                "--prompt",
                "hi",
                "--session",
                "x",
                "--session-path",
                "s.jsonl"
            ]))
            .is_err()
        );
    }

    #[test]
    fn parse_run_args_rejects_unknown_flags_and_bad_timeouts() {
        assert!(parse_run_args(&args(&["--prompt", "hi", "--nope"])).is_err());
        assert!(parse_run_args(&args(&["--prompt", "hi", "--turn-timeout-secs", "0"])).is_err());
        assert!(parse_run_args(&args(&["--prompt", "hi", "--turn-timeout-secs", "abc"])).is_err());
    }

    #[test]
    fn script_files_parse_one_prompt_per_line() {
        let dir = std::env::temp_dir().join(format!(
            "yach-headless-script-{}-{}",
            std::process::id(),
            line!()
        ));
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let path = dir.join("script.jsonl");
        assert!(
            std::fs::write(
                &path,
                "{\"prompt\": \"first\"}\n\n{\"prompt\": \"second\"}\n"
            )
            .is_ok()
        );
        let parsed = parse_script_file(&path);
        assert_eq!(
            parsed,
            Ok(vec![String::from("first"), String::from("second")])
        );

        let bad = dir.join("bad.jsonl");
        assert!(std::fs::write(&bad, "{\"not_prompt\": 1}\n").is_ok());
        assert!(parse_script_file(&bad).is_err());
    }

    fn quiet_options(prompts: Vec<String>, full_auto: bool) -> RunOptions {
        RunOptions {
            prompts,
            project_root: None,
            session_path: None,
            session_id: None,
            model: None,
            full_auto,
            turn_timeout: Duration::from_secs(5),
            outcome_path: None,
            quiet: true,
        }
    }

    /// A scripted fake backend: replies to each prompt with the given
    /// event batches, in order.
    fn spawn_fake_backend(
        mut client_rx: mpsc::UnboundedReceiver<ClientEvent>,
        backend_tx: mpsc::UnboundedSender<BackendEvent>,
        mut responses: Vec<Vec<ServerEvent>>,
    ) -> tokio::task::JoinHandle<Vec<ClientEvent>> {
        responses.reverse();
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(event) = client_rx.recv().await {
                let respond = matches!(event, ClientEvent::PromptSubmitted { .. });
                seen.push(event);
                if respond && let Some(batch) = responses.pop() {
                    for server_event in batch {
                        let _ = backend_tx.send(BackendEvent::Server(server_event));
                    }
                }
            }
            seen
        })
    }

    fn finished(outcome: PromptOutcome, message: Option<&str>) -> ServerEvent {
        ServerEvent::PromptFinished {
            session_id: String::from("default"),
            outcome,
            message: message.map(String::from),
        }
    }

    fn delta(text: &str) -> ServerEvent {
        ServerEvent::PromptDelta {
            session_id: String::from("default"),
            delta: String::from(text),
        }
    }

    #[test]
    fn drive_turns_collects_responses_and_stops_on_failure() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let fake = spawn_fake_backend(
                client_rx,
                backend_tx,
                vec![
                    vec![
                        delta("first "),
                        delta("answer"),
                        finished(PromptOutcome::Completed, None),
                    ],
                    vec![finished(PromptOutcome::Failed, Some("provider exploded"))],
                ],
            );
            let options = quiet_options(
                vec![
                    String::from("one"),
                    String::from("two"),
                    String::from("three"),
                ],
                false,
            );
            let turns = drive_turns(&client_tx, &mut backend_rx, &options).await;
            drop(client_tx);
            let _ = fake.await;

            assert_eq!(turns.len(), 3);
            assert_eq!(turns[0].outcome, TurnRunOutcome::Completed);
            assert_eq!(turns[0].response, "first answer");
            assert_eq!(turns[1].outcome, TurnRunOutcome::Failed);
            assert_eq!(
                turns[1].failure_reason.as_deref(),
                Some("provider exploded")
            );
            assert_eq!(turns[2].outcome, TurnRunOutcome::Skipped);
            assert_eq!(overall_outcome(&turns), TurnRunOutcome::Failed);
        });
    }

    #[test]
    fn review_request_without_full_auto_cancels_and_reports_approval_required() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let review = ServerEvent::ToolReviewRequested {
                request_id: String::from("review-1"),
                tool_name: String::from("write_text_file"),
                payload: ToolReviewPayload::Command {
                    command: yach_proto::CommandReviewSummary {
                        review_id: String::from("cmd-review-1"),
                        permission_decision_id: String::from("perm-1"),
                        command: String::from("rm -rf ."),
                        workdir: None,
                        timeout_ms: 1_000,
                    },
                },
            };
            let fake = spawn_fake_backend(
                client_rx,
                backend_tx,
                vec![vec![review, finished(PromptOutcome::Cancelled, None)]],
            );
            let options = quiet_options(vec![String::from("edit stuff")], false);
            let turns = drive_turns(&client_tx, &mut backend_rx, &options).await;
            drop(client_tx);
            let seen = fake.await;

            assert_eq!(turns.len(), 1);
            assert_eq!(turns[0].outcome, TurnRunOutcome::ApprovalRequired);
            let reason = turns[0].failure_reason.as_deref().unwrap_or_default();
            assert!(reason.contains("approval required"));
            assert!(reason.contains("--full-auto"));
            let Ok(seen) = seen else {
                unreachable!("fake backend must join");
            };
            assert!(
                seen.iter()
                    .any(|event| matches!(event, ClientEvent::PromptCancelled { .. }))
            );
            assert_eq!(overall_outcome(&turns), TurnRunOutcome::ApprovalRequired);
        });
    }

    #[test]
    fn review_request_with_full_auto_approves_with_payload_ids() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            let review = ServerEvent::ToolReviewRequested {
                request_id: String::from("review-1"),
                tool_name: String::from("bash"),
                payload: ToolReviewPayload::Command {
                    command: yach_proto::CommandReviewSummary {
                        review_id: String::from("cmd-review-1"),
                        permission_decision_id: String::from("perm-1"),
                        command: String::from("cargo test"),
                        workdir: None,
                        timeout_ms: 1_000,
                    },
                },
            };
            let fake = spawn_fake_backend(
                client_rx,
                backend_tx,
                vec![vec![
                    review,
                    delta("done"),
                    finished(PromptOutcome::Completed, None),
                ]],
            );
            let options = quiet_options(vec![String::from("run tests")], true);
            let turns = drive_turns(&client_tx, &mut backend_rx, &options).await;
            drop(client_tx);
            let seen = fake.await;

            assert_eq!(turns.len(), 1);
            assert_eq!(turns[0].outcome, TurnRunOutcome::Completed);
            let Ok(seen) = seen else {
                unreachable!("fake backend must join");
            };
            let approved = seen.iter().any(|event| {
                matches!(
                    event,
                    ClientEvent::ToolReviewDecisionSubmitted {
                        request_id,
                        preview_id,
                        permission_decision_id,
                        decision: LocalEditDecision::Apply,
                    } if request_id == "review-1"
                        && preview_id == "cmd-review-1"
                        && permission_decision_id == "perm-1"
                )
            });
            assert!(approved);
        });
    }

    #[test]
    fn turn_timeout_cancels_and_reports_timeout() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        assert!(runtime.is_ok());
        let Ok(runtime) = runtime else {
            return;
        };
        runtime.block_on(async {
            let (client_tx, client_rx) = mpsc::unbounded_channel();
            let (backend_tx, mut backend_rx) = mpsc::unbounded_channel();
            // Backend that never finishes the prompt, then acknowledges the
            // cancel.
            let fake = tokio::spawn(async move {
                let mut client_rx = client_rx;
                let mut cancelled = false;
                while let Some(event) = client_rx.recv().await {
                    if matches!(event, ClientEvent::PromptCancelled { .. }) && !cancelled {
                        cancelled = true;
                        let _ =
                            backend_tx.send(BackendEvent::Server(ServerEvent::PromptFinished {
                                session_id: String::from("default"),
                                outcome: PromptOutcome::Cancelled,
                                message: None,
                            }));
                    }
                }
            });
            let mut options = quiet_options(vec![String::from("hang forever")], false);
            options.turn_timeout = Duration::from_millis(100);
            let turns = drive_turns(&client_tx, &mut backend_rx, &options).await;
            drop(client_tx);
            let _ = fake.await;

            assert_eq!(turns.len(), 1);
            assert_eq!(turns[0].outcome, TurnRunOutcome::Timeout);
            assert_eq!(overall_outcome(&turns), TurnRunOutcome::Timeout);
        });
    }

    #[test]
    fn log_usage_sums_across_assistant_entries() {
        use yach_backend::{EntryId, ProviderMetadata, ProviderUsage, Role, SessionId, TurnId};
        let mut log = SessionLog::default();
        for (index, (input, output)) in [(100_u64, 20_u64), (200, 30)].iter().enumerate() {
            log.push(SessionEvent::EntryAppended {
                session_id: SessionId(String::from("default")),
                entry_id: EntryId(format!("entry-{index}")),
                parent_entry_id: None,
                turn_id: TurnId(format!("turn-{index}")),
                role: Role::Assistant,
                text: String::from("answer"),
                provider: Some(ProviderMetadata {
                    provider: String::from("anthropic"),
                    model: String::from("test-model"),
                    response_id: None,
                    usage: Some(ProviderUsage {
                        input_tokens: Some(*input),
                        output_tokens: Some(*output),
                        total_tokens: Some(input + output),
                    }),
                }),
            });
        }
        assert_eq!(sum_log_usage(Some(&log)), (300, 50, true));
        assert_eq!(sum_log_usage(None), (0, 0, false));
    }

    #[test]
    fn outcome_document_carries_schema_turns_and_estimated_tokens() {
        let turns = vec![
            TurnRun {
                prompt: String::from("one"),
                outcome: TurnRunOutcome::Completed,
                failure_reason: None,
                response: String::from("answer one"),
                duration_ms: 1_200,
            },
            TurnRun {
                prompt: String::from("two"),
                outcome: TurnRunOutcome::Failed,
                failure_reason: Some(String::from("provider exploded")),
                response: String::new(),
                duration_ms: 300,
            },
        ];
        let document = build_outcome_document(
            &turns,
            None,
            "test-model",
            std::path::Path::new("/tmp/session.jsonl"),
            2_000,
        );
        assert_eq!(document["schema"], OUTCOME_SCHEMA);
        assert_eq!(document["outcome"], "failed");
        assert_eq!(document["response"], "");
        assert_eq!(document["model"], "test-model");
        assert_eq!(document["turns"][0]["outcome"], "completed");
        assert_eq!(document["turns"][1]["failure_reason"], "provider exploded");
        assert_eq!(document["tokens"], serde_json::Value::Null);
        // No log → honest zeros flagged unreported, never estimated; the
        // object is always present so dotted-path consumers never dangle.
        assert_eq!(document["usage"]["input_tokens"], 0);
        assert_eq!(document["usage"]["output_tokens"], 0);
        assert_eq!(document["usage"]["reported"], false);
        assert_eq!(document["session_path"], "/tmp/session.jsonl");
        // The document is compact when serialized plainly (stdout is
        // line-oriented for final-line consumers).
        assert!(!format!("{document}").contains('\n'));
    }
}
