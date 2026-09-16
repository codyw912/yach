# Extension Capability Contract Design

**Outcome:** plane:YACH-10

Status: proposed 2026-09-16

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

The record stores the **approved capability set**:

```json
{
  "approved": ["uses_network"],
  "version_at_grant": "1.2.0",
  "granted_at": "2026-09-16T12:00:00Z"
}
```

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

`/extension-trust <id>` in the TUI and an equivalent CLI subcommand show the
derived capabilities and which tools requested each, then record the grant
and re-activate. Revoking deletes the record. A grant is a deliberate
user-initiated act, not a step buried in installation.

### Surfacing

Extension diagnostics currently list id, version, scope, root, source,
install, state, errors, and tool names — but not risk
(`crates/yach-cli/src/main.rs:510-523`). Since activation is the only
authority point, being unable to see what an extension may do makes that
consent hollow. Diagnostics gain each extension's derived capabilities and
its grant state.

### Evidence

Each grant and revocation records a `PermissionDecisionRecorded` session
event with its own reason, so an audit shows why an extension ran with
network capability and who allowed it. This follows the provenance pattern
the session-grant work established, where `shell_session_grant` is
distinguishable from `shell_user_allowlist` and `approval_mode_full_access`.

Note the limit, consistent with the honesty section: the evidence records
that a capability was *granted*, not that the extension confined itself to
it.

## Acceptance

- An extension declaring a `uses_network` tool registers successfully, where
  registration previously failed with `UnsupportedRisk`.
- A `UsesNetwork` tool reaches `Allowed` through `ToolPermissionPolicy` when
  allowlisted, and is advertised to providers.
- An extension requesting network capability with no grant activates as
  `PolicyBlocked`, and its diagnostic names the missing capability.
- After a grant, the same extension activates and its tool is callable.
- A manifest that widens capabilities while keeping the same version string
  is blocked until re-granted.
- A manifest that bumps its version without changing capabilities activates
  without re-prompting.
- Diagnostics show derived capabilities and grant state.
- Grant and revocation each record permission-decision evidence.
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
