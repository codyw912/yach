# File Credential Store Design

Date: 2026-08-05
Status: proposed

## Context

Provider API keys currently live in the OS credential manager through
`SystemCredentialStore` (keyring crate, service `yach`, account =
connection UUID). Owner testing (2026-08-05) found this unacceptable:

- Opening `/connect` produced 10+ macOS Keychain password prompts in one
  flow. The credential cache (#230) reduced this to one read per
  connection per launch, but even one prompt per launch is not
  acceptable UX.
- Root cause of prompting: the dev binary is ad-hoc signed, so its
  cdhash changes every build and the Keychain ACL treats each build as
  a new application.
- Cohort evidence (2026-08-05 research, source-verified): keychain-based
  harnesses ship this exact bug class — Claude Code #68195 (repeated
  dialogs despite "Always Allow"), goose #10549 (unsigned binary prompt
  hangs headless sessions; fixed via timeout + file fallback +
  `GOOSE_DISABLE_KEYRING`). Plain-file harnesses (opencode
  `auth.json`, pi `auth.json`, Crush `crush.json`, omp `agent.db`) have
  no prompt-fatigue reports. aider stores nothing centralized at all.

Owner decision 2026-08-05: migrate to a permissioned plaintext file
under `~/.yach/`. cachix/secretspec was evaluated and rejected: it is a
declaration-first dev-environment secrets orchestrator (static
`secretspec.toml`, CLI-driven `set`, launch-time injection), which does
not match runtime-created per-connection credentials, adds a framework
layer over our existing `CredentialStore` seam, and recommends the
system keyring we are leaving.

## Design

### Storage

One JSON document at `~/.yach/credentials.json`:

```json
{
  "schema": "yach.credentials.v1",
  "credentials": {
    "<connection-uuid>": { "api_key": "sk-..." }
  }
}
```

- Written atomically (temp file + rename), mirroring the registry.
- Permissions enforced at every write: parent directory `0700`, file
  `0600` (Unix). omp's WAL-at-0644 leak class does not apply to a
  single JSON document; there is no sidecar.
- Missing, malformed, or foreign-schema file reads as an empty store
  (same tolerance as the registry and active-model state file).
- Values stay inside `ProviderSecret` on read; the existing
  redaction/zeroization invariants are unchanged.

### Cutover

`FileCredentialStore` replaces `SystemCredentialStore` as the store
constructed by `CliProviderConnectionRuntime::system`. The keyring
implementation and the `keyring` dependency are removed (clean cutover;
git history preserves them). `CredentialStore` stays as the seam:
tests use in-memory stores, and a future backend (encrypted file,
external resolver) remains possible.

### Migration

On startup, when constructing the system runtime:

1. Load the registry; for each stored connection, check the file store.
2. For any connection missing from the file, attempt one legacy keyring
   read. On success: write the secret to the file store, then delete
   the keyring item.
3. Keyring reads happen at most once per connection ever, behind the
   existing credential cache, and only for unmigrated connections — so
   the migration cost is at most one prompt per stored connection on
   the first launch after upgrade, then never again.
4. Migration failures (denied prompt, absent item) are non-fatal: the
   connection surfaces as `PendingCredential` and can be repaired
   through `/connect`, exactly as a missing credential does today.

The legacy keyring code path needed for step 2 lives behind a small
`LegacyKeyringReader` used only by the migrator; when the last
connection migrates it becomes dead weight, but removing it is a later
cleanup, not part of this slice's behavior contract.

### Non-goals

- Encrypted-at-rest storage (Gemini-style host-derived key is
  obfuscation, not protection, at our threat model).
- Windows ACL hardening (Unix permissions only this slice).
- Headless/broker credential distribution (omp's auth-broker pattern;
  env-var configuration remains the headless path).
- Changing the ChatGPT-subscription token-dir flow.

## Testing

- File store: put/get/remove round-trip; permissions asserted (`0600`
  file, `0700` dir); malformed/missing/foreign-schema reads as empty;
  atomic write leaves no temp file.
- Migration: registry + legacy fixture secret migrates to the file and
  deletes the legacy item; denied/absent legacy read degrades to
  `PendingCredential` without failing startup; already-migrated
  connections are never re-read from the legacy store.
- Existing connection runtime suites run against the file store
  unchanged (trait conformance).

## Verification

- `just fmt`, `just lint`, `just test`.
- Live: create a connection via `/connect`, confirm
  `~/.yach/credentials.json` exists with `0600` and the key renders
  masked; relaunch; remembered selection activates without any keychain
  prompt.
