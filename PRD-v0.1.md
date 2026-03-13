yach PRD v0.1

Product name: yach
Meaning: Yet Another Coding Harness
Status: Draft

1. Thesis

yach starts as a Pi-shaped Rust shell and only becomes a native Rust runtime after that shell is validated. That sequence matches Pi's actual seams: Pi already supports custom interfaces through both an SDK and RPC mode; its ResourceLoader discovers extensions, skills, prompts, themes, and context files; settings merge global and project scopes; packages bundle resources through a pi manifest or conventional directories; and sessions are tree-based with in-place branching via SessionManager.

This only makes sense if yach wins on tail-latency and architecture, not on model latency. Pi's existing TUI already does differential rendering, synchronized output, and bracketed paste handling, so the bar is "feels smoother under stress" and "cleaner boundaries for future work," not "Rust automatically makes everything faster."

2. Goals

- Build an extremely responsive Rust TUI on top of the existing Pi backend.
- Preserve Pi's file-based hackability in phase 1: same settings, packages, skills, prompts, themes, and session semantics.
- Preserve a useful subset of the Pi extension ecosystem in phase 1.
- Design phase 2 so the native Rust backend still has a minimal core + infinite customization feel.
- Use the project itself to learn agent runtime, protocol, session, tool, and extension design.

3. Non-goals

- Be meaningfully faster than Pi on model or tool execution in phase 1.
- Preserve full TypeScript extension compatibility in the native Rust backend.
- Rebuild every Pi feature before validating whether the Rust shell is actually better.
- Make MCP the only extension mechanism.

4. Product principles

- Minimal core, maximal customization.
- File-first configuration and resources.
- Tail latency matters more than peak throughput.
- Process boundaries are good.
- Compatibility must be measured, not hand-waved.
- Native Rust plugins must not become the only way to customize the harness.

5. Architecture

5.1 High-level shape

yach will have three layers:
- yach-ui: fullscreen Rust TUI
- yach-proto: Yach-owned protocol and capability model
- backend adapters
- yach-adapter-pi-rpc
- yach-adapter-pi-sdk
- later, native Rust backend components

The Rust UI must never speak Pi RPC directly. It should only speak yach-proto.

5.2 Why a Yach-owned protocol

Pi RPC already gives us a useful base: strict LF-delimited JSONL over stdin/stdout, async prompt streaming, dialogs like select/confirm/input/editor, plus fire-and-forget UI calls like notify, setStatus, setWidget, setTitle, and set_editor_text. But RPC also has explicit gaps today: custom() returns undefined; setWorkingMessage(), setFooter(), setHeader(), and setEditorComponent() are no-ops; theme APIs are unavailable; and widget component factories are ignored in RPC mode. The SDK is the fallback because it exposes ResourceLoader and SessionManager directly for custom interfaces.

So the phase-1 adapter strategy is:
- Adapter v1: stock pi --mode rpc
- Adapter v2: thin Node sidecar built on Pi SDK
- UI contract: yach-proto, with capability negotiation

5.3 Initial crate layout

Inspired by the codex-rs split between core, exec, tui, cli, and its app-server boundary, yach should start as a Cargo workspace with separate crates for UI, protocol, adapters, and benchmarking.

Proposed initial crates:
- yach-cli
- yach-ui
- yach-proto
- yach-adapter-pi-rpc
- yach-adapter-pi-sdk
- yach-bench

Later crates:
- yach-core
- yach-providers
- yach-session
- yach-tools
- yach-plugins
- yach-mcp

6. Compatibility contract for phase 1

6.1 Settings, packages, and resources

Phase 1 must preserve Pi's file-first resource surface exactly:
- global settings: ~/.pi/agent/settings.json
- project settings: .pi/settings.json
- global extensions: ~/.pi/agent/extensions/
- project extensions: .pi/extensions/
- package/resource discovery through packages, local resource paths, and package.json pi manifests
- package sources from npm/git/local paths
- same skills, prompts, themes, and extension discovery rules
- same /reload-style local extension workflow where possible

Pi already documents global/project settings precedence, resource source fields, package sources, and package manifests/directories. Extensions in the standard auto-discovered locations are hot-reloadable with /reload, which is part of the ergonomics worth keeping.

Requirement: if a user points yach at an existing Pi setup, resource-only customization should work unchanged.

6.2 Session compatibility

Phase 1 must preserve session behavior, not just transcript display.

Pi sessions are tree-based; /tree navigates history by id/parentId; switching branches can summarize the path being abandoned; and SessionManager already supports tree traversal, labels, branching, file-backed sessions, and opening persisted session files.

