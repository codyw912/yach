# Wave 3 TUI Visual Design

Status: accepted 2026-08-19 — owner taste session complete
Date: 2026-08-19
Scope: UX sprint Wave 3 — transcript hierarchy, spacing, tool-row visuals, and a docked responsive input composer.

## Problem

Wave 1 fixed concrete ergonomics and Wave 2 unified tool lifecycle and review behavior, but the resulting TUI still reads as a functional debug surface:

- user, assistant, and tool rows occupy the same visual plane;
- the opposing `▸`/`◂` conversation glyphs add noise without establishing strong hierarchy;
- every transcript entry receives the same trailing blank line, so adjacent tool activity is looser than necessary while conversation turns remain weakly grouped;
- a pending or expanded tool row has the same structural treatment as a compact successful row;
- the full-width input border and verbose title carry more visual weight than the transcript;
- the composer grows with wrapped content, but its geometry is edge-to-edge and capped overflow is not communicated;
- the status bar does not align with a distinct composer surface.

The behavior is sound. This pass should improve scanability and calm without hiding evidence or introducing a dashboard-like UI.

## Owner direction

The 2026-08-19 taste session selected:

- **visual language:** OpenCode hierarchy crossed with Pi directness;
- **density:** balanced;
- **conversation roles:** a full-width user-message surface and prominent unboxed assistant prose;
- **tool rows:** compact evidence with useful bounded output visible by default;
- **composer:** a docked responsive card, visually inset but still participating in layout so it never covers transcript evidence.

These choices are binding for Wave 3.

## Cohort evidence

### OpenCode

Reference: `https://raw.githubusercontent.com/anomalyco/opencode/dev/packages/web/src/assets/lander/screenshot.png`

Useful traits:

- a framed user prompt creates immediate conversation hierarchy;
- assistant prose is quiet and unboxed;
- tool activity is compact and subordinate to the conversation;
- the inset composer is clearly interactive without dominating the entire terminal;
- whitespace separates semantic groups rather than every physical row.

Yach should not copy OpenCode's product-dashboard chrome, large header treatment, or agent/product controls.

### Pi

Reference: `https://raw.githubusercontent.com/badlogic/pi-mono/main/packages/coding-agent/docs/images/interactive-mode.png`

Useful traits:

- tool evidence remains direct and visibly part of the transcript;
- low chrome preserves terminal character;
- compact metadata and status information stay subordinate;
- expanded tool content remains readable as one component.

Yach should not give every successful tool a full-width tinted card. The owner selected a quieter exceptional-state surface.

### Codex CLI

Reference: `https://raw.githubusercontent.com/openai/codex/main/.github/codex-cli-splash.png`

Codex demonstrates strong boxed hierarchy and clear semantic accenting. It was retained as a contrast reference, not the selected anchor: its bold section treatment and workflow chrome are heavier than the desired Yach posture.

### Current Yach baseline

An actual `--backend fixture` session was exercised with a completed read and a pending command review. The current baseline confirms:

- conversation and tool rows visually run together;
- compact tool evidence is clear but uniformly spaced;
- pending review detail is readable but has no structural rail or surface;
- the edge-to-edge composer is the strongest box on screen;
- the status line is compact and already suitable for the selected direction.

## Owner correction (2026-08-22)

Normal-TUI dogfooding showed that the original rail-and-gray-prose interpretation
made user and assistant messages too visually similar and hid too much successful
tool output. Current Pi and Codex sources confirm a stronger shared convention:

- Pi gives user messages a full-width background, renders assistant prose as plain
  content, shows the first ten lines for ordinary tools, the last five lines for
  command output, and generates four-context unified diffs;
- Codex gives user messages a background and `›` marker, renders assistant prose
  with a `•` marker, and shows bounded command output with omitted-line markers.

This correction supersedes the original user-rail, gray-assistant, and
summary-only-success decisions below.

## Goals

1. Establish clear user, assistant, tool, and harness hierarchy without adding labels to ordinary prose.
2. Keep balanced conversation spacing while grouping adjacent tool activity compactly.
3. Keep successful completed tools quiet; make running, reviewed, expanded, failed, denied, and interrupted rows structurally obvious.
4. Render an inset, responsive composer that grows to a bounded cap, never overlays transcript content, and communicates capped overflow.
5. Preserve typing while reading a scrolled transcript.
6. Preserve protocol semantics, transcript evidence, expansion behavior, focus indication, and render-cache performance.
7. Degrade cleanly in narrow terminals and terminals without true-color support.

## Non-goals

- No theme configuration, custom color files, or light/dark theme system.
- No true floating overlay that obscures transcript content.
- No transcript row selection or per-row navigation redesign.
- No Vim-mode cursor states.
- No approval-policy, tool-loop, protocol, or session-evidence changes.
- No new header, sidebar, agent switcher, plan surface, or mid-turn progress UI.
- No fixed RGB background palette; the design stays on terminal-native named colors.

