# Resume a session

**Entry point:** `yach tui --resume`; `scripts/drive-resume`.

**Preconditions:** a finished session in `YACH_SESSION_DIR` from an earlier run in the same workspace.

**Steps:**

1. Prompt `first question`; scripted reply `First answer.` → both in the transcript.
2. `/quit`. → The process exits (driver pane shows `Pane is dead`).
3. `yach tui --resume`. → The transcript shows `› first question` and `• First answer.` before any input.
4. Prompt `second question`, then `/status`. → `messages: 4 (user 2, assistant 2, tool 0)`. Still exactly one session file.

**Gotchas:**

- A resumed process does not reattach the scripted provider. The status bar reads `backend: no provider configured; local read-only project inspection available`, and the second reply is the built-in fixture's `fixture response: second question`. The drive asserts the restored transcript and the message count, not the second reply's text. Whether `--resume` should keep the provider selected by the environment is an open product question; this records current behaviour.
