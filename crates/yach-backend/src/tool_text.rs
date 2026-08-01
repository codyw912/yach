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
// The dead_code expectation below is removed by the task that adds call sites; `notice` carries
// none because rustc flags only dead-cluster roots.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "call sites land with the per-tool renderers")
)]
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
        assert_eq!(
            verdict_with_guidance("denied: user_denied", ""),
            "[denied: user_denied]"
        );
    }
}
