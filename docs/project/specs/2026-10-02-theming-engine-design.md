# Theming Engine

**Outcome:** plane:YACH-20 (kata `yach#vadv`)

Status: draft for owner review, 2026-10-02.

Triggered by dogfooding (plane:YACH-19): successful tool results render as a
dark green band with gray text. Owner direction, 2026-10-02: "a cohesive
theming engine, though simple/minimal for now", "customizable and controllable
by extensions", light terminals and `NO_COLOR` included, and "some visual
separation, maybe more similar to omp and opencode for now".

Claims about current yach behavior cite `path:line` on `main` as of this
date. Claims about other projects cite a URL and were read on 2026-10-02.
`[INFERENCE]` marks anything not observed in code, docs, or a running program.

## Problem

1. **Defaults produce the reported look.** The built-in theme is fixed
   `pi_dark` (`crates/yach-ui/src/theme.rs:52-86`). Its
   `tool_success_background` is `Rgb(40,50,40)` (`theme.rs:68`) and its
   `tool_output` is `Rgb(128,128,128)` (`theme.rs:71`). Result rows use both
   (`transcript.rs:991-1004`, `1181-1191`). The owner's complaint is the
   default, working as designed. Pi 1.0's own dark theme ships the same shape;
   see Cohort evidence.
2. **Colors are tokens, not roles.** 22 color tokens plus 5 spacing tokens
   exist (`theme.rs:16-48`), and widgets read `theme.colors.X` directly and
   rebuild `Style::new().fg(..).bg(..)` at each call site: `transcript.rs`,
   `app.rs`, and 10 widget files (`diag-ThemeScout` inventory). Nothing
   guarantees a fg is paired with a legible bg, and nothing can change how a
   role *looks* (bold, dim, glyph) as opposed to its color.
3. **No light terminal story and no `NO_COLOR`.** A search of `crates/` finds
   no `NO_COLOR`, `COLORFGBG`, `COLORTERM`, or background detection. The
   default hardcodes `Color::White` text and dark RGB surfaces
   (`theme.rs:55-76`), which is unreadable on a light terminal.
4. **One theme file, no sources.** Resolution is `YACH_THEME` path, then
   `<project>/.yach/theme.json`, then `~/.yach/theme.json`, first found wins,
   no merging (`crates/yach-cli/src/main.rs:425-450`). Extensions cannot
   contribute a theme, although the posture spec lists themes as a future
   additive contribution (`2026-08-19-extension-first-product-posture-design.md:168-173`)
   and the original install design reserved a `themes/` directory
   (`2026-05-20-extension-runtime-tool-replacement-design.md:159-165`).
5. **`Capability::ThemeLoading` has no consumer.** It appears only in
   handshake plumbing (`crates/yach-proto/src/lib.rs:32,1146`;
   `crates/yach-ui/src/lib.rs:34,43,51`). Nothing negotiates or sends a theme.

## Non-goals

- No in-TUI theme picker, editor, or live reload (kept from
  `2026-08-19-wave3-tui-visual-design.md:112`). A theme is fixed per process;
  see Rendering model for why this also holds for extensions.
- No new color syntaxes (OKLCH, OKHSL, variables with math). Hex, ANSI names,
  indices 0-255, `default`, and `vars` stay as they are (`theme.rs:244-268`).
- No 24-bit to 256/16-color downgrading. Presets use `Rgb`; the `system`
  preset uses only terminal-relative colors and is the answer for limited
  terminals. See Open questions.
- No syntax-highlight or markdown token families. Markdown theming belongs to
  the markdown work (plane:YACH-19) and consumes the roles defined here.
- No runtime theme protocol between extension hosts and the UI in this slice.
  The seam is described under Deferred.
- No change to approval, trust, capability, or review semantics.
- No project-scoped themes. Owner decision, 2026-10-02: "i don't know that
  anyone wants that. More complexity for not much benefit." Themes are a
  user preference; see Theme sources and precedence.

## Cohort evidence

### Pi 1.0 (primary inspiration; shipped 2026-10-01)

- Announcement lists "A new TUI theme" and "Full-screen mode by default":
  https://earendil.com/posts/pi-1-0/. `tuiMode` is `"regular" | "fullscreen"`,
  default `"fullscreen"`: https://pi.dev/docs/latest/settings.
- **Definition.** JSON file with `name`, optional `appearance`
  (`dark`/`light`), `vars`, and `colors`; the schema marks 51 roles required
  (`theme-schema.json`, `colors.required`). Colors may be hex, OKLCH, OKHSL, a
  256 index, a variable, or `""` for the terminal default:
  https://pi.dev/docs/latest/themes.
