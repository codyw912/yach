//! Read-only inspector for native session logs (`yach sessions`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use yach_backend::{
    ClassificationSource, CompactionReason, ProviderAttemptOutcome, ProviderAttemptPurpose,
    ProviderAttemptSummary, Role, SessionEvent, SessionLoadWarning, SessionLog, StampedLoadResult,
    StampedSessionEvent, project_session_log_dir, session_id_from_log_path, session_log_path_in,
};

const PROMPT_LIMIT: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionsCommand {
    List { json: bool },
    Show { id: SessionSelector, json: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionSelector {
    Latest,
    Id(String),
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionListing {
    pub id: String,
    pub started_at_ms: Option<u64>,
    pub modified_at_ms: Option<u64>,
    pub turns: usize,
    pub last_outcome: Option<String>,
    pub model: Option<String>,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionTimeline {
    pub id: String,
    pub warnings: Vec<String>,
    pub turns: Vec<TurnTimeline>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TurnTimeline {
    pub turn_id: String,
    pub started_at_ms: Option<u64>,
    pub prompt: Option<String>,
    pub items: Vec<TimelineItem>,
    pub outcome: Option<String>,
    pub reason: Option<String>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TimelineItem {
    ProviderAttempt {
        offset_ms: Option<u64>,
        attempt: ProviderAttemptSummary,
    },
    Tool {
        offset_ms: Option<u64>,
        tool_request_id: String,
        tool_name: Option<String>,
        outcome: String,
        duration_ms: Option<u64>,
        arguments: Option<String>,
    },
    Permission {
        offset_ms: Option<u64>,
        summary: String,
    },
    Review {
        offset_ms: Option<u64>,
        tool_request_id: String,
        decision: String,
    },
    Compaction {
        offset_ms: Option<u64>,
        tokens_before: u64,
        tokens_after_estimate: u64,
        reason: String,
    },
}

#[derive(Clone)]
struct PendingItem {
    file_index: usize,
    timing_ms: Option<u64>,
    item: TimelineItem,
}

struct TurnBuild {
    turn_id: String,
    earliest_timing: Option<u64>,
    prompt: Option<String>,
    items: Vec<PendingItem>,
    outcome: Option<String>,
    reason: Option<String>,
    usage: Option<serde_json::Value>,
    tool_names: BTreeMap<String, String>,
    tool_arguments: BTreeMap<String, String>,
}

pub(crate) fn sessions_command_from_args(args: &[String]) -> Result<SessionsCommand, String> {
    let Some(action) = args.first().map(String::as_str) else {
        return Err(String::from(
            "sessions requires 'list' or 'show <session-id|latest>'",
        ));
    };
    match action {
        "list" => {
            let json = flag_json(&args[1..])?;
            Ok(SessionsCommand::List { json })
        }
        "show" => {
            let Some(id) = args.get(1) else {
                return Err(String::from("sessions show requires <session-id|latest>"));
            };
            if id.starts_with('-') {
                return Err(String::from("sessions show requires <session-id|latest>"));
            }
            let json = flag_json(&args[2..])?;
            let id = if id == "latest" {
                SessionSelector::Latest
            } else {
                SessionSelector::Id(id.clone())
            };
            Ok(SessionsCommand::Show { id, json })
        }
        other => Err(format!("unknown sessions command '{other}'")),
    }
}

fn flag_json(args: &[String]) -> Result<bool, String> {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other => return Err(format!("unknown sessions option '{other}'")),
        }
    }
    Ok(json)
}

pub(crate) fn run_sessions(
    command: &SessionsCommand,
    project_root: &Path,
) -> Result<Vec<String>, String> {
    let dir = project_session_log_dir(project_root).map_err(|error| error.to_string())?;
    match command {
        SessionsCommand::List { json } => {
            let listings = list_sessions(&dir)?;
            if *json {
                Ok(vec![json_line(&listings)?])
            } else {
                Ok(listings.iter().map(render_listing).collect())
            }
        }
        SessionsCommand::Show { id, json } => {
            let (session_id, path) = resolve_show(&dir, id)?;
            let loaded = SessionLog::load_stamped_from_file(&path)
                .map_err(|error| format!("failed to read session {session_id}: {error}"))?;
            let timeline = timeline_from_load(&session_id, &loaded);
            if *json {
                Ok(vec![json_line(&timeline)?])
            } else {
                Ok(render_timeline(&timeline))
            }
        }
    }
}

fn json_line(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("failed to encode sessions json: {error}"))
}

fn list_sessions(dir: &Path) -> Result<Vec<SessionListing>, String> {
    let mut files = session_files(dir)?;
    sort_sessions_newest_first(&mut files);
    files
        .into_iter()
        .map(|file| summarize_session(&file))
        .collect()
}

struct SessionFile {
    id: String,
    path: PathBuf,
    modified: Option<SystemTime>,
    modified_at_ms: Option<u64>,
}

fn session_files(dir: &Path) -> Result<Vec<SessionFile>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let Some(id) = session_id_from_log_path(&path) else {
            continue;
        };
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        let modified_at_ms = modified.and_then(system_time_ms);
        files.push(SessionFile {
            id,
            path,
            modified,
            modified_at_ms,
        });
    }
    Ok(files)
}

fn summarize_session(file: &SessionFile) -> Result<SessionListing, String> {
    let loaded = SessionLog::load_stamped_from_file(&file.path)
        .map_err(|error| format!("failed to read session {}: {error}", file.id))?;
    let mut turns = Vec::new();
    let mut last_outcome = None;
    let mut model = None;
    let mut started_at_ms = None;
    for stamped in &loaded.events {
        if started_at_ms.is_none() {
            started_at_ms = stamped.at_ms.filter(|at| *at != 0);
        }
        match &stamped.event {
            SessionEvent::TurnFinished {
                turn_id, outcome, ..
            } => {
                remember_turn(&mut turns, &turn_id.0);
                last_outcome = Some(snake_label(outcome));
            }
            SessionEvent::EntryAppended {
                turn_id,
                role: Role::Assistant,
                provider: Some(provider),
                ..
            } => {
                remember_turn(&mut turns, &turn_id.0);
                if !provider.model.is_empty() {
                    model = Some(provider.model.clone());
                }
            }
            SessionEvent::ProviderAttemptFinished {
                turn_id, attempt, ..
            } => {
                remember_turn(&mut turns, &turn_id.0);
                if !attempt.model.is_empty() {
                    model = Some(attempt.model.clone());
                }
            }
            other => {
                if let Some(turn_id) = event_turn_id(other) {
                    remember_turn(&mut turns, turn_id);
                }
            }
        }
    }
    Ok(SessionListing {
        id: file.id.clone(),
        started_at_ms: started_at_ms.or(file.modified_at_ms),
        modified_at_ms: file.modified_at_ms,
        turns: turns.len(),
        last_outcome,
        model,
        warnings: loaded.warnings.len(),
    })
}

fn remember_turn(turns: &mut Vec<String>, turn_id: &str) {
    if !turns.iter().any(|existing| existing == turn_id) {
        turns.push(String::from(turn_id));
    }
}

fn resolve_show(dir: &Path, selector: &SessionSelector) -> Result<(String, PathBuf), String> {
    match selector {
        SessionSelector::Latest => {
            let mut files = session_files(dir)?;
            sort_sessions_newest_first(&mut files);
            files
                .into_iter()
                .next()
                .map(|file| (file.id, file.path))
                .ok_or_else(|| String::from("no sessions found"))
        }
        SessionSelector::Id(id) => {
            let path = session_log_path_in(dir, id);
            if !path.is_file() {
                return Err(format!("session not found: {id}"));
            }
            Ok((id.clone(), path))
        }
    }
}

fn timeline_from_load(id: &str, loaded: &StampedLoadResult) -> SessionTimeline {
    let mut order = Vec::new();
    let mut turns: BTreeMap<String, TurnBuild> = BTreeMap::new();
    for (file_index, stamped) in loaded.events.iter().enumerate() {
        let Some(turn_id) = event_turn_id(&stamped.event) else {
            continue;
        };
        if !turns.contains_key(turn_id) {
            order.push(String::from(turn_id));
            turns.insert(
                String::from(turn_id),
                TurnBuild {
                    turn_id: String::from(turn_id),
                    earliest_timing: None,
                    prompt: None,
                    items: Vec::new(),
                    outcome: None,
                    reason: None,
                    usage: None,
                    tool_names: BTreeMap::new(),
                    tool_arguments: BTreeMap::new(),
                },
            );
        }
        let Some(turn) = turns.get_mut(turn_id) else {
            continue;
        };
        apply_event(turn, file_index, stamped);
    }

    let turns = order
        .into_iter()
        .filter_map(|turn_id| turns.remove(&turn_id))
        .map(finish_turn)
        .collect();
    SessionTimeline {
        id: String::from(id),
        warnings: loaded.warnings.iter().map(warning_label).collect(),
        turns,
    }
}

fn apply_event(turn: &mut TurnBuild, file_index: usize, stamped: &StampedSessionEvent) {
    let at_ms = known_timing(stamped.at_ms);
    note_timing(turn, at_ms);
    match &stamped.event {
        SessionEvent::EntryAppended {
            role,
            text,
            provider,
            ..
        } => {
            if *role == Role::User && turn.prompt.is_none() {
                turn.prompt = Some(prompt_preview(text));
            }
            if *role == Role::Assistant
                && let Some(provider) = provider
                && let Some(usage) = provider.usage
            {
                turn.usage = serde_json::to_value(usage).ok();
            }
        }
        SessionEvent::ProviderAttemptFinished { attempt, .. } => {
            let timing = known_timing(Some(attempt.started_at_ms)).or(at_ms);
            note_timing(turn, timing);
            turn.items.push(PendingItem {
                file_index,
                timing_ms: timing,
                item: TimelineItem::ProviderAttempt {
                    offset_ms: None,
                    attempt: attempt.clone(),
                },
            });
        }
        SessionEvent::ToolRequestRecorded {
            tool_request_id,
            tool_name,
            argument_summary,
            ..
        } => {
            turn.tool_names
                .insert(tool_request_id.0.clone(), tool_name.clone());
            turn.tool_arguments
                .insert(tool_request_id.0.clone(), argument_summary.summary.clone());
        }
        SessionEvent::ToolExecutionFinished {
            tool_request_id,
            outcome,
            started_at_ms,
            duration_ms,
            ..
        } => {
            let timing = known_timing(*started_at_ms).or(at_ms);
            note_timing(turn, timing);
            let id = tool_request_id.0.clone();
            turn.items.push(PendingItem {
                file_index,
                timing_ms: timing,
                item: TimelineItem::Tool {
                    offset_ms: None,
                    tool_request_id: id.clone(),
                    tool_name: turn.tool_names.get(&id).cloned(),
                    outcome: snake_label(outcome),
                    duration_ms: *duration_ms,
                    arguments: turn.tool_arguments.get(&id).cloned(),
                },
            });
        }
        SessionEvent::PermissionDecisionRecorded { summary, .. } => {
            turn.items.push(PendingItem {
                file_index,
                timing_ms: at_ms,
                item: TimelineItem::Permission {
                    offset_ms: None,
                    summary: format!(
                        "{} {} {}",
                        snake_label(&summary.capability),
                        snake_label(summary.outcome),
                        summary.reason
                    ),
                },
            });
        }
        SessionEvent::ToolReviewDecisionRecorded {
            tool_request_id,
            decision,
            ..
        } => {
            turn.items.push(PendingItem {
                file_index,
                timing_ms: at_ms,
                item: TimelineItem::Review {
                    offset_ms: None,
                    tool_request_id: tool_request_id.0.clone(),
                    decision: snake_label(decision),
                },
            });
        }
        SessionEvent::CompactionCheckpoint {
            tokens_before,
            tokens_after_estimate,
            reason,
            ..
        } => {
            turn.items.push(PendingItem {
                file_index,
                timing_ms: at_ms,
                item: TimelineItem::Compaction {
                    offset_ms: None,
                    tokens_before: *tokens_before,
                    tokens_after_estimate: *tokens_after_estimate,
                    reason: compaction_reason_label(*reason),
                },
            });
        }
        SessionEvent::TurnFinished {
            outcome, reason, ..
        } => {
            turn.outcome = Some(snake_label(outcome));
            turn.reason.clone_from(reason);
        }
        _ => {}
    }
}

fn finish_turn(mut turn: TurnBuild) -> TurnTimeline {
    let origin = turn.earliest_timing;
    sort_items(&mut turn.items);
    for item in &mut turn.items {
        let offset = item_offset(item.timing_ms, origin);
        set_offset(&mut item.item, offset);
    }
    TurnTimeline {
        turn_id: turn.turn_id,
        started_at_ms: origin,
        prompt: turn.prompt,
        items: turn.items.into_iter().map(|item| item.item).collect(),
        outcome: turn.outcome,
        reason: turn.reason,
        usage: turn.usage,
    }
}

struct SortKey {
    anchor: Option<u64>,
    file_index: usize,
}

fn sort_items(items: &mut Vec<PendingItem>) {
    let mut last_timed: Option<u64> = None;
    let mut keyed: Vec<(SortKey, PendingItem)> = items
        .drain(..)
        .map(|item| {
            if let Some(timing) = item.timing_ms {
                last_timed = Some(timing);
            }
            (
                SortKey {
                    anchor: last_timed,
                    file_index: item.file_index,
                },
                item,
            )
        })
        .collect();
    keyed.sort_by(|left, right| match (left.0.anchor, right.0.anchor) {
        (Some(left_timing), Some(right_timing)) => left_timing
            .cmp(&right_timing)
            .then(left.0.file_index.cmp(&right.0.file_index)),
        (None, None) => left.0.file_index.cmp(&right.0.file_index),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
    });
    items.extend(keyed.into_iter().map(|(_, item)| item));
}

fn note_timing(turn: &mut TurnBuild, timing: Option<u64>) {
    let Some(timing) = timing else {
        return;
    };
    turn.earliest_timing = Some(
        turn.earliest_timing
            .map_or(timing, |current| current.min(timing)),
    );
}

fn sort_sessions_newest_first(files: &mut [SessionFile]) {
    files.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| right.path.cmp(&left.path))
    });
}

