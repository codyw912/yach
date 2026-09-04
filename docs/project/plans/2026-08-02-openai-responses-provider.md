# OpenAI Responses Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** OpenAI proper talks to its canonical endpoint via a new `RigProviderConfig::OpenAi` variant on rig's default (Responses) client.

**Architecture:** One new enum variant, one new match arm delegating to the existing `PreparedCompletion::run`, env parsing in the CLI's provider match sites, and a fourth smoke function through the shared smoke helpers. Spec: `docs/project/specs/2026-08-02-openai-responses-provider-design.md`.

**Tech Stack:** Rust workspace; jj (not raw git); `just dev <cmd>` wraps the nix dev shell.

## Global Constraints

- Run every cargo command as `just dev cargo <...>` from /Users/cody/dev/yach.
- Strict clippy: `-D warnings`, `panic!` banned even in tests (use `assert!`/`unreachable!`), `#[expect]` over `#[allow]`, cognitive complexity max 15, 100-line functions max.
- `just dev cargo fmt -p <crate>` after edits; `--check` clean before commit.
- Never use `perl -pi -e` or multi-line `sed`; exact-match edits only.
- Commit with `jj commit -m "..."`; no AI attribution; `jj st` must list only intended files first.
- The compile check in Task 1 Step 2 is the spec's named risk gate: if the default client's `completion_model` does NOT resolve to the Responses model, STOP and report BLOCKED (the spec names `responses_api()`-explicit construction as the fallback, but that is a controller decision, not an implementer improvisation).
- Env-var naming: `YACH_RIG_OPENAI_API_KEY`, `YACH_RIG_OPENAI_MODEL` (the `OPENAI_COMPAT_*` family stays untouched).

---

### Task 1: The `OpenAi` variant, adapter arm, and CLI parsing

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs` — `RigProviderConfig` enum (~line 80) and the provider match in `run_provider_request_with_approved_tools` (~line 241).
- Modify: `crates/yach-cli/src/main.rs` — `rig_provider_adapter_config_from_env_with_model_override` (~773), the "must be anthropic..." reason strings (three sites: ~800, ~859, ~1002), the provider/model match at ~994, `provider_model_from_env` (~2280), `provider_label_from_config` (~2296).

**Interfaces:**
- Consumes: `PreparedCompletion::run` (Task-agnostic, exists), `provider_internal_error`, `openai::Client::builder()`.
- Produces: `RigProviderConfig::OpenAi { api_key: String }` — Task 2's smoke and Task 3's profile select it via `YACH_RIG_PROVIDER=openai`.

- [ ] **Step 1: Add the variant**

In the `RigProviderConfig` enum, after the `Anthropic` variant:

```rust
    /// OpenAI proper over the Responses API — rig's default client, the
    /// canonical endpoint. Aggregators wearing the chat-completions shape
    /// use `OpenAiCompatible` instead. No base-URL override until a
    /// Responses-speaking aggregator exists (design:
    /// `docs/project/specs/2026-08-02-openai-responses-provider-design.md`).
    OpenAi {
        api_key: String,
    },
```

- [ ] **Step 2: Add the adapter arm and prove the resolution**

In the provider match in `run_provider_request_with_approved_tools`, after the `Anthropic` arm:

```rust
        RigProviderConfig::OpenAi { api_key } => {
            let client = openai::Client::builder()
                .api_key(&api_key)
                .build()
                .map_err(|error| provider_internal_error(&error))?;
            let model = client.completion_model(attempt.request.model.model.clone());
            attempt.run(model).await
        }
```

Run: `just dev cargo check -p yach-backend`
Expected: compiles. This IS the spec's risk gate — it proves the default client's `completion_model` resolves to the Responses model and that its `StreamingResponse` satisfies `Clone + Unpin + GetTokenUsage`. If it fails to compile on the associated type or bounds, STOP: report BLOCKED with the exact error.

- [ ] **Step 3: CLI parsing arm**

In `rig_provider_adapter_config_from_env_with_model_override`, after the `"anthropic"` arm:

```rust
        // OpenAI proper over the Responses API (canonical endpoint).
        // Aggregators wearing the chat-completions shape use
        // openai-compatible. Like compat, no default model: require it
        // up front so misconfiguration fails at setup, unless the
        // caller overrides the model directly.
        "openai" => {
            if !model_overridden {
                let _ = required_env("YACH_RIG_OPENAI_MODEL")?;
            }
            RigProviderConfig::OpenAi {
                api_key: required_env("YACH_RIG_OPENAI_API_KEY")?,
            }
        }
