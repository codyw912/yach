# Hashline Extension Bundle Design

**Date:** 2026-08-21
**Status:** Implemented

## Motivation

Yach has an extension host, tool registration, provider catalog resolution, replacement policy, local-edit review, stdio RPC, and deterministic invariant matrices. Those seams have mostly been proven independently. The first extension-first dogfood slice should force them to compose.

The slice replaces the provider-facing `read_text_file` and `edit_text_file` tools as one coordinated bundle:

- `read_text_file` emits a whole-file content tag and addressable line gutter.
- `edit_text_file` accepts a compact line-anchored patch using that tag.
- the external extension process parses and prepares the patch;
- the Rust core remains the only filesystem and mutation authority;
- mutation still uses the existing preview, review, durable evidence, and commit path.

This is intentionally not a second editing subsystem. The extension owns provider ergonomics; the core owns policy and effects.

## Goals

1. Dogfood manifest discovery, activation, stdio transport, executable registration, schema-changing replacement, provider advertisement, resource brokerage, review, and result shaping in one first-party feature.
2. Replace read and edit atomically. A provider turn sees either both native tools or both hashline tools, never a mixed pair.
3. Keep project paths and sensitive-file policy under core authority. The extension host never receives the project root or direct project filesystem access.
4. Keep provider-supplied tags out of the trusted mutation boundary. The host resolves a valid patch to a full-SHA-256 edit proposal; the core independently checks the proposal against live files before preview and again before commit.
5. Use the same behavior through TUI, `yach rpc`, headless provider loops, persistence, resume, and compaction.
6. Establish deterministic lifecycle and collision scenarios that later contribution surfaces can reuse.

## Non-goals

- General extension hooks or a generic mutable lifecycle bus.
- OS sandboxing claims for extension processes.
- AST-aware block locators, clipboard registers, moves, deletes, or file creation in the first slice.
- Drift recovery or three-way merge. A stale tag fails closed and asks for a re-read.
- Replacing `search_project` or `list_project_paths`.
- Provider-visible extension management UI.
- Cross-process or cross-session snapshot persistence.

## Upstream OMP findings

OMP's current implementation informed the surface, but yach does not copy its internals blindly.

Reusable decisions:

- Provider edit arguments are one compact `input` string.
- Read output prefixes source lines as `N:TEXT` and emits `[path#TAG]` once per file.
- A session-bound snapshot store ties tags to exact normalized full-file text.
- Multi-section patches preflight every section before writes.
- Empty/no-op edits are rejected with actionable guidance.

Deliberate differences:

- OMP currently displays a 4-hex xxHash32 truncation. Yach uses the first 16 uppercase hex characters of SHA-256 and retains the full SHA-256 internally. The extra 12 characters are negligible beside file content and materially reduce accidental collisions.
- A tag is never accepted from hash equality alone. It must resolve to an unambiguous snapshot minted by this live extension host for the same canonical project-relative path, and that snapshot text must equal the core-brokered live text.
- The first slice has no stale-anchor recovery. Exact stale rejection is easier to reason about and keeps the first invariant matrix deterministic.
- The host cannot read the project filesystem directly. Every read is brokered and policy-checked by the core.

## Provider surface

### Hashline read

Provider name remains `read_text_file`.

The first slice intentionally retains the native input contract:

```json
{"path":"src/lib.rs"}
```

Successful non-empty output:

```text
[src/lib.rs#7A31C8D295A40B1E]
1:pub mod api;
2:
3:pub fn run() {}
```

Rules:

- Path syntax and sensitive-path behavior are identical to native `read_text_file` because the core resolves the request.
- Files are UTF-8 text and at most 32 KiB, matching the current native provider read limit.
- CRLF is normalized to LF for hashing and addressable lines. The final newline terminates the preceding line and does not create an extra addressable line.
- Empty files still emit a header and an explicit `(empty file)` notice.
- The host stores the exact normalized text, canonical project-relative path, full SHA-256, and displayed tag for the lifetime of the activated host session.

### Hashline edit

Provider name remains `edit_text_file`, but the replacement bundle deliberately supplies a new schema:

```json
{"input":"[src/lib.rs#7A31C8D295A40B1E]\nPUT 3.=3:\n+pub fn run() { work(); }"}
```

First-slice grammar:

```text
patch        := section+
section      := "[" path "#" tag "]" NEWLINE hunk+
tag          := 16 uppercase hex characters
hunk         := put-range | put-gap | cut-range
put-range    := "PUT " line ".=" line ":" NEWLINE body+
put-gap      := "PUT <" line ":" NEWLINE body+
             | "PUT >" line ":" NEWLINE body+
             | "PUT >$:" NEWLINE body+
cut-range    := "CUT " line ".=" line NEWLINE?
body         := "+" text NEWLINE?
line         := positive decimal integer
```

