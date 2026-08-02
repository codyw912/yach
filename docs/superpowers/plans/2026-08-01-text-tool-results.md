# Text Tool Results Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every built-in tool result reaches the model as plain text — content byte-exact, metadata as bracketed exception-only notice lines, errors legible in the text.

**Architecture:** Each tool renders text where it builds JSON today (tools.rs, runner.rs, agent_edit_tools.rs, rig_adapter.rs synthesis); a new tiny `tool_text` module owns the notice vocabulary. The UI progress shapers in runner.rs stop parsing payload JSON and shape the text directly. Spec: `docs/superpowers/specs/2026-08-01-text-tool-results-design.md`.

**Tech Stack:** Rust workspace; jj (not raw git); `just dev <cmd>` wraps the nix dev shell.

## Global Constraints

- Run every cargo command as `just dev cargo <...>` (nix dev shell).
- Workspace clippy is strict: `-D warnings`, `panic!` banned even in tests (use `assert!`/`unreachable!`), `#[expect]` over `#[allow]`, max cognitive complexity 15, max 100-line functions.
- Never use `perl -pi -e` or multi-line `sed` for edits; use exact-match editing. GNU sed here, not BSD.
- Commit with jj (`jj commit -m "..."` after each task); no AI attribution lines anywhere.
- Do not change tool *input* schemas, tool names, descriptions, or the review/approval flow — only result payload strings and the UI shaping that consumes them.
- The guidance prose strings (edit failure guidance, bash failure guidance, sensitive-path guidance) are deliberately written and must survive verbatim.

---

### Task 1: `tool_text` notice vocabulary

**Files:**
- Create: `crates/yach-backend/src/tool_text.rs`
- Modify: `crates/yach-backend/src/lib.rs` (add `mod tool_text;` next to the existing `mod tools;` declaration)

**Interfaces:**
- Produces (used by Tasks 2–6):
  - `pub(crate) fn notice(body: &str) -> String` — `"[body]"`
  - `pub(crate) fn append_notices(content: &str, notices: &[String]) -> String` — content, then each notice on its own line; notices alone when content is empty
  - `pub(crate) fn verdict_with_guidance(verdict: &str, guidance: &str) -> String` — `"[verdict]\nguidance"`, verdict alone when guidance is empty

- [ ] **Step 1: Write the module with failing-to-compile call sites in tests**

```rust
//! Text rendering vocabulary for built-in tool results.
//!
//! Results are plain text: the content is the result itself, and
//! structured metadata flattens into bracketed notice lines that appear
//! only when there is something to say (design:
//! `docs/superpowers/specs/2026-08-01-text-tool-results-design.md`).
//! Notices are presentation, not a parsing contract — nothing reads
//! them back.

/// One bracketed notice line: `[body]`.
pub(crate) fn notice(body: &str) -> String {
    format!("[{body}]")
}

/// Content followed by notice lines. Empty content yields the notices
/// alone (a denied call, an empty capture); no trailing newline is
/// added beyond the line separators.
pub(crate) fn append_notices(content: &str, notices: &[String]) -> String {
    if notices.is_empty() {
        return content.to_owned();
    }
    let mut out = String::from(content.trim_end_matches('\n'));
    for notice in notices {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(notice);
    }
    out
}

/// The error contract on every wire: a bracketed verdict line, then the
/// guidance prose unchanged. rig 0.41 cannot express `is_error`
/// (upstream gap), so this line is the only error signal every provider
/// shape receives.
pub(crate) fn verdict_with_guidance(verdict: &str, guidance: &str) -> String {
    if guidance.is_empty() {
        notice(verdict)
    } else {
        format!("{}\n{guidance}", notice(verdict))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_wraps_in_brackets() {
        assert_eq!(notice("exit code 1"), "[exit code 1]");
    }

    #[test]
    fn append_notices_returns_content_unchanged_when_no_notices() {
        assert_eq!(append_notices("body\n", &[]), "body\n");
    }

    #[test]
    fn append_notices_joins_content_and_notices_with_single_newlines() {
        let notices = vec![notice("exit code 1"), notice("truncated")];
        assert_eq!(
            append_notices("line1\nline2\n", &notices),
            "line1\nline2\n[exit code 1]\n[truncated]"
        );
    }

    #[test]
    fn append_notices_with_empty_content_is_notices_alone() {
        let notices = vec![notice("no output; exit code 0")];
        assert_eq!(append_notices("", &notices), "[no output; exit code 0]");
    }

    #[test]
    fn verdict_with_guidance_stacks_verdict_line_over_prose() {
        assert_eq!(
            verdict_with_guidance("error: timeout", "Retry with a larger timeout."),
            "[error: timeout]\nRetry with a larger timeout."
        );
    }

    #[test]
    fn verdict_without_guidance_is_the_bracketed_line_alone() {
        assert_eq!(verdict_with_guidance("denied: user_denied", ""), "[denied: user_denied]");
    }
}
```