```

Update the reason string in this function's `_` arm to:
`"must be anthropic, chatgpt-subscription, openai, or openai-compatible"`
and make the SAME wording change at the other two "must be anthropic..." sites (~859 and ~1002 — grep `must be anthropic` to catch all three).

- [ ] **Step 4: Model and label sites**

`provider_model_from_env` (~2280) gains, after the `"chatgpt-subscription"` arm:

```rust
        // No default model on OpenAI proper either; config parsing
        // requires this env when the provider is selected.
        "openai" => optional_env("YACH_RIG_OPENAI_MODEL").unwrap_or_default(),
```

`provider_label_from_config` (~2296) gains:

```rust
        RigProviderConfig::OpenAi { .. } => "openai",
```

The provider/model match at ~994 (mirrors 843's shape): add `"openai" => optional_env("YACH_RIG_OPENAI_MODEL").unwrap_or_default(),` following the pattern of the surrounding arms — read the site first; if it requires the model, mirror the compat arm's handling exactly.

- [ ] **Step 5: Document the MaxTokensParam boundary**

In the doc comment above `max_tokens_param` in `rig_provider_adapter_config_from_env_with_model_override` (~827–835), append one sentence:

```
        // Applies to the openai-compatible (chat-completions) shape; the
        // `openai` provider rides the Responses API, where rig maps
        // `max_tokens` to `max_output_tokens` natively.
```

- [ ] **Step 6: Tests**

Add beside the existing label test coverage (grep `provider_label_from_config` in main.rs tests; if none exists, add a small test module near the function):

```rust
    #[test]
    fn provider_label_covers_openai_responses_variant() {
        let config = RigProviderAdapterConfig {
            provider: RigProviderConfig::OpenAi {
                api_key: String::from("test-key"),
            },
            timeout: std::time::Duration::from_secs(5),
            max_tokens: 1024,
            context_window: 10_000,
            max_tokens_param: MaxTokensParam::default(),
        };
        assert_eq!(provider_label_from_config(&config), "openai");
    }
```

(Adapt field construction to the real struct if it has grown; the assertion is the point. No env-var tests — the repo deliberately avoids process-global env in tests outside the extension store.)

- [ ] **Step 7: Gate and commit**

Run: `just dev cargo fmt -p yach-backend && just dev cargo fmt -p yach && just dev cargo fmt --check -p yach-backend && just dev cargo fmt --check -p yach && just dev cargo test --workspace 2>&1 | grep -E "^test result" && just dev cargo clippy --workspace --all-targets`
Expected: all green, zero warnings. (The yach-cli crate's package name is `yach`; adjust the fmt invocation if `-p yach` errors — use the crate name Cargo reports.)

```bash
jj commit -m "feat: OpenAI Responses provider — canonical endpoint for OpenAI proper"
```

---

### Task 2: Smoke parity

**Files:**
- Modify: `crates/yach-backend/src/rig_adapter.rs` — new `run_openai_smoke` beside `run_openai_compatible_smoke` (~line 690), plus a config struct beside `RigOpenAiCompatibleSmokeConfig`.
- Modify: `crates/yach-cli/src/main.rs` — command parse table (~82–86), dispatch (~278–282), a `run_rig_openai_smoke` entry function modeled on `run_rig_anthropic_smoke` (~1232), and the `run_rig_provider_request_smoke` provider matches (~843–879): add `"openai"` arms (model default: none — mirror how the function errors for unsupported providers if the model env is missing; read the existing compat handling in that function first and follow its pattern exactly).

**Interfaces:**
- Consumes: `stream_smoke_completion`, `collect_rig_smoke_stream`, `provider_internal_error` (all exist).
- Produces: `run_openai_smoke(config: RigOpenAiSmokeConfig) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError>`; CLI command `smoke-rig-openai`.

- [ ] **Step 1: Config struct and smoke function**

```rust
#[derive(Debug, Clone)]
pub struct RigOpenAiSmokeConfig {
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
    pub max_tokens: u64,
}

