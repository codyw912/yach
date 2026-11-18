# Rig 0.46 reconciliation

Date: 2026-11-17

## Question

Can published `rig-core 0.46.0` replace yach's patched 0.41.0 without losing
required behavior, and what should 0.2.0 publish against?

## Result

Status: decided 2026-11-17. The owner confirmed the published-crate step and
the crate name.

Ship 0.2.0 against `yach-rig-core` 0.1.0, a yach-owned published crate
containing the current vendor, `rig-core 0.41.0` plus yach's patches. Keep the
MIT license, upstream copyright and attribution. Consume that crate through
Cargo's package rename, retaining `rig` as the dependency name so no Rust
caller changes are needed. Migrate to registry Rig after 0.2.0.

Published 0.46 resolves the old Responses capability blockers, but not yach's
expected-account checks, auth-entry fencing, Codex catalog document API,
8 MiB SSE cap, complete bounded/redacted error policy or explicit-null Anthropic
pagination. The registry-only compile probe stops at 47 name-resolution errors.
This requires an adapter rewrite, not a dependency-version edit.

The inventory is **38 changed files, +4,481/-1,071** against published 0.41.0,
excluding `Cargo.toml.orig` and `.cargo_vcs_info.json`. This includes tests,
documentation, blank lines and packaging metadata. The lockfile alone accounts
for +472/-673; it is not authored functionality. See the file inventory and
reproducible diff in [the vendor guide](../../../vendor/README.md).

## Patch families vs 0.46

Statuses describe published behavior, not source compatibility. `Fixed` often
requires a different API. Upstreamability is a recommendation, not an accepted
upstream commitment. Source references below use the named published crate's
`file:line`; repository references include the full repository-relative path.