- [ ] **Step 2: Run the module tests**

Run: `just dev cargo test -p yach-backend tool_text`
Expected: all 6 pass.

- [ ] **Step 3: Clippy the crate**

Run: `just dev cargo clippy -p yach-backend --all-targets`
Expected: clean. (`tool_text` items are `pub(crate)` and already referenced by tests, so no dead-code warnings.)

- [ ] **Step 4: Commit**

```bash
jj commit -m "feat: tool_text notice vocabulary for text tool results"
```

---

### Task 2: read_text_file, search_project, list_project_paths, project_path_info

**Files:**
- Modify: `crates/yach-backend/src/tools.rs` — `execute_read_text_file` (~line 1362), `execute_search_project` (~1394), `execute_list_project_paths` (~1442), `execute_project_path_info` (~1338), plus their tests in the same file.

**Interfaces:**
- Consumes: `crate::tool_text::{notice, append_notices}` from Task 1.
- Produces: `ToolExecutionResult.summary` strings in the new text shapes (consumed untouched by session/provider paths; UI shaping changes in Task 5).

- [ ] **Step 1: Rewrite `execute_read_text_file`'s summary**

Replace the `serde_json::json!({ "outcome": "read", ... })` block with:

```rust
    let summary = if read.text.is_empty() {
        // An empty tool-result string is ambiguous (blank file or
        // missing result?) and some provider shapes handle it poorly,
        // so the one read notice marks it explicitly.
        crate::tool_text::notice("empty file")
    } else {
        read.text
    };
```

The `relative_path` lookup above it is now unused by the summary — delete the lookup (the model supplied the path; nothing else consumed it). Keep `byte_count: summary.len()` and the other `ToolExecutionResult` fields exactly as they are.

- [ ] **Step 2: Rewrite `execute_search_project`'s summary**

Replace the matches-array + `json!` construction with grep-format lines and exception-only notices:

```rust
    let mut line_truncated = false;
    let mut lines = result
        .matches
        .into_iter()
        .map(|matched| {
            let (line, truncated) = bounded_provider_line(&matched.line);
            line_truncated |= truncated;
            let ellipsis = if truncated { "…" } else { "" };
            format!(
                "{}:{}: {line}{ellipsis}",
                matched.relative_path, matched.line_number
            )
        })
        .collect::<Vec<_>>();
    let mut notices = Vec::new();
    if lines.is_empty() {
        notices.push(crate::tool_text::notice(&format!(
            "no matches; {} files searched",
            result.searched_files
        )));
    }
    if result.truncated {
        notices.push(crate::tool_text::notice("truncated: match limit reached"));
    }
    if result.denied_paths_excluded {
        notices.push(crate::tool_text::notice("some paths excluded by policy"));
    }
    let truncated = result.truncated || line_truncated;
    let summary = crate::tool_text::append_notices(&lines.join("\n"), &notices);
```

(If `bounded_provider_line` already appends its own marker, keep its behavior and drop the `ellipsis` variable — check the helper before editing; the requirement is only that clipped lines are visibly clipped.)

- [ ] **Step 3: Rewrite `execute_list_project_paths`'s summary**

Replace the entries-array + `json!` construction with:

```rust
    let mut lines = result
        .entries
        .into_iter()
        .map(|entry| match entry.kind {
            crate::ResourceEntryKind::Directory => format!("{}/", entry.relative_path),
            _ => match entry.byte_size {
                Some(bytes) => format!("{}  {bytes} bytes", entry.relative_path),
                None => entry.relative_path,
            },
        })
        .collect::<Vec<_>>();
    let mut notices = Vec::new();
    if lines.is_empty() {
        notices.push(crate::tool_text::notice("empty directory"));
    }
    if result.truncated {
        notices.push(crate::tool_text::notice("truncated: entry limit reached"));
    }
    if result.denied_paths_excluded {
        notices.push(crate::tool_text::notice("some paths excluded by policy"));
    }
    let summary = crate::tool_text::append_notices(&lines.join("\n"), &notices);
```

