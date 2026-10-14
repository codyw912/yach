# Extension Reach and Runtimes

Date: 2026-10-06. Evidence for roadmap discussion (plane:M5), not a design or
plan.

Owner direction, 2026-10-06: the original goal is for extensions to modify
almost anything except an essential, safety-relevant core, as Pi does with a
single TypeScript file. Yach may not reach Pi's breadth, but the goal should
shape what gets built so later extension ideas do not each need core changes.
The advisor and observer work (plane:YACH-21, kata `ceax`) should be designed
as the first users of general primitives.

This record asks two questions: what can Pi extensions do that yach extensions
cannot, and which runtime should carry the gap. Sources: Pi at `69f0be6`
(`packages/coding-agent/src/core/extensions/types.ts`, 2245 lines), OpenCode
plugin contract at `ecc4916b` (`packages/plugin/src/index.ts`), the Claude Code
Mods reference (https://code.claude.com/docs/en/plugins/mods/reference), yach
`main@origin` `3031a214`. `[INFERENCE]` marks unobserved claims.

## Conclusion

The gap is not core access; it is missing protocol primitives. Pi's extension
surface reduces to about eight primitives, and yach's posture spec
(`2026-08-19-extension-first-product-posture-design.md`) already permits all of
them. Its owner decisions (posture:337-348) settle the shape: typed, versioned,
bounded interceptors are in; a generic mutable event bus (Pi's
`on(event, mutate)`) is out; UI goes through protocol descriptors and typed
requests, with "arbitrary client component code" left undecided and a future
client-side extension system reserved for separate design (posture:260-268).
So no posture reversal is needed; the work is the deferred seams. Yach's
process-hosted JSONL protocol can carry them. Pi's one-file ergonomics come from
a first-party SDK over that protocol, not from an embedded JavaScript runtime.
An in-process (Wasm) fast path is warranted only if measurement shows a
synchronous hook is too slow over IPC.

## Where yach stands

The host protocol has four core-to-host messages (`extension.initialize`,
`tool.invoke`, `resource.result`, `review.assess`;
`crates/yach-backend/src/extension.rs:331-355`) and seven host-to-core messages
(`extension.ready`, `tool.register`, `resource.request`, `tool.edit_proposal`,
`tool.result`, `review.ready`, `review.result`; `extension.rs:1726-1764`).
Hosts activate only on a command or after first paint (`extension.rs:62-65`).
Static context comes from package files. Capability declarations gate
activation but are not enforced: the host is an ordinary subprocess with the
agent's privileges (`crates/yach-backend/src/extension_capability.rs:1-7`).

The client protocol already negotiates notifications and widgets and carries
`StatusUpdated`, `NotificationRaised` and `WidgetUpdated`
(`crates/yach-proto/src/lib.rs:27-29, 993, 1073-1074`). Extensions have no path
to emit them.

## Pi's surface against yach

Pi exposes 41 typed `on(...)` events and a set of registration and action
methods (`types.ts`; e.g. `tool_call`/`tool_result` at 1609-1610, `context` at
1573, `turn_end` at 1600, `registerTool` 1619, `registerCommand` 1628,
`registerShortcut` 1631, `registerFlag` 1640, `sendMessage`/`sendUserMessage`
1676-1686, `appendEntry` 1692, `exec` 1708, `registerProvider` ~1766,
`registerVirtualModel` 1856; UI `setStatus`/`setWidget`/`setFooter` 166-201).

| Capability | Pi | OpenCode / Claude Mods | Yach today | Posture stance |
|---|---|---|---|---|
| Tools, tool replacement | `registerTool`, `ctx.executeTool` | `tool` hook; `$.tool.register` | Exists: manifest tools, replacement bundles | Allowed; replacement is policy-bound |
| Tool call veto / arg patch, result transform | `tool_call`, `tool_result` | `tool.execute.before/after`, `permission.ask`; `tool.call`, `tool.check` | Absent (core policy and Jev review only) | Typed interceptors decided (owner decision 2) |
| Prompt, system and context transforms | `before_agent_start`, `context` | `chat.message`, `experimental.chat.*.transform`; `prompt.compose` | Static package context only | Candidate interceptor phase |
| Mid-session message or context injection | `sendMessage`, `sendUserMessage` | `$.prompt.submit`, `$.session.send` | Absent | Reachable, unspecified |
| Lifecycle observation | `turn_end`, `agent_end`, message and tool events | generic `event`; `turn.complete`, `session.*` | Absent | Candidate phase ("observational lifecycle events") |
| Commands, shortcuts, flags | `registerCommand/Shortcut/Flag` | `command.execute.before`; `$.command.register` | Absent (commands only as activation triggers) | Additive contribution, allowed |
| Extension state and session entries | `appendEntry`, `pi.events` bus | `$.state`, `$.store` | Absent | Canonical state stays core; namespaced state reachable |
| Status, widgets, notices | `setStatus`, `setWidget`, `setFooter` | none (`tui?: never`, `index.ts:79`); render sites | Client protocol has them; no extension path | Protocol descriptors decided (owner decision 3) |
| Custom TUI components and renderers | in-process callbacks | OpenCode excludes; Claude render trees | Absent | Arbitrary component code "remains undecided" |
| Providers, virtual models | `registerProvider`, `registerVirtualModel` | `auth`, `provider`; `$.model.*` | Absent | Deferred, reachable |
| Compaction | `session_before_compact` | `experimental.session.compacting`; `session.compact` | Absent | Deferred; core keeps cut, checkpoint, replay |
| Subagents | example extensions | `$.agent.spawn` | Absent | Deferred, reachable |
| Themes | `setTheme`, theme resources | core config | Theming spec: declarative contribution | Additive, specified |
| Shell, files, network | `exec`, full user authority | Bun `$`; `$.fs`, `$.process`, `$.http` | Brokered tools/resources; host unsandboxed | Broker; never claim sandbox |

## Eight primitives

Most rows above reduce to:

1. **Lifecycle subscription** (read-only): typed turn, message, tool and session
   events with sequence IDs, bounded payloads, declared subscriptions, and a
   drop or backpressure policy. No catch-all mutator (posture:205).
2. **Named typed transforms**: tool arguments and results, prompt and context
   sections, provider metadata, compaction context. Each phase defines order,
   deadline, validation, provenance, failure mode and replay (posture:191-207).
3. **Context and message injection**: the extension asks core to queue text for
   the main agent, attributed to the extension rather than forged as user
   speech; core owns queueing and context accounting.
4. **Contribution registry**: commands, key actions, providers and models,
   subagent roles, themes, status descriptors; schema and conflict rules per
   kind.
5. **Brokered operations**: tools and resources today; deliberately extended to
   files, processes, model calls and user prompts, each validated by core.
6. **Extension-scoped state**: namespaced key-value and append-only typed
   session entries, persisted and replayed with the session; never direct edits
   to canonical state.
7. **UI descriptors and requests**: status, widgets, transcript rows, notices,
   dialogs; clients render and answer by request ID.
8. **Veto and decision requests**: pre-execution tool and permission decisions
   with a strict deadline; core keeps the final, fail-closed authority. The
   Jev reviewer (`review.assess`/`review.result`) is the existing precedent.

### Advisors and observers as compositions

- **Pi-style observer (writes to the user):** lifecycle subscription
  (`turn_end`) + bounded read-only session excerpt + a model call (brokered, or
  the extension's own) + UI notice descriptor + extension state for "already
  shown" and persistence.
- **omp-style advisor (writes to the agent):** the same, with context injection
  instead of (or as well as) a notice. Whether an injected note can start a
  turn is a product choice the protocol field should make explicit
  `[INFERENCE]`.

Neither needs arbitrary TUI code or write access to canonical state.

## Hard cases for a process boundary

- **Per-token stream callbacks** (Pi `provider_stream_event`): a JSONL round
  trip per token is costly. Expose coalesced notifications or turn boundaries
  instead.
- **Per-frame rendering and arbitrary components**: needs render-frequency
  synchronous calls and ratatui types. OpenCode excludes TUI from plugins
  (`tui?: never`). If descriptors prove insufficient, a client-side extension
  tier is a separate architecture, not callbacks over JSONL.
- **Synchronous veto**: feasible over IPC with a deadline and fail-closed
  default, as the reviewer already does. Measure before adding a fast path.
- **Providers with streaming and auth**: possible out of process but large;
  keep catalog contribution separate from transport and credential custody.

## Runtime options

| Model | One-file ergonomics | Sync hook latency | Isolation | Cost to yach |
|---|---|---|---|---|
| Process + JSONL (today) | Good with an SDK and launcher; any language | IPC per call; fine for low-frequency hooks `[INFERENCE]` | Crash containment; OS authority unless separately sandboxed | Lowest; protocol stays headless and language-neutral |
| Wasm components (Zed, Extism) | Weak: Rust or compiled PDK, WIT, build step (https://zed.dev/docs/extensions/developing-extensions) | No IPC; component boundary only | Only imported host functions; traps contained; limits need fuel/epochs | New ABI to version; toolchain burden on authors |
| Embedded script (Lua via mlua, Rhai, QuickJS/Boa/deno_core) | Best single file | Direct calls; long loops can stall the backend | Same address space; API-limited, not sandboxed | Runtime, async and security upkeep; JS/TS needs transpile and modules, not only an evaluator |
| Hybrid | Declarative data + process + optional Wasm | Fast path where needed | Tiered, must be labeled honestly | More systems to document |

Precedents: LSP and MCP (process protocols with broad ecosystems), Neovim remote
plugins (process hosts over MessagePack-RPC), Zed (Wasm, Rust-first), WezTerm
and Neovim (Lua), Pi (in-process TypeScript via jiti with full user authority;
its separate QuickJS-Wasm `codemode` sandbox is for model-written code, not
extensions).

## Recommendations

1. **Keep process-hosted JSONL as the one extension contract.** It already
   matches the headless boundary and works for every client.
2. **Grow the protocol by primitives, not features.** Start with the
   low-frequency ones the observer and advisor need: lifecycle subscription,
   UI notice descriptor, extension-scoped state, context injection, and
   usage accounting for extension model calls. Then commands, tool-call veto
   and tool-result transform. Each still gets a focused design per posture
   sequencing (posture:329-335).
3. **Ship a TypeScript SDK and `yach extension init`** that make a one-file
   extension (`registerTool`, `on("turn_end", ...)`) feel like Pi while speaking
   JSONL underneath. Python later if wanted. The SDK cannot add missing hooks;
   it only removes framing boilerplate.
4. **Measure before any fast path.** Benchmark veto and prompt-transform round
   trips with realistic extensions. Only if latency is material, add a narrow
   Wasm hook API for those phases.
5. **Owner choices within the posture** (none reverses it):
   - How rich the UI descriptor vocabulary should become (panels, transcript
     renderers, selection and input, not just status and notices) before the
     reserved client-side extension system (posture:267-268) is designed.
   - Read "deferred" surfaces (providers, subagents, roles, compaction, status)
     as sequencing toward the "almost anything" goal, not permanent core
     ownership; the posture's sequencing rule (posture:329-335) already allows
     this.
   - Process hosting is not sandboxing (posture:274-278). The "safe core" goal
     needs brokered operations and, eventually, real isolation, not more
     manifest toggles.

## Sources

- Pi: https://github.com/earendil-works/pi/blob/69f0be6/packages/coding-agent/src/core/extensions/types.ts ,
  https://github.com/earendil-works/pi/blob/69f0be6/packages/coding-agent/docs/extensions.md
- OpenCode: https://github.com/anomalyco/opencode/blob/ecc4916b5a9608c30e6dd58a67f2137b594407ca/packages/plugin/src/index.ts
- Claude Code Mods: https://code.claude.com/docs/en/plugins/mods/reference
- Runtimes: https://microsoft.github.io/language-server-protocol/ ,
  https://zed.dev/docs/extensions/developing-extensions ,
  https://extism.org/docs/concepts/pdk/ , https://wezterm.org/config/plugins.html ,
  https://rhai.rs/ , https://github.com/DelSkayn/rquickjs ,
  https://github.com/boa-dev/boa , https://docs.rs/deno_core/latest/deno_core/
- Yach: posture spec, capability contract spec
  (`2026-09-16-extension-capability-contract-design.md`), side-observer research
  (`2026-10-06-side-observers-research.md`, branch `docs-side-observers`).
