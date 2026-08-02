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

/// Counts result lines that are not bracketed notice lines (`[no matches;
/// ...]`, `[truncated: ...]`), so the persisted summary reports how much
/// content-bearing output the model actually saw without re-deriving it
/// from JSON the result no longer carries.
///
/// A line counts as a notice only if it is bracketed end to end
/// (`starts_with('[') && ends_with(']')`): a grep match line like
/// `[slug].tsx:3: text` or a list entry `[slug].tsx  120 bytes` starts
/// with `[` but does not end with `]`, so it still counts as content. A
/// pathological entry named exactly `[x]` with nothing appended after it
/// would still be misread as a notice — accepted, because this count is
/// telemetry, not the wire contract the model reads.
pub(crate) fn content_line_count_summary(
    tool_name: &str,
    label: &str,
    summary: &str,
    truncated: bool,
) -> String {
    let count = summary
        .lines()
        .filter(|line| !(line.starts_with('[') && line.ends_with(']')))
        .count();
    format!("{tool_name} {label}={count} truncated={truncated}")
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
        assert_eq!(
            verdict_with_guidance("denied: user_denied", ""),
            "[denied: user_denied]"
        );
    }

    #[test]
    fn content_line_count_summary_counts_a_bracket_leading_match_line_as_content() {
        // `[slug].tsx:3: text` starts with '[' like a notice, but does not
        // end with ']' — it must still count as a match, not be mistaken
        // for a notice line and dropped from the count.
        let summary = content_line_count_summary(
            "search_project",
            "matches",
            "[slug].tsx:3: text\nother.rs:1: text",
            false,
        );
        assert_eq!(summary, "search_project matches=2 truncated=false");
    }

    #[test]
    fn content_line_count_summary_excludes_a_full_notice_line() {
        let summary = content_line_count_summary(
            "search_project",
            "matches",
            "[no matches; 2 files searched]",
            false,
        );
        assert_eq!(summary, "search_project matches=0 truncated=false");
    }

    #[test]
    fn content_line_count_summary_counts_content_and_excludes_trailing_notice() {
        let summary = content_line_count_summary(
            "list_project_paths",
            "entries",
            "src/a_dir/\nsrc/lib.rs  3 bytes\n[some paths excluded by policy]",
            true,
        );
        assert_eq!(summary, "list_project_paths entries=2 truncated=true");
    }
}
