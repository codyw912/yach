# Pi Backend Removal

Date: 2026-07-16

Owner decision: remove the Pi reference backend and its adapter crates
before the first release.

## Rationale

The Pi backend served two purposes during development: a behavior reference
while the native path took shape, and a same-machine performance comparison
target. Both are complete: the MVP bar was met on 2026-07-16 entirely on
native primitives, and the comparison evidence is preserved in dated records
under `docs/benchmarks/`. What remained was cost and confusion — ~2,300
lines of adapter code that PRs still had to touch, a `--backend pi` flag
that could not work without a locally installed Pi binary, and a Pi-based
`run` command that failed cryptically without one.

## What was removed

- `crates/yach-adapter-pi-rpc` (~2,258 lines) and `crates/yach-adapter-pi-sdk`
  (45 lines, imported by nothing).
- `yach --backend pi` and the `TuiBackendSelection::Pi` path.
- The Pi-based `run` command and the `smoke-pi-rpc*` dev commands.
- The `pi-transcript-fixture` and `pi-clean-startup-report` bench modes and
  the `protocol`/`serialize` benches that measured Pi RPC parsing.
- Pi session-file discovery helpers in the CLI.

`print-capabilities` and the TUI dialog/bench smoke paths now use the native
backend handshake. "Inspired by Pi" remains in the README pitch: Pi is
history and inspiration, not a component.

## Recovery

Everything removed is recoverable from git history (last present at the
merge base of this change). The comparison methodology is documented in
`docs/benchmarks/pi-comparison-methodology.md` if a same-machine comparison
is ever wanted again.