(Adjust the `byte_size` arm to the actual field type — if it is not an `Option`, render `format!("{}  {} bytes", entry.relative_path, entry.byte_size)` for files and the bare path for `Other`.)

- [ ] **Step 4: Rewrite `execute_project_path_info`'s summary**

Replace the `json!` block with one prose line; `provider_visibility` is hardcoded `"never"` today and carries no information — drop it:

```rust
    let summary = match metadata.byte_size {
        Some(bytes) => format!(
            "{}: {}, {bytes} bytes",
            metadata.relative_path,
            resource_entry_kind_label(metadata.kind)
        ),
        None => format!(
            "{}: {}",
            metadata.relative_path,
            resource_entry_kind_label(metadata.kind)
        ),
    };
```

(Same `Option` caveat as Step 3 — match the real field type.)

- [ ] **Step 5: Update the tests beside these functions**

Run: `just dev cargo test -p yach-backend tools 2>&1 | grep -E "^test|FAILED"`

Every failing assertion is a payload-shape assertion. Rewrite each to the new expected strings. Anchor cases that MUST exist after this task (add any that are missing):

```rust
// read: byte-exact — the summary IS the file text
assert_eq!(result.summary, "alpha\nbeta\n");

// read: empty file
assert_eq!(result.summary, "[empty file]");

// search: grep lines, no notices on a clean hit
assert_eq!(result.summary, "notes/a.txt:3: beta line");

// search: no matches
assert_eq!(result.summary, "[no matches; 2 files searched]");

// list: dirs slash-suffixed, files sized
assert_eq!(result.summary, "sub/\na.txt  6 bytes");

// path info: one prose line
assert_eq!(result.summary, "a.txt: file, 6 bytes");
```

- [ ] **Step 6: Run the crate tests and clippy**

Run: `just dev cargo test -p yach-backend && just dev cargo clippy -p yach-backend --all-targets`
Expected: green. Failures outside tools.rs (runner progress tests) belong to Task 5 — do NOT fix them here by re-adding JSON; if the suite cannot pass before Task 5, run `just dev cargo test -p yach-backend tools` for this task's gate and note the deferral in the commit message.

- [ ] **Step 7: Commit**

```bash
jj commit -m "feat: read/search/list/info tool results as text"
```

---

### Task 3: bash result and the runner failure builders

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` — the bash success `content` (~line 3845), `failed_tool_result` (~3605), `sensitive_denied_tool_result` (~3140).

**Interfaces:**
- Consumes: `crate::tool_text::{notice, append_notices, verdict_with_guidance}`.
- Produces: `ProviderToolResult.content` text shapes (UI shaping consumes them in Task 5).

- [ ] **Step 1: Rewrite the bash success content**

Replace:

```rust
    let content = serde_json::json!({
        "outcome": "completed",
        "tool_request_id": request.request_id,
        "approved_by": approved_by,
        "exit_code": outcome.exit_code,
        "duration_ms": outcome.duration_ms,
        "output": outcome.output,
        "output_bytes_total": outcome.output_bytes_total,
        "truncated": outcome.truncated,
    })
    .to_string();
```

with:

```rust
    let mut notices = Vec::new();
    if outcome.output.is_empty() {
        notices.push(crate::tool_text::notice(&format!(
            "no output; exit code {}",
            outcome.exit_code.map_or_else(|| String::from("unknown"), |code| code.to_string())
        )));
    } else if outcome.exit_code != Some(0) {
        notices.push(crate::tool_text::notice(&format!(
            "exit code {}",
            outcome.exit_code.map_or_else(|| String::from("unknown"), |code| code.to_string())
        )));
    }
    if outcome.truncated {
        notices.push(crate::tool_text::notice(&format!(
            "truncated: kept {} of {} output bytes",
            outcome.output.len(),
            outcome.output_bytes_total
        )));
    }
    let content = crate::tool_text::append_notices(&outcome.output, &notices);