Requirement: yach phase 1 must be able to:
- open existing Pi session files
- continue recent sessions
- fork and branch
- browse the tree
- surface branch summaries and compaction state

6.3 Extension parity definition

"Extension compatibility" means four different things:
1. Discovery/package parity
Same settings, paths, package manifests, resource loading.
2. Logic parity
Lifecycle events, tools, commands, providers, input transforms, session hooks, provider hooks.
3. UI parity
Dialogs, notifications, status, widgets, title, footer/header/editor replacement, custom overlays, custom renderers, theme access.
4. Session parity
appendEntry() persistence, custom entry behavior, branch/session lifecycle behavior.

Pi extensions are TypeScript modules that can subscribe to lifecycle events, register tools and providers, add commands, persist state via appendEntry(), and control rendering. The documented examples explicitly cover stateful tools, dynamic tools, provider payload hooks, footer/header replacement, custom editors, widgets, overlays, and dialog-driven custom tools.

6.4 Parity tiers

Tier A: stock RPC parity target

Must work over pi --mode rpc:
- prompt streaming
- dialogs: select, confirm, input, editor
- notifications
- status entries
- widgets
- title changes
- editor text updates
- session switching/forking/stats/export

Tier B: rich parity target

Requires SDK sidecar or richer remote UI protocol:
- ui.custom()
- header/footer replacement
- editor component replacement
- theme inspection and setting
- component-backed widgets
- richer overlay surfaces
- custom message/tool renderers

Stock RPC is enough for Tier A and explicitly incomplete for Tier B.

6.5 Canonical compatibility suite

Use Pi's documented example extensions as the first real compatibility suite.

Logic suite
- hello.ts
- question.ts
- todo.ts
- dynamic-tools.ts
- provider-payload.ts

Rich UI suite
- questionnaire.ts
- custom-footer.ts
- custom-header.ts
- modal-editor.ts
- overlay-test.ts

7. Phase plan

7.1 Phase 1: Pi-shaped Rust shell

Ship a Rust TUI with Pi backend compatibility.

Must-have
- fullscreen TUI
- streaming transcript
- tool output panes
- input composer
- slash command completion
- model selector
- thinking level control
- session picker
- session fork
- tree navigation
- branch summary visibility
- compaction visibility
- theme loading
- settings/package/resource loading compatibility
- benchmark harness
- capability negotiation between UI and adapter

Explicitly not in scope
- native provider drivers
- native auth flows
- native plugin host
- IDE integration
- web UI

7.2 Phase 1.5: rich parity pass

After the basic shell is validated, add the SDK sidecar and remote UI parity needed for Tier B extensions.

Must-have
- custom overlays
- header/footer replacement
- editor replacement
- theme parity
- richer renderer surfaces
- compatibility suite green for the rich UI test set

7.3 Phase 2: native Rust backend

Only starts once phase 1 is proven worthwhile.

Recommended replacement order:
1. yach-session
2. yach-providers
3. yach-auth
4. yach-core agent loop
5. yach-tools
6. yach-plugins
7. optional native approvals/sandbox refinements

That order keeps the Rust UI stable while the backend is swapped piece by piece.

8. Extensibility model

8.1 Phase 1

Keep Pi's TypeScript extension runtime. Don't fight the ecosystem yet.

8.2 Phase 2

Keep resource compatibility exact where possible:
- skills
- prompts
- themes
- context files
- settings shape where it makes sense

But do not fake Node-package semantics for native plugins.

The phase-2 code plugin model should be:
- out-of-process
- versioned
- stdio-based by default
- restartable instead of hot-linked
- language-agnostic

That preserves hackability much better than "compile a shared library to add a command."

8.3 MCP stance

MCP is a separate lane for external tools and context, not the harness-local plugin system. Codex's MCP docs show the right mental model: stdio and streamable HTTP MCP servers, bearer-token and OAuth support, and shared config across clients. That is excellent for browser control, docs lookup, design tools, and other external servers. It is not enough for harness-local concerns like renderers, keybindings, session hooks, or editor replacement.

Decision:
- MCP support: yes
- MCP as only extension model: no

9. Provider and auth strategy for phase 2

The native backend should separate transport, auth, and optional gateways.

9.1 Transport families

Start with wire families, not vendor-specific snowflakes.

Pi's provider model already spans:
- anthropic-messages
- openai-completions
- openai-responses
- openai-codex-responses
- mistral-conversations
- google-generative-ai
- google-gemini-cli
- google-vertex
- bedrock-converse-stream

