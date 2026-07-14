# How Other Harnesses Handle Stale Filesystem Evidence

Date: 2026-07-14

Context: the 2026-07-14 live dogfood run showed the native-provider model
(haiku) asserting filesystem state from in-session memory instead of fresh
tool evidence (denied a duplicate create without a tool call; on 2026-07-07 it
claimed a deleted file still existed). This record summarizes how other
agentic coding harnesses prevent stale-evidence claims, from a source survey
of public system prompts and open-source harness code.

## Key finding

No surveyed harness relies on model judgment alone. Common practice is
defense in depth with a mechanical layer as the backstop and prompt steering
as calibration on top. The loudness of prompt steering is inversely
proportional to model capability and mechanical support.

## Per-harness mechanisms

| Harness | Mechanical enforcement | Prompt steering |
| --- | --- | --- |
| Claude Code | Read-before-edit gate (Edit errors unless the file was Read this conversation); modified-since-read mtime check; injected `<system-reminder>` when a tracked file changes on disk | Steers the opposite way ("Do NOT re-read a file you just edited... the harness tracks file state for you") to save tokens, because mechanics cover staleness |
| OpenAI Codex CLI | `apply_patch` context lines must match current file; mismatch fails back to the model | "You may be in a dirty git worktree... If you notice unexpected changes you didn't make, STOP IMMEDIATELY"; per-model prompt files tune steering per generation |
| aider | SEARCH/REPLACE must match character-for-character; failures reflect back for retry | Architectural: files are re-read from disk and re-sent every request with "Trust this message as the true contents of these files! Any other messages in the chat may contain outdated versions." |
| Cline | Per-file filesystem watchers; timestamps distinguish own edits from user edits; write results return `final_file_content` as authoritative post-edit state | Injected "CRITICAL FILE STATE ALERT: ... Your cached understanding is now stale and unreliable ... you must execute read_file" |
| Gemini CLI | `replace` requires current-content match, with LLM-based self-correction that re-reads and reports "the file has been modified ... since that edit attempt" | Tool docs instruct reading current content before replacing |
| opencode | Read-before-edit gate plus per-session read-time vs on-disk mtime staleness rejection | "You must use your Read tool at least once in the conversation before editing" |
| Pi | Edit `oldText` must match a unique region of current content at execution time; nothing else | None |

Notable implementation caveat: opencode's mtime-based staleness check causes
false rejections when git snapshots or formatters touch files without
changing content (sst/opencode#5840); content hashes avoid this. Yach's
`NativeEditEngine` already uses expected content hashes.

## Weak-model vs frontier-model tradeoff

- aider selects edit formats per model: lesser models get the "whole file"
  format because it is easiest; capable models get search/replace diffs.
- Codex ships per-model prompt variants rather than one prompt for all.
- Cline (arbitrary models) uses the loudest steering; Claude Code (frontier
  model plus mechanical staleness tracking) steers against redundant re-reads.

## What yach already has

- Exact-hash edit transactions: `NativeEditEngine` checks expected content
  hashes and applies exact hunks, so stale edits fail rather than corrupt.
- Bounded, evidence-recorded read/search/list results.

## Gaps this research suggests, smallest first

1. System-prompt guardrail lines (aider/Cline style): tool results are the
   only source of truth; file contents earlier in the conversation may be
   outdated; verify current state with a tool call before asserting that a
   file exists, does not exist, or has particular contents.
2. Actionable tool-failure text: on create-conflict or hash mismatch, tell
   the model the next step ("file changed on disk; re-read it before
   retrying") — cheap models follow explicit next-step instructions in
   errors far better than they infer them.
3. Return the authoritative post-edit content region in edit results
   (Cline-style `final_file_content`) so the model's picture stays current
   without extra read calls.
4. Change notifications when a previously-read file changes on disk
   (Claude Code system-reminder / Cline watcher style). Largest scope;
   needs its own design if pursued.