fn system_time_ms(modified: SystemTime) -> Option<u64> {
    modified
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

fn item_offset(timing: Option<u64>, origin: Option<u64>) -> Option<u64> {
    match (timing, origin) {
        (Some(timing), Some(origin)) if timing != 0 && origin != 0 => {
            Some(timing.saturating_sub(origin))
        }
        _ => None,
    }
}

fn set_offset(item: &mut TimelineItem, offset: Option<u64>) {
    match item {
        TimelineItem::ProviderAttempt { offset_ms, .. }
        | TimelineItem::Tool { offset_ms, .. }
        | TimelineItem::Permission { offset_ms, .. }
        | TimelineItem::Review { offset_ms, .. }
        | TimelineItem::Compaction { offset_ms, .. } => *offset_ms = offset,
    }
}

fn render_listing(listing: &SessionListing) -> String {
    let started = match listing.started_at_ms {
        Some(started) => started.to_string(),
        None => String::from("-"),
    };
    let outcome = listing.last_outcome.as_deref().unwrap_or("-");
    let model = listing.model.as_deref().unwrap_or("-");
    format!(
        "session {}  started={}  turns={}  last_outcome={}  model={}  warnings={}",
        listing.id, started, listing.turns, outcome, model, listing.warnings
    )
}

fn render_timeline(timeline: &SessionTimeline) -> Vec<String> {
    let mut lines = vec![format!(
        "session {}  turns={}  warnings={}",
        timeline.id,
        timeline.turns.len(),
        timeline.warnings.len()
    )];
    for warning in &timeline.warnings {
        lines.push(format!("warning {warning}"));
    }
    for turn in &timeline.turns {
        let prompt = turn
            .prompt
            .as_deref()
            .map(|prompt| format!("  \"{prompt}\""))
            .unwrap_or_default();
        lines.push(format!("turn {}{prompt}", turn.turn_id));
        for item in &turn.items {
            lines.push(render_item(item));
        }
        lines.push(render_outcome(turn));
    }
    lines
}

fn render_item(item: &TimelineItem) -> String {
    match item {
        TimelineItem::ProviderAttempt { offset_ms, attempt } => {
            format!(
                "  {}attempt {}{} {} {}{}",
                offset_label(*offset_ms),
                attempt.attempt_sequence,
                retry_label(attempt.retry_index),
                purpose_label(attempt.purpose),
                snake_label(attempt.outcome),
                attempt_details(attempt)
            )
        }
        TimelineItem::Tool {
            offset_ms,
            tool_name,
            outcome,
            duration_ms,
            arguments,
            ..
        } => {
            let name = tool_name.as_deref().unwrap_or("-");
            let duration = duration_ms
                .map(|duration| format!(" {duration}ms"))
                .unwrap_or_default();
            let arguments = arguments
                .as_deref()
                .map(|arguments| format!("  {arguments}"))
                .unwrap_or_default();
            format!(
                "  {}tool {name} {outcome}{duration}{arguments}",
                offset_label(*offset_ms)
            )
        }
        TimelineItem::Permission { offset_ms, summary } => {
            format!("  {}permission {summary}", offset_label(*offset_ms))
        }
        TimelineItem::Review {
            offset_ms,
            tool_request_id,
            decision,
        } => format!(
            "  {}review {tool_request_id} {decision}",
            offset_label(*offset_ms)
        ),
        TimelineItem::Compaction {
            offset_ms,
            tokens_before,
            tokens_after_estimate,
            reason,
        } => format!(
            "  {}compaction {reason} tokens_before={tokens_before} tokens_after={tokens_after_estimate}",
            offset_label(*offset_ms)
        ),
    }
}

fn retry_label(retry_index: u8) -> String {
    if retry_index == 0 {
        String::new()
    } else {
        format!(" retry {retry_index}")
    }
}

fn attempt_details(attempt: &ProviderAttemptSummary) -> String {
    let mut details = String::new();
    if attempt.outcome != ProviderAttemptOutcome::Succeeded {
        let source = attempt
            .classification_source
            .unwrap_or(ClassificationSource::Variant);
        let _ = write!(
            details,
            " {} source={}",
            snake_label(attempt.error_kind),
            snake_label(source)
        );
        if let Some(variant) = &attempt.error_variant {
            let _ = write!(details, " variant={variant}");
        }
        if let Some(status) = attempt.status_code {
            let _ = write!(details, " status={status}");
        }
        if let Some(code) = &attempt.provider_code {
            let _ = write!(details, " code={code}");
        }
        if let Some(phase) = attempt.timeout_phase {
            let _ = write!(details, " timeout_phase={}", snake_label(phase));
        }
    }
    if let Some(request) = &attempt.provider_request_id {
        let _ = write!(details, " request={request}");
    }
    if let Some(delay) = attempt.next_delay_ms {
        let _ = write!(details, " next_delay={delay}ms");
    }
    if let Some(retry_after) = attempt.retry_after_ms {
        let _ = write!(details, " retry_after={retry_after}ms");
    }
    if let Some(first_event) = attempt.first_event_ms {
        let _ = write!(details, " first_event={first_event}ms");
    }
    let _ = write!(details, " {}ms", attempt.duration_ms);
    if let Some(capture) = &attempt.capture {
        let _ = write!(details, " capture={capture}");
    }
    details
}

fn render_outcome(turn: &TurnTimeline) -> String {
    let outcome = turn.outcome.as_deref().unwrap_or("-");
    let mut line = format!("  outcome {outcome}");
    if let Some(reason) = &turn.reason {
        let _ = write!(line, "  reason={reason}");
    }
    if let Some(usage) = &turn.usage {
        let input = usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64);
        let output = usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64);
        if let (Some(input), Some(output)) = (input, output) {
            let _ = write!(line, "  usage input={input} output={output}");
        }
    }
    line
}

