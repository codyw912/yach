# Execution Isolation Approaches For Shell Tools

Date: 2026-07-16

Context: research for the shell execution design
(`docs/project/specs/2026-07-16-shell-execution-design.md`) — what the
isolated/sandboxed alternatives to a host shell tool actually are, verified
against primary sources.

## Key findings

1. Hermetic bash interpreters cannot run real toolchains — confirmed.
   Vercel's just-bash (vercel-labs/just-bash) is a bash reimplementation in
   TypeScript over a virtual filesystem with ~100 built-in commands and no
   fork/exec path at all; Vercel's own guidance is to use a full VM/sandbox
   product "if you need arbitrary binary execution." Rust analogs exist and
   are active (everruns/bashkit: 164 built-ins, no fork/exec, overlay/
   in-memory filesystems, embeddable builder API). Useful someday as a safe
   read-only explore tool; disqualified as the shell tool.
2. OS-native per-command sandboxing is the proven middle tier that can run
   cargo/pytest with near-zero startup cost. Codex CLI is a liftable
   Apache-2.0 reference in Rust:
   - macOS: dynamically generated SBPL profile spawned under hardcoded
     `/usr/bin/sandbox-exec`; deny-default base policy (~122 lines, modeled
     on Chrome's) with writable roots as parameters, `.git`-style protected
     subpaths, and pty/POSIX-semaphore/shm allowances that real toolchains
     need (`codex-rs/sandboxing/src/seatbelt.rs`).
   - Linux: bubblewrap (vendored) for the read-only rootfs + writable bind
     mounts, `seccompiler` BPF filters (network deny; unconditional
     ptrace/io_uring deny), official `landlock` crate as legacy fallback;
     self-exec helper binary pattern.
   - Windows: young and weak everywhere; Claude Code punts to WSL2.
3. The escalation UX both incumbents converged on: sandbox-caused failure →
   the model may request an unsandboxed retry with justification → user
   approves → rerun outside the sandbox; a strict mode disables the hatch.
   Claude Code's sandbox "auto-allow" runs in-boundary commands without any
   prompt (Anthropic reports 84% fewer prompts) — the sandbox replaces the
   prompt, matching the Codex auto-review posture.
4. Correction to earlier cohort research: Codex's default env stripping of
   `*KEY*`/`*SECRET*`/`*TOKEN*` has been turned off by default on current
   main (`ignore_default_excludes` default flipped between v0.39.0 and
   main); the machinery remains as a configurable policy object. Claude
   Code likewise ships no default env deny — opt-in `sandbox.credentials`
   (unset or proxy-side mask) and a subprocess scrub for Anthropic/cloud
   creds. The industry trend: env filtering defaults permissive, with
   OS/network boundaries doing the real work.
5. Container tier is cheap and complements: `docker run --rm -v $PWD:/work
   --network=none <image>` gives a whole-process boundary for unattended
   runs (~a day of work); the real cost is image curation. Codex cloud runs
   setup with network then air-gaps the agent phase; Claude Code positions
   devcontainers with an egress firewall as the supported way to run
   unattended; Pi's philosophy is exactly this ("run pi inside a
   container").
6. Rust building blocks: generate SBPL directly (no crate needed),
   `landlock` and `seccompiler` crates are solid, vendor bwrap like Codex
   if needed. Avoid cross-platform sandbox crates: birdcage is archived,
   gaol dormant. `brush-core` exists as an embeddable real bash-compatible
   shell (a runtime, not an isolation mechanism).

## Implications adopted into the shell design

- v1 ships the host executor behind a pluggable seam. The isolation
  landscape (OS sandbox, containers, hermetic/virtual filesystems) is
  deliberately left open by owner decision — this record is reference
  material for that exploration, not a chosen direction. The seam
  requirements the design commits to are listed in the spec's
  "Isolation: Open Exploration Space" section.
- Hermetic interpreters are disqualified as the shell executor specifically
  (verified: no real binary execution); anything else about them stays
  open.
- Env stripping: yach ships it ON by default while host is the only tier —
  diverging from the incumbents' current defaults deliberately, because
  they relaxed theirs after the sandbox/network boundary took over
  enforcement, and yach has no such boundary yet. Revisit when any
  isolation boundary lands. `shell.env_allow` is the escape hatch.
- Sources: github.com/vercel-labs/just-bash, github.com/everruns/bashkit,
  github.com/openai/codex (codex-rs/sandboxing, linux-sandbox, bwrap,
  protocol/src/shell_environment.rs), code.claude.com/docs/en/sandboxing,
  anthropic-experimental/sandbox-runtime, developers.openai.com/codex
  (security, cloud/environments), mariozechner.at pi post.
