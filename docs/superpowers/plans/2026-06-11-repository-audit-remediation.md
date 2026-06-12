# Repository Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address the repository audit efficiently by removing the highest reliability and enforcement risks first, then reducing structural drag without mixing refactors with behavior changes.

**Architecture:** Treat the audit as a remediation program with small reviewable slices. CI and regression tests come before risky behavior changes; session durability and load behavior come before large native-runner extraction; perimeter fixes stay narrow unless they block native MVP dogfooding.

**Tech Stack:** Rust 2024 workspace, `just`/devenv command entry points, tokio, serde JSONL session logs, ratatui TUI, Jujutsu (`jj`) for checkpoints.

---

## Scope

This is a sequencing plan for the audit findings, not a single giant patch. The audit spans independent subsystems: session persistence, backend event-loop behavior, CI/tooling, CLI parsing, Pi adapter lifecycle, extension process environment, and large-file decomposition. Each milestone below should become one or more focused implementation branches or child plans before code changes begin.

The plan intentionally optimizes for MVP dogfooding risk:

1. Keep the native default usable and debuggable.
2. Make durable session evidence trustworthy.
3. Add machine enforcement for the lint/test culture already present locally.
4. Defer low-payoff cleanup until High findings are closed.

## Current Evidence

Verified on 2026-06-11:

- Working copy is clean at `pkxunqnn 568e0493`, parent `zzppmvkn 8586f49b main main@origin`.
- `README.md` still describes Pi-first phases and omits `crates/yach-backend` from the workspace list.
- `crates/yach-backend/src/session_store.rs` uses `flush()` after append but not `sync_data()` or `sync_all()`.
- `NativeSessionLog::load_from_file` in `crates/yach-backend/src/session.rs` fails the full load on one bad JSONL line.
- `crates/yach-backend/src/native_runner.rs` still has `store.load().unwrap_or_default()` at startup and prompt/resume paths.
- `crates/yach-cli/src/main.rs` still maps unknown subcommands to `Command::BootstrapStub`.
- `crates/yach-adapter-pi-rpc/src/session.rs` pipes child stderr, does not drain it, and has no `Drop` cleanup for `PiRpcSession`.
- No `.github/` CI directory is present.
- Large files are still concentrated: `native_runner.rs` 9632 lines, backend `lib.rs` 6504, UI `app.rs` 5155, CLI `main.rs` 4590.

## Triage Rules

- Treat session durability, corruption visibility, and per-prompt reloads as the top correctness track.
- Add CI before the large-file extraction track.
- Keep quick wins only if they are truly local and reduce future ambiguity.
- Do not refactor `native_runner.rs` while changing session semantics in the same change.
- Use `just` recipes for verification: `just fmt`, `just lint`, `just test`.
- Use `jj describe -m "<completed intent>"` and `jj new` after each reviewable slice.
- Before handoff or publish, inspect `jj log -r 'main..@'`.

## Milestone 0: Safety Net And Planning Alignment

**Purpose:** Make the repository's next move match the audit, then add the missing machine gate.

**Files:**

- Modify: `README.md`
- Modify: `docs/project/next.md`
- Optionally modify: `docs/project/state.md`
- Create: `.github/workflows/ci.yml`

### Task 0.1: Align README With Native-First Reality

- [ ] Update `README.md` workspace layout to include `crates/yach-backend`.
- [ ] Replace the Pi-first opening paragraph with native-first language: native backend is the default; Pi remains an explicit reference backend.
- [ ] Keep the PRD reference as historical/product-direction source, but point active work selection to `docs/project/README.md`.
- [ ] Verify the README renders cleanly by reviewing the first 80 lines.

Run:

```sh
sed -n '1,100p' README.md
```

Expected: the backend crate is listed and the opening no longer implies current work is Pi-first.

### Task 0.2: Add CI

- [ ] Create `.github/workflows/ci.yml`.
- [ ] Run on pull requests and pushes to `main`.
- [ ] Include Linux first; add macOS only if runner cost is acceptable for the owner.
- [ ] Use the same entry points humans use:

```sh
just fmt
just lint
just test
```

- [ ] Cache Cargo/Nix only if the first version is too slow. Do not block CI on cache tuning.

Acceptance:

- A seeded formatting or clippy error fails CI.
- A clean checkout passes CI with the same commands that pass locally.

### Task 0.3: Update Active Planning Docs

- [ ] Update `docs/project/next.md` so the recommended next move is audit remediation safety net plus session reliability, not a fresh dogfood checkpoint.
- [ ] Keep the dogfood checkpoint as the next validation step after session reliability is fixed.
- [ ] Add this plan to the relevant sources list.
- [ ] Update `docs/project/state.md` only if the risk summary or relevant-record list needs the audit context.

Verification:

```sh
sed -n '1,180p' docs/project/next.md
```

Expected: a future session can choose the first remediation slice without rereading the full audit.

## Milestone 1: Narrow Quick Wins

**Purpose:** Close cheap, localized defects before deeper session work.

**Files:**

- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-adapter-pi-rpc/src/session.rs`
- Modify: `crates/yach-adapter-pi-rpc/src/parse.rs`
- Test: inline test modules in the same files

### Task 1.1: Unknown CLI Commands Fail

- [ ] Add an explicit unknown-command representation instead of falling through to `Command::BootstrapStub`.
- [ ] Preserve empty-argument behavior if bootstrap stub is still the intended default.
- [ ] Return usage text and an exit code equivalent to command-line misuse for unknown commands.
- [ ] Add tests for:
  - empty args still map to bootstrap stub;
  - `yach tiu` does not map to bootstrap stub;
  - unknown command output names the unknown command and lists the main commands.

Run:

```sh
just dev cargo test -p yach-cli cli_defaults_to_bootstrap_stub cli_unknown_command -- --nocapture
```

Expected: default bootstrap behavior remains covered; unknown command test proves non-zero misuse behavior.

### Task 1.2: Pi RPC Child Hygiene

- [ ] Drain child stderr in a background thread after spawn.
- [ ] Store the stderr-drain handle only if joining is safe and non-blocking at drop time; otherwise detach it and document why.
- [ ] Implement `Drop` for `PiRpcSession` that kills a still-running child and waits for it.
- [ ] Add tests using a local shell/Rust fixture command that writes enough stderr to prove the pipe is drained.
- [ ] Add tests that dropping a spawned session terminates the child process.

Run:

```sh
just dev cargo test -p yach-adapter-pi-rpc session -- --nocapture
```

Expected: adapter tests pass and no child process remains after the drop test.

### Task 1.3: Cap Pi RPC Line Length And Fix Parsed Session ID

- [ ] Add a maximum line length to `PiRpcReader::read_next`.
- [ ] Return a structured parse/session error when the cap is exceeded.
- [ ] Replace hardcoded parsed session id `"active"` in `parse.rs` with the id from the incoming payload when present.
- [ ] Add tests for oversized line rejection and non-`active` session id parsing.

Run:

```sh
just dev cargo test -p yach-adapter-pi-rpc parse session -- --nocapture
```

Expected: parse tests cover real session ids and reader tests cover line caps.

Checkpoint:

```sh
jj describe -m "fix CLI misuse and Pi RPC process hygiene"
jj new
```

## Milestone 2: Session Store Correctness

**Purpose:** Make the canonical transcript durable and recoverable before changing runner state ownership.

**Files:**

- Modify: `crates/yach-backend/src/session_store.rs`
- Modify: `crates/yach-backend/src/session.rs`
- Modify: `crates/yach-backend/src/native_runner.rs`
- Test: inline tests in `session_store.rs`, `session.rs`, and existing native runner tests
- Benchmark: `crates/yach-bench`

### Task 2.1: Regression Tests First

- [ ] Add a test that writes a valid JSONL log with a truncated final line and expects valid earlier events to load.
- [ ] Add a test that writes a valid log with one corrupt middle line and expects surrounding events to load.
- [ ] Add a test that load warnings include line number and reason without including file contents.
- [ ] Add a test proving `NativeJsonlSessionStore::append_events` performs exactly one durability operation per batch by using an injectable writer or small test seam.

Run:

```sh
just dev cargo test -p yach-backend session_store session_log_corruption -- --nocapture
```

Expected before implementation: new corruption-tolerant tests fail against current all-or-nothing load behavior.

### Task 2.2: Add Durable Append Semantics

- [ ] Change append paths to write complete newline-terminated events and call `sync_data()` after the batch write.
- [ ] Keep `append_events` as the preferred batching path, with one sync per batch.
- [ ] Set session log file permissions to owner-read/write on Unix when creating the file.
- [ ] Keep cross-platform behavior compiling on non-Unix targets.
- [ ] Consider a later `Durability::Strict | Relaxed` setting only if benchmarks show fsync cost is unacceptable.

Run:

```sh
just dev cargo test -p yach-backend session_store -- --nocapture
```

Expected: append/load tests pass, including Unix permission assertions where applicable.

### Task 2.3: Load With Warnings, Not Silent Empty Logs

- [ ] Change the load API to return both `NativeSessionLog` and structured load warnings, or add a new tolerant load API and migrate call sites.
- [ ] Preserve hard I/O errors for missing/unreadable files where they indicate real filesystem failure.
- [ ] Treat malformed non-empty JSONL lines as recoverable warnings.
- [ ] Stop using `store.load().unwrap_or_default()` in `native_runner.rs`.
- [ ] Surface load warnings as a bounded UI status event and durable metric/evidence event if a suitable event already exists.

Run:

```sh
just dev cargo test -p yach-backend native_session session_store -- --nocapture
```

Expected: corrupted logs resume with valid events intact and user-visible warning coverage.

### Task 2.4: Measure Durability Cost

- [ ] Run the existing native session benchmark/report path.
- [ ] Record append and load timing before and after strict sync behavior.
- [ ] If strict fsync is too slow for practical interactive use, write a follow-up design for bounded-loss batching instead of weakening silently.

Run:

```sh
just dev cargo run -p yach-bench -- native-session-profile-report
```

Expected: benchmark output is captured in a short docs note or PR summary.

Checkpoint:

```sh
jj describe -m "harden native session store durability and corruption recovery"
jj new
```

## Milestone 3: Stop Per-Prompt Disk Reloads

**Purpose:** Remove O(session-size) blocking disk work from the prompt path.

**Files:**

- Modify: `crates/yach-backend/src/native_runner.rs`
- Consider create: `crates/yach-backend/src/native_runner/session_state.rs`
- Test: existing native runner session/resume/provider transcript tests

### Task 3.1: Introduce Runner-Owned Session State

- [x] Load the session log once at backend loop startup.
- [x] Keep the in-memory `NativeSessionLog` as the authoritative state for turn indexing and provider transcript projection.
- [x] Append-through to `NativeJsonlSessionStore` whenever a new event is recorded.
- [ ] Update in-memory state only after append succeeds when durability is required for correctness.
- [x] Replace reload sites at startup and prompt/resume paths with references to runner-owned state.

Run:

```sh
just dev cargo test -p yach-backend native_provider_agent native_session -- --nocapture
```

Expected: provider transcript, resume, turn-index, tool evidence, and edit evidence tests still pass.

### Task 3.2: Move Remaining Session File I/O Off The Reactor

- [x] Use `tokio::task::spawn_blocking` for startup load and any unavoidable synchronous session file operations from async runner code.
- [x] Ensure load warnings and load errors still route back through normal backend events.
- [x] Add a test or instrumentation seam proving prompt handling does not call `NativeJsonlSessionStore::load`.

Run:

```sh
just dev cargo test -p yach-backend native_runner -- --nocapture
```

Expected: no per-prompt load occurs in the tested prompt path.

### Task 3.3: Revisit `std::sync::Mutex` In Async Runner State

- [x] Identify extension scan/activation state locks held in async code.
- [x] Replace with `tokio::sync::Mutex` only where waits can cross async scheduling points, or move ownership into the backend event loop if that is simpler.
- [x] Keep the change separate from session-state behavior if it grows beyond a local mechanical edit.

Run:

```sh
just lint
just test
```

Expected: lint wall remains clean and no async lock regression is introduced.

Checkpoint:

```sh
jj describe -m "keep native session log in memory during backend runs"
jj new
```

## Milestone 4: Extension Host Environment Boundary

**Purpose:** Keep third-party extension processes from inheriting provider/API secrets by default.

**Files:**

- Modify: `crates/yach-backend/src/extension.rs`
- Test: inline extension host process tests

### Task 4.1: Add Explicit Environment Policy

- [ ] Add a small extension-host env policy helper near `configure_extension_host_process`.
- [ ] Call `env_clear()` for extension host commands.
- [ ] Allowlist only required execution environment variables, initially `PATH`, `HOME`, `LANG`, `LC_ALL`, `LC_CTYPE`, and platform-specific variables proven necessary by tests.
- [ ] Do not pass provider API keys or arbitrary parent env by default.
- [ ] Add a test that sets a sentinel parent env var and proves the extension child cannot read it.

Run:

```sh
just dev cargo test -p yach-backend extension_host_env -- --nocapture
```

Expected: sentinel env is absent; allowed vars required for process launch remain available.

Checkpoint:

```sh
jj describe -m "restrict extension host inherited environment"
jj new
```

## Milestone 5: Backend Structure Extraction

**Purpose:** Reduce the blast radius of future backend changes after behavior is stable.

**Files:**

- Modify/split: `crates/yach-backend/src/native_runner.rs`
- Consider create:
  - `crates/yach-backend/src/native_runner/mod.rs`
  - `crates/yach-backend/src/native_runner/session_state.rs`
  - `crates/yach-backend/src/native_runner/tool_loop.rs`
  - `crates/yach-backend/src/native_runner/provider_prompt.rs`
  - `crates/yach-backend/src/native_runner/local_edit.rs`
  - `crates/yach-backend/src/native_runner/trace.rs`

### Task 5.1: Prepare A Move-Only Extraction Plan

- [ ] Run `rg -n "^fn |^async fn |^struct |^enum |^impl " crates/yach-backend/src/native_runner.rs`.
- [ ] Group private items by responsibility and existing section comments.
- [ ] Choose the first extraction by lowest coupling, likely tool continuation/result shaping or session state after Milestone 3.
- [ ] Do not change behavior in the same commit as a move.

### Task 5.2: Extract One Module At A Time

- [ ] Move a cohesive group of functions/types into the new module.
- [ ] Use `pub(crate)` only where tests or sibling modules require it.
- [ ] Move only directly related tests, or keep tests in parent module until extraction stabilizes.
- [ ] Run the focused backend tests after each extraction.

Run:

```sh
just dev cargo test -p yach-backend native_provider_agent native_session -- --nocapture
just lint
```

Expected: no behavior changes; production `native_runner` files move toward sub-3000-line modules.

Checkpoint after each reviewable extraction:

```sh
jj describe -m "extract native runner <responsibility> module"
jj new
```

## Milestone 6: CLI Library And Clap

**Purpose:** Make CLI behavior reusable and testable before adopting a parser crate.

**Files:**

- Create: `crates/yach-cli/src/lib.rs`
- Modify: `crates/yach-cli/src/main.rs`
- Modify: `crates/yach-cli/Cargo.toml`

### Task 6.1: Move CLI Logic Into A Library

- [ ] Create `src/lib.rs` and move command parsing, command execution, smoke helpers, and extension command helpers out of `main.rs`.
- [ ] Leave `main.rs` as a thin shim that parses args, runs the command, emits lines, and exits appropriately.
- [ ] Keep public API narrow: expose only the command-runner entry point needed by the binary and tests.

Run:

```sh
just dev cargo test -p yach-cli -- --nocapture
```

Expected: existing CLI tests run against library code and binary behavior is unchanged.

### Task 6.2: Adopt Clap

- [ ] Add `clap` through workspace dependencies if Milestone 7 has already centralized dependencies; otherwise add it only to `yach-cli`.
- [ ] Replace hand parsing with derive-based command definitions.
- [ ] Preserve existing command names and flags.
- [ ] Ensure unknown commands exit with misuse status and usage text.
- [ ] Add tests for `--help`, `--version`, typo suggestions, `tui --backend`, and extension subcommands.

Run:

```sh
just dev cargo test -p yach-cli -- --nocapture
just run --help
```

Expected: help lists supported commands, typo handling is explicit, smoke command behavior remains compatible.

## Milestone 7: Workspace Dependency And Toolchain Hygiene

**Purpose:** Reduce dependency drift without blocking correctness work.

**Files:**

- Modify: `Cargo.toml`
- Modify: crate `Cargo.toml` files
- Create: `rust-toolchain.toml` or pin Rust through `devenv.nix`

### Task 7.1: Centralize Repeated Dependencies

- [ ] Move common dependency versions into `[workspace.dependencies]`.
- [ ] Update crate manifests to use `.workspace = true` for repeated dependencies.
- [ ] Keep path dependencies explicit where that is clearer.

Run:

```sh
just dev cargo metadata --no-deps
just test
```

Expected: metadata resolves and all tests pass.

### Task 7.2: Pin The Rust Toolchain

- [ ] Choose either `rust-toolchain.toml` or a `devenv.nix` pin as the canonical toolchain pin.
- [ ] Prefer matching local/devenv/CI behavior over adding a second source of truth.
- [ ] Document the pin in README or the dev environment file.

Run:

```sh
just fmt
just lint
just test
```

Expected: CI and local dev shell use the same Rust toolchain family.

## Milestone 8: Typed Tool Result Schemas

**Purpose:** Replace silent `serde_json::Value` key-poking for provider-visible tool result summaries.

**Files:**

- Modify: `crates/yach-backend/src/native_runner.rs`
- Consider create: `crates/yach-backend/src/tool_result_schema.rs`
- Test: native provider tool result summary tests

### Task 8.1: Define Shared Result Types

- [ ] Add typed structs/enums for built-in provider-visible tool result payloads used in summaries.
- [ ] Use the same types at executor result creation and summary parsing where feasible.
- [ ] Convert malformed result payload handling from silent `None` to bounded diagnostic text or logged parse warning.

Run:

```sh
just dev cargo test -p yach-backend native_provider_content_result_count_summary -- --nocapture
```

Expected: schema drift fails in tests or produces an explicit diagnostic, not silent loss of summary.

## Deferred Until High Findings Are Closed

- Splitting `crates/yach-ui/src/app.rs`.
- Deduplicating `centered_rect` helpers.
- `.gitignore`-aware search.
- Log encryption.
- Unbounded channel replacement.
- Broad path-validation consolidation.
- Deleting or implementing `yach-adapter-pi-sdk`.
- Structured tracing facade.
- More extension packaging UX.

These can be reconsidered after CI is green, session reliability is fixed, and `native_runner.rs` has begun shrinking.

## Validation Before Returning To Dogfood

Run the core verification set:

```sh
just fmt
just lint
just test
just dev cargo run -p yach-bench -- native-session-profile-report
```

Then rerun the native MVP dogfood checkpoint in `docs/project/records/2026-06-03-native-mvp-dogfood-checkpoint.md`, especially explicit resume and recoverable failure visibility.

## Open Decisions

- CI target: GitHub Actions is assumed because repo history uses PR-style references. Confirm before adding macOS runners.
- Session durability: default plan is strict sync per append batch. If this breaks latency targets, design a bounded-loss batching policy with an explicit maximum loss window.
- Multi-instance sessions: default plan documents single-writer behavior. If two `yach` processes on one project are in scope, add file locking to Milestone 2.
- Extension threat model: default plan treats extensions as untrusted enough to hide inherited secrets, but not sandboxed. Real sandboxing needs a separate design.
- Pi adapter future: default plan keeps minimal hygiene because Pi is a reference backend. Deletion or deeper investment needs owner direction.