```

`approved_by`, `tool_request_id`, and `duration_ms` drop from the payload (block id binds the result; approval and timing are session-log facts). `approved_by` may become an unused binding — if so, keep the variable where the review flow produces it but stop embedding it; remove only the payload use.

- [ ] **Step 2: Rewrite `failed_tool_result`'s content**

```rust
    let content = crate::tool_text::verdict_with_guidance(&format!("error: {reason}"), guidance);
```

- [ ] **Step 3: Rewrite `sensitive_denied_tool_result`'s content**

Same shape, guidance string verbatim from the current `json!` block:

```rust
    let content = crate::tool_text::verdict_with_guidance(
        "error: sensitive_path_denied",
        "This path matches the sensitive-file deny list, so its contents are \
    not available to tools. If access is intended, ask the user to allow the path under \
    files.allow in .yach/config.json and retry.",
    );
```

- [ ] **Step 4: Run the bash/failure tests**

Run: `just dev cargo test -p yach-backend runner 2>&1 | grep -E "^test result|FAILED" | head`
Expected: payload-shape assertions fail; progress-shaping failures belong to Task 5. Update only assertions on `content` strings here, to shapes like:

```rust
assert_eq!(result.content, "out line\n[exit code 3]");
assert_eq!(result.content, "[no output; exit code 0]");
assert!(result.content.starts_with("[error: timeout]\n"));
```

- [ ] **Step 5: Commit**

```bash
jj commit -m "feat: bash and failure tool results as text"
```

---

### Task 4: edit status builders and the denied/cancelled synthesis

**Files:**
- Modify: `crates/yach-backend/src/agent_edit_tools.rs` — `applied_content` (~line 834), `rejected_content` (~853), `denied_content` (~861), `failed_content` (~870), plus tests in the file.
- Modify: `crates/yach-backend/src/rig_adapter.rs` — `provider_tool_result_block` (~line 399) empty-content synthesis, and its comment.

**Interfaces:**
- Consumes: `crate::tool_text::{notice, verdict_with_guidance}`.
- Produces: edit result contents `[applied]` / `[rejected by review]` / `[denied: <op>]` / `[error: <label>]\n<guidance>`; synthesized `[denied: <reason>]` / `[cancelled: <reason>]` blocks.

- [ ] **Step 1: Rewrite the four edit content builders**

```rust
fn applied_content(
    _request_id: &str,
    _preview_id: &EditPreviewId,
    _transaction_id: &str,
    _operation: &str,
    _path: &str,
    diff_summary_truncated: bool,
) -> String {
    if diff_summary_truncated {
        format!(
            "{}\n{}",
            crate::tool_text::notice("applied"),
            crate::tool_text::notice("diff summary truncated")
        )
    } else {
        crate::tool_text::notice("applied")
    }
}

fn rejected_content(_request_id: &str, _operation: &str, _path: &str) -> String {
    crate::tool_text::notice("rejected by review")
}

fn denied_content(_request_id: &str, _operation: &str, _path: &str) -> String {
    crate::tool_text::notice("denied by review")
}

fn failed_content(_request_id: &str, _operation: &str, error: &str, guidance: &str) -> String {
    crate::tool_text::verdict_with_guidance(&format!("error: {error}"), guidance)
}
```

If clippy objects to the now-unused parameters, prune the parameters and their call sites instead of keeping `_`-prefixed ones — whichever produces the smaller diff while staying under the strict lints. `agent_edit_failure_guidance` and every guidance string stay byte-identical.

- [ ] **Step 2: Rewrite the empty-content synthesis in `provider_tool_result_block`**

Replace the `serde_json::json!({ "outcome": ..., "reason": ... })` arm with:

```rust
    let content = if result.content.trim().is_empty() {
        // Denied and cancelled calls carry no payload at all, so the
        // verdict is the only thing left worth sending.
        match &result.reason {
            Some(reason) if !reason.is_empty() => crate::tool_text::notice(&format!(
                "{}: {reason}",
                tool_outcome_label(result.status)
            )),
            _ => crate::tool_text::notice(tool_outcome_label(result.status)),
        }
    } else {
        result.content.clone()
    };
