# M2 TUI Alpha Design

## Goal

Build a fullscreen terminal UI with ratatui/crossterm that replaces the CLI readline loop with proper panes, streaming, and input handling — while reusing the M1 adapter/dispatch layer.

## Scope (Core 4 Panes)

1. **Transcript pane** — scrollable view of user/assistant messages with streaming support
2. **Tool output area** — shows active tool calls (3 lines, collapsible)
3. **Input composer** — bottom bar for typing prompts
4. **Status bar** — model name, session info, status messages

## Architecture

```
yach-cli (tui mode)
  └── yach-ui (ratatui app)
        ├── App state (transcript, tool states, input buffer, status)
        ├── Event loop (tokio select! on crossterm + adapter channel)
        └── Render (ratatui layout: status | transcript | tools | input)
  └── yach-adapter-pi-rpc (unchanged)
        └── PiRpcSession spawned on spawn_blocking, bridged via mpsc
```

### Event Loop

```
tokio runtime
  ├── crossterm event stream (keyboard/mouse input)
  ├── adapter message channel (Pi RPC events via mpsc)
  └── App run loop (select! on both streams)
```

The adapter's blocking `PiRpcSession` runs in `tokio::task::spawn_blocking` and sends `DispatchAction` events through an `mpsc::UnboundedSender`. The TUI receives both keyboard events and adapter events via `tokio::select!`.

### Layout

```
┌─────────────────────────────────────┐
│ [model:gpt-5] [session:default]     │ ← Status bar (1 line)
├─────────────────────────────────────┤
│                                     │
│  User: hello                        │
│  Assistant: Hi there!               │
│  (streaming...)                     │ ← Transcript pane (flex)
│                                     │
├─────────────────────────────────────┤
│ [tool: bash] running...             │ ← Tool output area (3 lines)
├─────────────────────────────────────┤
│ > _                                 │ ← Input composer (3 lines)
└─────────────────────────────────────┘
```

### Key Decisions

- **Tokio from the start** — avoids painful cutover later, matches industry pattern (codex-rs does the same)
- **spawn_blocking for adapter** — bridges existing blocking I/O without rewriting yach-adapter-pi-rpc
- **App state owns transcript** — dispatch actions mutate app state, which triggers re-renders
- **Built-in ratatui widgets** — Paragraph, List, Block for alpha; custom widgets later

### Dependencies

- `ratatui` — TUI rendering
- `crossterm` with `event-stream` — terminal input as async stream
- `tokio` with `rt-multi-thread`, `macros`, `signal` — async runtime
- `unicode-width` — text width calculations for input wrapping

### yach-ui Crate Structure

```
yach-ui/src/
  lib.rs        # exports
  app.rs        # App state + tokio run loop
  layout.rs     # 4-pane layout rendering
  transcript.rs # transcript widget
  input.rs      # input composer widget
  status_bar.rs # status bar widget
  tool_area.rs  # tool output area widget
```

### Interaction Model

- **Enter** — submit prompt (when not streaming)
- **Escape** — clear input
- **Ctrl+C** — cancel current stream / quit
- **Up/Down** — scroll transcript when focused
- **Slash commands** — `/quit` exits, `/clear` clears transcript

### Error Handling

- Adapter disconnect → show error in status bar, return to input mode
- Spawn failure → print to stderr, exit with code 1
- Render errors → ratatui handles gracefully

### Testing

- Unit tests for App state mutations
- Unit tests for input buffer behavior
- Snapshot tests for layout rendering (future)
