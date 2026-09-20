# Extension Capability Contract Design

**Outcome:** plane:YACH-10

Status: original contract landed in PR #276; follow-up design approved in
conversation 2026-09-17; written amendment awaiting review.

The 2026-09-17 amendment replaces session-bound grant evidence with an
atomic user-scoped authority/history record and specifies discovery,
CLI failure reporting, and protocol-documentation corrections. The original
implementation plan remains historical; its Task 7 evidence prescription is
superseded by this amendment. YACH-10 and `yach#wc3k` remain open until the
amended behavior is implemented, verified, and published.

## Motivation

Yach's roadmap says most functionality arrives as extensions while the kernel
keeps policy, state, evidence, and execution invariants. The code does not
currently allow that. An extension tool may declare only three risks —
`ReadsLocalMetadata`, `ReadsLocalContent`, `MutatesLocalState`
(`crates/yach-backend/src/extension.rs:80`) — and registration rejects
anything else (`crates/yach-backend/src/tools.rs:2088`).

So every capability the minimal core deliberately omits — web fetch, web
search, an MCP client, an LSP bridge, a background process manager — cannot
be built as an extension at all. The extension-first posture cannot be
demonstrated, let alone adopted by someone outside this codebase.

The vocabulary for it already exists. `ToolRisk` has carried `UsesNetwork`
and `RunsProcess` since before this change
(`crates/yach-backend/src/tools.rs:17`), and `bash` is a built-in tool of
risk `RunsProcess`. What is missing is a contract that lets an extension
declare those risks and a user grant them.

## What this contract is, and what it is not

It is a **declaration** the kernel records, displays, and gates activation
on.

It is **not enforcement**. The extension host is an ordinary subprocess
launched with the agent's own privileges
(`crates/yach-backend/src/extension.rs:399`). It can open a socket or spawn
a child whether or not it declared those capabilities. An extension that
uses the network without declaring it violates this contract, and **this
design neither detects nor prevents that**: the kernel reads the host's
protocol stdout and does not observe its socket or process activity.

This is deliberate, and it matches the roadmap's non-goal of claiming OS
sandboxing or making an OS sandbox a readiness gate. What the contract buys
is that capability becomes *stated and consented to* rather than silent. It
does not buy a technical guarantee, and no surface built on it may imply one.

## Design

### Vocabulary

`ExtensionToolRisk` gains `UsesNetwork` and `RunsProcess`, mapping onto the
native `ToolRisk` variants of the same names. Manifest parsing and host
`tool.register` accept the two new snake-case strings. Registration's risk
allowlist (`crates/yach-backend/src/tools.rs:2088`) accepts all five.

`ToolPermissionPolicy` gains a network allowlist so `UsesNetwork` can reach
`Allowed`. It is currently hardcoded denied in both authorization
(`crates/yach-backend/src/tools.rs:1987`) and provider advertising
(`crates/yach-backend/src/tools.rs:2004`), which is why the risk variant
existing was not by itself sufficient.

### The manifest must bind what the host may register

Consent is computed from the manifest, but today the manifest does not
constrain what the host actually registers. Activation passes only the tool
*count* into registration (`crates/yach-backend/src/extension.rs:1224`), and
the registration loop accepts the host's `name`, `risk`, `description`,
`provider_visible`, and schema verbatim
(`crates/yach-backend/src/extension.rs:1800`).

Widening the allowed risks without fixing that would make this whole
contract cosmetic: a manifest declaring one `metadata` tool would be granted
silently under the file-scoped path, and the host could then register a
`uses_network` tool that registration accepts. The user would have consented
to a summary the host never had to honor.

So two requirements, in order:

1. **Check consent before spawn.** The capability set is derived from the
   manifest and checked against the grant *before* the host process starts,
   not after registration reports what it wants.
2. **Reject registrations that do not match the manifest.** Each
   `tool.register` must correspond to a manifest-declared tool with the same
   name and the same risk. A registration naming an undeclared tool, or
   declaring a different risk than the manifest, fails activation as a
   protocol error.