fn offset_label(offset: Option<u64>) -> String {
    match offset {
        Some(offset) => format!("+{offset}ms "),
        None => String::new(),
    }
}

fn prompt_preview(text: &str) -> String {
    let first = text.lines().next().unwrap_or(text);
    let mut preview = String::new();
    for character in first.chars() {
        let next = character.len_utf8();
        if preview.len().saturating_add(next) > PROMPT_LIMIT {
            break;
        }
        preview.push(character);
    }
    preview
}

fn known_timing(value: Option<u64>) -> Option<u64> {
    value.filter(|value| *value != 0)
}

fn event_turn_id(event: &SessionEvent) -> Option<&str> {
    match event {
        SessionEvent::EntryAppended { turn_id, .. }
        | SessionEvent::ToolRequestRecorded { turn_id, .. }
        | SessionEvent::ToolExecutionFinished { turn_id, .. }
        | SessionEvent::TurnFinished { turn_id, .. }
        | SessionEvent::StaticContextIncluded { turn_id, .. }
        | SessionEvent::PermissionDecisionRecorded { turn_id, .. }
        | SessionEvent::ToolReviewRequested { turn_id, .. }
        | SessionEvent::ToolReviewDecisionRecorded { turn_id, .. }
        | SessionEvent::ToolReviewInterrupted { turn_id, .. }
        | SessionEvent::EditTraceRecorded { turn_id, .. }
        | SessionEvent::EditTransactionPrepared { turn_id, .. }
        | SessionEvent::EditTransactionFinished { turn_id, .. }
        | SessionEvent::CompactionCheckpoint { turn_id, .. }
        | SessionEvent::ToolResultMasked { turn_id, .. }
        | SessionEvent::ProviderAttemptFinished { turn_id, .. } => Some(turn_id.0.as_str()),
        SessionEvent::MetricRecorded { turn_id, .. } => {
            turn_id.as_ref().map(|turn_id| turn_id.0.as_str())
        }
        SessionEvent::ApprovalModeChanged { .. }
        | SessionEvent::ThinkingLevelChanged { .. }
        | SessionEvent::SessionModelChanged { .. }
        | SessionEvent::ReviewPolicyChanged { .. }
        | SessionEvent::ReviewRequestRecorded { .. }
        | SessionEvent::ReviewAssessmentRecorded { .. }
        | SessionEvent::ExactActionGrantRecorded { .. }
        | SessionEvent::Unknown => None,
    }
}

fn warning_label(warning: &SessionLoadWarning) -> String {
    match warning {
        SessionLoadWarning::InvalidJson {
            line_number,
            reason,
        } => format!("invalid_json line={line_number} reason={reason}"),
    }
}

fn purpose_label(purpose: ProviderAttemptPurpose) -> &'static str {
    match purpose {
        ProviderAttemptPurpose::Turn => "turn",
        ProviderAttemptPurpose::CompactionSummary => "compaction_summary",
        ProviderAttemptPurpose::CompactionNative => "compaction_native",
    }
}

fn compaction_reason_label(reason: CompactionReason) -> String {
    match reason {
        CompactionReason::Threshold => String::from("threshold"),
        CompactionReason::Manual => String::from("manual"),
        CompactionReason::Overflow => String::from("overflow"),
    }
}

fn snake_label<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(String::from))
        .unwrap_or_else(|| String::from("-"))
}
