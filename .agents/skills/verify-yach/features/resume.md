# Resume transcript persistence

**Entry point:** `yach tui --resume`; `scripts/drive-resume`.

**Preconditions:** a finished session in `YACH_SESSION_DIR` from an earlier run in the same workspace.

**Steps:**

1. Prompt `first question`; scripted reply `First answer.` → both in the transcript.
2. `/quit`. → The process exits (driver pane shows `Pane is dead`).
3. `yach tui --resume`. → The transcript shows `› first question` and `• First answer.` before any input.
4. Prompt `second question`; require a new durable completed turn in the same session, then `/status`. → `messages: 4 (user 2, assistant 2, tool 0)`. Inspector retains the unchanged first turn, raw JSONL retains four correlated user/assistant entries, and exactly one session file remains. Earlier transcript may have moved into scrollback after continuation; its restoration is checked before input, not in the final current pane.

**Gotchas:**

- Observed resumed scripted processes can fall back to the built-in fixture with `backend: no provider configured`. This recipe proves transcript persistence and same-session continuation, not provider restoration or history in a provider request; it does not pin the second reply's wording. Use [provider recovery](provider-resume.md) for actual new provider-attempt/request evidence. That separate local recipe requires explicit model re-selection and does not claim automatic restoration.