This is ordinary protocol validation — the kernel comparing two things it
already has — not sandboxing. It does not prevent the subprocess from using
the network directly; see the honesty section. What it prevents is the
*registered tool surface* diverging from the declaration the user consented
to, which is a claim this design can actually keep.

### The capability set is derived, not declared twice

An extension's capability set is computed from its tools' declared risks.
There is no second manifest field.

A separate top-level `capabilities` block was considered and rejected: two
declarations of the same fact drift, and the failure mode is the dangerous
direction — a tool carrying authority the extension-level summary omits.
Deriving means the summary cannot understate what the tools requested.

Only `UsesNetwork` and `RunsProcess` participate in consent. The three local
risks are unchanged, so existing extensions — including the bundled hashline
extension — activate exactly as they do today.

### Consent is stored as an approved capability set

A grant lives in user state at `~/.yach/extensions/<extension-id>.json`,
following the authority boundary the approval-modes design already
establishes (`docs/project/specs/2026-08-24-approval-modes-design.md`): user
home is authoritative, and repository content may restrict but never grant.
A project-scoped extension still requires project trust before any of this
applies (`crates/yach-backend/src/extension.rs:1126`).

The current grant stores the **approved capability set**, manifest version
at grant, and grant timestamp. In the amended format it is the nullable
`current` member of a versioned authority/history document at the same path.
It is not duplicated in a separate grant file. A revoked document remains on
disk with `current: null`; file existence no longer means approval. See
Evidence for the document and migration contract.

Activation requires **requested ⊆ approved**. The version is display
metadata and is never compared.

This matters. An earlier draft keyed the grant on extension id plus version
and claimed a widened manifest would re-prompt. That is false: `version` is
an unvalidated string on the manifest
(`crates/yach-backend/src/extension.rs:1351`), unrelated to contents, so an
extension could widen from `content` to `network` while keeping the same
version and the grant would still match. Storing the capability set removes
the dependence on a self-reported field.

Binding consent to a manifest digest was also considered. It re-prompts on
every harmless edit, which trains the habitual approval the approval-modes
design names as weakening real escalations. The subset check re-prompts when
authority actually widens and stays silent otherwise:

| change | activation |
|---|---|
| widens `content` → `network`, version unchanged | blocked; re-prompts |
| version bumped, capabilities unchanged | proceeds |
| narrows `network` → `content` | proceeds; subset holds |
| unrelated code edit | proceeds |

### Activation stays non-interactive

`activate_background_metadata_extensions` is synchronous and cannot prompt.
It already expresses refusal by marking a diagnostic rather than asking —
that is how project-scope extensions are handled today
(`crates/yach-backend/src/extension.rs:1126`).

An extension requesting an ungranted capability therefore activates as
`PolicyBlocked`, with a diagnostic naming the missing capabilities and how to
grant them. `PolicyBlocked` already exists as an activation error kind.

Making activation itself interactive was considered and deferred. It would
mean threading a dialog channel into background extension startup and
defining behavior for headless and RPC clients that may have no user
present — a larger change than this contract, and one whose prompt would
write exactly the grant record specified here. This design is a prerequisite
for that, not an obstacle to it.

### Granting is explicit

`/extension-trust <id>` in the TUI and `yach extension trust <id>` in the CLI
show the derived capabilities and which tools requested each, then commit
the grant and evidence together. TUI trust reloads after persistence; CLI
trust affects subsequent activation and does not start a host. A failed
reload does not undo a successfully recorded grant.

Revocation clears the current grant and retains history. TUI revoke also
stops its managed host after the commit; CLI revoke does not reach into
another running process to stop its hosts. Lifecycle operations must not
reload a stale grant snapshot after a concurrent revoke: the activation gate
reads current authority. A grant is a deliberate user-initiated act, not a
step buried in installation.

### Surfacing

Extension diagnostics currently list id, version, scope, root, source,
install, state, errors, and tool names — but not risk
(`crates/yach-cli/src/main.rs:510-523`). Since activation is the only
authority point, being unable to see what an extension may do makes that
consent hollow. Diagnostics gain each extension's derived capabilities and
its grant state.

