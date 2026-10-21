# Scripted provider failure

**Entry point:** `yach run --prompt "..."`, `yach sessions show latest --json`; `scripts/drive-provider-failure`.

**Preconditions:** scripted provider. Round 1 is `[Started, Failed]` with the error kind under test; round 2 is a text reply that must never be used. No TTY needed.

**Steps:**

1. `failed_round rate_limited`, then `yach run --prompt "provider failure rate_limited"`. → Exit 1. Outcome JSON `outcome: "failed"`, `response: ""`.
2. `yach sessions show latest --json`. → One turn, `outcome: "failed"`, exactly one `provider_attempt` item: `outcome: "failed"`, `error_kind: "rate_limited"`, `retry_index: 0`.
3. Fresh run with `failed_round authentication`. → The same shape with `error_kind: "authentication"`.

**Gotchas:**

- A `Failed` event inside a provider round is terminal: the attempt layer retries request errors, not provider-emitted failures (`docs/project/specs/2026-08-28-provider-attempt-reliability-design.md:77-81`). The queued success round proves the turn was not retried into success. It does not exercise the live retry ladder (1 s, then 2 s, at most three attempts).
- The inspector nests the session's `ProviderAttemptFinished` event as `items[] | select(.kind == "provider_attempt") | .attempt`. There is no literal `ProviderAttemptFinished` key in its JSON.