- **Semantic roles, not components.** "Theme colors describe interface roles
  rather than individual components"; groups include `toolPendingBg`,
  `toolSuccessBg`, `toolErrorBg`, `toolTitle`, `toolOutput`, `userMessage*`,
  `selectedBg`, `md*`, `toolDiff*`, `syntax*` (same page). Components apply
  them through `theme.fg()`, `theme.bg()`, and `theme.style({ fg, bg, bold })`:
  https://pi.dev/docs/latest/tui ("Apply themes correctly").
- **Light/dark.** Built-ins are `system` (default), `dark`, `light`. The
  setting can be one name or `"light/dark"`. Detection order: terminal-reported
  background and foreground, then the terminal's light/dark notification, then
  `COLORFGBG`, then dark. `system` queries the terminal's default colors and
  16 ANSI colors with a 100 ms startup cap, keeps body text at 4.5:1 contrast,
  and falls back to ANSI indices and terminal defaults with **no panel
  backgrounds** when the terminal reports nothing:
  https://pi.dev/docs/latest/themes.
- **Extension control.** Extensions receive the active theme in render
  callbacks. `ExtensionUIContext` exposes `getAllThemes()`, `getTheme(name)`,
  and `setTheme(name | Theme)` (`packages/coding-agent/src/core/extensions/types.ts:284-293`,
  https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/extensions/types.ts).
  Themes also ship in Pi packages; project themes "load only after project
  trust is granted": https://pi.dev/docs/latest/themes ("Load a theme from a
  project or package"). Extensions run in-process, so this is runtime control
  by a trusted module.
- **Tool separation.** Each tool is a padded `Box(1, 1, bg)` full-width card
  tinted by state (`toolPendingBg` / `toolSuccessBg` / `toolErrorBg`):
  `packages/coding-agent/src/modes/interactive/components/tool-execution.ts:105,309-312`
  (https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/modes/interactive/components/tool-execution.ts).
  Built-in dark values: pending `okhsl(229 5% 24%)`, **success
  `okhsl(158 46% 25%)`** (a saturated green wash), error `okhsl(19 54% 25%)`,
  `toolOutput: muted` (`dark.json:37-41`); light success is
  `okhsl(156 21% 91%)` (`light.json:38`):
  https://github.com/earendil-works/pi/tree/main/packages/coding-agent/src/modes/interactive/theme.
  No border, rule, or header bar: separation is background only.
  Yach's complaint therefore reproduces Pi's own default dark theme.
- **Full-screen implication.** Pi 1.0 owns the viewport in fullscreen mode,
  scrolls its own transcript, and rebuilds the theme when the terminal's
  appearance changes. Docs warn that "a theme change ... cannot remove old
  ANSI colors embedded in application state" (https://pi.dev/docs/latest/tui).
  In regular mode "the terminal owns scrollback" (same page, "Handle mouse input").

### omp (oh-my-pi)

- Same token names as Pi (`toolPendingBg`, `toolSuccessBg`, `toolErrorBg`,
  `toolTitle`, `toolOutput`; 7 background roles). Auto light/dark order:
  OSC 11 luminance, then `COLORFGBG` (background index `< 8` dark), then a
  macOS fallback, then dark: https://github.com/can1357/oh-my-pi/blob/main/docs/theme.md.
