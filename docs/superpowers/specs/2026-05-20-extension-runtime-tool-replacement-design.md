# Extension Runtime And Tool Replacement Design

Date: 2026-05-20
Status: draft

## Context

Yach now has the pieces needed for a practical native-provider tool loop:
provider-visible read/search/list and exact/create edit tools, durable redacted
session evidence, local edit review, and a backend-owned multi-round loop. The
loop is intentionally registry-oriented, but the extension runtime that should
feed that registry is still only partially designed.

The current extension surface supports manifest parsing, a minimal catalog,
one-shot host registration, extension-owned metadata tools, schema-only provider
advertising, and in-memory executor routing. It deliberately rejects tool name
collisions and only allows safe metadata tools. That was the right first slice,
but it is not enough for the product direction: yach should support a robust
extension ecosystem where extensions can add tools, optionally replace built-in
tools through explicit policy, contribute context, and eventually provide richer
workflows without making the Rust TUI feel slow.

Other harnesses point at useful tradeoffs:

- Pi optimizes for extension ergonomics. It can install npm, git, URL, and local
  packages, load conventional package directories, and even load a single local
  file. That gives excellent hackability, but packages run with full system
  access and installation/loading are coupled to the TypeScript/npm world.
- OpenCode plugins are JavaScript/TypeScript modules loaded at startup. They
  can hook lifecycle events, add custom tools, and let plugin tools take
  precedence over built-ins. That is powerful, but startup loading and implicit
  replacement are exactly the paths yach should make explicit.
- Codex keeps extensibility surfaces declarative and policy-aware: MCP servers,
  app tools, hooks, plugins, skills, tool allow/deny lists, approval modes, and
  project trust boundaries are configured separately from core model/provider
  behavior. That is closer to yach's desired safety model.

Yach should borrow the user experience from Pi where it matters, the hook/tool
surface clarity from OpenCode, and the explicit policy/profile boundaries from
Codex, while preserving a Rust-native core that owns startup, safety, sessions,
tool execution semantics, provider continuation, and evidence.

## Goal

Design the next extension-runtime slice so yach can:

- install or reference extension packages from local paths, git URLs, and
  package refs;
- support TypeScript and Rust extensions through the same language-agnostic
  process protocol;
- keep extension discovery and activation off the TUI first-paint path;
- make provider-visible extension tools available before a provider turn when
  activation has completed;
- allow extensions to replace built-in tools only through explicit
  user/profile/project policy;
- preserve provenance so users and session evidence can tell which
  implementation handled a tool call.

This design should be sufficient to plan implementation slices. It should not
try to implement every contribution surface at once.

## Non-Goals

- No implementation in this slice.
- No compatibility with Pi extension APIs.
- No in-process dynamic libraries or arbitrary in-process JavaScript runtime.
- No automatic provider-visible advertising from manifest-only tool schemas.
- No hidden or implicit built-in replacement.
- No shell/process/network/mutation extension tools beyond the already designed
  built-in exact/create edit surface.
- No production marketplace or ranking service.
- No broad hook system that can mutate arbitrary runtime internals.
- No extension-provided system-prompt changes without explicit visible user
  configuration.
- No sandbox design beyond preserving the existing permission/risk boundaries
  and leaving room for future sandboxes.

## Recommended Shape

Use a **manifest-first, process-hosted runtime**.

Extensions live in package directories. Each package contains a
`yach.extension.json` manifest or a `package.json` with a `yach` section that
points to one or more yach manifests. The manifest declares identity, version,
activation events, contribution metadata, package source metadata, and the
command used to start the extension host.

The host is an external process that speaks `yach.extension-host.v1` over stdio
JSONL. A TypeScript extension can run through `node`, `deno`, `bun`, or a
package-local wrapper. A Rust extension can be a compiled binary. Yach core does
not embed a JS runtime and does not require npm at runtime; npm is only one
possible package source/resolution adapter.

Runtime phases:

1. **First paint:** do not scan packages deeply and do not spawn hosts. Load only
   a small cached extension index if it is already available and within a strict
   budget.
2. **Background discovery:** after first render, scan configured extension
   roots, validate manifests, and update an in-memory extension catalog.
3. **Activation:** start hosts for explicit activation events, manual reload,
   and post-first-paint provider-tool registration candidates.
4. **Registration:** accept tool/context/hook registrations only after a
   versioned handshake and local policy validation.
5. **Turn resolution:** before each provider turn, build a resolved tool catalog
   from built-ins plus activated extension registrations plus explicit
   replacement policy.

Late activation affects future turns only. Yach should not mutate an in-flight
provider request because an extension finished loading in the background.

## Install And Package UX

The user-facing install UX should stay close to Pi:

```text
yach install <ref>
yach extension install <ref>
yach extension remove <id-or-ref>
yach extension list
yach extension update [<id-or-ref>]
yach extension doctor [<id>]
```

`yach install` should be a short alias for `yach extension install` while the
extension ecosystem is young. If yach later installs other resource types, the
long form remains unambiguous.

Supported refs:

