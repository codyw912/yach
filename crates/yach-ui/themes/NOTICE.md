# Bundled palette attribution

Each file in this directory is a yach theme document derived from one Base16
scheme. Slot values (`base00`..`base0F`) are copied verbatim from Tinted
Theming's scheme repository, commit `a70da1dab18008023cfd55a94053f3b6cab4f86e`:

https://github.com/tinted-theming/schemes

Tinted Theming schemes are MIT licensed. A yach `colors` entry that is a hex
literal rather than a `baseNN` name is a precomputed contrast adjustment (a
blend toward `base05`, or a `base01` panel blended 15% toward `base08`); it is
not an upstream slot.

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
