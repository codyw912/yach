#!/usr/bin/env bash
# Run through `just tui-themes` so cargo uses the project's development shell.
set -euo pipefail

fail() { printf 'tui-themes: %s\n' "$*" >&2; exit 2; }
require() { command -v "$1" >/dev/null 2>&1 || fail "missing tool: $1 (add it to PATH)"; }

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
label="${1:-capture}"
[[ $# -le 1 && "$label" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || fail 'usage: just tui-themes [out-label] (letters, digits, dots, underscores, hyphens)'
for tool in tmux jq node cargo; do require "$tool"; done

chromium="${CHROMIUM:-}"
if [[ -z "$chromium" ]]; then
  for candidate in chromium chromium-browser google-chrome google-chrome-stable; do
    if command -v "$candidate" >/dev/null 2>&1; then chromium="$candidate"; break; fi
  done
fi
[[ -n "$chromium" ]] || fail 'missing tool: chromium/google-chrome (add it to PATH or set CHROMIUM=/path/to/chromium)'
require "$chromium"
font="${YACH_SHOT_FONT:-}"
if [[ -z "$font" ]]; then
  require fc-match
  font="$(fc-match -f '%{file}' 'DejaVu Sans Mono:spacing=100')"
  [[ "$(fc-match -f '%{spacing}' 'DejaVu Sans Mono:spacing=100')" == 100 ]] || fail 'fc-match found no monospace font; set YACH_SHOT_FONT=/path/to/DejaVuSansMono.ttf'
fi
[[ -r "$font" ]] || fail "font is not readable: $font (set YACH_SHOT_FONT=/path/to/DejaVuSansMono.ttf)"

out="$repo_root/target/tui-themes/$label"
# Never mix obsolete captures into a new comparison, or overwrite a baseline.
[[ ! -e "$out" ]] || fail "output already exists: $out (choose a new label or remove it deliberately)"
cd "$repo_root"
cargo build -p yach
binary="${CARGO_TARGET_DIR:-$repo_root/target}/debug/yach"
binary="$(cd -- "$(dirname -- "$binary")" && pwd)/yach"
mkdir -p "$out"
work="$(mktemp -d)"
socket="yach-themes-$$"
cleanup() {
  tmux -L "$socket" kill-server 2>/dev/null || true
  rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$work/home" "$work/sessions" "$work/project" "$work/browser"
cp "$repo_root/tests/visual/session.jsonl" "$work/sessions/session-visual.jsonl"
manifest="$out/panels.jsonl"

capture() {
  local name="$1" fg="$2" bg="$3" theme="${4:-$1}" cols="${5:-120}" envs launch screen previous='' stable=0 settled=0
  local -a theme_env=("YACH_THEME=$theme")
  if [[ "$name" == nocolor ]]; then theme_env=(YACH_THEME=dark NO_COLOR=1); fi
  printf -v envs '%q ' "${theme_env[@]}"
  printf -v launch 'exec env -i HOME=%q PATH=%q TERM=xterm-256color COLORTERM=truecolor COLORFGBG=15\;0 YACH_SESSION_DIR=%q %s %q tui --backend fixture --resume' \
    "$work/home" "$PATH" "$work/sessions" "$envs" "$binary"
  tmux -L "$socket" -f /dev/null new-session -d -s capture -x "$cols" -y 40 -c "$work/project" "$launch"
  # Wait for the resumed transcript, then three identical frames. Bound failure
  # rather than silently screenshotting a blank pane or startup error.
  # Preserve trailing cells: their backgrounds are part of the rendered UI.
  for ((attempt = 0; attempt < 80; attempt++)); do
    sleep 0.25
    screen="$(tmux -L "$socket" capture-pane -e -p -N -t capture:0.0)" || fail "TUI exited while capturing $name"
    if [[ "$screen" == *'Updated the greeting'* && "$screen" == "$previous" ]]; then
      stable=$((stable + 1))
      if ((stable >= 3)); then settled=1; break; fi
    else
      stable=0
    fi
    previous="$screen"
  done
  printf '%s\n' "$screen" > "$out/$name.ansi"
  ((settled)) || fail "screen did not settle for $name; inspect $out/$name.ansi"
  tmux -L "$socket" kill-session -t capture
  jq -nc --arg name "$name" --arg fg "$fg" --arg bg "$bg" --argjson cols "$cols" '{name:$name,fg:$fg,bg:$bg,cols:$cols}' >> "$manifest"
  printf 'captured %s\n' "$name"
}

capture dark '#d0d0d0' '#1c1c1c'
capture light '#303030' '#fafafa'
capture system '#d0d0d0' '#1c1c1c'
capture nocolor '#d0d0d0' '#1c1c1c'
for palette in "$repo_root"/crates/yach-ui/themes/*.json; do
  name="${palette##*/}"
  name="${name%.json}"
  # Bundled themes express text via a variable or a literal RGB color.
  fg="$(jq -er '. as $p | .colors.text | $p.vars[.] // .' "$palette")"
  bg="$(jq -er '.vars.base00' "$palette")"
  capture "$name" "$fg" "$bg"
done
# Each composer style over the presets, at the full and the narrow width.
# `system` shows `band` falling back to `rule`.
for style in box rule band; do
  for base in dark light system; do
    file="$work/composer-$style-$base.json"
    printf '{"base":"%s","composer":{"style":"%s"}}\n' "$base" "$style" > "$file"
    if [[ "$base" == light ]]; then fg='#303030' bg='#fafafa'; else fg='#d0d0d0' bg='#1c1c1c'; fi
    for cols in 120 80; do
      capture "composer-$style-$base-$cols" "$fg" "$bg" "$file" "$cols"
    done
  done
done
node "$repo_root/tests/visual/ansi2html.mjs" "$out" "$font" "$manifest" > "$out/grids.json"
while IFS= read -r grid; do
  "$chromium" --headless=new --screenshot="$out/$grid.png" \
    --window-size=1850,1300 --hide-scrollbars --force-device-scale-factor=1 \
    --disable-dev-shm-usage --no-first-run --no-default-browser-check \
    --user-data-dir="$work/browser" --timeout=15000 --virtual-time-budget=1000 \
    "file://$out/$grid.html" > "$out/$grid.chromium.log" 2>&1 || fail "Chromium failed: inspect $out/$grid.chromium.log"
  [[ -s "$out/$grid.png" ]] || fail "Chromium produced no screenshot: $out/$grid.png"
done < <(jq -r '.[]' "$out/grids.json")
printf 'theme artifacts: %s\n' "$out"
