# Native Provider Error UX Plan

Date: 2026-05-04
Status: ready for narrow implementation
Related: `.project/phases/04-minimal-real-native-dogfood-path.md`, `docs/spikes/2026-04-28-rig-provider-evaluation.md`, `docs/protocol/yach-proto-v0.md`

## Goal

Make explicit `yach tui --backend native-provider` failures understandable enough for dogfood debugging while preserving the current protocol boundary and avoiding provider UX scope creep.

## Current evidence

- `yach tui --backend native-provider` is explicit and non-default; Pi remains default.
- Anthropic and ChatGPT/Codex subscription provider-request controls succeeded through `ProviderRequest -> run_provider_request(...) -> ProviderStreamEvent`.
- Invalid-model manual evidence exists for both supported provider paths.
- Provider stream timeouts map to `ProviderErrorKind::Timeout`.
- Provider model-shaped failures classify as unavailable-model failures for observed `not_found` / `not supported` provider bodies.
- Native-provider cancellation is negotiated through `PromptCancellation`, aborts the active task, and persists a cancelled turn marker.

## UX options considered

### Option A — Existing status/finish events only

Use current `StatusUpdated` and `PromptFinished` events to surface concise native-provider setup/runtime failures. Keep normalized provider kind and actionable hints in status/prompt-finished copy; keep redacted debug details in logs or CLI smoke output where already present.

Pros:

- No protocol churn.
- Lowest implementation risk.
- Keeps `yach-ui` independent from backend/provider internals.
- Enough for current dogfood, where native-provider remains explicit/experimental.

Cons:

- UI cannot render provider errors in a dedicated component.
- Error fields remain unstructured at the protocol layer.
- Future retry/help actions cannot attach to a typed error object.

### Option B — Add typed protocol error event now

Add a `ServerEvent::Error` or prompt-scoped error event with kind/message/action/debug fields.

Pros:

- Better long-term surface for UI rendering and possible retry/help actions.
- Can distinguish setup, provider, protocol, and backend errors explicitly.

Cons:

- Higher risk and broader validation surface.
- Needs policy decisions for stable error kinds, redacted debug visibility, prompt/session correlation, and Pi adapter behavior.
- Premature for the current native-provider dogfood slice.

### Option C — TUI layout-specific error panel without protocol changes

Special-case status/error presentation in `yach-ui` using existing status strings.

Pros:

- Could improve visual salience.

Cons:

- Couples UI behavior to string conventions.
- Adds UI complexity without a typed boundary.
- Worse than Option A or B architecturally.

## Recommendation

Implement Option A first: narrow status/error copy polish using existing protocol events.

Do not add a typed protocol error event yet. Revisit Option B only after at least one more dogfood pass shows status-only UX is insufficient, or when retry/help/error inspection needs become concrete.

## Implementation slice

### Scope

- Improve native-provider setup failure copy in `crates/yach-cli/src/main.rs`.
- Improve native-provider runtime failure copy for `ProviderError` values.
- Include normalized provider error kind in user-facing copy where useful.
- Add targeted helper tests for copy stability and redaction expectations.
- Keep existing protocol events: `StatusUpdated` and `PromptFinished`.

### Suggested copy shape

Setup/config failure:

```text
native provider setup failed: missing required env var YACH_RIG_ANTHROPIC_API_KEY
```

Runtime provider failure:

```text
native provider failed (unavailable_model): provider model is unavailable or unsupported; check YACH_RIG_*_MODEL
```

Cancellation:

```text
native provider cancelled
```

Timeout/network:

```text
native provider failed (timeout): provider stream timed out; try again or increase YACH_RIG_PROVIDER_TIMEOUT_SECS
native provider failed (network): provider network error; check connectivity and provider endpoint
```

Keep long redacted debug details out of ordinary TUI status unless already part of explicit smoke command output.

## Non-goals

- No typed protocol error event in this slice.
- No TUI layout/panel redesign.
- No retry/backoff policy.
- No credential persistence or provider settings UI.
- No raw provider payload persistence.
- No default backend change.
- No provider tools/resources.

## Validation

```bash
just dev cargo fmt
just dev cargo clippy -p yach-cli -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-cli -p yach-backend
git diff --check
```

## Stop / ask conditions

Stop before implementation if good UX appears to require:

- a new protocol event;
- a persistent credential/config story;
- a retry policy;
- a TUI layout redesign;
- a default backend or compatibility policy change.
