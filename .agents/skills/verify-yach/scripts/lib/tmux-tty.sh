#!/usr/bin/env bash
# Drive a CLI or TUI through a private tmux server, for project verify skills.
# Copied into .agents/skills/verify-<project>/scripts/lib/ by
# sjujperpowers:creating-a-verification-skill.
#
#   tmux-tty.sh start <name> <dir> <command...>   start command in a detached 200x50 pane
#   tmux-tty.sh send <name> <text>                 type text, then Enter
#   tmux-tty.sh keys <name> <tmux-key>...          send raw keys (C-c, Up, Escape)
#   tmux-tty.sh wait <name> <ERE> [seconds]        wait until the screen matches (default 10)
#   tmux-tty.sh capture <name> <file>              write the visible screen to file
#   tmux-tty.sh stop <name>                        kill the session's server
#   tmux-tty.sh evidence-dir <project>             create and print a fresh evidence directory
#
# Each <name> gets its own tmux server socket, so a run never touches the
# operator's tmux sessions. Evidence lives outside the repository, so Jujutsu
# never snapshots it into a change.
set -euo pipefail

die() { printf 'tmux-tty: %s\n' "$*" >&2; exit 2; }
sock() { printf 'sjujp-verify-%s' "$1"; }
t() { local name=$1; shift; tmux -L "$(sock "$name")" "$@"; }
screen() { t "$1" capture-pane -p -S - -t "$1"; }

cmd=${1:-}; shift || true
case "$cmd" in
  start)
    [[ $# -ge 3 ]] || die "usage: start <name> <dir> <command...>"
    name=$1 dir=$2; shift 2
    [[ -d "$dir" ]] || die "no such directory: $dir"
    t "$name" has-session -t "$name" 2>/dev/null && die "session $name is already running"
    # The placeholder keeps a pane alive long enough to arm remain-on-exit;
    # respawn then swaps in the command, which can exit immediately. Several
    # argv words make tmux exec directly, with no default-shell parsing, so
    # the bash wrapper also keeps a lone "word with spaces" one program name.
    t "$name" -f /dev/null new-session -d -s "$name" -x 200 -y 50 -c "$dir" cat
    t "$name" set-option -t "$name" remain-on-exit on >/dev/null
    t "$name" respawn-pane -k -t "$name" -c "$dir" bash -c 'exec "$@"' bash "$@"
    ;;
  send)
    [[ $# -eq 2 ]] || die "usage: send <name> <text>"
    t "$1" send-keys -t "$1" -l -- "$2"
    t "$1" send-keys -t "$1" Enter
    ;;
  keys)
    [[ $# -ge 2 ]] || die "usage: keys <name> <tmux-key>..."
    name=$1; shift
    t "$name" send-keys -t "$name" "$@"
    ;;
  wait)
    [[ $# -ge 2 && $# -le 3 ]] || die "usage: wait <name> <ERE> [seconds]"
    name=$1 pattern=$2 seconds=${3:-10}
    deadline=$((SECONDS + seconds))
    while (( SECONDS < deadline )); do
      if screen "$name" | grep -Eq -- "$pattern"; then exit 0; fi
      sleep 0.2
    done
    printf 'tmux-tty: timed out after %ss waiting for /%s/; screen:\n' "$seconds" "$pattern" >&2
    screen "$name" >&2 || true
    exit 1
    ;;
  capture)
    [[ $# -eq 2 ]] || die "usage: capture <name> <file>"
    screen "$1" > "$2"
    ;;
  stop)
    [[ $# -eq 1 ]] || die "usage: stop <name>"
    t "$1" kill-server 2>/dev/null || true
    ;;
  evidence-dir)
    [[ $# -eq 1 ]] || die "usage: evidence-dir <project>"
    dir="${XDG_STATE_HOME:-$HOME/.local/state}/sjujperpowers/evidence/$1/$(date -u +%Y%m%dT%H%M%SZ)-$$"
    mkdir -p "$dir"
    printf '%s\n' "$dir"
    ;;
  *)
    sed -n '2,13p' "$0" >&2
    exit 2
    ;;
esac
