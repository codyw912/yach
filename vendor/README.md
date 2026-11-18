# Carrying the Rig vendor

`rig-core/` contains published `rig-core 0.41.0` plus yach's local patches,
packaged as `yach-rig-core`. The root `[patch.crates-io]` selects it for
workspace builds (`Cargo.toml:46-47`), and `crates/yach-backend/Cargo.toml:20`
consumes the published crate as `rig`. Published 0.46 supplies
Responses replay replacements but still lacks required auth fencing, catalog,
transport safety and gateway behavior. See the
[0.46 reconciliation and release decision](../docs/project/records/2026-11-17-rig-0-46-reconciliation.md).

## Patch inventory

Paths in the table are relative to `vendor/rig-core/`. Rows overlap where a
file contains more than one family. Together they cover all 38 changed files,
+4,481/-1,071 against published 0.41.0, with the two exclusions below. Status
means published 0.46.0, not unreleased upstream main. Source-level capability
references and migration cautions are in the reconciliation record.

| Family | Files | Purpose | Upstream status/ref |
| --- | --- | --- | --- |
| Auth safety/lifecycle | `src/auth.rs`, `src/lib.rs`, `src/providers/internal/auth.rs`, `src/providers/chatgpt/auth/{mod,native,wasm}.rs`, `src/providers/chatgpt/mod.rs`, `src/completion/request.rs`, `Cargo.toml`, `Cargo.lock` | Public typed auth, expected account, metadata fencing, safe files/private writes, short cross-process lock, refresh/login deadlines, redacted DTOs and test-host injection. Manifest adds libc/fs2; completion errors retain typed auth. | Partly. Public stores, private writes/locks and refresh exist via [#2815](https://github.com/0xPlaygrounds/rig/pull/2815). Expected-account/entry fencing and typed outcomes remain missing. [#3284](https://github.com/0xPlaygrounds/rig/pull/3284) hardens credential reads after 0.46, not lock-file links. |
| HTTP/SSE errors | `src/http_client/{mod,retry,sse}.rs`, `src/provider_response.rs`, `src/json_utils.rs`, `CHANGELOG.md` | 16 KiB discard-on-overflow error capture, bounded/redacted Retry-After, status preservation, redacted formatting, borrowed in-band error scan, 8 MiB SSE cap/nonretryable overflow and opt-in `text/plain`. Changelog documents Retry-After. | Partly. [#2333](https://github.com/0xPlaygrounds/rig/pull/2333) exposes full headers; bundled reqwest keeps a 64 KiB prefix. Event cap, full redaction and exact capture policy remain missing; `text/plain` still fails. |
| Provider error handling | `src/providers/anthropic/{completion,streaming}.rs`, `src/providers/internal/openai_chat_completions_compatible.rs`, `src/providers/openai/completion/{mod,streaming}.rs`, `src/providers/openai/responses_api/{mod,streaming}.rs`, `src/providers/chatgpt/mod.rs` | Use bounded non-success capture, bound in-band errors, remove raw provider/stream diagnostics and make Responses request TRACE metadata-only. | Partly. [#2495](https://github.com/0xPlaygrounds/rig/pull/2495) centralizes errors, but in-band allocation/retention and raw TRACE/body confidentiality still differ. |
| Error-shape plumbing/tests | `src/audio_generation.rs`, `src/client/{mod,verify}.rs`, `src/completion/request.rs`, `src/embeddings/embedding.rs`, `src/image_generation.rs`, `src/providers/{deepseek,xiaomimimo}.rs`, `src/test_utils/http.rs`, `src/transcription.rs` | Follow the HTTP error tuple's Retry-After field in matches/constructors/tests; fixtures also support ETag/304. These are not separate provider features. | Replacement error API in rig-core 0.46.0 `src/error.rs:313-369,514-556`; migrate callers and tests rather than preserve tuple shape. |
| Responses/replay | `src/providers/openai/responses_api/{mod,streaming}.rs`, `src/providers/copilot/mod.rs` | Opaque native input, public native request send/stream, complete ordered terminal output, strict terminal validation and tests. | Required input/output/request capabilities fixed via [#2713](https://github.com/0xPlaygrounds/rig/pull/2713) and [#2888](https://github.com/0xPlaygrounds/rig/pull/2888). Strict missing-output rejection differs from upstream backfill. |
| Model discovery | `src/providers/chatgpt/{mod,model_listing}.rs`, `src/providers/openai/model_listing.rs`, `src/test_utils/http.rs` | OAuth/account-aware Codex models, visibility/data schemas, display names, caller `client_version`, raw catalog ETag/304; optional OpenAI `created`/`owned_by`. | OpenAI optional metadata fixed, rig-core 0.46.0 `src/providers/openai/wire/modality.rs:1294-1310`. Codex missing in 0.46; [#2973](https://github.com/0xPlaygrounds/rig/pull/2973) adds basic discovery/version after release, not the raw ETag/304 document API. |
| Anthropic gateways | `src/providers/anthropic/{client,model_listing}.rs` | Exactly one API-key or Bearer auth header; absent/null `has_more` means last page. | Bearer choice fixed via [#2889](https://github.com/0xPlaygrounds/rig/pull/2889). Explicit null still fails in rig-core 0.46.0 `src/providers/anthropic/modality.rs:101-110`; [#2334](https://github.com/0xPlaygrounds/rig/pull/2334) is a cursor fix only. |
| Packaging | `.cargo-ok`, `Cargo.lock` | Cargo cache marker and dependency resolution. Lockfile +472/-673 includes fs2 and broad resolution churn, not that much functional patch code. | Local packaging metadata; `.cargo-ok` is included in the 38-file count, not a capability to upstream. |

## Provenance rule

Every new patch hunk must carry a `// yach patch: <purpose>` comment and have a
row in this table. For non-Rust files, use the file format's comment syntax for
the same `yach patch: <purpose>` attribution; describe non-commentable metadata
in its table row. Keep the upstream license and attribution intact.

Today only two hunks have explicit patch markers,
`vendor/rig-core/src/providers/anthropic/client.rs:45-47` and
`vendor/rig-core/src/providers/anthropic/model_listing.rs:13-14`.
`vendor/rig-core/src/auth.rs:25` mentions the yach mapping layer, but is not a
patch marker. The rest of the inventory comes from a diff against the
published crate, not from marker search.

Run this exact comparison from the repository root. It downloads the baseline
into a temporary directory and writes the full diff there:

```sh
work=$(mktemp -d)
curl --fail --location \
  https://static.crates.io/crates/rig-core/rig-core-0.41.0.crate \
  --output "$work/rig-core-0.41.0.crate"
tar -xzf "$work/rig-core-0.41.0.crate" -C "$work"
diff -ruN --exclude Cargo.toml.orig --exclude .cargo_vcs_info.json \
  "$work/rig-core-0.41.0" vendor/rig-core \
  > "$work/vendor-vs-0.41.diff"
```

`diff` exits 1 when patches exist. Inspect `"$work/vendor-vs-0.41.diff"` before
removing the temporary directory. The audited baseline archive's SHA-256 is
`35f5520515ae8f6851adcbc6fde9eea8e96f657418c062e16c82cd81cce44e8e`.

## Re-check and release process

At each new rig-core minor release, and before every yach release, repeat the
registry-only compile probe described in the reconciliation record. Recompare
the patches with the published sources, then update this table and the record.
Separate renamed APIs from absent behavior; a name-resolution failure count is
only a lower bound. Do not equate unreleased merged PRs with a published crate.

The vendor is published as `yach-rig-core`, a yach-owned MIT crate that keeps
the upstream copyright, license and attribution
(`vendor/rig-core/LICENSE:1-7`). `yach-backend` consumes it through Cargo's
package rename, keeping the dependency name `rig`, its features and current
Rust callers unchanged. The checked-in upstream provenance files
(`Cargo.toml.orig`, `.cargo_vcs_info.json`, `.cargo-ok`) are listed in
`exclude` so Cargo does not reject them as reserved names; the published crate
carries Cargo's own regenerated `Cargo.toml.orig` and `.cargo_vcs_info.json`
instead. That is why the release check compares sources through
`Cargo.toml.orig` and skips `.cargo_vcs_info.json`.

Any change under `vendor/rig-core/` ships in this order:

1. Bump `version` in `vendor/rig-core/Cargo.toml` and the `yach-rig-core`
   requirement in `crates/yach-backend/Cargo.toml`, then update `Cargo.lock`.
2. Merge the change, then publish from merged `main`:
   `just dev cargo publish --manifest-path vendor/rig-core/Cargo.toml`.
3. Release yach. `just release-check` runs `scripts/vendor-rig-check.sh`,
   which packages `vendor/rig-core` and compares it with the published crate
   for the required version. A vendor edit that was not published, or a
   version that is not on crates.io, blocks the release.

A path patch does not reach downstream registry consumers
(`README.md:557-565`), which is why the published crate must equal the vendor.
After 0.2.0, upstream the small general fixes, move auth fencing to yach's
`TokenStore` and capture/error policy to its `HttpClientExt`, then migrate to
registry Rig with the remaining decoder and TRACE safety gaps resolved. The
reconciliation record gives the order and verification requirements.