| Family | Capability | 0.46 status | Upstream evidence | Upstreamable? |
| --- | --- | --- | --- | --- |
| Auth | Public auth, refresh reuse/rotation, atomic private writes and cross-process locking | Fixed, replacement store API; lock wait is 120 s rather than yach's 150 ms | rig-core 0.46.0 `src/providers/chatgpt/auth/mod.rs:104-157`, `src/providers/chatgpt/auth/native.rs:99-163`, `src/providers/internal/auth/store.rs:200-292`; [#2815](https://github.com/0xPlaygrounds/rig/pull/2815) | No duplicate implementation; keep short busy UX in yach |
| Auth | Expected-account check and metadata-fenced probe/adopt/delete/login | Missing; resolving the current account and unconditional save/delete are not equivalent | rig-core 0.46.0 `src/providers/chatgpt/auth/mod.rs:139-162`, `src/providers/internal/auth/store.rs:179-198`; vendor contract at `vendor/rig-core/src/auth.rs:47-86`, `vendor/rig-core/src/providers/chatgpt/auth/native.rs:401-417,660-749` | General account-mismatch error; yach owns entry fencing in its `TokenStore` |
| Auth | Refuse unsafe credential and lock files, narrow existing modes, revalidate paths | Missing in 0.46; later credential-read fix still leaves lock-file links open | rig-core 0.46.0 `src/providers/internal/auth/store.rs:169-177,201-212`; [#3284](https://github.com/0xPlaygrounds/rig/pull/3284) | Reuse merged credential fix; upstream lock-file symlink refusal |
| Auth | Typed sign-in/account failures, full Debug redaction and per-request device-flow deadlines | Partly; auth errors lose yach's classifications, some DTOs expose strings in Debug, loop deadline does not stop a stalled request | rig-core 0.46.0 `src/providers/internal/auth.rs:15-21,67-98`, `src/client/signed_in.rs:102-114`, `src/providers/chatgpt/auth/native.rs:36-65,303-384`; [#2546](https://github.com/0xPlaygrounds/rig/pull/2546) covers only part of redaction | General typed errors, redaction and deadlines; repair UX stays in yach |
| Transport | Recover status, body and Retry-After | Fixed access, different API; full headers remain available | rig-core 0.46.0 `src/error.rs:514-556`; rig-http 0.46.0 `src/http_client/mod.rs:32-43,108-135`; [#2333](https://github.com/0xPlaygrounds/rig/pull/2333) | Adapt accessors, not another metadata API |
| Transport | 16 KiB discard-on-overflow capture, bounded/redacted Retry-After and body formatting | Partly; bundled reqwest keeps a 64 KiB prefix plus marker and partial read-failure data; full headers/body can appear in diagnostics | rig-reqwest 0.46.0 `src/lib.rs:196-256`; rig-http 0.46.0 `src/http_client/mod.rs:26-43`; rig-core 0.46.0 `src/provider_response.rs:14-24,172-187` | General safe formatting; yach owns its 16 KiB/NUL sentinel and 128-byte single-header policy |
| Transport | Bound in-band error parsing/retention and omit raw stream/request diagnostics | Partly; Responses parses a full JSON value and retains the error string; request TRACE uses pretty JSON | rig-core 0.46.0 `src/providers/openai/responses_api/streaming.rs:100-141,1002-1004`, `src/providers/openai/responses_api/wire.rs:146-150` | General bounds/redaction; encoder TRACE requires a separate upstream decision |
| Transport | 8 MiB per-SSE-event accumulation limit, fatal non-retried overflow | Missing; raw and joined event buffers have no byte cap | rig-http 0.46.0 `src/http_client/framing.rs:208-233,284-336,389-396`; rig-core 0.46.0 `src/driver/http_transport.rs:231-253` | Yes, configurable event byte cap and nonretryable overflow |
| Transport | Accept Codex SSE labelled `text/plain; charset=utf-8` | Partly; absent content type is accepted, present `text/plain` is rejected | rig-core 0.46.0 `src/providers/openai/responses_api/wire.rs:161-166`, `src/driver/http_transport.rs:391-414` | Yes, dialect-specific accepted MIME type |
| Responses | Opaque caller-held input, phase, compaction history, full terminal raw output and native request settings | Fixed through replacement options/raw params and stream APIs; output can be reconstructed from item events | rig-core 0.46.0 `src/providers/openai/extension/responses.rs:14-19,60-65`, `src/providers/openai/responses_api/mod.rs:235-274,508-521,555-565`, `src/providers/openai/responses_api/streaming/document.rs:12-32,61-89,112-133`; [#2713](https://github.com/0xPlaygrounds/rig/pull/2713), [#2888](https://github.com/0xPlaygrounds/rig/pull/2888) | No passthrough patch needed; migrate the adapter |
| Responses | Reject malformed terminal output instead of treating it as a delta | Partly; explicit terminal dispatch fixes fallback, but missing/non-array output is accepted and backfilled | rig-core 0.46.0 `src/providers/openai/responses_api/streaming.rs:790-804,971-994`, `src/providers/openai/responses_api/streaming/document.rs:118-132` | Strict rejection is a yach policy choice, not an upstream regression claim |
| Discovery | OAuth Codex account models, caller-selected `client_version`, raw document with ETag/304 | Missing in 0.46; later basic discovery/version support does not supply the raw-document contract | rig-core 0.46.0 `src/providers/chatgpt/mod.rs:50-84`, `src/providers/openai/wire/modality.rs:1360-1395`; [#2973](https://github.com/0xPlaygrounds/rig/pull/2973); vendor API at `vendor/rig-core/src/providers/chatgpt/model_listing.rs:42-49,154-222` | Reuse basic discovery; yach owns dynamic version/cache policy, with a raw HTTP adapter if needed |
| Discovery | OpenAI-compatible entries without `created`/`owned_by` | Fixed | rig-core 0.46.0 `src/providers/openai/wire/modality.rs:1294-1310`, `CHANGELOG.md:405` | No patch needed |
| Anthropic | Exactly one API-key or Bearer auth header, including listing | Fixed via public dialect auth configuration | rig-core 0.46.0 `src/providers/anthropic/wire.rs:87-98,329-339,458-470`, `src/providers/anthropic/modality.rs:93-96`; [#2889](https://github.com/0xPlaygrounds/rig/pull/2889) | Migrate configuration; this is not full subscription OAuth |
| Anthropic | Missing or explicit-null `has_more` means last page | Partly; absent defaults false, explicit null fails boolean decoding | rig-core 0.46.0 `src/providers/anthropic/modality.rs:101-110`; [#2334](https://github.com/0xPlaygrounds/rig/pull/2334) fixes cursors, not null | Yes, test absent/null/true/false |

[#2973](https://github.com/0xPlaygrounds/rig/pull/2973) merged November 1 and
[#3284](https://github.com/0xPlaygrounds/rig/pull/3284) merged November 13.
Neither is in published 0.46. Source inspection establishes these statuses;
no live ChatGPT or Anthropic journey was verified.

## Compile probe

An isolated workspace based on `main@origin` removed the root patch and vendor
exclusion (`Cargo.toml:14,46-47`), then changed only the backend Rig dependency
(`crates/yach-backend/Cargo.toml:20`) to registry 0.46.0. Both `reqwest` and
`rustls` stayed enabled. The recorded probe used Rust 1.98.1 and:

```sh
set -o pipefail
export CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1
direnv allow . && direnv exec . cargo check --workspace --all-targets --message-format short 2>&1 | tee check.log
```

The requested check reported **47 diagnostics**, all in `yach-backend`'s
library: 11 E0432, 28 E0433 and 8 E0425. File totals were `rig_adapter.rs` 17,
`model_discovery.rs` 17, `chatgpt_auth.rs` 6, `recording_http.rs` 3,
`responses_replay.rs` 2 and `error_dialect.rs` 2. The environment's automatic
Clippy entry hook repeated those errors; it is not a second set to count.

- **MIGRATION:** provider clients/models and request construction, `OneOrMany`
  to vectors, usage fields, streaming items/tool deltas, Responses options,
  listing methods, error accessors and Anthropic Bearer configuration. Examples
  are rig-core 0.46.0 `src/driver.rs:396-431`,
  `src/streaming/event.rs:81-124,150-159`,
  `src/completion/request.rs:310-348,680-727` and
  `src/providers/openai/extension/responses.rs:13-19`.
- **MISSING:** `AuthFileGuard`, `AuthEntryToken`, `ExpectedAuthEntry`, Codex
  catalog-document APIs, `BoundedErrorBody`, `ERROR_BODY_MAX_BYTES` and the
  bounded/redacted Retry-After helper. Their vendor contracts are at
  `vendor/rig-core/src/providers/chatgpt/auth/native.rs:40-97`,
  `vendor/rig-core/src/auth.rs:47-86`,
  `vendor/rig-core/src/providers/chatgpt/model_listing.rs:42-55,151-175` and
  `vendor/rig-core/src/http_client/mod.rs:14-80,115-150`.

The 47 errors are a **lower bound**, not 47 independent missing capabilities
or the complete rewrite cost. Name resolution stopped later type checking and
dependent CLI/benchmark checks. No second adaptation/check round, runtime
provider test or saved-window replay was reached. A rewritten adapter still
needs ordering and terminal-output verification. Typed Responses input prepends
to canonical messages; using it with already-replayed history would duplicate
input. Raw replacement is available, but upstream also applies request rewrites
and terminal backfill rather than promising byte-for-byte passthrough.

## Persistence

No session converter or new artifact version is required for Rig's generic
serde changes. Sessions store yach `SessionEvent` strings/tool fields, not Rig
`Message` or `AssistantContent` (`crates/yach-backend/src/session.rs:79-140,373-435,621-635`;
`crates/yach-backend/src/session_store.rs:66-73,104-115,148-163`). Native compaction
stores provider JSON in yach's existing version-1 envelope
(`crates/yach-backend/src/compaction.rs:816-827,862-873,966-985`;
`crates/yach-backend/src/runner.rs:7420-7472`). Generic `AssistantContent` did
change from untagged to tagged serde, but it is not that durable schema
(rig-core 0.41.0 `src/completion/message.rs:57-69`; 0.46.0
`src/completion/message.rs:209-226`).

Rig directly persists the OAuth record. Both versions have the same five
optional fields: `access_token`, `refresh_token`, `id_token`, `expires_at` and
`account_id` (`vendor/rig-core/src/providers/chatgpt/auth/native.rs:113-120`;
rig-core 0.46.0 `src/providers/chatgpt/auth/native.rs:36-43`). No JSON converter
is needed, but **the filename must be preserved or deliberately migrated**.
0.46 `FileTokenStore` uses `<dir>/chatgpt.json`; yach uses
`chatgpt-subscription.json` and records the managed path in connection metadata
(rig-core 0.46.0 `src/providers/chatgpt/auth/mod.rs:22-24`,
`src/providers/internal/auth/store.rs:149-180,290-313`;
`crates/yach-connections/src/lib.rs:261-282,1021-1025,1095-1103`). A naive store
swap would look in the wrong file and appear signed out. Preserve the path in
yach's `TokenStore` rather than silently moving credentials.

Optional request recordings store wire bytes, not a Rig serde session schema
(`crates/yach-backend/src/recording_http.rs:134-160,209-225,260-273`). Catalog
caches are yach-owned projections (`crates/yach-cli/src/catalog_refresh.rs:54-85,178-200`).
OAuth compatibility is source-established, not a live refresh test; persisted
compact-window replay against 0.46 remains unverified.

## Decision and rejected alternatives

The 0.2.0 release path is `yach-rig-core` 0.1.0, the published yach-owned
crate. Preserve the current behavior and upstream MIT notice
(`vendor/rig-core/Cargo.toml:12-25`; `vendor/rig-core/LICENSE:1-7`). Keep the
release guard intact. This record changes no dependency or release recipe.

- Reject migration now. The planning assessment is weeks of adapter and safety
  work, not a measured duration; making it a prerequisite would hold 0.2.0.
- Reject keeping the path vendor and publishing anyway. `just release-check`
  refuses the vendor resolution by design (`justfile:485-490`). A workspace
  path patch is not shipped to downstream registry consumers; publication
  resolves the registry dependency instead (`README.md:557-563`).

## Migration order after 0.2.0

1. Upstream the small general fixes first: SSE event byte cap, dialect-specific
   `text/plain` Codex SSE, nullable `has_more`, typed sign-in-required and
   account-mismatch errors, and lock-file symlink refusal. Build on #3284 for
   credential reads and #2973 for basic catalog discovery, not duplicate PRs.
2. Move yach's expected-account enforcement, auth-entry probe/adopt/repair
   fencing, configured filename and short busy policy into a yach `TokenStore`.
   Move bounded/redacted capture, Retry-After projection and classifier policy
   into its `HttpClientExt`. These extension points exist in rig-core 0.46.0
   `src/providers/internal/auth/store.rs:30-76` and
   `src/providers/chatgpt/auth/mod.rs:104-157`; their sufficiency for all yach
   journeys has not been verified. A transport wrapper alone cannot suppress
   encoder TRACE or bound in-band decoder errors, so resolve those upstream
   confidentiality/bounding gaps before removing the corresponding patches.
3. Rewrite the adapter and catalog integration. Keep dynamic Codex-version
   selection, cache/ETag document policy and any stricter terminal validation
   in yach. Verify replay ordering, output reconstruction, auth races and error
   confidentiality before switching to registry Rig. Do not convert sessions
   or bump the native artifact version for the generic Rig serde change.

## Supersedes

This updates the current conclusions of
[the August reconciliation](2026-08-24-rig-upstream-reconciliation.md), not its
historical 0.42 probe:

- [#2269](https://github.com/0xPlaygrounds/rig/issues/2269) is closed via
  [#2888](https://github.com/0xPlaygrounds/rig/pull/2888), merged October 24.
  Caller-held input, opaque output, streamed phase and terminal raw response
  support are no longer missing capability blockers. Old API names are still
  gone; the 41-error 0.42 result is not the current compile result.
- Public auth and ordinary private/locked persistence now exist. Stronger
  expected-account and entry-fencing contracts remain missing.
- Basic Codex model discovery now has upstream work in #2973, but it is after
  0.46 and does not replace ETag/304 raw catalog handling.
- Upstreaming Responses passthrough is no longer the first release task. The
  yach-owned crate is the selected 0.2.0 path rather than merely a fallback.
  The instruction not to relax the release guard still holds.
