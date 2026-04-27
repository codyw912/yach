---
title: feat: Polish M2 basic TUI dogfood loop
type: feat
status: completed
date: 2026-04-27
origin: docs/status/m2-tui-checkpoint.md
---

# feat: Polish M2 basic TUI dogfood loop

## Goal

Make the M2 TUI more usable for normal dogfooding by addressing the next basic-loop issues surfaced by manual smoke: model selection should not present unusable placeholder choices as confirmed state, `/help` should be readable, and live dialog UX should have an explicit harness/evidence path.

This is intentionally not a session-tree/fork polish pass. Ctrl+F clone/fork remains partial and deferred until broader session compatibility work.

## Scope

### In

1. Replace or supplement the static model selector with backend-provided model metadata from stock Pi RPC `get_available_models` when available.
2. Make model selection non-optimistic: show requested/pending status and update current model only from backend confirmation/state.
3. Move `/help` from the narrow status bar into a readable overlay or transcript-visible help surface.
4. Add a dialog smoke path or harness sufficient to manually exercise confirm/input/select/editor dialog UI without waiting for organic backend behavior. Use reliable editor semantics: `Ctrl+J` inserts newline and Enter submits.
5. Update project OS status/next-work with the new evidence and remaining caveats.

### Out

- Full session tree, real fork-from-entry UX, and cloned-session picker visibility.
- Rich UI / SDK sidecar parity.
- Performance SLO benchmarking beyond routine test/lint/smoke checks.
- Replacing all model/session/thinking rollback semantics; this pass only removes misleading model optimism.

## Implementation notes

- Preserve architecture invariant: `yach-ui` talks through `yach-proto`; Pi RPC shapes stay isolated in `yach-adapter-pi-rpc`.
- Stock RPC `set_model` expects `{ type: "set_model", provider, modelId }` and `get_available_models` returns full `Model` objects.
- If available model fetching fails or is absent, the selector should communicate the alpha-static/fallback state instead of pretending static choices are verified.
- `/help` can be a modal overlay closed by Esc, Enter, `q`, `h`, or `?`; prefer vim-style keys where they fit.
- Selection lists should support both arrow keys and j/k movement. Slash-command completion should be visible while typing `/` and use Tab to accept the selected completion. For future searchable selectors, reserve an explicit search trigger such as `/` so plain typing does not unexpectedly filter before search mode exists.
- Dialog smoke can be a yach-cli command that runs the TUI against a small scripted in-process backend, or another narrow harness that exercises the real `yach-ui` dialog modes.

## Verification

Completed 2026-04-27:

- Unit tests for model list parsing/serialization and App model selection behavior.
- Unit tests for help overlay command/key behavior.
- Unit/smoke coverage for the dialog harness path.
- `just fmt`
- `just test`
- `just lint`
- `just run smoke-pi-rpc`
- `git diff --check`
- Manual TUI smoke for `/help`, model selector loading/scrolling/selection/status label, slash completion, j/k selector movement, and `tui-dialog-smoke` confirm/input/select/editor dialogs.

Manual findings fixed in this pass:

- `/help` closes with `q`.
- Model selector uses backend-provided models, keeps long-list selection visible, supports j/k, and updates status after backend confirmation.
- Startup requests backend state so the status bar shows the actual model name rather than the `default` placeholder.
- Dialog editor uses reliable `Ctrl+J` newline and Enter submit semantics.
- Dialog input/editor uses `ratatui_textarea` rendering so Unicode cursor movement has a visible, non-shifting cursor.
- Slash completion is visible while typing `/`; Tab accepts selected completion and Enter executes exact commands.
