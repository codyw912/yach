# Two-file edit and verifier

**Entry point:** `yach run --prompt "..." --full-auto`, `yach sessions show latest --json`, the session log; `scripts/drive-multi-file-edit`.

**Preconditions:** scripted provider, default (full) preset. The workspace is seeded with `a.txt` (`alpha`), `b.txt` (`beta`), and the verifier `verify.sh`, which exits 0 only when the files read `ALPHA` and `BETA`. No TTY needed.

**Steps:**

1. Round 1 reads both files (`read_text_file` ×2). Round 2 sends two `edit_text_file` hashline patches whose `[path#TAG]` headers come from `file_tag` on the seeds. Round 3 runs `./verify.sh` with `bash`. Round 4 is the reply. → `yach run` exits 0.
2. Compare the files with the seeds. → Both differ: `ALPHA`, `BETA`.
3. Run `./verify.sh` again from the workspace. → Exit 0.
4. `yach sessions show latest --json`. → The turn is `completed`, with five completed `tool` items.
5. Read the session log's `tool_execution_finished.result_content`. → Two `[applied]` edits and `[no output; exit code 0]` from the verifier.

**Gotchas:**

- The full preset replaces `edit_text_file` with hashline (`crates/yach-hashline-extension/yach.extension.json:27-40`). Its argument is `{"input": "<patch>"}`, not `{"path","find","replace"}`. A patch's tag must match a `read_text_file` of that file earlier in the same session, so reads come first. `--preset minimal` has no edit tool at all.
- A bash tool that exits non-zero still has tool `outcome: "completed"`. The exit code is only in the result text, and the inspector redacts it, which is why the drive reads the session log.
- `--full-auto`: headless runs are read-only unless told otherwise. Review gating of edits and commands is `drive-tool-review`'s job.
- This is the harness contract in the scratch workspace. The live multi-tool scenario runs on ylog with `uv run pytest`; see the portfolio record.
