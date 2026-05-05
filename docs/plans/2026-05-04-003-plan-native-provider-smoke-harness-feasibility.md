# Native Provider Smoke Harness Feasibility Plan

Date: 2026-05-04
Status: feasibility recommendation; implementation not started
Related: `.project/phases/04-minimal-real-native-dogfood-path.md`, `docs/plans/2026-05-04-001-feat-native-provider-error-ux-plan.md`, `docs/benchmarks/README.md`

## Goal

Determine whether yach can add a scripted, no-secret smoke harness for native-provider dogfood UX without real provider credentials or network calls.

The target is not to prove provider SDK behavior. That evidence remains manual/approval-gated. The target is to cheaply regress the yach-owned TUI/backend flow for setup/status/cancel/error presentation.

## Existing harness surfaces

Relevant existing surfaces:

- `yach tui --backend native` fixture mode exercises TUI/protocol/session persistence without provider credentials.
- `yach tui --backend native-provider` exercises the real provider seam when env is configured.
- `YACH_NATIVE_PROVIDER_TEST_DELAY_MS` makes native-provider cancellation deterministic but still requires real provider config.
- CLI smoke commands cover provider-request paths and no-env missing-config behavior.
- `yach-bench` has headless `BenchmarkApp` / replay seams plus live terminal report commands, but those are performance harnesses and not provider-backend integration tests.

## Feasibility options

### Option A — Reuse fixture native mode only

Use `--backend native` fixture prompts to exercise status, failure, cancellation, and persistence.

Pros:

- Already no-secret and deterministic.
- Uses real TUI/protocol path.
- Low implementation cost.

Cons:

- Does not cover native-provider setup/runtime copy exactly.
- Does not exercise provider config failure paths.

Fit: good baseline, but insufficient as the native-provider UX harness by itself.

### Option B — No-env native-provider setup smoke

Add or document a command/test path that invokes `yach tui --backend native-provider` without provider env and asserts the setup failure copy is actionable.

Pros:

- No secrets or network.
- Covers native-provider setup failure UX.
- Existing code already returns before launching TUI when config is missing.

Cons:

- Does not cover runtime provider failures after prompt submission.
- TUI itself may not launch in missing-config setup failure mode.

Fit: useful and safe as a small CLI-level smoke, not a full TUI dogfood harness.

### Option C — Inject a fake provider adapter into native-provider runner

Add an explicit test-only or CLI-hidden provider config that routes `native-provider` through deterministic fake `ProviderStreamEvent`s: success, failure, timeout-like failure, cancellation delay.

Pros:

- Covers native-provider code path and status/error copy without secrets/network.
- Can exercise runtime failure and cancellation deterministically.
- Keeps provider SDK behavior out of no-secret harness.

Cons:

- Needs careful naming so it is not mistaken for a real provider.
- Adds more code/test surface to the native-provider runner.
- Must not become a broad provider mocking framework.

Fit: best implementation candidate if fixture native mode is not enough.

### Option D — Headless UI replay only

Use `yach_ui::BenchmarkApp` or similar replay seams to feed `StatusUpdated`, `PromptFinished`, and cancellation events directly.

Pros:

- Fast, deterministic, no terminal, no secrets.
- Good for UI state assertions.

Cons:

- Does not exercise CLI backend selection or native-provider runner behavior.
- More of a component test than a dogfood smoke.

Fit: useful supplement after a fake provider runner exists, not the primary integration harness.

## Recommendation

Do not add a broad smoke harness yet.

If/when implementation is approved, start with a **two-layer no-secret harness**:

1. **CLI setup smoke:** validate missing native-provider env produces `native provider setup failed: ...` without launching a provider/network path.
2. **Narrow fake provider runtime path:** add an explicit non-default test-only provider mode (for example `YACH_RIG_PROVIDER=fixture-test` or a `cfg(test)` helper, naming TBD during implementation) that emits deterministic `ProviderStreamEvent` sequences through the native-provider handler for success/failure/cancel tests.

Keep live terminal/performance harnesses out of this first smoke. They solve a different problem and often require a TTY.

## Implementation constraints if approved later

- No real provider credentials.
- No network calls.
- No raw payload persistence.
- No provider settings UI.
- No default backend change.
- No production-like fixture provider name; make test/fixture status explicit.
- Keep Rig/provider SDK types below backend adapter code.
- Prefer unit/integration tests before a new user-facing smoke command.

## Candidate implementation chunks if approved

### Candidate A — Native-provider missing-config smoke assertion

- Add a small CLI test or smoke helper for missing native-provider config copy.
- Expected files: `crates/yach-cli/src/main.rs`.
- Validation: `just dev cargo fmt && just dev cargo test -p yach-cli && git diff --check`.
- Risk: low.

### Candidate B — Fixture provider event injection for native-provider runtime tests

- Add a narrow fake provider runtime path used only by tests or an explicitly named fixture env.
- Cover success, provider failure, unavailable model, and cancellation delay.
- Expected files: `crates/yach-cli/src/main.rs`, maybe `crates/yach-backend/src/lib.rs` if fake events belong below the provider seam.
- Validation: `just dev cargo fmt && just dev cargo clippy -p yach-cli -p yach-backend --all-targets -- -D warnings && just dev cargo test -p yach-cli -p yach-backend`.
- Risk: medium; stop if it starts becoming a provider mocking framework or requires protocol changes.

### Candidate C — Headless UI replay for error rendering

- Feed status/finish/error events directly into UI app tests after runtime semantics are covered.
- Expected files: `crates/yach-ui/src/app.rs` tests.
- Validation: `just dev cargo fmt && just dev cargo test -p yach-ui && git diff --check`.
- Risk: low to medium.

## Non-goals

- No harness that requires provider credentials or token directories.
- No real provider failure automation.
- No terminal performance benchmark changes.
- No typed protocol error event implementation.
- No broad backend-runner test framework.

## Validation for this feasibility chunk

```bash
git diff --check
```

## Stop / ask conditions for future implementation

Ask before:

- adding a user-facing fixture provider mode;
- introducing a new CLI smoke command;
- adding protocol events;
- using credentials/network calls;
- expanding into live terminal/performance benchmarking.
