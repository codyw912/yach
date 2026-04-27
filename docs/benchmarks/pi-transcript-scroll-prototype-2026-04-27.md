# Pi Large-Transcript Scroll Prototype — 2026-04-27

## Summary

This report records a methodology prototype for same-machine Pi large-transcript scroll comparison. It does **not** contain a valid yach-vs-Pi latency result. It establishes that clean Pi can load a synthetic large-session fixture, and it documents why scroll timing is not yet strong enough to count as comparison evidence.

**Measurement class:** PTY harness / methodology prototype.

## Environment

- **Date:** 2026-04-27
- **Yach commit SHA:** `fd56580` plus branch `feat/pi-transcript-scroll-comparison` docs work
- **Pi version:** `0.70.2`
- **Pi binary:** `pi` from `/Users/cody/.local/share/mise/installs/npm-mariozechner-pi-coding-agent/0.70.2/bin/pi`
- **Machine/environment:** Apple M2 Max, macOS 26.3.1 build 25D2128, Darwin 25.3.0 arm64
- **Harness tools tried:** `/usr/bin/expect`, macOS PTY, synthetic Pi v3 session JSONL
- **Equivalence assessment:** `unsupported` for latency comparison; `approximate` for fixture shape exploration

## Clean Pi invocation

The prototype used a clean Pi baseline invocation consistent with `pi-comparison-methodology.md`:

```sh
pi \
  --session "$PI_FIXTURE" \
  --no-extensions \
  --no-skills \
  --no-prompt-templates \
  --no-themes \
  --no-context-files \
  --offline
```

## Fixture generation finding

A synthetic Pi session can be loaded if it follows Pi session v3 JSONL shape and assistant messages include usage metadata. Minimal assistant messages without `message.usage.input` caused Pi's footer render path to throw:

```text
TypeError: Cannot read properties of undefined (reading 'input')
```

Adding assistant usage fields made the synthetic fixture render successfully in clean Pi. The tested fixture alternated user and assistant text messages. A reduced 100-message fixture rendered visible `fixture message ...` transcript rows in Pi's TUI.

Representative fixture shape:

```json
{"type":"session","version":3,"id":"bench-fixture","timestamp":"2026-04-27T00:00:00.000Z","cwd":"/Users/cody/dev/yach"}
{"type":"message","id":"msg0000","parentId":null,"timestamp":"2026-04-27T00:00:00.000Z","message":{"role":"user","content":[{"type":"text","text":"fixture message 0 hello world"}],"timestamp":0}}
{"type":"message","id":"msg0001","parentId":"msg0000","timestamp":"2026-04-27T00:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"fixture message 1 hello world"}],"timestamp":0,"provider":"openai-codex","model":"gpt-5.5","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}}}
```

## Timing boundary attempted

The attempted scroll timing boundary was:

- `t0`: immediately before sending a PageUp escape sequence (`ESC [ 5 ~`) to Pi's PTY.
- `t1`: first observed post-input redraw marker or transcript output from the PTY.

This did not produce reliable latency samples. The PTY stream contains large amounts of initial render output and shell integration escape sequences. Simple `expect` patterns either matched stale buffered output or waited for timeout-scale intervals, so the resulting numbers reflected harness synchronization failure rather than Pi redraw latency.

## Result

No valid latency result.

| Workload | Status | Reason |
|---|---|---|
| Clean Pi synthetic transcript load | `prototype succeeded` | Pi rendered the fixture once assistant `usage` metadata was included. |
| Pi PageUp/PageDown scroll latency | `unsupported/no-data` | PTY redraw completion detection is not robust enough yet. |
| Yach-vs-Pi large transcript scroll comparison | `unsupported/no-data` | Timing boundaries differ: yach has an internal live terminal draw/flush sampler; Pi currently has only noisy PTY output observation. |

## Claim supported

This report supports only methodology claims:

- Synthetic Pi session fixtures are viable for large-transcript comparison if they include required assistant metadata.
- A stronger PTY redraw detector is required before publishing Pi scroll latency or yach-vs-Pi comparison results.

It does **not** support any product claim that yach is faster than Pi.

## Follow-up

1. Build a robust PTY harness that can drain initial render output, send a scroll key, and detect completed redraw without relying on stale transcript text.
2. Consider detecting terminal synchronization markers or using a controlled terminal emulator/parser instead of raw `expect` string matching.
3. Once Pi timing is robust, run equivalent 10,000-entry and 50,000-entry fixture dimensions and compare against yach's live terminal scroll reports.
4. Keep fixture generation code/report artifacts reproducible if this evolves into an executable benchmark target.