```text
npm:@scope/pkg@1.2.3
npm:pkg
git:github.com/user/repo@v1
https://github.com/user/repo
/absolute/path/to/extension
./relative/path/to/extension
```

This does not mean the Rust core depends on npm. It means the installer can have
source adapters. The npm adapter shells out to a configured package manager
(`npm`, `pnpm`, `bun`, or a user-provided command) during install/update only.
Git refs use `git` during install/update only. Local paths are referenced
directly and are useful for development and single-file/drop-in workflows.

Install scopes:

- **User scope:** `~/.yach/extensions/` plus `~/.yach/extensions.json`.
- **Project scope:** `.yach/extensions/` plus `.yach/extensions.json`.
- **Ephemeral scope:** `yach -e <ref>` or `yach --extension <ref>` for one run.

Project-scoped extension settings may be committed, but project-scoped extension
activation should require the project to be trusted. Missing project extensions
may be installed on demand, but that install must not happen before first paint.

Package directories may expose resources through either explicit manifest paths
or conventional directories:

```text
extensions/       host manifests or single-file hosts
skills/           future SKILL.md-style resources
prompts/          future prompt templates
themes/           future TUI themes
static-context/   manifest-referenced context files
```

For the next implementation, only `extensions/` and manifest-referenced
`static-context/` need to be recognized.

## Single-File And TypeScript Ergonomics

Yach should support a Pi-like drop-in workflow without putting TypeScript inside
the Rust core.

For a single `.ts` or `.js` file in an extension directory, yach can synthesize a
development manifest if the file either:

- exports a well-known manifest object in a later SDK shape; or
- has a neighboring minimal manifest that points at it.

The first implementation should prefer the neighboring manifest because it is
inspectable, versioned, and easy to validate without executing code. A later
TypeScript SDK can provide a `yach extension init --ts` helper that creates:

```text
yach.extension.json
src/index.ts
package.json
```

Rust extensions should use the same host protocol. A later Rust SDK should make
registration and tool result shaping ergonomic, but compiled Rust binaries
should not get a privileged in-process path by default. A future high-performance
in-process Rust plugin mode can be designed separately if profiling shows the
process boundary is a real bottleneck.

## Activation And Startup Performance

The first-frame rule is strict:

- no extension host spawn before TUI first render;
- no dependency install before TUI first render;
- no network access before TUI first render;
- no project extension activation before project trust is known;
- no provider-visible extension advertisement from unactivated code.

Post-first-paint background work should be divided into budgets:

- `manifest_index_load`: load cached extension index, target sub-millisecond;
- `manifest_scan`: scan configured roots, cancellable and resumable;
- `host_registration`: bounded per extension, default timeout small for
  background activation;
- `turn_ready_activation`: optional longer timeout only when a user explicitly
  requests activation before a turn.

Diagnostics should report:

- installed extensions;
- discovered manifests;
- activation state;
- provider-visible tools pending activation;
- activation failures with redacted command/source details;
- startup marks for index load, scan scheduling, scan completion, host spawn,
  registration complete, and skipped activation due to budget.

The TUI should show extension state, but slow extension activation should degrade
the extension, not the input box.

## Tool Resolution And Replacement

Tool names are globally resolved at the start of each provider turn. Resolution
uses these layers:

1. built-in definitions;
2. activated extension registrations;
3. user/profile/project replacement policy;
4. session-local temporary overrides, if later added.

Default collision behavior remains fail-closed. Replacement requires explicit
configuration naming the built-in, the extension id, and the extension tool
implementation:

```toml
[tools.replace.search_project]
extension = "com.example.ff-tools"
tool = "ffgrep"
mode = "replace_builtin"
scope = "profile"
```

Resolution modes:

- `deny`: reject colliding extension registration or ignore the candidate;
- `alias_only`: expose the extension under its own name, such as `ffgrep`;
- `replace_builtin`: provider-facing name stays `search_project`, but execution
  routes to the extension implementation;
- `disable_builtin`: remove a built-in from the active catalog without replacing
  it.

Replacement must preserve provenance:

- provider-facing tool name;
- resolved owner;
- implementation tool name;
- extension id and version;
- replaced built-in name;
- risk class and permission family;
- replacement source: user config, project config, profile, or ephemeral flag.

Provider-visible docs and session evidence should make replacement visible. If
`search_project` is handled by `com.example.ff-tools/ffgrep`, the tool request,
permission decision, execution summary, and optional diagnostics should all
record that provenance.

Replacement does not lower risk. If an extension replaces `edit_text_file`, it
inherits mutation policy and review requirements. If an extension replaces
`search_project`, it is still a content-read tool. If the extension declares a
weaker risk than the built-in family requires, yach should fail closed.

## Provider-Turn Availability

Provider-visible extension tools are advertised only when all of these are true:

- the extension is installed and enabled;
- the project/user trust policy permits activation;
- the host has registered the tool for the current process;
- the tool has an executable route;
- policy permits the risk class for provider visibility;
- any replacement policy has resolved successfully;
- the schema can be projected into yach's provider-advertising representation.

