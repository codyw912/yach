# Tool review (approve and reject)

**Entry point:** `yach tui` in the default `review` approval mode; `scripts/drive-tool-review`.

**Preconditions:** scripted provider. Round 1 requests a `bash` call; round 2 is the reply after the tool result. The workspace holds a `notes.txt` the command touches.

**Steps:**

1. Prompt `count the lines` (script: `wc -l notes.txt`). → Inline review row: `Review: pending`, `Command: wc -l notes.txt`, options `› Approve once` / `Approve for this session` / `Reject`.
2. Enter (Approve once). → `✓ bash wc -l notes.txt completed ... review approved`, output `3 notes.txt`, then `• Counted the lines.`
3. Fresh run; prompt `delete the notes` (script: `rm notes.txt`). Down, Down (`› Reject`), Enter. → `! denied bash ... review rejected`, `[error: user_rejected]`, then the round-2 reply. `notes.txt` still exists.

**Gotchas:**

- The review row holds the keyboard until a decision; `j`/`k` also move the selection.
- The review row's command line wraps at narrow widths; the driver pane is 200 columns, so assertions match on one line.
- The rejected command is never executed. The file check is the proof, not the transcript text alone.
