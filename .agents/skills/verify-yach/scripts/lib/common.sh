# Shared paths and helpers for verify-yach scripts. Sourced, not executed.
#
# Every run uses a private state directory outside the repository: HOME,
# session storage, a throwaway workspace, and the scripted provider's reply
# file. Nothing touches the operator's ~/.yach or real provider credentials.
#
# yach runs on the `scripted` provider (`--features bench`): each turn's
# provider rounds come from $YACH_BENCH_SCRIPT, a JSON array of rounds, each
# an array of provider stream events. A tool-call round is followed by the
# round that answers after the tool result.

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
root=$(cd "$here/../../../.." && pwd)
tty="$here/lib/tmux-tty.sh"
session=yach
state="${XDG_STATE_HOME:-$HOME/.local/state}/sjujperpowers/verify-yach"
bin_file="$state/binary"

# The scripted provider exists only in `--features bench` builds. devenv
# sets CARGO_TARGET_DIR, so ask cargo where the binary landed rather than
# assuming target/.
build_yach() {
  (cd "$root" && just dev cargo build --quiet -p yach --features bench)
  local target
  target=$(cd "$root" && just dev cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
  mkdir -p "$state"
  printf '%s\n' "$target/debug/yach" > "$bin_file"
}

yach_bin() {
  [[ -s "$bin_file" ]] || { echo "verify-yach: no build recorded; run scripts/launch" >&2; exit 1; }
  cat "$bin_file"
}

started=$'{"type":"started","turn_id":"turn-1","model":{"provider":"scripted","model":"scripted-model"}}'
completed=$'{"type":"completed","turn_id":"turn-1","finish_reason":null,"usage":null,"provider_response_id":null}'

# One provider round that streams `text` as a single assistant reply.
text_round() {
  jq -cn --arg d "$1" --argjson s "$started" --argjson c "$completed" \
    '[$s, {type: "text_delta", turn_id: "turn-1", delta: $d}, $c]'
}

# One provider round that requests a bash tool call.
bash_round() {
  jq -cn --arg cmd "$1" --argjson s "$started" --argjson c "$completed" \
    '[$s, {type: "tool_call_completed", turn_id: "turn-1",
           tool_call: {call_id: "call-1", name: "bash", arguments_json: {command: $cmd}}}, $c]'
}

# One provider round that fails with a ProviderErrorKind, given in its
# serialized snake_case form (rate_limited, authentication, ...).
failed_round() {
  jq -cn --arg k "$1" --argjson s "$started" \
    '[$s, {type: "failed", turn_id: "turn-1",
           error: {kind: $k, message: ("scripted " + $k), redacted_debug: null}}]'
}

# One provider round that the provider ends as cancelled.
cancelled_round() {
  jq -cn --argjson s "$started" \
    '[$s, {type: "cancelled", turn_id: "turn-1", reason: "scripted cancellation"}]'
}

# One provider round that requests every tool call in a JSON array of
# {call_id, name, arguments_json} objects.
tool_round() {
  jq -cn --argjson calls "$1" --argjson s "$started" --argjson c "$completed" \
    '[$s] + [$calls[] | {type: "tool_call_completed", turn_id: "turn-1", tool_call: .}] + [$c]'
}

# The snapshot tag hashline_read mints for a file ([path#TAG]): the first 16
# hex digits, uppercase, of the SHA-256 of its LF-normalized text. Seeds
# written with LF need no normalization. macOS has shasum, not sha256sum.
file_tag() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi \
    | cut -c1-16 | tr a-f A-F
}

# Write the reply script from rounds given as arguments.
write_script() {
  jq -cs . <<<"$(printf '%s\n' "$@")" > "$state/run/script.json"
}

# Fresh run directory: home, sessions, workspace.
reset_run() {
  rm -rf "$state/run"
  mkdir -p "$state/run/home" "$state/run/sessions" "$state/run/workspace"
}

# UI checks never consult scrollback. Two equal matching frames, sampled
# apart, prevent a transient render from satisfying the wait. Actions also
# require that the settled frame differs from their pre-action frame.
# Patterns retain grep -E's per-line semantics, including ^ and $ anchors.
ui_frame() { tmux -L "sjujp-verify-$session" capture-pane -p -t "$session"; }

# Private arguments: pattern, timeout, require-change, pre-action frame,
# optional second pattern (startup requires model and composer together).
_ui_wait() {
  local pattern=$1 seconds=$2 changed=$3 before=${4-} also=${5-}
  local frame previous='' matched=0 description='settled frame matching'
  local deadline=$((SECONDS + seconds))
  if (( changed )); then description="changed $description"; fi
  while (( SECONDS < deadline )); do
    frame=$(ui_frame) || return
    if { (( !changed )) || [[ "$frame" != "$before" ]]; } \
      && grep -Eq -- "$pattern" <<< "$frame" \
      && { [[ -z "$also" ]] || grep -Eq -- "$also" <<< "$frame"; }; then
      if (( matched )) && [[ "$frame" == "$previous" ]]; then return 0; fi
      matched=1
    else
      matched=0
    fi
    previous=$frame
    sleep 0.2
  done
  printf 'verify-yach: timed out after %ss waiting for %s /%s/%s; current pane:\n' \
    "$seconds" "$description" "$pattern" "${also:+ and /$also/}" >&2
  ui_frame >&2 || true
  return 1
}

# Public UI API: ui_wait PATTERN [SECONDS], ui_send TEXT PATTERN [SECONDS],
# ui_keys PATTERN KEY... . All default to 10 seconds.
ui_wait() { _ui_wait "$1" "${2:-10}" 0; }

ui_send() {
  local text=$1 pattern=$2 seconds=${3:-10} before
  before=$(ui_frame) || return
  "$tty" send "$session" "$text" || return
  _ui_wait "$pattern" "$seconds" 1 "$before"
}

ui_keys() {
  local pattern=$1 before
  shift
  before=$(ui_frame) || return
  "$tty" keys "$session" "$@" || return
  _ui_wait "$pattern" 10 1 "$before"
}

# Start the TUI in the private tmux session against the current run state.
start_tui() {
  "$tty" start "$session" "$state/run/workspace" env -i \
    HOME="$state/run/home" PATH="$PATH" TERM=xterm-256color \
    YACH_SESSION_DIR="$state/run/sessions" \
    YACH_RIG_PROVIDER=scripted YACH_BENCH_SCRIPT="$state/run/script.json" \
    "$(yach_bin)" tui "$@"
  _ui_wait 'scripted-model' 30 0 '' 'enter send · ctrl\+j newline'
}

# Run yach as a CLI with the same isolated environment.
yach_cli() {
  (cd "$state/run/workspace" && env -i \
    HOME="$state/run/home" PATH="$PATH" \
    YACH_SESSION_DIR="$state/run/sessions" \
    YACH_RIG_PROVIDER=scripted YACH_BENCH_SCRIPT="$state/run/script.json" \
    "$(yach_bin)" "$@")
}

capture() { ui_frame > "$1"; }

# Fail unless the file contains the extended regex.
expect() {
  grep -Eq -- "$2" "$1" || { echo "verify-yach: expected /$2/ in $1" >&2; cat "$1" >&2; exit 1; }
}
