Output "target/tui-visual/session.gif"

Set Shell "bash"
Set Width 1200
Set Height 720
Set FontSize 16
Set TypingSpeed 1ms
Set Framerate 30
Set CursorBlink false
Set Padding 0
Set Margin 0

Hide
Type `cd "$YACH_VISUAL_ROOT/workspace" && env -i HOME="$HOME" PATH="$PATH" TERM=xterm-256color COLORTERM=truecolor "$YACH_VISUAL_BINARY" tui --backend fixture --resume`
Enter
Wait+Screen /Updated the greeting/
Wait+Screen /no model/
Show
Sleep 500ms
Screenshot "target/tui-visual/session.png"
Sleep 1s

Type "/status"
Enter
Wait+Screen /Session status/
Sleep 500ms
Screenshot "target/tui-visual/status.png"
Sleep 1s
Hide

Ctrl+C
Sleep 500ms
