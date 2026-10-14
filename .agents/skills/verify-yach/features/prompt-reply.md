# Prompt and reply

**Entry point:** `yach tui` (interactive TUI); `scripts/drive-prompt-reply`.

**Preconditions:** a `--features bench` build; `YACH_RIG_PROVIDER=scripted` with a reply script whose single round streams Markdown. No credentials.

**Steps:**

1. Start the TUI. → Status bar shows `scripted-model` and `approval:review`.
2. Type `say hello`, Enter. → Transcript shows `› say hello`, then `• Verify heading`, `Some bold text and inline code.`, and list items `- first item` / `- second item`. No literal `**` or `##`. Status bar reads `session messages: 2`.
3. `/status`. → `Session status` block with `messages: 2 (user 1, assistant 1, tool 0)` and `approval: review`.

**Gotchas:**

- On first start in a fresh HOME the TUI prints `yach: applied preset full (first run)`, and a background catalog refresh may replace the status-bar text. Wait on transcript text or `session messages:`, not on the status message.
- Styles (bold, code colour) do not survive `capture-pane -p`. The capture proves delimiters were consumed, not how they are coloured.
