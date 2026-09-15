Output "target/tui-visual/hardening.gif"

Set Shell "bash"
Set Width 1000
Set Height 560
Set FontSize 16
Set TypingSpeed 1ms
Set Framerate 30
Set CursorBlink false
Set Padding 0
Set Margin 0

Hide
Type `cd "$YACH_VISUAL_ROOT/workspace" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color COLORTERM=truecolor YACH_SESSION_DIR="$YACH_SESSION_DIR" "$YACH_VISUAL_BINARY" tui --backend fixture --resume`
Enter
Show
# `Wait+Screen` matches the pre-alt-screen buffer once the TUI takes over,
# so this tape settles on timing and proves state through screenshots.
Sleep 6s

# `/status` reports the real compaction count and the session token total.
Type "/status"
Sleep 500ms
Enter
Sleep 2s
Screenshot "target/tui-visual/hardening-status.png"
Sleep 500ms

# A mistyped command reports the nearest match and is not sent as a prompt.
# It deliberately leaves the typed text in place, so clear it afterwards.
Type "/aproval"
Sleep 500ms
Enter
Sleep 1500ms
Screenshot "target/tui-visual/hardening-unknown-command.png"
Sleep 500ms
Ctrl+U
Sleep 300ms

# Prompt history recalls the previous submission with Up.
Type "remember this prompt"
Sleep 500ms
Enter
Sleep 1500ms
Up
Sleep 1s
Screenshot "target/tui-visual/hardening-history.png"
Sleep 500ms

Hide
Ctrl+C
Sleep 500ms
