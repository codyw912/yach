# Theme Engine and Named Palette Survey

Date: 2026-10-06. Evidence for the theming spec
(`docs/project/specs/2026-10-02-theming-engine-design.md`, plane:YACH-20), not a
design or plan.

Owner direction, 2026-10-06: ship the usual named palettes (Monokai,
Catppuccin, Rosé Pine and similar) alongside custom themes, while keeping the
engine minimal and fast. This record surveys how other tools do that and
recommends how yach should.

`[INFERENCE]` marks anything not observed in code, docs, or a running program.
Online sources were read on 2026-10-06; license identifiers come from the GitHub
license API on the same date.

## Conclusion

Keep yach's own strict JSON as the only runtime theme format, and ship named
palettes as yach theme files that map a source palette onto yach's roles.
Source those palettes from the Tinted Theming Base16/Base24 scheme set (MIT),
which already carries every requested family in one 16-slot shape, and record
upstream attribution per palette. Resolve the selected theme once at startup
into the existing fixed `Theme` struct; nothing about named palettes needs
per-frame work. No change to the spec's model (roles, `base` plus overrides,
single winner, no project themes, declarative extension themes) is needed; the
spec gains a "bundled palettes" section.

## How other tools do it

| Tool | Theme format and roles | Named themes | Custom themes |
|---|---|---|---|
| Helix | TOML; UI and syntax scopes map to styles; optional `[palette]` table; `inherits` from a named theme | Many in-tree, community-contributed | `~/.config/helix/themes/*.toml` (https://docs.helix-editor.com/themes.html) |
| Zed | JSON theme family; each theme has `appearance` and detailed UI, editor, syntax and terminal fields | A few built in, hundreds via theme extensions | Local JSON and extension store; settings pair a light and a dark theme (https://zed.dev/docs/themes) |
| Ghostty | Config keys: fg, bg, cursor, selection, 16-slot palette | Hundreds bundled, synced from iTerm2-Color-Schemes; `dark:X,light:Y` pairing | Files in the themes directory (https://ghostty.org/docs/features/theme) |
| WezTerm, Alacritty | Terminal palettes (fg, bg, cursor, selection, ANSI 16) | WezTerm embeds a large index; Alacritty keeps a separate `alacritty-theme` repo | Config or imported TOML |
| bat, delta | Sublime/TextMate syntax themes only | Bundled set | Install theme files |
| Pi | JSON: `vars` plus a broad set of semantic UI roles; hex, ANSI index, OKLCH | Three built-ins (`system`, `dark`, `light`); `system` reads terminal colors at startup | User directory; packages may ship themes (`packages/coding-agent/docs/themes.md` at `69f0be6`) |
| opencode | JSON: hue palettes per light/dark plus a semantic token tree | Many built-ins | Global and project directories (https://opencode.ai/v2/docs/cli/theme) |

Patterns that recur:

- **Two layers.** A small palette (named colors or 16 slots) and a mapping from
  palette to the app's own roles. Apps never consume another app's role file
  directly; they consume palettes.
- **Named themes are data, embedded.** Terminals embed hundreds; editors embed
  a few and rely on extensions or community repos for the rest.
- **Light/dark pairing** (Zed, Ghostty, Pi) is the common answer to "follow the
  terminal": the user names one light and one dark theme.
- **Base16/Base24 is the cross-app palette standard.** Tinted Theming publishes
  schemes as YAML (`system`, `name`, `author`, `variant`, `palette:
  base00..base0F`) and templates that generate configs for many apps
  (https://github.com/tinted-theming/schemes).

## Licensing of the requested palettes

| Palette | Source checked | License | Base16 scheme in tinted-theming/schemes |
|---|---|---|---|
| Catppuccin (Latte, Frappé, Macchiato, Mocha) | `catppuccin/palette` | MIT | yes, all four |
| Rosé Pine (Main, Moon, Dawn) | `rose-pine/rose-pine-palette` | MIT | yes, all three |
| Dracula | `dracula/dracula-theme` | MIT | yes |
| Gruvbox | `morhetz/gruvbox`: README states MIT/X11, but the repo has no LICENSE file (GitHub license API returns none) | upstream unverified; Base16 YAML is MIT | yes, several contrasts |
| Nord | `nordtheme/nord` | MIT | yes (plus Nord Light) |
| Solarized | `altercation/solarized` | MIT | yes, dark and light |
| Tokyo Night | `folke/tokyonight.nvim` | Apache-2.0 | yes: `tokyo-night-dark`, `-light`, `-storm`, `-moon` |
| Monokai | original palette by Wimer Hazenberg (monokai.nl) has no stated license; ports such as `tanvirtin/monokai.nvim` are MIT | no upstream license; Base16 YAML is MIT | yes (`base16/monokai.yaml`) |
| Scheme collection | `tinted-theming/schemes`, `tinted-theming/base24` | MIT | n/a |

Notes:

- The tinted-theming MIT license covers that repository's YAML files, not
  the original palettes they repackage. Where the upstream states no license
  (Monokai) or has no LICENSE file (Gruvbox), bundling rests on hex color values
  being generally not copyrightable, not on a clean license. That is low risk
  but not a license grant `[INFERENCE: legal reading, not legal advice]`.
- Each palette has a name and an author; bundling with per-palette attribution
  (name, author, upstream URL, license or "none stated") in a NOTICE file is the
  conventional practice.
- "Monokai Pro" is a separate commercial product; ship only the classic Monokai
  palette and do not use the "Pro" name.
- Tokyo Night's canonical source is Apache-2.0, which requires keeping the
  license and notice; fine for an MIT project.

## Mapping a 16-slot palette onto yach roles

Base16 slot meanings are standardized (base00-07 a ramp from background to
brightest foreground; base08-0F accents: red, orange, yellow, green, cyan,
blue, magenta, brown). A fixed mapping onto the spec's semantic roles:

| Yach role (spec) | Base16 slot |
|---|---|
| background / user surface | `base00` (light schemes put their light background here, e.g. `solarized-light` `base00` = `#fdf6e3`) |
| raised panel (tool rows, user message) | `base01` |
| selection, focus | `base02` |
| dim / muted text | `base03` / `base04` |
| primary text | `base05` |
| emphasized text, titles | `base06` |
| error, diff removed | `base08` |
| warning | `base0A` (or `base09`) |
| success, diff added | `base0B` |
| info, harness status | `base0C` |
| accent, links | `base0D` |
| alternate accent, diff hunk | `base0E` |

Each mapped theme still sets an explicit foreground on every painted surface,
as the spec requires for `dark` and `light`. A palette file may override any
role, so a mapping that reads badly for one palette is fixed in that file, not
in code.

## Performance

The current `Theme` is a `Copy` struct of colors resolved at load
(`crates/yach-ui/src/theme.rs`). Bundled palettes embedded as static data and
resolved once at startup keep rendering at field reads. None of the surveyed
tools converts colors per frame; Pi approximates truecolor to 256 once. Cost of
named palettes is binary size: about 15 palettes of 16 colors is under 10 KiB
`[INFERENCE]`.

## Recommendations for the theming spec

1. **Format.** Keep yach JSON as the only runtime format. Do not parse
   Pi, opencode, Helix or Zed theme files; their role sets do not match yach's.
2. **Bundled palettes.** Generate yach theme files at development time from
   the Tinted Theming Base16 YAML for: Catppuccin (Latte, Frappé, Macchiato,
   Mocha), Rosé Pine (Main, Moon, Dawn), Gruvbox (dark, light), Dracula, Nord,
   Solarized (dark, light), Monokai, and Tokyo Night (night, storm, moon,
   light). Check them in as data with a NOTICE listing each palette's author,
   upstream and license. A small generator keeps updates mechanical.
3. **Selection.** Named palettes are selectable exactly like presets: by name in
   `YACH_THEME` or as the `base` of the user's theme file. Reserved names grow
   from four to the bundled set; the spec's rule that reserved names shadow
   file paths needs to hold for them.
4. **Light/dark.** Allow the user theme to name one light and one dark palette
   for `auto` to choose between, as Zed, Ghostty and Pi do. This is a small
   addition to the spec's `auto` preset rather than a new mechanism.
5. **Import (optional, later).** A `yach theme import <base16.yaml>` command
   that writes a yach theme file lets users bring any of the hundreds of
   Base16/Base24 schemes without yach shipping them. It reuses the generator.
6. **Color depth.** Unchanged (owner decision 2026-10-06): limited-color
   terminals use `system`. Named palettes are 24-bit only.

## Sources

- Tinted Theming: https://github.com/tinted-theming/schemes (scheme listing and
  `base16/monokai.yaml` read 2026-10-06), https://github.com/tinted-theming/base24
- Palette upstreams: https://github.com/catppuccin/palette ,
  https://github.com/rose-pine/rose-pine-palette ,
  https://github.com/dracula/dracula-theme , https://github.com/morhetz/gruvbox ,
  https://github.com/nordtheme/nord , https://github.com/altercation/solarized ,
  https://github.com/folke/tokyonight.nvim , https://github.com/tanvirtin/monokai.nvim
- Tool docs: Helix, Zed, Ghostty, opencode (URLs in the table); Pi
  `packages/coding-agent/docs/themes.md` at `69f0be6`.