### Evidence

#### Identity and authority boundary

Grant and revoke decisions are user-scoped, not turn-scoped. The original
`PermissionDecisionRecorded` prescription is withdrawn: that session event
requires a real session and turn, while the standalone CLI has neither.
Existing session-event schemas and replay remain unchanged. No synthetic
session, turn, user name, or authenticated-human identity is recorded.

Both CLI and native lifecycle requests call one backend mutation API. Each
committed decision records an operation UUID, UTC timestamp, action, stable
reason (`extension_capability_grant` or `extension_capability_revoke`),
initiating surface, and before/after grant values. The document identifies
the extension. Grant values retain the approved set, version, and grant time.
Surface is `cli` or `lifecycle`; the latter includes the TUI but does not
pretend an arbitrary RPC client is the TUI or a verified human.

This evidence shows which local interface requested authority, not who was
at the keyboard, whether activation succeeded, or whether a host confined
itself to declared capabilities. It is user-editable local evidence, not a
tamper-proof security log.

#### One document, one commit

Use one document per validated extension id at the existing
`~/.yach/extensions/<id>.json` path, with these required fields:

| Field | Meaning |
| --- | --- |
| `schema` | `yach.extension-authority.v1` |
| `extension_id` | Must match the validated filename id |
| `current` | Current grant object or `null` |
| `legacy_baseline` | Imported grant object or `null`; not a decision |
| `history` | Ordered array of committed grant/revoke decisions |

Each history entry has `operation_id`, `recorded_at`, `action`, `reason`,
`surface`, `before`, and `after`. Array order under the writer lock defines
decision order; wall-clock timestamps need not be monotonic. Operation ids
are unique. History starts from `legacy_baseline`; each entry's `before`
equals the preceding state and `current` equals the final `after`. A grant
entry ends in a nonempty approved set; a revoke entry ends in `null`.
Validate these invariants and action/reason consistency when loading.

Re-granting writes a new decision, even for unchanged capabilities. Revoking
an already absent grant records an explicit null-to-null decision and
reports that no active grant existed. Trust of an extension requesting no
capabilities remains a no-op: no authority is granted, no grant decision is
invented, and existing file-scoped extensions incur no new state writes.

Reuse the persistence pattern in `UserConfigStore`, not its config schema:

1. Validate the id and private user-state paths; reject unsafe path types
   rather than following a symlink to a replacement authority file.
2. Acquire a stable per-extension sidecar file lock. The lock is not the
   replaced document inode. Reload and validate state under that lock.
3. Compute the next authority state and append its decision in memory.
4. Write the complete document to a unique, exclusively created sibling
   temporary file with private permissions; flush and sync the file.
5. Atomically rename it over the authority document, then sync the containing
   directory on platforms supporting that guarantee. Newly created state
   directories must also have their directory entries durably established.
6. Release the lock after the commit result is known. Remove uncommitted
   temporary files where possible; never treat them as authority on restart.

Readers see either a complete old document or a complete new document, never
an in-place truncate. All production grant writers/removers use this API;
there is no unaudited delete or second writable legacy path. Concurrent
processes serialize per extension and retain each committed decision.
Activation remains a read-only authority check, without audit writes or a
new process/network dependency. Benchmark its added document-read cost.

The history is logically append-only through Yach but rewritten with its
document. No automatic pruning silently erases decisions. This makes update
cost proportional to that extension's history; grant/revoke is infrequent.
Use the existing JSON/filesystem/locking dependencies, not a new database.

#### Failure and migration semantics

Before rename, any mutation error leaves previous authority and history
unchanged and reports failure. After rename, a directory-sync error means
the new document is visible but crash durability is uncertain. Report that
distinctly; do not claim rollback or successful durable recording. Do not
automatically retry a decision or reload a host after this uncertain result.
On platforms without directory-sync support, document the weaker guarantee
instead of claiming power-loss durability. A concurrent reader may observe
the complete new document between rename and the writer's final response.

