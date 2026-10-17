# Bundled palette attribution

Each file in this directory is a yach theme document derived from one Base16
scheme. Slot values (`base00`..`base0F`) are copied verbatim from Tinted
Theming's scheme repository, commit `a70da1dab18008023cfd55a94053f3b6cab4f86e`:

https://github.com/tinted-theming/schemes

Tinted Theming schemes are MIT licensed. Role mapping prefers an upstream
slot or another upstream color of the same scheme, then a different surface,
and only then an adjusted role color. Hex literals in `colors` are either
named upstream colors or the precomputed blends documented below; they do
not change the copied Base16 slots. Blends interpolate sRGB channels and
round each channel to the nearest integer (half rounds up).

## Attribution

| Palette | Scheme | Author | Upstream project | Upstream license |
|---|---|---|---|---|
| `catppuccin-latte` | Catppuccin Latte | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-frappe` | Catppuccin Frappe | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-macchiato` | Catppuccin Macchiato | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `catppuccin-mocha` | Catppuccin Mocha | https://github.com/catppuccin/catppuccin | https://github.com/catppuccin/palette | MIT |
| `rose-pine` | Rosé Pine | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `rose-pine-moon` | Rosé Pine Moon | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `rose-pine-dawn` | Rosé Pine Dawn | Emilia Dunfelt <edun@dunfelt.se> | https://github.com/rose-pine/rose-pine-palette | MIT |
| `gruvbox-dark` | Gruvbox dark | Tinted Theming (https://github.com/tinted-theming), morhetz (https://github.com/morhetz/gruvbox) | https://github.com/morhetz/gruvbox | none on file; README states MIT/X11, repository has no LICENSE file |
| `gruvbox-light` | Gruvbox Light | Tinted Theming (https://github.com/tinted-theming), morhetz (https://github.com/morhetz/gruvbox) | https://github.com/morhetz/gruvbox | none on file; README states MIT/X11, repository has no LICENSE file |
| `tokyo-night-dark` | Tokyo Night Dark | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-storm` | Tokyo Night Storm | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-moon` | Tokyo Night Moon | Ólafur Bjarki Bogason | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `tokyo-night-light` | Tokyo Night Light | Michaël Ball | https://github.com/folke/tokyonight.nvim | Apache-2.0 |
| `solarized-dark` | Solarized Dark | Ethan Schoonover (modified by aramisgithub) | https://github.com/altercation/solarized | MIT |
| `solarized-light` | Solarized Light | Ethan Schoonover (modified by aramisgithub) | https://github.com/altercation/solarized | MIT |
| `dracula` | Dracula | clach04 (https://github.com/clach04) | https://github.com/dracula/dracula-theme | MIT |
| `nord` | Nord | arcticicestudio | https://github.com/nordtheme/nord | MIT |
| `monokai` | Monokai | Wimer Hazenberg (http://www.monokai.nl) | http://www.monokai.nl | none stated |

Classic Monokai states no license, and Gruvbox's README claims MIT/X11 without
a LICENSE file. They ship on the basis that color values are generally not
copyrightable, and NOTICE says so rather than claiming a license. Monokai Pro
is a separate commercial product; only classic Monokai ships, under that name.

## Role colors outside the Base16 slots

`diffContext` follows `muted` when both use a literal; `diffAdded` follows
`success`, and `diffRemoved` follows `error`.

| Palette | Literal role colors and source |
|---|---|
| `catppuccin-latte` | `muted` `#6c6f85` = upstream subtext0; `dim` `#7c7f93` = overlay2; `success` `#438c3b` = base0B toward base05 25%; `warning` `#a4753b` = base0A toward base05 40%; `harness` `#24818d` = base0C toward base05 25%. |
| `catppuccin-frappe` | `muted` `#949cbb` = upstream overlay2; `dim` `#838ba7` = overlay1. |
| `catppuccin-macchiato` | `muted` `#939ab7` = upstream overlay2; `dim` `#6e738d` = overlay0. |
| `catppuccin-mocha` | `muted` `#9399b2` = upstream overlay2; `dim` `#6c7086` = overlay0. |
| `gruvbox-dark` | Upstream bright blue `#83a598` (accent), bright green `#b8bb26` (success), bright red `#fb4934` (error), bright yellow `#fabd2f` (warning), bright aqua `#8ec07c` (harness); `muted` `#a89984` = upstream gray (also base04 toward base05 25%). |
| `gruvbox-light` | Upstream faded green `#79740e` (success), faded aqua `#427b58` (harness); `warning` `#997229` = base0A toward base05 40%. |
| `tokyo-night-dark`, `tokyo-night-storm` | `warning` `#e0af68` = upstream Tokyo Night yellow; error uses base0F, the red in these nonstandard slot maps. Storm's `#1f2335` user/pending/success surface is upstream bg_dark. |
| `rose-pine-dawn` | `warning` `#a17857` = gold/base09 toward base05 50%; `harness` `#568a99` = base0C toward base05 15%. |
| `solarized-dark` | `error` `#c25957` = base08 toward base05 35%. |
| `solarized-light` | `success` `#788c23` = base0B toward base05 30%; `warning` `#907e2f` = base0A toward base05 40%; `harness` `#3c8d8a` = base0C toward base05 40%. |
| `nord` | `muted` `#a8aeba` = base03 toward base05 60%; `dim` `#7a8292` = base03 toward base05 30%; `border` `#9198a6` = midpoint of dim and muted; `error` `#c77c85` = base08 toward base05 20%. |

## Error surfaces

The default error surface is the pending panel blended 15% toward the
scheme's red (base08, except base0F for Tokyo Night dark/storm/light).
Changing the surface before further adjusting semantic colors preserves
the upstream hues wherever possible. Exceptions retain a visible tint while
keeping semantic roles at least 3:1 on both panels:

| Palette | `toolErrorBackground` derivation |
|---|---|
| `catppuccin-latte` | `#ecdae2` = base00 toward base08 10%. |
| `gruvbox-dark` | `#523532` = base01 toward base08 15% (the original neutral red, not the bright foreground red). |
| `gruvbox-light` | `#f4d2ae` = base00 toward base08 15%. |
| `tokyo-night-dark` | `#38242f` = base01 toward base0F 15%. |
| `tokyo-night-storm` | `#3f2f42` = bg_dark `#1f2335` toward base0F 15%. |
| `tokyo-night-light` | `#c2b7be` = base01 toward base0F 15%. |
| `rose-pine` | `#2a1e2d` = base00 toward base08 8%, preserving pine success contrast. |
| `solarized-dark` | `#212c35` = base00 toward base08 15%. |
| `solarized-light` | `#fae2d1` = base00 toward base08 10%. |
| `nord` | `#523f4b` = base00 toward upstream red/base08 25%. |
| `monokai` | `#47282e` = base00 toward base08 15%. |

