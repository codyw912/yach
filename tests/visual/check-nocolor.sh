#!/usr/bin/env bash
# VHS records pixels, not terminal bytes. Capture the real fixture TUI in a PTY.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_root="$repo_root/target/tui-visual"
workspace="$artifact_root/workspace"
typescript="$artifact_root/nocolor.typescript"
binary="${CARGO_TARGET_DIR:-$repo_root/target}/debug/yach"

cd "$repo_root"
cargo build -p yach
mkdir -p "$artifact_root/home" "$artifact_root/sessions" "$workspace"
cp "$repo_root/tests/visual/session.jsonl" "$artifact_root/sessions/session-visual.jsonl"

# Delay actual input, not literal text in a here-document. script allocates a
# PTY but is not a terminal emulator: answer crossterm's initial cursor-position
# query before sending /quit. script -e preserves the child's exit status.
# timeout bounds a hung capture without hiding that status.
printf -v launch 'stty rows 50 cols 200; cd %q && exec env -i HOME=%q PATH=%q TERM=xterm-256color COLORTERM=truecolor NO_COLOR=1 YACH_SESSION_DIR=%q %q tui --backend fixture --resume' \
  "$workspace" "$artifact_root/home" "$PATH" "$artifact_root/sessions" "$binary"
if (sleep 0.5; printf '\033[1;1R'; sleep 3; printf '/quit\r') | timeout --kill-after=2s 15s script -qfec "$launch" "$typescript" >/dev/null; then
  :
else
  status=$?
  printf 'nocolor capture failed (status %s): %s\n' "$status" "$typescript" >&2
  exit "$status"
fi

# Parse SGR parameters, not arbitrary digit substrings. Default-color resets
# (0/39/49) and emphasis (1/2/7) are not color-setting commands. Skip extended
# color operands so an RGB component cannot be mistaken for another command.
node - "$typescript" <<'JS'
const fs = require('node:fs');
const data = fs.readFileSync(process.argv[2]).toString('latin1');
if (!data.includes('Updated the greeting')) {
  console.error('nocolor capture never painted the fixture session');
  process.exit(1);
}
const found = new Set();
for (const match of data.matchAll(/\x1b\[([0-9;]*)m/g)) {
  const parameters = match[1].split(';').map(value => Number(value || 0));
  for (let i = 0; i < parameters.length; i++) {
    const p = parameters[i];
    if ((p >= 30 && p <= 37) || (p >= 40 && p <= 47) ||
        (p >= 90 && p <= 97) || (p >= 100 && p <= 107)) {
      found.add(match[0]);
    } else if ((p === 38 || p === 48) &&
               (parameters[i + 1] === 5 || parameters[i + 1] === 2)) {
      found.add(match[0]);
      i += parameters[i + 1] === 5 ? 2 : 4;
    }
  }
}
if (found.size) {
  console.error('color SGR in NO_COLOR capture:');
  for (const sequence of [...found].sort()) console.error(JSON.stringify(sequence));
  process.exit(1);
}
JS

printf 'nocolor bytes: %s\n' "$typescript"