```

Trim the stale sentences from the function's doc comment (the parts describing payload `outcome`/`error`/`guidance` JSON keys) so the comment describes text results; keep the history sentences that explain why the envelope was dropped.

- [ ] **Step 3: Update tests in both files**

Run: `just dev cargo test -p yach-backend agent_edit 2>&1 | grep -E "^test result|FAILED"` and `just dev cargo test -p yach-backend rig_adapter 2>&1 | grep -E "^test result|FAILED"`.
Rewrite failing content assertions to the new strings, e.g.:

```rust
assert_eq!(content, "[applied]");
assert!(content.starts_with("[error: target_exists]\n"));
assert_eq!(block.content, "[denied: user_denied]");
```

- [ ] **Step 4: Commit**

```bash
jj commit -m "feat: edit statuses and denied/cancelled synthesis as text"
```

---

### Task 5: UI progress shaping goes text-native

**Files:**
- Modify: `crates/yach-backend/src/runner.rs` — `provider_visible_tool_progress_output` (~4095) and the per-tool parsers it dispatches to: `provider_visible_read_progress` (~4205), `provider_visible_search_progress` (~4217), `provider_visible_list_progress` (~4237), `provider_visible_bash_progress` (~4142), `provider_visible_path_info_progress` (~4124), `provider_visible_edit_progress` (~4107), `provider_visible_failed_progress` (~4115); tests around lines 8500–8800, 9156, 13590.

**Interfaces:**
- Consumes: the text contents produced by Tasks 2–4. These functions receive only the persisted `content` string — the same string on live runs and on session resume — so everything they show must derive from it.
- Produces: progress display strings for the TUI and session re-rendering. `provider_tool_progress_output`'s signature is unchanged.

- [ ] **Step 1: Rewrite the shapers**

The JSON field extraction collapses into text shaping. Keep `BASH_PROGRESS_TAIL_LINES = 8`:

```rust
fn provider_visible_tool_progress_output(tool_name: &str, content: &str) -> Option<String> {
    match tool_name {
        "read_text_file" => Some(read_progress_line(content)),
        "search_project" | "list_project_paths" => Some(head_lines_progress(content, 8)),
        "bash" => Some(tail_lines_progress(content, BASH_PROGRESS_TAIL_LINES)),
        "project_path_info" | "edit_text_file" | "create_text_file" => {
            Some(format!("completed: {}", content.lines().next().unwrap_or_default()))
        }
        _ => None,
    }
}

/// Reads are byte-exact file text; the row shows its size, not its body.
fn read_progress_line(content: &str) -> String {
    let line_count = content.lines().count().max(1);
    let line_label = if line_count == 1 { "line" } else { "lines" };
    format!("completed: {line_count} {line_label}, {} bytes", content.len())
}

/// First lines of a line-oriented result (search matches, listing
/// entries), with an elision marker. Notice lines count like any other
/// line — they are part of what the model saw.
fn head_lines_progress(content: &str, keep: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut out = vec![format!("completed: {} lines", lines.len())];
    out.extend(lines.iter().take(keep).map(|line| (*line).to_owned()));
    if lines.len() > keep {
        out.push(format!("... {} more lines", lines.len() - keep));
    }
    out.join("\n")
}

/// Trailing lines of a command capture, so the evidence survives the
/// live stream being replaced by the finished row.
fn tail_lines_progress(content: &str, keep: usize) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let mut out = vec![format!("completed; {} bytes", content.len())];
    if lines.len() > keep {
        out.push(format!("... {} earlier lines", lines.len() - keep));
    }
    out.extend(lines.iter().rev().take(keep).rev().map(|line| (*line).to_owned()));
    out.join("\n")
}

