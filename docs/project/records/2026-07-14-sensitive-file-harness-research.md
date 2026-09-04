# How Other Harnesses Protect Sensitive Files

Date: 2026-07-14

Context: yach's provider-visible read/search/list tools can currently read
any project file, including `.env.local` with live API keys, and the session
tool payload persistence change means anything the model reads also lands in
plaintext session logs. This record summarizes how the comparison cohort
(Codex CLI, Claude Code, opencode, Pi; aider and Cline as secondary
datapoints) protects sensitive files, from a source survey.

## Key finding

Nobody in the cohort ships a real default deny for sensitive files. The
strongest shipped defaults are opencode's `*.env` -> ask (read tool only,
with documented gaps in grep/subagents) and Codex CLI's default-on stripping
of `*KEY*`/`*SECRET*`/`*TOKEN*` env vars from subprocess environments.
Everything else is user-configured (Claude Code `permissions.deny`, Codex
permission profiles, `.aiderignore`, `.clineignore` — the latter ships
empty). Nobody redacts tool output before provider send, and every harness
persists provider-visible payloads verbatim in plaintext local session
files: whatever entered context lands on disk.

## Per-harness mechanisms

| Harness | Default protection | Mechanism | Known gaps |
| --- | --- | --- | --- |
| Codex CLI | None for file reads (`has_full_disk_read_access()` hardcoded true); env vars matching `*KEY*`/`*SECRET*`/`*TOKEN*` stripped from subprocesses by default | Opt-in permission profiles with OS-enforced deny globs (Seatbelt/Landlock), e.g. `"**/*.env" = "deny"` as a docs example | Rollouts persist verbatim; shell snapshots leaked `*_TOKEN` env vars (openai/codex#30971); `.codexignore` requests closed unimplemented |
| Claude Code | None | User-configured `permissions.deny` (`Read(./.env)`), deny -> ask -> allow, gitignore-spec patterns; best-effort coverage of recognized Bash file commands | `cat`/`grep`/`head` run unprompted by default; app-layer denies bypassed by arbitrary subprocesses (#6002, #52182); transcripts plaintext under `~/.claude/projects/` |
| opencode | `*.env` / `*.env.*` -> ask on the read tool; `.env.example` allowed | Pattern permission rules, last-match-wins | Only gates `read`: grep and bash return `.env` contents unprompted; explore subagent merged `read: allow` over the default; earlier substring match blocked `src/environment.ts` (#4969) |
| Pi | None, by explicit design | "Containerize or sandbox Pi"; opt-in example extensions block writes (not reads) to `.env` | `/share` uploads sessions unredacted |
| aider | None shipped; strong incidental barrier | Repo map enumerates only git-tracked files, so untracked `.env` never enters context unless explicitly added; opt-in `.aiderignore` | Explicit `/add`/`/read` bypass; chat history persists file contents verbatim in the repo dir |
| Cline | None shipped | `.clineignore` enforced at the tool layer across read/search/list | Ships empty; terminal command bypass documented (cline/cline#2431); task history verbatim in globalStorage |

## Recurring failure class

Enforcement-point mismatch: per-tool rules drift (opencode's explore-agent
hole, Cline's tool-by-tool coverage), app-layer read denies bypassed by
shell commands, and secrets entering context in "safe" auto modes because
read-only commands are unprompted. Only OS-level enforcement closes the
shell hole. Session artifacts are the second leak surface everywhere.

## Recommendation for yach

Yach can ship ahead of common practice here, consistent with its
deny-by-default posture:

1. One path-authorization chokepoint: route read/search/list/edit through a
   single `authorize(path, access)` applying the project-root boundary and
   deny patterns. Search/list must filter both content matches and denied
   filenames. Per-tool checks are how the cohort's bypasses happened.
2. Ship default deny patterns, deny-first precedence (Claude Code's
   deny -> ask -> allow ordering, not opencode's last-match-wins), matched
   with gitignore/basename semantics, never substring. Default set
   synthesized from cohort defaults, docs examples, and the Codex community
   fork list:
   - `.env`, `.env.*`, `*.env` (allow `.env.example`, `.env.sample`,
     `.env.template`)
   - `*.pem`, `*.key`, `*.p12`, `*.pfx`, `id_rsa*`, `id_ecdsa*`,
     `id_ed25519*`, `*.keystore`
   - `.netrc`, `.npmrc`, `.pypirc`
   - `**/.aws/credentials`, `**/.ssh/**`, `**/.config/gcloud/**`,
     `**/.azure/**`
   - `secrets/**`, `credentials.json`
   Keep the list visible and overridable in file-first config (Codex's TOML
   `"**/*.env" = "deny"` shape is good precedent); the opencode revert
   history shows users need `.env.example`-style escape hatches.
3. Deny at the tool layer is the session-log protection too: keeping secrets
   out of context keeps them out of persisted payloads for free. Add
   restrictive permissions (0700/0600) on `.yach/native-sessions`. Treat any
   log redaction pass as best-effort defense-in-depth, never the mechanism.
4. When yach gains a shell/process tool, app-layer denies will not cover it;
   the sound posture is OS-level enforcement (Codex) or an explicit
   containerize-it stance (Pi). Adopt Codex's cheap default-on win
   regardless: strip `*KEY*`/`*SECRET*`/`*TOKEN*` env vars from subprocess
   environments (yach's extension hosts already start from an allowlisted
   environment, which is the same idea).
5. Treat .gitignore as relevance filtering, not security: respect it in
   search/list for noise, but direct-path reads are governed by the deny
   list alone.

This needs a focused Superpowers design before implementation (new
permission/policy surface). Related records:
`docs/project/records/2026-07-14-stale-evidence-harness-research.md`,
`docs/project/records/2026-07-14-resume-transcript-research.md`,
`docs/project/specs/2026-07-14-session-tool-payload-persistence-design.md`.