- Default `titanium` theme sets `toolPendingBg` and `toolSuccessBg` to the
  **same neutral** `darkTitanium` and only `toolErrorBg` to a near-black red
  `#1a0f10`; `toolTitle` is `""` (terminal default) and `toolOutput` a dim
  aluminum: `packages/tui/src/theme/defaults/titanium.json:35-39`
  (https://github.com/can1357/oh-my-pi/blob/main/packages/tui/src/theme/defaults/titanium.json).
  Success is not green.
- Tools that draw their own frame use a bordered block: rounded corners, an
  optional header in the top border, and a **state-colored border** (running
  and pending `accent`, success `dim`, warning `warning`, error `error`) with
  an optional state-tinted background (`getStateBgColor`, three states):
  `packages/tui/src/render/output-block.ts:76-103`,
  `packages/tui/src/render/utils.ts:105-109`
  (https://github.com/can1357/oh-my-pi/blob/main/packages/tui/src/render/output-block.ts).
  Other tools (generic and extension renderers) get the padded tinted card:
  `packages/coding-agent/src/modes/components/tool-execution.ts:1073-1080`
  (https://cdn.jsdelivr.net/npm/@oh-my-pi/pi-coding-agent@17.4.0/src/modes/components/tool-execution.ts).

### opencode

- Named semantic tokens including `background`, `backgroundPanel`,
  `backgroundElement`, `text`, `textMuted`, `error`; `system` theme "uses
  `none` for text and background colors", derives grays from the terminal
  background, and uses ANSI colors; theme directories layer built-in, user
  config, project root, then cwd (later wins): https://opencode.ai/docs/themes/.
- Two tool shapes (`packages/tui/src/routes/session/index.tsx` on `dev`,
  https://github.com/anomalyco/opencode/blob/dev/packages/tui/src/routes/session/index.tsx):
  - `InlineTool` (`:1914-1960`): no background, 3-column indent, an icon plus
    muted text; failure turns the row `error`; a denied tool is struck
    through; one blank line is inserted only after a multi-row sibling.
  - `BlockTool` (`:1994-2020`): `backgroundPanel` background, padding 1 top,
    bottom, and 2 left, `marginTop` 1, a **left rule** `┃` drawn through
    `SplitBorder` (`packages/tui/src/ui/border.ts`), errors appended in
    `theme.error`.
- The rule's color is a known pain point: it was drawn in `theme.background`
  and rendered as a black strip on transparent themes
  (https://github.com/anomalyco/opencode/issues/27590). Lesson: any glyph on a
  panel must take its color from the panel pairing, never from "no color".

### Others (from `diag-ThemeScout`)

- Codex style guide: terminal-default foreground for most text, ANSI colors for
  status, no arbitrary foregrounds over painted backgrounds:
  https://github.com/openai/codex/blob/main/codex-rs/tui/styles.md.
- Claude Code: tokenized presets with dark, light, and ANSI bases and an auto
  light/dark mode: https://code.claude.com/docs/en/terminal-config.
- Helix: palettes default to the terminal's 16 colors; themes inherit:
  https://docs.helix-editor.com/themes.html.

### What the cohort establishes

- All of Pi, omp, and opencode converge on a **role-named JSON token set** with
  separate light/dark handling and a `system`/terminal-default option. Yach's
  existing token file already matches that shape and keeps it.
- Pi/omp separate tool calls with a **state-keyed background band**.
  opencode separates with a **left rule and a panel**. omp additionally colors
  a border by state. Only the rule and the border survive when no background
  is painted (`system`, `NO_COLOR`); Pi's background-only separation does not.
  Pi's own `system` theme drops panel backgrounds when it cannot learn the
  terminal's colors (https://pi.dev/docs/latest/themes), which leaves Pi tool
  rows unseparated in that case.
- Extension control in Pi and omp is in-process runtime API. Yach's extensions
  are out-of-process and capability-declared, so the cohort's runtime
  `setTheme` does not transfer directly; see Extension control.

## Rendering model constraint (inline viewport)

Yach renders into a ratatui inline viewport and archives finished rows into
native scrollback with `insert_before` (`crates/yach-ui/src/app.rs:4238-4241`,
`4299-4302`). Consequences, all affecting theming:

1. **Rows in scrollback are immutable.** Their SGR colors were emitted when
   archived. A runtime theme switch (user or extension) would restyle only the
   live viewport and leave history in the old theme. This, plus
   `2026-08-19-wave3-tui-visual-design.md:112`, is why the theme is resolved
   once at startup and why runtime control is deferred.
2. **Yach never paints the page background.** Only explicit surfaces (user
   message, tool panel, selection) carry a `bg`. Everything else sits on the
   terminal's own background. So `text` may legitimately be the terminal
   default foreground in any preset, which is what makes `system` viable.
   Pi fullscreen also needs no page-background role in its required schema
   (`theme-schema.json` required list has none for the TUI), but it can
   re-query and rebuild on appearance change; yach cannot repaint scrollback.
3. **Pairing rule.** Any text drawn on a painted `bg` must set an explicit
   `fg`, never `Color::Reset`: a Reset foreground on a dark panel in a light
   terminal is dark-on-dark. Presets that paint surfaces (`dark`, `light`)
   therefore carry an explicit fg for every surface role. `system` paints
   nothing, so Reset fg is safe there. This invariant is testable (see
   Testing).
4. **Painted bands are space-padded cells** (`render_tool_surface` fills each
   row with spaces up to the width, `transcript.rs:1136-1179`).
   [INFERENCE] After a terminal resize, archived padded rows reflow in the
   terminal's own scrollback and can show ragged band edges. A left rule plus
   no-background degrades more gracefully. Fullscreen repaint would not have
   this problem; whether yach should move to fullscreen is outside this spec.
5. **OSC 11 detection costs startup latency and stdin ownership.**
   [INFERENCE] Yach starts a `crossterm::event::EventStream` on stdin
   (`app.rs:4246`); a terminal's OSC reply would arrive as stray input unless
   read first, and a terminal that does not answer needs a timeout (Pi uses
   100 ms). See Open questions.

## Design

### Semantic role set

Roles are what widgets ask for. Each maps to existing JSON tokens, so **no new
color token is added**.

| Role | Used for | Token(s) |
|---|---|---|
| `text` | assistant prose, default body | `text` |
| `muted` | metadata, tool summaries, hints | `muted` |
| `dim` | rails, disabled, unselected options | `dim` |
| `accent` | primary interaction, focused title/border | `accent` |
| `border` | composer and dialog frames | `border` |
| `success` / `warning` / `error` | state glyphs and labels | `success`, `warning`, `error` |
| `harness` | harness-authored outcomes | `harness` |
| `selection` | selected row | `selectedBackground`, `selectedText` |
| `user_message` | user surface (bg+fg pair) | `userMessageBackground`, `userMessageText` |
| `tool_surface(state)` | tool panel bg+fg pair | `toolPendingBackground`, `toolSuccessBackground`, `toolErrorBackground`, `text` |
| `tool_title` / `tool_output` | header name; output body | `toolTitle`, `toolOutput` |
| `diff(kind)` | added, removed, context, hunk | `diffAdded`, `diffRemoved`, `diffContext`, `diffHunk` |

`ToolState` is `Pending`, `Success`, `Error`, or `Harness`. `Harness` is the
existing refined-outcome case that today uses the error surface
(`transcript.rs:1181-1191`). The rule color per state is derived, not
configured: `Pending` uses `warning`, `Success` uses `dim`, `Error` uses
`error`, `Harness` uses `harness`.

### Style helper API (`theme.rs`)

Widgets stop reading `theme.colors.*`. The `ThemeColors` fields become private
to `theme.rs`, so a sibling module that bypasses a helper no longer compiles.

```rust
pub enum ToolState { Pending, Success, Error, Harness }
pub enum DiffKind { Added, Removed, Context, Hunk }

impl Theme {
    // text roles
    pub fn text(&self) -> Style;
    pub fn muted(&self) -> Style;
    pub fn dim(&self) -> Style;
    pub fn accent(&self) -> Style;
    pub fn border(&self, focused: bool) -> Style;
    pub fn success(&self) -> Style;
    pub fn warning(&self) -> Style;
    pub fn error(&self) -> Style;
    pub fn harness(&self) -> Style;

    // surfaces (each returns a fg+bg pair)
    pub fn selected(&self) -> Style;
    pub fn user_message(&self) -> Style;
    pub fn tool_surface(&self, state: ToolState) -> Style;

    // tool row parts
    pub fn tool_rule(&self, state: ToolState) -> Style;   // the `┃` gutter
    pub fn tool_title(&self) -> Style;
    pub fn tool_output(&self, state: ToolState) -> Style;
    pub fn diff(&self, kind: DiffKind) -> Style;
}
```

Rules:

- A helper sets only what its role needs and always returns a complete
  fg+bg pair for surfaces.
- Helpers own **non-color emphasis** too: `tool_title` is bold, `selected` is
  reversed in monochrome. This is the one place where `NO_COLOR` behavior is
  decided (next sections), so no widget needs a `NO_COLOR` branch.
- Layout (padding, gap, rule width) stays in `ThemeSpacing` and the
  renderers. The theme module owns no widget state or layout.

### Theme sources and precedence

A theme is data: the existing strict JSON schema (`deny_unknown_fields`,
`theme.rs:192-201`) with one added optional top-level key:

```json
{ "base": "dark" | "light" | "system" | "auto" | "<extension-id>:<theme-id>",
  "vars": { }, "colors": { }, "spacing": { } }
```

`colors` entries override tokens of the `base`. `base` defaults to `dark`, so
every existing theme file keeps its meaning (`from_json` today always starts
from the dark default, `theme.rs:88-97`). A file wanting detection writes
`"base": "auto"`. An extension theme's own `base` may name only a built-in
preset, never another extension theme; this removes cycles.

Built-in presets:

| Preset | Surfaces | Notes |
|---|---|---|
| `dark` | painted | Today's `pi_dark` with the tool treatment below |
| `light` | painted | Panels slightly darker than white, explicit dark fg on every surface |
| `system` | none (`Reset`) | `Reset` fg/bg; ANSI colors for status; rules and glyphs separate rows |
| `auto` | per resolved | Resolves to `dark` or `light` (see Light/dark) |

Selection is a single winner, as today. Highest first:

1. `YACH_THEME`. If its value is exactly `dark`, `light`, `system`, or `auto`
   it names a preset; otherwise it is a file path, as today
   (`main.rs:426,439-441`). Those four names are therefore reserved.
2. User `~/.yach/theme.json`.
3. Default: `auto`.

`NO_COLOR` is applied after the winner is chosen and overrides it (see
`NO_COLOR` semantics). No file merges with another file; layering happens only
through `base`. The existing resolver `tui_theme_path`
(`main.rs:434-450`) becomes one pure `resolve_theme(env, home,
installed_extensions) -> (Theme, Vec<Diagnostic>)` function so the whole order
is unit-testable and the TUI path (`main.rs:3762-3772`) stays one call. The
resolver never sees the project root; the legacy-file notice below comes from
the TUI startup path, which already has it.

### Project themes removed

Themes are a user preference, so the project lookup
(`<project>/.yach/theme.json`, `main.rs:425-450`) is removed outright rather
than gated behind a future project trust. That removes a precedence row, the
question of whether repository content may restyle approval or diff text, and
a trust dependency.

Behavior change: an existing `<project>/.yach/theme.json` stops applying. When
that file exists, the TUI startup path (not the resolver) shows one
diagnostic: "project themes are not supported; move it to
`~/.yach/theme.json` or set `YACH_THEME`". The file is never deleted or
rewritten. A user who wants a per-project look can set `YACH_THEME` in that
project's environment (for example via direnv).

### Extension control

Extensions can **contribute** themes declaratively. Runtime control is
deferred. The five requirements the posture spec sets for any new additive
surface (`2026-08-19-extension-first-product-posture-design.md:171-173`):

**Schema.** A new manifest contribution, parallel to `static_context`
(`extension.rs:1662-1673`, `1706-1721`):

```json
"contributes": {
  "themes": [{
    "id": "night",
    "title": "Night",
    "source": { "type": "extension_file", "path": "themes/night.json" },
    "max_bytes": 16384
  }]
}
```

The theme file is the same JSON schema as a user file. `id` and `path` reuse
the existing static-context validators (`extension.rs:3154-3170`: relative
path, no escape from the package root). The file is parsed by the strict theme
parser; it is data and never executes. Manifest structs use
`deny_unknown_fields` (`extension.rs:1662-1665`), so an older yach rejects a
manifest that declares `themes`; that is the intended forward-compat failure.

**Scope precedence.** User-scope extensions only. A project-scope extension's
themes are not offered, consistent with themes being a user preference and
with project extensions being blocked pending trust (`extension.rs:1021-1028`).
The theme's fully qualified name is `<extension-id>:<theme-id>`.

**Selection (the user's act).** A theme applies only when the user names it as
a `base` (in `~/.yach/theme.json`) or sets `YACH_THEME` to a file whose `base`
names it. Installing an extension never changes the look. This is consistent
with the contract that capability is "stated and consented to"
(`2026-09-16-extension-capability-contract-design.md:48-51`) and with
distribution presets' rule that installing grants nothing
(`2026-09-27-distribution-presets-design.md:141-143`).

**Capability.** None. A theme adds no tool risk, and the capability set is
derived only from tool risks (`2026-09-16-extension-capability-contract-design.md:100-112`),
so a theme-only extension has an empty capability set and needs no grant.
`Capability::ThemeLoading` keeps its present meaning: this UI can load themes.
It is not an extension capability and is not checked per extension.

**Lifecycle.** Manifest-only. No host process starts for a theme, so it never
touches the first-paint-extension-cold rule
(`2026-06-02-extension-activation-manager-design.md:60-70`).
[INFERENCE: whether the cached manifest index available at first paint
contains `contributes`; if not, an extension theme resolves right after first
paint and the UI repaints once at startup. To be measured with the visual
tape; if the flash is visible, the resolved palette can be snapshotted next to
the manifest cache.]

**Conflict resolution.** Fail closed, as with tool names
(`2026-09-27-distribution-presets-design.md:259-265`):

- Names are namespaced by extension id, so two extensions cannot collide.
- An extension theme cannot define `dark`, `light`, `system`, or `auto`; the
  manifest is rejected with a diagnostic.
- A theme overrides roles only within itself; there is no cross-extension
  stacking or ordering.

**Fallback when disabled, removed, or invalid.** The selected name no longer
resolves, so the selection falls back to the user file's tokens over `auto`
(or `auto` alone) and the TUI shows one diagnostic naming the missing theme.
The selection is not edited: re-enabling the extension restores the theme.
Disabling an extension contributes nothing, consistent with disable-not-delete
(`2026-09-27-distribution-presets-design.md:179-180`).

### Deferred: runtime control

A host that wants to retheme at runtime (the Pi `setTheme` model) needs: a
new `RawExtensionHostMessage` variant (the set is closed,
`extension.rs:1723-1725`), a `ServerEvent` to the UI gated by
`ThemeLoading`, and a decision for scrollback (Rendering model, point 1).
The safe shape, when needed: a host *requests* `ui.theme.request { theme }`
for one of its own declared themes, and the user confirms through the existing
dialog capability. Not designed further here; the posture spec's UI boundary
says clients decide how to render negotiated descriptors
(`2026-08-19-extension-first-product-posture-design.md:260-272`), which fits.

### Light and dark selection

- **Explicit wins.** `YACH_THEME=light` or `"base": "light"` is always
  honored. There is no setting that is more reliable than a stated choice.
- **`auto` detection in this slice reads `COLORFGBG` only.** Its value is
  `fg;bg` or `fg;default;bg`. Take the last field as the background index and
  apply omp's rule: index below 8 is dark, otherwise light
  (https://github.com/can1357/oh-my-pi/blob/main/docs/theme.md). Absent or
  unparsable resolves to `dark`.
- **Honesty about reliability.** `COLORFGBG` is set by only some terminals,
  is inherited stale by shells and multiplexers, and does not update when the
  terminal switches appearance. [INFERENCE: from omp ranking it below OSC 11
  and Pi ranking it below terminal-reported colors; yach has not surveyed which
  terminals set it.] A wrong guess is possible, so the one-line escape hatch
  (`YACH_THEME=light|dark|system`) is documented in the same place as
  `auto`, and `system` is the guaranteed-legible choice because it paints no
  surface.
- **Not in this slice:** OSC 10/11 queries and the terminal's appearance
  notification. They are the reliable signals, but they need a read before
  `EventStream` starts, a timeout, and a multiplexer story (Rendering model,
  point 5). See Open questions.

### `NO_COLOR` semantics

Standard: when `NO_COLOR` is "present and not an empty string (regardless of
its value)", software that adds ANSI color by default should not
(https://no-color.org/). The standard is about color, not text attributes.

Yach behavior:

1. `NO_COLOR` set and non-empty selects the **monochrome** theme and
   **overrides every theme source**, including `YACH_THEME` and extension
   themes. (The standard allows user-level config to override `NO_COLOR`; this
   spec takes the simpler rule. See Open questions.)
2. Monochrome sets `Theme.mono = true` and every color to `Color::Reset`.
   Helpers then add attribute emphasis in place of hue:

   | Role | Monochrome rendering |
   |---|---|
   | `muted`, `dim` | `DIM` |
   | `accent`, `tool_title`, `error` | `BOLD` |
   | `selected` | `REVERSED` |
   | `success`, `warning`, `harness` | no change; glyph and label carry meaning |
   | `diff` | no change; `+`/`-`/`@@` prefixes carry meaning |
   | surfaces (user, tool) | no background |

3. Meaning never rides on color alone: glyphs (`✓ ✗ ⚙ ›`) and outcome labels
   already exist (`2026-08-19-wave3-tui-visual-design.md:138`). The tool left
   rule is a glyph (`┃`), so it survives monochrome.
4. `TERM=dumb` or non-TTY output is unchanged by this spec (the TUI already
   requires a TTY).

### Tool row treatment

**Pick:** a neutral panel with a state-colored left rule, i.e. opencode's
`BlockTool` shape with omp's rule that success is not green.

| | Pi 1.0 | omp titanium | opencode | **Yach (proposed)** |
|---|---|---|---|---|
| Separator | state bg band | state bg band, or colored border box | `┃` rule + panel; or bare inline row | **`┃` rule + panel** |
| Success tone | green wash | same neutral as pending | none or neutral panel | **neutral panel, same as pending** |
| Error tone | red wash | near-black red | `error` text | **red-tinted panel + `error` rule** |
| Output text | `muted` | dim aluminum | `textMuted`/text | **`tool_output`, a step below `text`, not `muted`** |
| Survives no-bg / `NO_COLOR` | no | border box yes; band no | rule yes | **rule yes** |

Why not Pi's pure background band: it is exactly the reported look and it
vanishes on `system` and `NO_COLOR`. Why not omp's full border box: it spends
two extra columns and two rows per tool, against Wave 3's compact direction
(`2026-08-19-wave3-tui-visual-design.md:103-104`). Why not Codex-style heavy
boxing: the owner said without a broader UI design the theming "won't quite
work" (Codex is kept as a contrast reference, as in Wave 3).

Concrete layout (a panel is one row of rule plus the existing padded surface):

```
┃                                          <- panel top padding (rule + panel bg)
┃ ✓ read_text_file  src/main.rs            <- glyph: success; name: tool_title (bold); summary: muted
┃   fn main() {                            <- output: tool_output on panel
┃       …
┃   … 12 more lines                        <- omitted-lines marker: muted
┃                                          <- panel bottom padding
```

- **Rule.** One extra column left of the surface: `┃` (U+2503; box drawing,
  safest across fonts, same glyph family as opencode's) on every panel row,
  including padding rows. Color from `tool_rule(state)`. Existing
  `toolHorizontalPadding` and `toolVerticalPadding` are unchanged. The rule
  replaces the dim continuation `│ ` rail used for expanded rows today
  (`transcript.rs:1007`), so there is one rail, not two.
- **Surface.** One neutral panel for `Pending` and `Success`; a tint only for
  `Error` and `Harness`. State is carried by glyph, rule color, and label.
  Defaults change: `toolSuccessBackground` becomes the pending value; success
  is no longer green.
- **Output.** `tool_output` default moves from `muted` gray 128 to a
  readable step below `text`; `muted` stays for metadata and summaries.
  Target: at least 4.5:1 against the panel in `dark` and `light`
  (Pi's own bar for body text, https://pi.dev/docs/latest/themes).
- **Spacing.** Unchanged: `toolGap` and padding tokens
  (`transcript.rs:938-942`, `1136-1179`) keep their meaning. Whether the
  default stack is too tall is a taste call to make from the tape, and is a
  token edit, not a design change.
- **`system` / monochrome.** No panel background: the rule, glyph, and the
  existing blank line before a tool group separate rows.
- **Wave 3 reconciliation.** Wave 3 contradicts itself: it accepted an
  "outcome-tinted full-width surface" per tool (`:93-98`) and also "a compact
  successful row receives no background band" (`:169`); the code follows the
  former (`transcript.rs:1181-1191`). This spec supersedes both with: neutral
  panel for every tool row, tint for failure only. Whether compact
  summary-only successes should drop the panel (opencode's inline shape) is an
  Open question.

### Migration of existing widgets

1. Add the helpers and `ToolState`/`DiffKind` to `theme.rs`; make
   `ThemeColors` fields private to that module.
2. Replace every `theme.colors.X` read with a helper. Sites (from
   `diag-ThemeScout`): `transcript.rs:912-1238`, `app.rs:4525-4854` (pending
   dialog, local edit, diff helper at `4788-4800`), `approval_selector.rs`,
   `fork_picker.rs`, `help_overlay.rs`, `input.rs`, `model_selector.rs`,
   `perf_overlay.rs`, `session_picker.rs`, `slash_popup.rs`, `status_bar.rs`,
   `thinking_selector.rs`. Mechanical; behavior-preserving for `dark` apart
   from the tool defaults.
3. Tool treatment in `transcript.rs` (`render_tool_surface`, `tool_background`,
   `1073-1075`, `1181-1191`): rename to state-based and add the rule column.
   `transcript.rs` is also being edited by plane:YACH-19's markdown and layout
   work; land this step after those changes merge to avoid conflicts.
4. CLI: `resolve_theme`, `NO_COLOR`, presets, `COLORFGBG` (`main.rs:425-450`,
   `3762-3772`); extend the existing precedence test (`main.rs:6019-6043`).
5. Manifest: `contributes.themes` parsing and index plumbing
   (`extension.rs:1662-1673`).
6. Docs: theme file format and `base`, presets, `NO_COLOR`, extension themes
   (`docs/extensions.md`), and a supersession note on the Wave 3 spec.

Order: 1, 2, 4 first (no visible change except `NO_COLOR` and `YACH_THEME`
names), then 3 (the owner-visible fix), then 5.

### JSON token compatibility

| Item | Change |
|---|---|
| 22 color tokens, 5 spacing tokens, `vars` | Unchanged names and parsing |
| `deny_unknown_fields`, unknown token error | Unchanged |
| New top-level `base` | Optional; default `dark` = today's behavior |
| Defaults: `toolSuccessBackground` | Green to neutral (= `toolPendingBackground`) |
| Defaults: `toolOutput` | Gray 128 to a readable step below `text` |
| Files that set the above explicitly | Unaffected; their values still win |
| Files that did not | Pick up the new defaults (the intended fix) |
| Project `<root>/.yach/theme.json` | No longer applied; startup diagnostic (Project themes removed) |
| `YACH_THEME=<path>` | Unchanged, except four reserved preset names |

## Testing

**Unit (`yach-ui` `theme.rs`):**

- Legacy file with all 22 tokens round-trips to the same values as before
  (regression for compatibility); file without `base` equals the `dark`
  preset plus its overrides.
- Preset invariants: in `dark` and `light`, every surface helper returns an
  explicit non-`Reset` fg and bg, and its text/panel contrast is at least
  4.5:1 (WCAG relative luminance over `Rgb` values); `muted` is at least 3:1.
  In `system`, every `bg` is `Reset`.
- Monochrome: no color field is anything but `Reset`; each helper in the
  table above returns the stated modifier.
- Tool success surface equals the pending surface and is not the previous
  green; error and harness differ from both.
- `COLORFGBG` parsing: `15;0`, `0;default;15`, `default;7`, empty, garbage.
- `base` parsing: presets, `<ext>:<theme>`, extension theme naming a
  non-preset base is rejected, reserved names rejected.

**Unit (`yach-ui` `transcript.rs`, `BenchmarkApp` + `TestBackend`):**

- Rendered tool rows: rule glyph in column 0 on every row of a panel, rule
  color by state, glyph and label present for each state.
- Under monochrome: no cell in the buffer has a non-`Reset` fg or bg for
  pending, success, failed, harness outcome, review, and user-message states.
- Existing assertions on `theme.colors.tool_pending_background`
  (`transcript.rs:1543-1545,1838-1840,1895-1897`) move to the helper.

**Unit (`yach-cli`):** precedence table for `resolve_theme` (preset name vs
path, user file, default, `NO_COLOR` overriding all), extending
`main.rs:6019-6043`; plus a startup-path test that a legacy
`<project>/.yach/theme.json` is not applied and produces the notice.

**Unit (`yach-backend`):** `themes` manifest parsing (valid, bad id, escaping
path, oversize, reserved name), project-scope blocked, disabled extension
unresolved with fallback and diagnostic, project extension theme not offered.

**Visual (`tests/visual`, `render.sh`):** the existing tapes build a
fixture session under `COLORTERM=truecolor` (`session.tape:14`). Add:

- `light.tape`: same session with `COLORFGBG=0;15` and a light VHS theme
  (the exact built-in VHS theme name is chosen at implementation; verify
  with `vhs themes`). Check legibility of user message, tool panel, error.
- `nocolor.tape`: `NO_COLOR=1`; screenshot plus a raw-byte check that the
  captured stream contains no color SGR (`38;`, `48;`, `3x`, `4x`, `9x`,
  `10x`) — a deterministic check the screenshot cannot give.
- Existing `session` and `narrow` tapes re-reviewed for the new tool panel
  (compare against the reported screenshot of green band and gray text).

## Open questions

1. ~~**Project themes.**~~ Resolved 2026-10-02: no project-scoped themes (see
   Project themes removed).
2. **Default theme.** `auto` (COLORFGBG, else dark) as recommended; or keep a
   fixed `dark`; or default to `system`, which is always legible but paints no
   user-message surface, weakening Wave 3's conversation hierarchy.
3. **Compact successes.** Neutral panel for every tool row (proposed), or
   panel only for rows with output, pending, or error and an unpaneled inline
   row for compact successes (opencode `InlineTool` shape)?
4. **`NO_COLOR` vs explicit theme.** `NO_COLOR` always wins (proposed), or let
   an explicit user theme or `YACH_THEME` override it, as no-color.org permits?
5. **Extension runtime control.** Is declarative contribution plus user
   selection enough for the owner's "controllable by extensions", or is a
   runtime `ui.theme.request` with user confirmation needed in this slice?
6. **Terminal queries.** Add OSC 10/11 detection (with a ~100 ms cap, before
   `EventStream` starts) in a follow-up, or stay with `COLORFGBG` only?
7. **Color depth.** Presets use 24-bit RGB; Wave 3 required degrading cleanly
   without true color (`2026-08-19-wave3-tui-visual-design.md:108`) but nothing
   implements it. Add a `COLORTERM`-based downgrade, or point limited-terminal
   users to `system`?