## Visual system

The palette remains terminal-native:

- cyan: primary interaction accent;
- white: assistant content and high-priority active text;
- dark gray background: user-message surface;
- gray: ordinary tool names;
- dark gray foreground: metadata, compact successful results, rails, and key hints;
- yellow: running tools and pending review;
- red: failed or rejected/error emphasis;
- green: successful state glyphs and added diff lines;
- magenta: harness-authored outcomes.

Color is never the only signal. Existing glyphs and outcome labels remain.

No new global theme abstraction is required unless implementation reveals repeated style definitions across modules. A small palette helper is acceptable; a configurable theme layer is not.

## Transcript hierarchy

### User messages

A user message receives a full-width dark-gray background with one column of
horizontal padding and a bright `›` prefix. The background extends across wrapped
lines so the prompt remains immediately distinguishable from the response.

### Assistant messages

Ordinary assistant prose is unboxed bright text with a `•` prefix and two-column
continuation inset. This is the primary reading plane.

### Tool rows

All tool prefixes occupy a consistent four-column gutter.

- running: yellow `⚙` plus a visible tool name;
- completed success: green `✓`, gray tool name, dark-gray summary, and a bounded
  output preview when detail exists;
- failed: red `✗` and explicit error styling;
- harness-refined outcome: existing visible outcome label plus semantic color;
- expanded/live/review rows: continuation lines use a dark-gray `│` rail;
- pending review retains yellow emphasis and its explicit selector controls;
- rejected/interrupted/failed states retain text labels so meaning does not depend on color.

Command-like output shows its bounded tail because the result is usually at the
end. Other tools show the first ten lines. Omitted-line markers make both policies
explicit. A compact successful row receives no background band.

### Spacing

Spacing is semantic:

- no leading or trailing padding solely because an entry exists;
- one blank line before a new user message, assistant prose block, harness outcome, or error when prior content exists;
- one blank line before the first tool in a tool group;
- no blank lines between adjacent tool entries;
- wrapped lines remain within the entry gutter;
- short transcripts continue to bottom-align above the composer.

This yields balanced turns and Pi-like compact tool sequences.

## Docked responsive composer

The composer is a layout participant, not an overlay.

- horizontally centered;
- two-column side gutters when the terminal is at least 40 columns wide;
- maximum width of 112 columns so ultrawide terminals do not create an unreadably long input line;
- falls back to full width on narrow terminals;
- one blank layout row separates transcript content from the composer;
- minimum height remains three rows;
- content grows with explicit and soft-wrapped lines to a maximum of eight rows;
- wrapping and height are computed from the actual inset card width;
- beyond the cap, the textarea scrolls and the title indicates additional hidden content;
- top title is concise: `message`, with a running-state suffix while streaming;
- focused title uses cyan, border remains restrained, and the cursor remains visible;
- unfocused border/title dim and the cursor hides, preserving the Wave 1 focus signal;
- send/newline hints move to a muted bottom title and may be omitted when the card is too narrow.

The status bar uses the same horizontal bounds as the composer so the bottom region reads as one quiet dock.

## Scroll behavior

Editing the prompt must not call `scroll_to_bottom`. A user may page upward, type in the docked composer, and keep the same transcript offset. Submitting the prompt still appends the user message and moves to the live turn, as today.

Incoming live events retain current follow behavior. Changing that policy belongs to the deferred mid-turn visibility work.

## Narrow-terminal behavior

- side gutters collapse before content width becomes unusable;
- composer hints disappear before the message title;
- status segments continue dropping atomically by existing priority;
- transcript prefixes remain fixed-width and wrapped content uses the remaining width;
- viewport height saturates safely when the composer reaches its cap.

## Performance and state

Visual presentation remains derived from existing transcript entries. No canonical session fields are added.

`TranscriptRenderCache` still keys by transcript revision and width. Semantic spacing and styled lines are produced only during cache rebuild. Composer geometry performs one bounded pass over the current input, as it already does.

## Verification

1. transcript render tests assert the full-width user surface, prominent assistant
   marker, compact adjacent tool rows, exceptional-state continuation rails, and
   semantic labels;
2. tool-row tests assert bounded head previews for ordinary tools and bounded tail
   previews for command-like tools;
3. layout/input tests assert inset/max-width geometry, narrow fallback,
   actual-width wrapping, growth cap, and overflow indication;
4. app tests assert typing while scrolled preserves transcript offset and
   submission still follows the new turn;
5. existing review, expansion, focus, status, RPC, and hydration tests remain green.

Actual-TUI verification must exercise:

- a user/assistant exchange with adjacent completed tools;
- a pending and resolved inline review;
- compact and expanded tool rows;
- a multiline composer growing to its cap;
- typing while the transcript is scrolled;
- a narrow terminal if the harness can provide one.

Workspace validation remains `just fmt-check`, `just lint`, and `just test`.