/// Failed contents are already `[error: ...]` + guidance — show them as-is.
fn provider_visible_failed_progress(content: &str) -> Option<String> {
    if content.starts_with('[') {
        Some(content.to_owned())
    } else {
        None
    }
}
```

Delete `provider_visible_read_progress`, `provider_visible_search_progress`, `provider_visible_list_progress`, `provider_visible_bash_progress`, `provider_visible_path_info_progress`, and `provider_visible_edit_progress` (their behavior is absorbed above). The bash duration line disappears from finished rows — timing was payload-only and the payload no longer carries it; live streaming rows are unaffected.

- [ ] **Step 2: Update the progress and session-render tests**

Run: `just dev cargo test -p yach-backend 2>&1 | grep -E "^test result|FAILED"`.
The tests near 8500–8800 (`provider_agent_loop_emits_tool_progress_before_final_answer` and neighbors) and 13590 (`session_messages_render_persisted_tool_content_like_live_progress`) assert display strings — update them to the new shapes. The 13590 test is the important one: it proves resume re-shaping equals live shaping over the SAME persisted text.

- [ ] **Step 3: Full crate green + clippy**

Run: `just dev cargo test -p yach-backend && just dev cargo clippy -p yach-backend --all-targets`
Expected: green, including anything deferred from Tasks 2–3.

- [ ] **Step 4: Commit**

```bash
jj commit -m "feat: text-native tool progress shaping"
```

---

### Task 6: Workspace audit and full verification

**Files:**
- Modify: none expected; whatever the audit finds.

- [ ] **Step 1: Audit for surviving payload-JSON parsing**

Run: `grep -rn "get(\"outcome\")\|get(\"output\")\|get(\"text\")\|get(\"matches\")\|get(\"entries\")" crates/*/src/ --include="*.rs"`
Expected: no hits outside tests of old shapes (which should already be rewritten). Fix any live hit by deriving from the text or from event metadata — never by re-adding payload JSON.

- [ ] **Step 2: Audit user-visible copy**

Run: `grep -rn "JSON" crates/yach-backend/src/tools.rs crates/yach-backend/src/runner.rs | grep -i "description\|summary\|result"`
Expected: no tool description or user-facing string promising JSON results. (Descriptions were audited shape-neutral at design time; this re-checks after the diff.)

- [ ] **Step 3: Full workspace gate**

Run: `just dev cargo clippy --workspace --all-targets && just dev cargo test --workspace`
Expected: clean and green.

- [ ] **Step 4: Commit (only if the audit changed anything)**

```bash
jj commit -m "fix: audit fallout from text tool results"
```

---

### Task 7: Eval verification (needs the owner available for credential approval)

**Files:** none — measurement only.

- [ ] **Step 1: Rebuild and verify the runtime image**

Run: `just runtime-image && bash evals/scripts/check-image-fresh.sh`
Expected: `FRESH`. The stale-image guard makes the runs below trustworthy.

- [ ] **Step 2: Gate**

Run (credentials resolve through the owner's profile runner; DO NOT start without the owner available — authorization lapses mid-run waste the whole measurement):
`YACH_ROTATE_PROFILE_RUNNER=~/bin/yach-profile-run bash evals/scripts/gate.sh` with the anthropic profile env, i.e. `~/bin/yach-profile-run ~/tmp/yach-rotation/profiles/anthropic-haiku.env bash evals/scripts/gate.sh`
Expected: 7/7 tasks reward 1 plus all three driver checks.

- [ ] **Step 3: The 125-cell sweep**

Run each of the five measured tasks (tool-call-economy, tool-result-dependence, multi-round-sequence, compaction-continuation, notes-tally-fix) via:
`just eval-sweep ~/tmp/yach-rotation/profiles evals/tasks/<task> ~/tmp/yach-rotation/sweeps/2026-08-XX-text-results 5`
with `YACH_ROTATE_PROFILE_RUNNER=~/bin/yach-profile-run` exported.
Reference: the 2026-08-01 rig-0.41 baseline (99/100 + 24/25 patch; both misses zen-nemotron noise). Expected: statistically indistinguishable. Any cell recorded `reward=error` did not run — re-run that task block into a patch directory rather than reading it as a rate.

- [ ] **Step 4: The directional check**

Scan the sweep's outcome documents for envelope echo:
`grep -rl '"outcome"\|"exit_code"\|"byte_count"' ~/tmp/yach-rotation/sweeps/2026-08-XX-text-results/*/*/work/*.txt ~/tmp/yach-rotation/sweeps/2026-08-XX-text-results/*/*/work/.yach-eval/outcome.json 2>/dev/null`
Expected: zero answer files containing result-structure fragments (the class was already at zero; it must stay there).

- [ ] **Step 5: Record and board**

Write `docs/project/records/2026-08-XX-text-tool-results-measurement.md` in the style of `2026-07-31-payload-slim-measurement.md` (rates table, comparison to reference, failure classification, caveats), and move the board item from "next (spec in review)" to MEASURED with the headline numbers.

- [ ] **Step 6: Commit**

```bash
jj commit -m "docs: record the text tool-results measurement"
```
