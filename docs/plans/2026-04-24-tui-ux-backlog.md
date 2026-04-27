# TUI UX Backlog

This tracks UX issues noticed while exercising the live Pi-backed TUI. These are not all immediate blockers, but they should stay visible for polish passes.

## Input Composer

### Textarea Crate

- Adopted `ratatui-textarea = "0.9.1"` after upgrading to the current Ratatui stack.
- Current terminal dependencies: `ratatui = "0.30"`, `crossterm = "0.29"`.
- The prompt composer now uses a maintained textarea state machine for editing behavior instead of bespoke text editing code.
- Preserve yach-specific submit/newline semantics: `Enter` submits, `Ctrl+J` inserts newline, `Shift+Enter` inserts newline when reported.
- Still worth validating manually across terminals because command/meta key reporting varies by terminal emulator and OS.

### Expand Long Prompts

- Current behavior: long input continues horizontally and eventually disappears off screen.
- Desired behavior: input wraps within the composer and the input box grows vertically up to a capped height.
- Notes: once the cap is reached, the composer should scroll internally while keeping the cursor visible.

### Multiline Input

- Support `Shift+Enter` for inserting a newline when terminals report it distinctly.
- Support `Ctrl+J` as a reliable newline shortcut.
- Keep plain `Enter` as prompt submit.
- Preserve cursor movement and deletion semantics across lines.

## Transcript Readability

### User/Assistant Separation

- Add a stronger visual cue between user messages and assistant responses.
- Keep this low priority until core interaction flows stabilize.
- Possible directions: subtle separators, left gutter colors, spacing changes, or role-specific blocks.

### Continuation Alignment

- Wrapped transcript lines should align cleanly under their message text.
- Avoid odd one-column visual drift on assistant continuation lines.

## Tool Display

### Completed Tool Rows

- Current direction: replace the original tool-start row with a compact completion summary.
- Future improvement: consider expandable tool details once the transcript interaction model supports focus/selection.

## Status Bar

### Bottom Status Placement

- Status belongs below the input box visually.
- Lifecycle events like `agent_end` should drive state but not appear as raw status text.
