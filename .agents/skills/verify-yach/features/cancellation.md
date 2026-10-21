# Scripted cancellation

**Entry point:** `yach run --prompt "..."`, `yach sessions show latest --json`; `scripts/drive-cancellation`.

**Preconditions:** scripted provider. Round 1 is `[Started, Cancelled]`; round 2 is a text reply that must never be used. No TTY needed.

**Steps:**

1. `yach run --prompt "cancelled turn"`. → Exit 1, `response: ""`.
2. `yach sessions show latest --json`. → One turn with `outcome: "cancelled"` and exactly one `provider_attempt` item with `outcome: "cancelled"` and no `error_kind`.

**Gotchas:**

- `yach run`'s outcome JSON has no cancelled value: a provider cancellation is reported as `outcome: "failed"` (`crates/yach-cli/src/headless.rs:931-934`). Assert cancellation on the session inspector, never on the run outcome.
- This is a provider-ended cancellation. A user cancel (Ctrl-C in the TUI, or `CancelTurn` over rpc) is a different path, and the RPC matrix covers it (`crates/yach-cli/tests/rpc_matrix.rs:910`).
