---
title: spike: Design Rig OpenAI-compatible provider smoke
type: spike
status: implemented-no-network-smoke
 date: 2026-05-03
---

# spike: Design Rig OpenAI-compatible provider smoke

## Goal

Design the smallest opt-in real-provider smoke for Rig behind yach-owned provider seam types. This plan does not approve implementation, credentials, network calls, native TUI provider dogfood, tools, resources, or a durable provider choice.

## Target provider shape

Use an OpenAI-compatible configuration rather than hardcoding official OpenAI only.

Required runtime inputs:

- `YACH_RIG_OPENAI_COMPAT_BASE_URL` — OpenAI-compatible API base URL.
- `YACH_RIG_OPENAI_COMPAT_API_KEY` — API key or token for that endpoint.
- `YACH_RIG_OPENAI_COMPAT_MODEL` — model id.

Optional runtime inputs:

- `YACH_RIG_OPENAI_COMPAT_PROVIDER_LABEL` — display-only provider label, default `openai-compatible`.
- `YACH_RIG_OPENAI_COMPAT_TIMEOUT_SECS` — default 30 seconds, bounded to a small range such as 5–120.
- `YACH_RIG_OPENAI_COMPAT_MAX_TOKENS` — default 32, bounded to a low smoke-test maximum such as 128.

Rationale: this can exercise official OpenAI, local/subscription proxies, or other OpenAI-compatible endpoints without committing to one provider or credential model.

## Implemented command

The explicit smoke command is implemented:

```bash
YACH_RIG_OPENAI_COMPAT_BASE_URL=...
YACH_RIG_OPENAI_COMPAT_API_KEY=...
YACH_RIG_OPENAI_COMPAT_MODEL=...
just dev cargo run -p yach-cli -- smoke-rig-openai-compatible
```

No CLI flag should accept the API key literal in the first pass. Credentials come from an environment variable only.

## Prompt and expected result

Prompt:

```text
Reply with exactly: yach-rig-smoke-ok
```

Expected pass condition:

- command exits successfully;
- one provider stream is mapped through yach-owned `ProviderStreamEvent` shapes;
- final collected assistant text contains `yach-rig-smoke-ok` or exactly equals it after trimming, depending on observed provider behavior;
- output prints a concise success line and high-level event counts, not raw provider payloads.

## Explicit non-goals

- No TUI integration.
- No native backend default change.
- No persistent credentials.
- No provider config file.
- No tools or resource loading.
- No session JSONL persistence for real provider output in the first smoke.
- No raw provider payload logging by default.
- No retry loop.
- No durable provider-library decision beyond this smoke evidence.

## Credential and privacy rules

- Read token from `YACH_RIG_OPENAI_COMPAT_API_KEY` only.
- Fail before network if required env vars are missing or empty.
- Never print or persist the API key.
- Redact obvious secret patterns in error/debug text before mapping to `ProviderError::redacted_debug`.
- Do not persist prompt/response unless a future approved evidence path explicitly opts in.
- Keep the prompt tiny and non-sensitive.

## Runtime constraints

- Use a short timeout around the smoke command.
- Set low max tokens if Rig/OpenAI-compatible builder exposes it cleanly.
- No automatic retries in yach code.
- If Rig has implicit retries that cannot be disabled or bounded, stop and reassess.
- Treat any streaming error as a single normalized `ProviderError`; do not panic.

## Mapping expectations

The smoke should reuse the existing U5 seam:

- emit/collect `ProviderStreamEvent::Started` before provider stream consumption;
- map Rig text chunks through `RigStreamMapper` into `TextDelta`;
- map message id into `provider_response_id` if Rig exposes it;
- map final response into `Completed`;
- map timeout/abort into `Cancelled` or `Failed` with explicit reason;
- map provider/auth/network/context/malformed errors into `ProviderErrorKind` as specifically as Rig exposes without raw payload persistence.

Tool-call events should be tolerated and surfaced as mapping evidence, but no tool execution should occur. If the provider unexpectedly requests tools for the tiny prompt, the smoke should fail safely rather than execute anything.

## Acceptance criteria for implementation approval

Before implementing this smoke, owner should approve:

1. Adding the explicit `smoke-rig-openai-compatible` command.
2. Reading `YACH_RIG_OPENAI_COMPAT_*` env vars.
3. Making one opt-in network call when the command is run.
4. Using a configured OpenAI-compatible endpoint/model/token.

Implementation is acceptable when:

- all normal tests/clippy pass;
- running without env vars fails locally without network;
- a manual run against an approved endpoint produces a concise evidence line;
- no secrets appear in terminal output, logs, session files, or docs;
- no Rig types leak into `yach-ui`, `yach-proto`, or native session records.

## Validation plan

Code validation after implementation approval:

```bash
just dev cargo clippy -p yach-backend -p yach-cli --all-targets -- -D warnings
just dev cargo test -p yach-backend -p yach-cli
```

No-env validation, which must fail before network:

```bash
env -u YACH_RIG_OPENAI_COMPAT_BASE_URL -u YACH_RIG_OPENAI_COMPAT_API_KEY -u YACH_RIG_OPENAI_COMPAT_MODEL just dev cargo run -p yach-cli -- smoke-rig-openai-compatible
```

Diagnostic direct HTTP smoke, useful when Rig fails but curl/provider connectivity works:

```bash
YACH_RIG_OPENAI_COMPAT_BASE_URL=...
YACH_RIG_OPENAI_COMPAT_API_KEY=...
YACH_RIG_OPENAI_COMPAT_MODEL=...
just dev cargo run -p yach-cli -- smoke-openai-compatible-http
```

Manual Rig smoke only when env/provider are explicitly available:

```bash
YACH_RIG_OPENAI_COMPAT_BASE_URL=...
YACH_RIG_OPENAI_COMPAT_API_KEY=...
YACH_RIG_OPENAI_COMPAT_MODEL=...
just dev cargo run -p yach-cli -- smoke-rig-openai-compatible
```

Docs-only validation for this plan:

```bash
git diff --check
```

## Stop conditions

Stop and ask before implementation if:

- Rig only exposes the desired path through an agent loop that owns history/tools/session semantics.
- Rig cannot construct an OpenAI-compatible client from explicit base URL and token without using a panic-prone `from_env()` path.
- The smoke needs persistent config or credential storage.
- The smoke requires tool execution, resources, TUI integration, native backend default changes, or provider-specific core protocol changes.
- The endpoint requires OAuth/browser/session handling not representable as an OpenAI-compatible API token/base URL pair.
