# Native Provider Checkpoint — 2026-05-03

## Status

Branch: `feat/provider-seam-spike`

Validated checkpoint after native backend/provider-seam spike.

## What works

- Pi RPC remains default backend.
- Fixture native backend remains explicit:
  - `yach tui --backend native`
- Real provider native backend is explicit/non-default:
  - `yach tui --backend native-provider`
- Rig upgraded to `rig-core = 0.36.0`.
- Working real-provider paths:
  - Anthropic API-key provider.
  - ChatGPT/Codex subscription OAuth provider.
- `ProviderRequest -> run_provider_request(...) -> ProviderStreamEvent` seam passed manual diagnostics for both providers.
- Human dogfood confirmed `yach tui --backend native-provider` launched and completed a chat/response turn.
- Native-provider assistant entries persist provider metadata in `.yach/native-sessions/default.jsonl`.
- Native/native-provider handshakes advertise `PromptCancellation`; human retest confirmed Ctrl+C cancellation works with `YACH_NATIVE_PROVIDER_TEST_DELAY_MS`.
- `.yach/` is ignored as local runtime/session state.

## Key commands

Provider request seam diagnostics:

```bash
export YACH_RIG_PROVIDER=anthropic
just dev cargo run -p yach-cli -- smoke-rig-provider-request

export YACH_RIG_PROVIDER=chatgpt-subscription
just dev cargo run -p yach-cli -- smoke-rig-provider-request
```

Native-provider dogfood:

```bash
export YACH_RIG_PROVIDER=chatgpt-subscription
export YACH_RIG_CHATGPT_TOKEN_DIR="$HOME/.cache/yach/rig-chatgpt-smoke"
just dev cargo run -p yach-cli -- tui --backend native-provider
```

Cancellation dogfood with fast models:

```bash
YACH_NATIVE_PROVIDER_TEST_DELAY_MS=5000 \
just dev cargo run -p yach-cli -- tui --backend native-provider
```

## Known limitations / deferred

- Rig OpenAI-compatible streaming failed against OpenCode Zen and OpenRouter while direct Rust HTTP controls succeeded. Deferred/non-blocking.
- No provider tools/resources execution.
- No retry loop.
- No raw provider payload persistence.
- No default backend change.
- Native session JSONL format is provisional.
- Native-provider active-turn tracking is first-pass; broader concurrent/session semantics are not designed yet.

## Validation at checkpoint

Passed:

```bash
just dev cargo fmt
just dev cargo clippy --workspace --all-targets -- -D warnings
just dev cargo test --workspace
```

## Recommended next chunks

1. Provider error dogfood: invalid key/model/auth/unavailable model, ensure redacted failed turns persist cleanly.
2. Native-provider UX polish: clearer status/finish messages and session-history reload testing.
3. Rig OpenAI-compatible investigation: compare non-streaming/direct completion/base URL usage; consider upstream issue.
4. Tool-call seam fixture/adapter pressure, still no execution.
