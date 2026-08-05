# Provider API-Key Connections Measurement

Date: 2026-08-04

## Scope

Measured the first provider-connections slice from the accepted design and plan:

- TUI-first `/connect` for named Anthropic, OpenAI, and OpenAI-compatible API-key connections;
- system credential-store secrets with durable, secret-free JSON metadata;
- environment-provider compatibility;
- bounded connection-aware discovery and exact connection/model activation;
- restart durability and the real Ratatui path.

ChatGPT subscription/OAuth lifecycle and role-based routing remain later slices. The 125-cell provider sweep remains a pre-release gate under the 2026-08-03 owner ruling; it is not a missing slice result.

## Final Code Gates

All final gates ran from the formatted working stack after review fixes:

```text
just fmt
just fmt-check
just lint
just check
just test
```

Results:

- formatter and formatter check: pass;
- Clippy across all targets/features with `-D warnings`: pass;
- workspace check: pass;
- workspace tests: pass, 882 unit tests total and 0 failures:
  - `yach`: 113;
  - `yach-backend`: 534;
  - `yach-bench`: 13 library + 8 binary;
  - `yach-catalog`: 35;
  - `yach-connections`: 31;
  - `yach-proto`: 22;
  - `yach-ui`: 126;
  - doc tests: 0.

The first full gate exposed one stale stored-only activation test ordering race and strict-lint findings. The test now synchronizes cached publication and activation start instead of increasing its timeout; the exact regression passes.

## Contract Regressions

```text
just dev cargo test -p yach-backend provider_connections::tests::provider_connection_flow_retries_persisted_create_as_repair_with_the_same_id -- --exact
just dev cargo test -p yach-backend provider_connections::tests::provider_connection_flow_retries_unpersisted_create_as_create -- --exact
just dev cargo test -p yach provider_connections::tests::persisted_create_failure_retries_by_repairing_the_same_pending_connection -- --exact
just dev cargo test -p yach-backend runner::tests::stored_only_runtime_catalog_activates_first_provider -- --exact
just dev cargo test -p yach tests::provider_connections_survive_restart_and_complete_a_real_provider_turn -- --exact
just dev cargo test -p yach provider_connections::tests::provider_connection_switch_a_b_a_restores_complete_config -- --exact
```

Each command ran exactly one test and passed. The restart test completed in 2.49 s; the A -> B -> A test completed in 1.17 s.

Final review found and fixed a post-persistence create retry defect: once pending metadata exists, credential/ready failure now carries the durable connection ID through storage, CLI runtime, and reducer state, so retry repairs that row rather than allocating a duplicate. Pre-persistence failures still retry as fresh creates. Final bounded whole-stack re-review: READY.

## Behavioral Gate

```text
just eval-validate
```

All seven evaluator oracles passed:

- compaction-continuation;
- multi-round-sequence;
- notes-explore;
- notes-tally-fix;
- session-continuation;
- tool-call-economy;
- tool-result-dependence.

## Startup Profile

```text
just dev cargo run -p yach-bench -- yach-tui-startup-profile-report --samples 10
```

Collected 10/10 samples. Final profile:

- process-to-first-render PTY: p50 41.532 ms, p95/p99/max 81.367 ms;
- first-render start since main: p50 2.256 ms, p95/p99/max 23.464 ms;
- first-render end since main: p50 3.175 ms, p95/p99/max 25.324 ms.

The traced first-render path performs no registry, keyring, or provider request. Runtime construction stores dependencies only. `/connect` begins metadata and credential-status work after the command opens; `/model` emits the completed cache and starts bounded discovery after the picker opens.

## Live Ratatui Smoke

```text
just dev cargo run -q -p yach -- tui-provider-connection-smoke
```

The final supervised PTY run exited 0 after exercising the production Ratatui and backend path:

1. open `/connect` and add an OpenAI-compatible connection;
2. enter label, loopback fixture URL, and a Unicode sentinel credential;
3. observe mask glyphs only and successful creation without active-model change;
4. open `/model` and select the exact connection-aware row rather than the legacy fixture row;
5. send a prompt and receive the streamed completion;
6. reopen `/connect`, select Remove, confirm, and observe active-removal rejection;
7. quit cleanly.

Secret-free terminal predicates:

```text
fixture_models=true
fixture_prompt=true
prompt_finished=true
exact_activation_count=1
active_removal_rejected=true
```

The correct run's captured PTY output did not contain the sentinel credential. Provider authentication/model checks happened inside the fixture and only the boolean predicates were emitted after terminal teardown.

## Deferred Release Sweep

The 125-cell provider sweep remains deferred to the pre-release gate. Before that run, fix per-task-block credential re-resolution in the sweep driver so authorization expiry cannot erase the trailing block.