Semantics:

- `PUT N.=M:` replaces the inclusive original range `N..=M` with its body rows.
- `PUT <N:` inserts before original line `N`; `<1` inserts at file head.
- `PUT >N:` inserts after original line `N`; `>$` appends at file tail.
- `CUT N.=M` deletes the inclusive original range.
- Every locator in a section addresses the same pre-edit snapshot. Hunks may not overlap. Application order does not renumber later anchors.
- `+` is syntax, not content. A source line beginning with `+` is encoded as `++...`; a source line beginning with `-` is encoded as `+-...`.
- Each path may appear in one section only.
- Absolute paths, parent traversal, duplicate paths, zero lines, reversed ranges, out-of-bounds ranges, malformed headers, unknown tags, ambiguous tags, overlapping hunks, no-op sections, and empty patches fail before review.
- A patch may modify multiple existing files. Every section is validated before the host returns a proposal.

## Core-brokered resource protocol

The extension transport becomes a bounded bidirectional request protocol during `tool.invoke`.

New host-to-core frame:

```json
{
  "type":"resource.request",
  "request_id":"resource-1",
  "operation":{"kind":"read_text_file","path":"src/lib.rs","max_bytes":32768}
}
```

New core-to-host frame:

```json
{
  "type":"resource.result",
  "request_id":"resource-1",
  "result":{
    "status":"completed",
    "path":"src/lib.rs",
    "text":"pub mod api;\n",
    "sha256":"…"
  }
}
```

Failure uses `status: "failed"` plus a stable reason label and bounded message.

Protocol invariants:

- Core clamps `max_bytes` to the host capability ceiling; the extension cannot enlarge policy.
- Core uses `ResourceRoot` for canonicalization, root containment, symlink handling, UTF-8 validation, and sensitive-path denial.
- The host receives only the canonical project-relative path and requested text, never the absolute root.
- Resource requests are accepted only while handling the matching in-flight tool invocation.
- Duplicate request IDs, replies for unknown IDs, unsupported operations, oversized frames, and out-of-order terminal results fail the invocation and mark the host unhealthy.
- Resource traffic is internal protocol evidence. Provider-visible tool request/result evidence remains one logical tool call.

## Structured edit proposal

A mutating extension tool does not return arbitrary success text. It returns:

```json
{
  "type":"tool.edit_proposal",
  "request_id":"tool-request-1",
  "summary":"2 files, 3 hunks",
  "operations":[
    {
      "kind":"modify_text_file",
      "path":"src/lib.rs",
      "expected_sha256":"<64 lowercase hex>",
      "after_text":"<complete UTF-8 file>"
    }
  ]
}
```

Core handling:

1. Require the registered tool risk to be `mutates_local_state`.
2. Reject duplicate paths, unsupported operation kinds, malformed SHA-256, oversized files, and oversized aggregate proposals.
3. Convert each operation to the existing `EditTransactionRequest`; the provider never supplies this transaction directly.
4. Independently read every target through `ResourceRoot` and require the full SHA-256 to match.
5. Produce one core-owned multi-file diff preview and one review request.
6. Persist the generic review request before emitting it to TUI/RPC.
7. On approval, revalidate every path and hash, stage every replacement, and commit the transaction. On rejection or interruption, write nothing.
8. Shape the final provider result from the core apply result, not host-authored prose.

The host's displayed 16-hex tag is an ergonomic anchor. The proposal's full SHA-256 and the core's revalidation are the mutation boundary.

## Coordinated replacement bundles

The manifest gains a typed replacement contribution:

```toml
[[tool_replacement_bundles]]
id = "hashline"

[[tool_replacement_bundles.members]]
builtin = "read_text_file"
tool = "yach.hashline.read"
contract = "preserve"

[[tool_replacement_bundles.members]]
builtin = "edit_text_file"
tool = "yach.hashline.edit"
contract = "replace"
```

Rules:

- Bundle IDs and builtin names are unique within a manifest.
- `preserve` requires exact risk and input-schema equality.
- `replace` requires exact risk equality but uses the extension tool's description and input schema under the builtin provider name.
- Schema-changing replacement is valid only inside a declared bundle. Existing one-tool replacement rules remain schema-preserving.
- A bundle activates only when every member is registered, executable, policy-allowed, collision-free, and owned by the declaring extension version.
- Any member failure disables the whole bundle for that catalog snapshot. Native tools remain available and a diagnostic names the failed member.
- The provider catalog is pinned per turn. Activation, disable, removal, reload, or crash changes the next turn only.