It also documents compatibility flags for OpenAI-shaped but quirky providers.

Decision: transport layer in Yach should mirror this approach.

9.2 Auth modules

Target these auth primitives:
- API key
- browser OAuth
- device-code auth
- token refresh
- file-backed credential store
- OS keyring-backed credential store

Pi's provider docs already expose OAuth/device-code style callbacks, and OpenAI's Codex docs show the expected OpenAI-side baseline: ChatGPT sign-in, API key sign-in, device-code fallback, local caching, token refresh, and configurable keyring/file storage.

9.3 First provider/auth targets

For the first native backend milestone, support:
- OpenAI API-compatible endpoints
- OpenAI ChatGPT/Codex-style login
- one generic OpenAI-compatible custom endpoint
- OpenCode-style curated/provider UX
- optional gateway mode

OpenCode is useful here because it documents three flows worth emulating: curated OpenCode Zen, OpenAI with ChatGPT Plus/Pro or manual API key, and a generic "Other" path for arbitrary OpenAI-compatible providers via custom baseURL.

9.4 Optional gateways

Support these as optional, not foundational:
- LiteLLM
- Vercel AI Gateway
- other OpenAI-compatible gateways

LiteLLM is especially worth supporting as a gateway because it presents a unified OpenAI-style interface, supports retries/fallbacks, and offers proxy-side spend/rate features. But it should remain optional.

10. Performance requirements

These are product requirements, not stretch goals.

10.1 Interaction SLOs
- startup to interactive prompt: <250 ms after backend is ready
- p95 keypress-to-paint, idle: <16 ms
- p95 keypress-to-paint, active stream: <32 ms
- p99 keypress-to-paint, heavy tool output: <50 ms
- large paste handling: 0 corruption / 0 accidental submit
- viewport changes on huge transcripts: no full-buffer re-render behavior

10.2 Required implementation behaviors
- transcript virtualization
- tool output virtualization
- bounded queues between backend and renderer
- explicit backpressure handling
- no synchronous paint path on stream append
- separate input and render scheduling
- record/replay bench harness for real sessions

11. Benchmark plan

Benchmark against current Pi on the same machine using:
- long transcript replay
- high-rate token streaming
- huge tool output
- deep session tree navigation
- giant paste bursts
- rapid model/session switching

A Rust shell that does not clearly improve one or more of those cases does not justify phase 2.

12. Milestones

M0 -- bootstrap
- repo created
- Cargo workspace created
- yach-proto v0 spec
- adapter capability model defined
- baseline benchmark harness skeleton

M1 -- stock Pi RPC adapter
- spawn/connect to pi --mode rpc
- stream transcript
- send prompts
- handle Tier A dialogs and fire-and-forget UI
- basic session/model controls

M2 -- TUI alpha
- fullscreen TUI
- transcript + tool panes
- input composer
- slash completion
- model/thinking controls
- session picker
- basic performance instrumentation

M3 -- compatibility beta
- load real Pi settings/resources
- open real Pi sessions
- tree navigation/fork
- canonical logic suite green
- benchmark comparison against Pi

M4 -- rich parity beta
- SDK sidecar adapter
- remote rich UI surfaces
- canonical rich UI suite green

M5 -- validation gate

Proceed to native Rust backend only if:
- phase 1 feels materially better in real use
- compatibility is good enough to matter
- architecture is clearly cleaner, not just different

13. Acceptance criteria

Phase 1 is successful only if all of these are true:
- a Pi user can point yach at an existing Pi setup and reuse settings, skills, prompts, themes, and packages
- yach can open and navigate existing Pi sessions
- the canonical logic suite passes
- the canonical rich UI suite passes after the SDK sidecar exists
- the Rust TUI beats Pi on at least one important tail-latency workload
- phase 1 still feels hackable, not more locked-down than Pi

14. Main risks
- Stock Pi RPC may be too limited for the most interesting extensions.
- The Rust shell may only be modestly better because Pi's TUI is already optimized.
- A native Rust backend could accidentally kill the thing worth preserving: low-friction customization.
- Native plugin design can become overengineered very fast.

15. Open questions
- Which 5-10 real community packages/extensions count as the non-negotiable parity set beyond Pi's documented examples?
- Do we treat Windows as first-class from day one, or as beta until the input/paste/render loop is solid on macOS/Linux?
- Does the SDK sidecar stay a permanent adapter, or only exist until native replacements land?
- Which session file format details do we promise to preserve exactly in phase 2?

My recommendation: freeze this as v0.1, then immediately break M0-M2 into issues.
