# Sensitive File Deny-By-Default Design

Date: 2026-07-14

Status: draft for review

## Context

Yach's provider-visible file tools (`read_text_file`, `search_project`,
`list_project_paths`, exact/create edit tools) can touch any file under the
project root, including `.env.local` with live API keys. Since the session
tool payload persistence design landed, anything the model reads is also
persisted verbatim in plaintext session logs and re-fed into later provider
requests. Cohort research
(`docs/project/records/2026-07-14-sensitive-file-harness-research.md`) found
no comparison harness ships a real default deny; the recurring failure class
is per-tool rule drift and enforcement-point mismatch. The owner decision
(2026-07-14) is to ship deny-by-default with a config-surfaced, overridable
pattern list.

## Goal

- Provider-visible file tools refuse to read, match, list, or edit files
  matching a sensitive-path deny list, by default, with no configuration.
- The default list is visible and overridable through file-first config.
- Denied reads fail with a categorical, actionable tool result (the model
  can explain and adapt; the turn continues).
- Search and list silently exclude denied paths from results so filenames
  and contents of secrets never enter provider context or session logs.
- Session log directories and files are created with restrictive
  permissions.

## Non-Goals

- Interactive ask/prompt flows for sensitive paths (deny or allow only; the
  existing review pipeline is for mutations, not reads).
- Secrets scanning or redaction of tool output content. Keeping denied files
  out of context is the mechanism; redaction is explicitly not.
- Shell/process tool enforcement (no such tool exists yet; when one is
  designed, it must address this boundary itself — OS enforcement or
  Codex-style env stripping — and this design's list does not cover it).
- Gitignore integration. Gitignore stays relevance filtering in search;
  security is this deny list alone.
- Encrypting session logs or redacting already-written logs.

## Design Principles

### One Chokepoint

All sensitive-path decisions go through a single
`NativeSensitivePathPolicy::denies(relative_path) -> bool` check owned by
`yach-backend`, consulted by:

- `NativeResourceRoot::read_text_file` (deny -> categorical error),
- `NativeResourceRoot::search_text` (denied files skipped before read;
  skipped files do not consume the search budget and are not counted in
  `searched_files`),
- `NativeResourceRoot::list_paths` (denied entries omitted from listings,
  like the existing generated/heavy skip),
- `NativeEditEngine` path validation (deny -> `NativeEditError` variant,
  flowing through the existing recoverable failed-tool-result path from
  PR #128).

Per-tool checks are how the cohort's bypasses happened; new tools that
touch paths must route through the same policy.

### Deny-First, Match-Correct

Rule evaluation: deny patterns beat allow patterns beat the built-in
default; allow patterns exist to carve exceptions out of broader denies
(`.env.example` out of `.env*`). Matching uses `globset` (ripgrep's glob
engine) compiled once per policy: correct gitignore-style semantics
(basename patterns without `/` match at any depth; `**` crosses
directories) from a battle-tested crate rather than a bespoke matcher —
hand-rolled glob logic in a security boundary is how bypasses are born.
Substring matching is explicitly forbidden (opencode blocked
`src/environment.ts` with `.includes(".env")`).

`globset` is a new dependency of `yach-backend`. It is small, widely used,
and maintained as part of ripgrep.

### Visible, Overridable Defaults

Built-in default deny patterns (constants in `yach-backend`, documented in
config docs and shown by a future `yach config` surface):

```
.env
.env.*
*.env
*.pem
*.key
*.p12
*.pfx
id_rsa*
id_ecdsa*
id_ed25519*
*.keystore
.netrc
.npmrc
.pypirc
**/.aws/credentials
**/.ssh/**
**/.config/gcloud/**
**/.azure/**
secrets/**
credentials.json
```

Built-in default allow patterns:

```
.env.example
.env.sample
.env.template
*.env.example
```

### File-First Config, Zero New Config Dependencies

Config lives in JSON (serde_json is already in-tree; `extensions.json` is
the precedent) at two scopes, mirroring extension install records:

- user: `~/.yach/config.json`
- project: `<project>/.yach/config.json`

Shape (only this section is defined by this design; the file is the seed of
a general config surface):

```json
{
  "files": {
    "deny": ["internal-secrets/**"],
    "allow": [".env.ci"],
    "use_default_deny": true
  }
}
```

Resolution: start from built-in defaults (unless `use_default_deny` is
`false` in either scope, project winning), then union user and project
`deny` lists, then union user and project `allow` lists. Evaluation stays
deny-first: a path matching any effective deny pattern is denied unless it
matches an effective allow pattern. Invalid patterns fail closed: a config
whose globs do not compile disables nothing — the built-in defaults still
apply — and the load error surfaces as a status message at startup.

`.yach/config.json` itself remains protected by the existing metadata-path
policy (the `.yach` directory is already excluded from tool access), so the
model cannot edit its own restrictions.

### Denied Reads Are Recoverable, Not Silent

- `read_text_file` / edit tools on a denied path: failed tool result with
  reason `sensitive_path_denied` and guidance ("this path matches the
  sensitive-file deny list; ask the user to allow it in .yach/config.json
  if access is intended"), following the PR #128 recoverable-failure shape.
  The tool event persists the categorical reason; never the content.
- `search_project` / `list_project_paths`: denied paths silently excluded
  (no filename leak, no per-file notice). The result remains honest via the
  existing bounded/truncated flags; a `denied_paths_excluded: true` marker
  is added to result metadata so evidence records that filtering occurred
  without saying what was filtered.

### Session Log Permissions

`.yach/native-sessions/` is created with mode `0700` and session JSONL
files with `0600` on Unix (no-op on other platforms). Applied at store
creation; existing files are not rewritten.

## Approach Options Considered

### Option A: Hardcoded List Only

Thirty lines against the existing path-policy seams, no config. Rejected by
owner decision: the list must be visible and overridable; opencode's revert
history shows users need escape hatches (`.env.ci`, example files).

### Option B: Config-Surfaced Single Chokepoint (Recommended)

As specified above.

### Option C: Full Permission-Rule Language

Claude-Code-style per-tool allow/deny/ask rules with tool selectors.
Rejected for now: yach has no interactive ask flow for reads, and the added
rule surface is exactly the mechanism sprawl the Pi-inspired posture
avoids. The `files` config section leaves room to grow if ever needed.

## Verification

- Unit: policy resolution (defaults, scope union, allow carve-outs,
  `use_default_deny: false`, invalid-pattern fail-closed), matcher semantics
  (basename at depth, `**` crossing, no substring matches).
- Tool level: denied read fails with `sensitive_path_denied` and guidance;
  search never returns matches or filenames from denied files and does not
  spend budget on them; list omits denied entries; edit tools refuse denied
  targets through the recoverable path.
- Loop level: a provider turn that attempts to read `.env` continues with
  the failed tool result and completes.
- Session evidence: no denied-file content in the log; categorical reason
  present; `denied_paths_excluded` marker on filtered search/list evidence.
- Permissions: session dir/file modes asserted on Unix.
- Live dogfood: ask the model to read `.env.local` and to search for a
  string that exists only in `.env.local`; confirm the deny result and the
  empty search, then allow `.env.ci` via project config and confirm the
  override works.