The first-party hashline package is enabled as a bundled user-trusted source.
The installed binary materializes a versioned manifest and seeds a persisted
bundled install record, preserving user enable/disable state across launches
and keeping list/doctor diagnostics complete. Bundling special-cases executable
location so the installed `yach` artifact can launch its host, but manifest
parsing, activation, stdio RPC, tool registration, replacement resolution,
review, persistence, and provider execution use the same public contracts as
third-party extensions.

## Host lifecycle

- Discovery and activation remain post-first-paint.
- Before activation completes, both native tools are advertised.
- After both hashline tools register and the bundle resolves, the next provider turn advertises both replacements.
- Disable/removal prevents new turns from selecting the bundle. An in-flight turn retains its pinned catalog and host lease.
- Host crash fails the current tool call once. Yach does not silently replay a mutating provider call through the native tool. The next turn falls back to both native tools and records the host diagnostic.
- Restart creates a fresh snapshot store. Tags minted by the old host become unknown and fail closed.

## Durable review evidence

Review evidence must be contribution-neutral. Session records and RPC/TUI events retain:

- provider-visible tool name;
- resolved implementation name;
- extension ID and version;
- replacement bundle ID and replaced builtin;
- original sanitized argument content;
- proposal summary and full transaction hashes, subject to existing masking policy;
- preview ID, permission-decision ID, decision, interruption, resolution, and apply outcome.

No record type may encode `hashline` as a special review kind. Future mutating extensions reuse the same proposal and review path.

## Invariant matrix

The deterministic matrix must cover at least these scenarios:

| Scenario | Provider catalog | Host/effect outcome | Required evidence |
|---|---|---|---|
| Bundle absent | native read + native edit | no host | native provenance |
| Installed but disabled | native pair | no activation | disabled diagnostic |
| Malformed contribution | native pair | activation rejected | manifest reason |
| Host start/initialize failure | native pair | no executable tools | host reason |
| Only one member registers | native pair | no partial replacement | missing member |
| Both members ready | hashline pair | host leased | bundle + extension provenance |
| Member collision | native pair | fail closed | colliding owner/member |
| Disable/remove between turns | current turn pinned; next native pair | graceful lease drain | generation transition |
| Reload succeeds | current turn pinned; next hashline pair | new generation | old/new generation |
| Host crashes during read | current call fails; next native pair | no replay | crash + fallback transition |
| Sensitive read | catalog unchanged | broker denies; no content leaves core | sensitive-path reason |
| Fresh read then edit | hashline pair | one generic review, approved apply | request → decision → apply |
| User rejects edit | hashline pair | no write | durable rejection |
| Review interrupted | hashline pair | no write | durable interruption |
| Unknown/stale tag | hashline pair | no proposal and no review | stale-tag failure |
| One stale section in batch | hashline pair | no proposal and no writes | failed section |
| Multi-file approval | hashline pair | one preview; all files change | one transaction, per-file hashes |
| Apply-time drift | hashline pair | no writes | core hash mismatch |
| RPC and TUI | identical logical events | transport-specific rendering only | same IDs and outcomes |

## Implementation sequence

1. Add protocol frames for resource requests/results and edit proposals; keep v1 rejection deterministic and negotiate the new capability.
2. Generalize extension invocation to service bounded resource requests until one terminal tool result/proposal arrives.
3. Add generic extension edit-proposal normalization and route it through the existing local-edit preview/review state machine.
4. Lift the edit engine's one-operation apply restriction and implement staged multi-file modify commit with rollback on ordinary publish errors. Document that process death between filesystem renames is not a cross-file transactional guarantee.
5. Add manifest replacement-bundle contributions and catalog resolution with clean, turn-pinned all-or-none activation.
6. Add the first-party external hashline host and bundled package source.
7. Add deterministic unit/integration matrix coverage, then exercise the same read/edit/review flow through `yach rpc` and the actual TUI.

## Acceptance criteria

- A fresh default session initially has a usable native pair and switches only to a fully activated hashline pair.
- Provider read output contains a 16-hex whole-file tag and line-addressable body.
- A valid hashline edit creates the existing core preview, waits for one review decision, writes only after approval, and returns a core-shaped result.
- Rejection, interruption, unknown tag, stale tag, sensitive path, malformed patch, member collision, partial registration, and host crash write nothing.
- A valid multi-file patch produces one review and either commits every file or rolls back ordinary publish failures.
- Session evidence preserves sanitized arguments, replacement provenance, generic review lifecycle, and final outcome across resume/compaction.
- Equivalent RPC and TUI scenarios produce the same canonical session events.
- Focused tests, workspace format/lint/tests, and an actual TUI smoke path pass.