Missing state means no grant. Malformed, unreadable, unsupported-schema, or
inconsistent state grants no authority. Mutation must surface the error and
must not overwrite damaged history as though it were an empty store. A
user can preserve and repair/remove a damaged document explicitly; no
automatic recovery fabricates lost decisions.

Recognize the previously shipped three-field grant object only when it has
no schema and is a valid legacy grant. Read-only activation accepts that
existing grant without rewriting it. At the next explicit grant or revoke,
under the same lock, preserve it as `legacy_baseline` and commit the new
decision and current state in the new format. Retain its recorded timestamp
as legacy metadata, not as proof of a historical operation, surface, or
human identity. Migration and the requested mutation are one replacement;
failure before rename leaves the legacy file intact. Unknown versioned
formats must never fall back to the legacy parser. All new writes use the
versioned format; no dual-write compatibility layer is kept.

The private JSON document is the inspectable audit artifact, including after
revocation. Document its location and fields. A new audit browser/command,
automatic retention, and compatibility with simultaneously running older
Yach binaries are not part of this change.

#### Alternatives rejected

- A separate append-only audit file plus live grant file needs transaction
  recovery to avoid authority-without-evidence or evidence-without-authority
  after a crash. A shared lock alone cannot make two writes atomic.
- Making session/turn ids optional changes unrelated session replay and
  still mixes user-scoped decisions into a session-scoped log.
- A database or general audit framework is unnecessary for this low-frequency
  per-extension state. Keep the implementation local to capability storage.

### Duplicate discovery and truthful CLI failures

The landed CLI loads the user install store and project install store
independently. When cwd and home coincide, including via a symlink, they
identify the same `.yach/extensions.json`. Loading it twice duplicates the
manifest and produces `catalog_error`. A real CLI probe reproduced this;
the same installed fixture works with distinct home and project directories.

Identify aliased install-store files before loading them twice and load the
shared file once. Preserve each stored record's scope; do not relabel a
project record as user-owned, infer a new precedence rule, or deduplicate by
extension id. Distinct stores/packages with conflicting ids or tool names
continue to fail catalog validation. This correction belongs in the shared
installed-record loader used by CLI and runner, not a doctor-only exception.

Repeated references to the same physical manifest within a package are
likewise discovery duplicates and should be read once after containment
validation. This does not authorize merging package roots with different
scope/provenance. Cross-scope overrides and general package precedence stay
out of scope.

Extension CLI management/diagnostic results with outcome `Failed` exit with
status 1 while retaining their structured output. Successful/no-op results
retain status 0 and malformed CLI usage retains status 2. Other outcome
semantics are unchanged. Restrict the change to extension command results;
do not redesign unrelated command exits.

### Protocol documentation completion

Update `docs/protocol/yach-proto-v0.md` to describe existing lifecycle and
diagnostic request/response events, selectors, outcomes, and record fields.
Document diagnostic `capabilities` and `capability_grant` as optional,
nullable arrays: missing/null is unknown; an empty array means a known empty
requested set or no recorded grant, respectively. Nonempty arrays contain
capability names. Array order is not a wire guarantee.

Keep existing protocol compatibility/stability caveats. Correct
`docs/extensions.md`: TUI diagnostics use live activation snapshots, while
CLI diagnostics freshly scan packages/install state and expose more fields.
Both display the capability/grant fields, not identical runtime state.
No new lifecycle wire event or session event is required for the audit store.

## Acceptance

- An extension declaring a `uses_network` tool registers successfully, where
  registration previously failed with `UnsupportedRisk`.
- A `UsesNetwork` tool reaches `Allowed` through `ToolPermissionPolicy` when
  allowlisted, and is advertised to providers.
- A host that registers a tool absent from its manifest fails activation as
  a protocol error, and the tool is not registered.
- A host that registers a manifest-declared tool with a *different* risk
  than the manifest declares — a `metadata` declaration registering as
  `uses_network` — fails activation, so consent cannot be obtained for one
  surface and spent on another.
- Consent is checked before the host process is spawned, so an extension
  without a grant never starts a subprocess.