pub async fn run_openai_smoke(
    config: RigOpenAiSmokeConfig,
) -> Result<RigOpenAiCompatibleSmokeReport, ProviderError> {
    let client = openai::Client::builder()
        .api_key(&config.api_key)
        .build()
        .map_err(|error| provider_internal_error(&error))?;
    let model = client.completion_model(config.model.clone());
    let stream = stream_smoke_completion(&model, config.max_tokens).await?;
    collect_rig_smoke_stream(stream, "openai", config.model, config.timeout).await
}
```

(Mirror the existing smoke functions' exact style; the report type is shared deliberately — do not add a new report struct.)

- [ ] **Step 2: CLI wiring**

- Parse table: `Some("smoke-rig-openai") => Command::SmokeRigOpenAi,`
- Enum + dispatch: `Self::SmokeRigOpenAi => run_rig_openai_smoke(),`
- `run_rig_openai_smoke`: copy `run_rig_anthropic_smoke`'s body shape (~1232), reading `YACH_RIG_OPENAI_API_KEY` (required) and `YACH_RIG_OPENAI_MODEL` (required — no default, matching Task 1's parsing posture), same timeout/max_tokens envs as the other smokes, calling `run_openai_smoke` and reusing the same `CommandResult` variant the anthropic smoke uses for its report (read the anthropic entry first; whatever result variant it returns, return the same shape with the openai label).
- `run_rig_provider_request_smoke` (~843): add `"openai"` arms to both provider matches (model from `YACH_RIG_OPENAI_MODEL` — required, no fallback default; config arm builds `RigProviderConfig::OpenAi`), and confirm the function's error message site was already updated by Task 1.

- [ ] **Step 3: Tests**

The smoke functions are network functions; existing coverage pattern is compile + the shared collector's unit tests, plus CLI parse-table coverage if a test exists for command parsing (grep `smoke-rig-anthropic` in tests; mirror whatever exists for it — if nothing, add nothing).

- [ ] **Step 4: Gate and commit**

Run: `just dev cargo fmt --check` (both crates), `just dev cargo test --workspace 2>&1 | grep -E "^test result"`, `just dev cargo clippy --workspace --all-targets`
Expected: green, zero warnings.

```bash
jj commit -m "feat: openai Responses smoke — smoke-rig-openai"
```

---

### Task 3: Eval verification (controller-led; needs the owner for credentials and the profile edit)

**Files:**
- The owner's local profile `~/tmp/yach-rotation/profiles/openai.env` (scaffold below; the owner confirms the secret reference).
- Create: `docs/project/records/2026-08-XX-openai-responses-measurement.md`
- Modify: `docs/project/board.md`

- [ ] **Step 1: Flip the profile (owner confirms)**

New `openai.env` contents (the key reference line is the owner's local secret-manager reference — keep whatever reference the current file carries):

```
# OpenAI proper over the Responses API (canonical endpoint) — flipped
# from the chat-completions shape 2026-08-02 per the spec's matrix
# decision. Aggregators keep the openai-compatible shape.
YACH_RIG_PROVIDER=openai
YACH_RIG_OPENAI_API_KEY=<same secret reference as the current file>
YACH_RIG_OPENAI_MODEL=gpt-5.4-mini
```

(`YACH_RIG_PROVIDER_MAX_TOKENS_PARAM` drops — the Responses path maps the spelling natively.)

- [ ] **Step 2: Image + gate**

`just runtime-image && bash evals/scripts/check-image-fresh.sh`, then the gate via the owner's profile runner with the anthropic profile. Expected: FRESH; 7/7 + driver checks.

- [ ] **Step 3: The 125-cell sweep**

Five tasks × five profiles × five repeats into `~/tmp/yach-rotation/sweeps/2026-08-XX-openai-responses/`, `YACH_ROTATE_PROFILE_RUNNER` set, owner present for credential resolution. Reference: 2026-08-02 text-results sweep (125/125). Launch-failure rows are re-run as a patch block, never read as a rate.

- [ ] **Step 4: Spot checks**

- openai cells: `"reported": true` with real token counts in outcome documents (Responses usage flows through `GetTokenUsage`).
- Any openai cell failure: classify against the known classes (behavioral, rate limit, classifier gap on the new error dialect — the last is DATA for the slated classifier item, record it).

- [ ] **Step 5: Record + board + commit**

Record in the style of `2026-08-02-text-tool-results-measurement.md`: rates table, comparison, the Responses-first-baseline framing for the openai column, coverage note (chatgpt-subscription still unmeasured), any classifier-gap findings. Board: move the item to MEASURED; note the compactor item is now unblocked.

```bash
jj commit -m "docs: record the OpenAI Responses measurement"
```
