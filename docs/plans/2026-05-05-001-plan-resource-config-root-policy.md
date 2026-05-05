# Resource/config Root Policy Plan

Date: 2026-05-05
Status: planning recommendation; implementation not started
Related: `.project/phases/05-native-tools-resources-session-hardening.md`, `docs/project-os/architecture-invariants.md`, `docs/project-os/compatibility.md`, `docs/protocol/yach-proto-v0.md`

## Goal

Define the first native resource/config root model before yach exposes local files, settings, or generated state to native tools or providers.

The goal is not broad Pi resource parity or a provider-visible file browser. It is a small, yach-owned policy that makes the first read-only resource slice testable, inspectable, and safe to expand.

## Constraints to protect

- `yach-proto` remains the UI/backend seam; `yach-ui` must not import backend resource loaders or provider SDKs.
- File-first configuration/resources remain first-class and inspectable.
- Pi resource locations are compatibility/reference inputs, not canonical native ownership.
- Native session JSONL and generated state remain backend-internal/provisional.
- No credentials, raw provider payloads, or sensitive local file contents are persisted by default.
- Native/native-provider stay explicit and non-default.

## Proposed root classes

### 1. Project root

Canonical path: the current workspace/project root selected by the native backend at launch.

Purpose:

- Primary source for project-local config, plans, prompts, and future safe read-only resources.
- First implementation root because it is inspectable and naturally scoped to the current working tree.

Initial policy:

- Canonicalize with symlink resolution before use.
- Reject reads that resolve outside the canonical project root unless an explicit compatibility/import root is configured later.
- Treat contents as local-trusted, not automatically provider-visible.

### 2. User config root

Canonical path: a yach-owned user config directory, selected later.

Purpose:

- Future home for user-level settings, prompt snippets, themes, provider-independent preferences, and policy files.

Initial policy:

- Planning-only for now; do not implement in the first slice.
- Never persist provider credentials here as part of this phase unless separately approved.
- Provider-visible use requires explicit per-resource or per-operation policy.

### 3. Generated state root

Canonical path: a yach-owned generated state/cache/session directory, currently represented by native session output such as `.yach/native-sessions/default.jsonl`.

Purpose:

- Native session logs, derived indexes, caches, and temporary artifacts.

Initial policy:

- Backend-internal by default.
- Contents are not resources for provider context unless explicitly copied through a policy-bound resource API.
- Keep provisional file-format labels until migration/stability is approved.

### 4. Compatibility import roots

Canonical paths: Pi settings/resource/package/session locations discovered or configured later.

Purpose:

- Preserve migration-critical Pi behavior and file-first learning without forcing yach to copy Pi's whole resource model.

Initial policy:

- Reference/import only, not canonical native roots.
- Discovery/import semantics require separate design and approval.
- Do not read or persist Pi-owned credentials/secrets by default.

## Trust and visibility model

Use two independent labels for each resource:

- **Local trust:** whether yach trusts the source enough to parse/use it locally (`project`, `user`, `generated`, `compat-import`, `untrusted`).
- **Provider visibility:** whether contents may be sent to a provider (`never`, `explicit`, `allowed-by-policy`).

Default for first slice:

- Project root paths may be read only by explicit backend/test calls.
- Provider visibility is `never` until a user-approved resource-to-provider flow exists.
- Generated state is backend-internal and provider visibility is `never`.
- Compatibility import roots are not loaded.

This avoids conflating "file exists locally" with "safe to include in model context."

## Path policy

First implementation should provide a backend-internal helper that:

1. Stores canonical root paths.
2. Canonicalizes requested paths before opening.
3. Rejects path traversal, missing paths, non-file paths where a file is expected, and resolved paths outside the selected root.
4. Treats symlinks as allowed only when their resolved target remains inside the root.
5. Returns normalized, non-secret error kinds suitable for status/session logs without raw absolute-path spam unless debug mode is explicitly approved later.

Open questions for implementation:

- Whether to allow hidden files in the first slice. Recommendation: allow explicit reads inside the root for tests, but do not add broad discovery/globbing yet.
- Whether to cap file size at the path helper layer or at resource-read layer. Recommendation: cap at resource-read layer so path policy stays focused.

## Reload/discovery semantics

For the first slice, prefer explicit read calls over background discovery.

- No watcher.
- No automatic provider context injection.
- No recursive scan.
- No `/reload` protocol surface.
- Tests can create temporary project roots and request specific relative paths.

A later resource discovery plan can decide watch/reload behavior after the safe read path exists.

## Protocol impact

No `yach-proto` changes are recommended for the first implementation slice.

Resource root policy can begin entirely inside `yach-backend` because there is no user-visible resource browser or provider context operation yet. Add protocol events only when the UI needs to display roots, ask for resource approval, trigger reload, or show resource-read results.

## Recommended first implementation slice

Implement backend-internal resource root/path canonicalization helpers and tests.

Expected scope:

- Add a small module in `yach-backend` for root registration and relative path resolution.
- Support only project-root reads/resolution in tests.
- Add tests for in-root file, `..` traversal rejection, symlink-to-outside rejection, missing path, and directory-vs-file mismatch.
- No provider submission, no TUI UI, no Pi import, no credential persistence, no broad discovery.

Suggested validation:

```bash
just dev cargo fmt
just dev cargo clippy -p yach-backend --all-targets -- -D warnings
just dev cargo test -p yach-backend
git diff --check
```

## Deferred decisions requiring approval

- Provider-visible file/resource reads.
- Credential/config persistence.
- Pi resource/package/session import semantics.
- User/global config root location and migration behavior.
- Background discovery, file watchers, or `/reload` protocol behavior.
- Any broad resource UI or automatic context injection.
