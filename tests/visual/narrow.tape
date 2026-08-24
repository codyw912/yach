Output "target/tui-visual/narrow.gif"

Set Shell "bash"
Set Width 760
Set Height 620
Set FontSize 16
Set TypingSpeed 1ms
Set Framerate 30
Set CursorBlink false
Set Padding 0
Set Margin 0

Hide
Type `cd "$YACH_VISUAL_ROOT/workspace" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color COLORTERM=truecolor YACH_SESSION_DIR="$YACH_SESSION_DIR" "$YACH_VISUAL_BINARY" tui --backend fixture --resume`
Enter
Wait+Screen /Updated the greeting/
Wait+Screen /no model/
Show
Sleep 500ms
Screenshot "target/tui-visual/narrow.png"
Sleep 1s
Hide

Ctrl+C
Sleep 500ms