- Reloading an extension applies the same gate. `/extension-reload` reaches a
  second activation entry point that spawns a host on its own path
  (`crates/yach-backend/src/extension.rs:801`), so the check is shared rather
  than duplicated, and reloading an ungranted or widened manifest is blocked.
- An extension requesting network capability with no grant activates as
  `PolicyBlocked`, and its diagnostic names the missing capability.
- After a grant, the same extension activates and its tool is callable.
- A manifest that widens capabilities while keeping the same version string
  is blocked until re-granted.
- A manifest that bumps its version without changing capabilities activates
  without re-prompting.
- Diagnostics show derived capabilities and grant state.
- CLI and native lifecycle grant/revoke each persist the defined decision
  evidence atomically with authority, without synthetic session identities.
- Revocation retains grant timestamps and history across process restart;
  an absent current grant denies capability even while its document exists.
- Valid legacy grants retain authority and migrate on mutation with explicitly
  legacy provenance; no historical decision is invented.
- Concurrent writers preserve ordered decisions; injected failures before
  rename preserve old state, and post-rename sync failures report uncertainty.
- Invalid/unsupported authority state fails closed and cannot be silently
  overwritten through a grant/revoke command.
- Identical or aliased home/project install stores load once without changing
  stored scope. Same-manifest discovery duplicates are suppressed, while
  distinct-package id/tool conflicts still fail.
- Failed extension CLI outcomes exit nonzero, verified through the real binary.
- Extension protocol documentation covers the implemented messages and
  nullable fields without promising capability-array ordering or identical
  CLI/TUI state.
- Existing file-scoped extensions, including hashline, activate unchanged
  with no grant.

## Out of scope

- **Per-call review** of extension network or process calls. Activation is
  the authority point; per-call prompts would imply an enforcement the
  subprocess boundary does not provide, and would add review friction to
  every fetch.
- **Enforcement or detection** of undeclared capability use. See the honesty
  section: the host is an ordinary subprocess and nothing observes its
  sockets.
- **Signing, pinning, or integrity verification** of extension packages. The
  install record carries source, kind, scope, enabled, and package root, with
  no hash or signature, and adding one is its own design.
- **Any OS-sandbox claim.**
- **The in-process `ToolExecutor` seam for embedders.** `ToolExecutor` is
  already a trait with multiple implementations
  (`crates/yach-backend/src/tools.rs`), so an in-process tool path for a
  Rust embedder is feasible and wants its own spec. It shares the trait with
  this work but not the authority question. Revisit when the extension
  surface stabilizes or a concrete embedding requirement firms up.

## Risks

- A capability granted at activation covers every later call by that
  extension, so a grant is meaningfully broader than a single `bash`
  approval. Mitigated by making the grant explicit, per-extension, visible
  in diagnostics, revocable, and recorded as evidence — and by the subset
  check, so the grant cannot silently widen.
- The contract could be read as a security boundary. Mitigated by stating
  the limit in this spec, in the grant prompt, and in the diagnostics
  wording, rather than only here.

## Follow-up verification

Keep regression coverage at the actual storage and caller seams: durable
grant/revoke/restart, legacy migration, repeated revoke, malformed state,
concurrent processes, and fault injection around the atomic replacement.
Verify decisions contain honest surface/provenance and current authority
matches retained history. Do not simulate success by forging permission
validation in the integration proof.

Run the real CLI with temporary home and project directories, both identical
and symlink-aliased, and verify install/doctor/trust/revoke outcomes and exit
codes. Preserve negative cases for genuine catalog conflicts. Exercise the
native lifecycle path as well as standalone CLI persistence. No test may
mutate the parent test process's HOME; use path-injected storage or subprocess
environments.

Final validation uses `just test`, `just lint`, `just fmt-check`, CI-toolchain
Clippy, and `just perf --filter 'extension/*'`. Measure authority-history
loading as well as existing file-scoped extension performance; no audit work
belongs on individual tool calls. These are acceptance checks to execute
during implementation, not claims that the amended behavior already exists.