For a normal provider turn, yach should use the active catalog at turn start.
Extensions that activate while a turn is running become available on the next
turn. If a user needs a tool immediately, they can run an explicit activation or
reload command before submitting the prompt.

This model keeps provider advertising non-circular. The provider does not see a
tool unless yach already has a concrete, policy-accepted route for executing it.

## Permissions And Trust

Extension runtime policy should have separate knobs:

- package install trust;
- project extension activation trust;
- provider visibility;
- tool risk permission;
- per-tool approval mode;
- replacement permission.

Initial default posture:

- user-installed extensions can be discovered after first paint;
- project-installed extensions require trusted project state before activation;
- provider-visible metadata tools may activate in the background;
- content, mutation, shell/process, and network extension tools require future
  focused designs before provider exposure;
- replacement of built-ins is disabled unless explicitly configured.

Project config cannot silently enable high-risk provider-visible tools or replace
built-ins in an untrusted project. If project config requests replacement, yach
should surface it as visible activation/provenance state.

## Host Protocol Evolution

Keep the protocol versioned and small. The next runtime slice should extend the
current one-shot registration toward a persistent host:

- `extension.initialize`: yach sends protocol version, extension id, workspace
  metadata allowed by policy, enabled capabilities, and redacted config.
- `extension.ready`: host confirms protocol version and process metadata.
- `tool.register`: host registers one tool definition.
- `tool.invoke`: yach invokes a registered tool with validated arguments and a
  request id.
- `tool.result`: host returns structured output, bounded text, or structured
  failure.
- `extension.shutdown`: yach asks the host to exit.

Do not let hosts append session events, mutate transcripts, call provider
adapters, or decide provider continuation. Hosts return data to yach; yach owns
validation, permission, review, result shaping, evidence, and provider
continuation.

## Static Context And Prompt Customization

Extension-packaged static context should remain explicit and visible:

- manifest-referenced files may contribute `background_context`;
- yach records accepted context and omissions as redacted evidence;
- extension context should appear in diagnostics with source extension id;
- extension context activation should not block first paint.

Extension-provided system-prompt additions are not part of this runtime slice.
If yach later supports extension `append_system` contributions, it should mirror
the visible-file spirit of `.yach/APPEND_SYSTEM.md`: explicit source, visible to
the user, inspectable in diagnostics, and gated by policy. Hidden system-prompt
mutation by extension code should remain disallowed.

## Error Handling

Fail closed when:

- a manifest is malformed or uses unsupported schema;
- package install cannot resolve the ref;
- project extension activation is requested before trust;
- dependency install is required during startup;
- host protocol version is unsupported;
- host exits, times out, or emits malformed JSONL;
- registration collides without explicit replacement policy;
- replacement policy references missing extension/tool/built-in names;
- replacement weakens required risk classification;
- provider advertising is requested before an executable route exists;
- result exceeds limits or does not match the output contract.

User-facing errors should name extension id, tool name, and categorical reason.
Session evidence should avoid raw local data, raw arguments, provider payloads,
secret-bearing command lines, and raw extension output.

## Testing And Evidence

Implementation plans should include tests for:

- parsing package and extension manifests;
- building a catalog without spawning hosts;
- installing local path refs into user, project, and ephemeral scopes;
- rejecting or deferring project extensions in untrusted projects;
- preserving first-paint startup metrics with installed inactive extensions;
- post-first-paint manifest scan and activation diagnostics;
- persistent host handshake, registration, invocation, timeout, crash, and
  malformed result handling;
- provider-visible extension tool advertising only after activation;
- explicit alias-only and replace-built-in resolution;
- accidental built-in collision failing closed;
- replacement provenance in tool evidence and provider loop execution;
- replacement risk escalation preserving permission/review behavior.

Performance evidence should include:

- baseline native TUI first render;
- installed inactive extension first render;
- large manifest set scan after first paint;
- background provider-tool activation latency;
- extension tool invocation latency through the process protocol.

## Acceptance Criteria

This design is ready for implementation planning when it can drive small slices:

1. extension package roots, install refs, and manifest index cache;
2. post-first-paint manifest scan and diagnostics;
3. persistent extension host invoke/result protocol for a metadata `toy_tool`;
4. provider-turn catalog resolution from built-ins plus activated extensions;
5. explicit alias/replacement policy with provenance evidence;
6. static-context file contribution from installed extensions;
7. startup and activation profiling.

The first implementation should still be conservative: no broad mutation,
shell/process, network, hidden system prompt changes, or implicit replacement.
The product direction, though, should be clear: yach supports Pi-like extension
ergonomics while keeping runtime authority, safety, and performance in the Rust
core.

## Sources

- Pi package docs:
  `https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md`
- OpenCode plugins:
  `https://opencode.ai/docs/plugins/`
- OpenCode tools:
  `https://dev.opencode.ai/docs/tools/`
- Codex config reference:
  `https://developers.openai.com/codex/config-reference`
- Local prior design:
  `docs/superpowers/specs/2026-05-12-extension-tool-registration-design.md`
- Local multi-round loop design:
  `docs/superpowers/specs/2026-05-18-native-provider-multi-round-tool-loop-design.md`
