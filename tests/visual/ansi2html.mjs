import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const base16 = ['#000000', '#cd0000', '#00cd00', '#cdcd00', '#0000ee', '#cd00cd', '#00cdcd', '#e5e5e5',
  '#7f7f7f', '#ff0000', '#00ff00', '#ffff00', '#5c5cff', '#ff00ff', '#00ffff', '#ffffff'];
const escape = value => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const rgb = values => `#${values.map(value => value.toString(16).padStart(2, '0')).join('')}`;
function indexed(n) {
  if (n < 16) return base16[n];
  if (n >= 232) return rgb(Array(3).fill(8 + (n - 232) * 10));
  const cube = [0, 95, 135, 175, 215, 255];
  n -= 16;
  return rgb([cube[Math.floor(n / 36)], cube[Math.floor(n / 6) % 6], cube[n % 6]]);
}

// capture-pane has already interpreted cursor movement; only SGR remains.
export function convert(text, foreground, background) {
  let state = {};
  const output = [];
  function span(value) {
    if (!value) return;
    let fg = state.fg ?? foreground;
    let bg = state.bg ?? background;
    if (state.reverse) [fg, bg] = [bg, fg];
    // Inline-block makes the background cover the whole line box, as a
    // terminal fills the whole cell; a plain inline span paints only the glyph
    // height and leaves page-colored seams between rows of a filled panel.
    let css = `display:inline-block;color:${fg};background:${bg}`;
    if (state.bold) css += ';font-weight:bold';
    if (state.italic) css += ';font-style:italic';
    if (state.underline) css += ';text-decoration:underline';
    if (state.dim) css += ';opacity:.6';
    // An inline-block cannot span lines, so emit each line's run separately.
    output.push(value.split('\n').map(line => (line ? `<span style="${css}">${escape(line)}</span>` : '')).join('\n'));
  }
  let position = 0;
  for (const match of text.matchAll(/\x1b\[([0-9;:]*)m/g)) {
    span(text.slice(position, match.index));
    position = match.index + match[0].length;
    // ISO colon RGB may include an empty color-space operand (38:2::r:g:b).
    const params = match[1].replace(/([34]8):2::/g, '$1:2:').split(/[;:]/).map(value => Number(value || 0));
    for (let k = 0; k < params.length; k++) {
      const code = params[k];
      if (code === 0) state = {};
      else if (code === 1) state.bold = true;
      else if (code === 2) state.dim = true;
      else if (code === 3) state.italic = true;
      else if (code === 4) state.underline = true;
      else if (code === 7) state.reverse = true;
      else if (code === 22) state.bold = state.dim = false;
      else if (code === 23) state.italic = false;
      else if (code === 24) state.underline = false;
      else if (code === 27) state.reverse = false;
      else if (code === 39) delete state.fg;
      else if (code === 49) delete state.bg;
      else if (code >= 30 && code <= 37) state.fg = base16[code - 30];
      else if (code >= 90 && code <= 97) state.fg = base16[code - 82];
      else if (code >= 40 && code <= 47) state.bg = base16[code - 40];
      else if (code >= 100 && code <= 107) state.bg = base16[code - 92];
      else if (code === 38 || code === 48) {
        const key = code === 38 ? 'fg' : 'bg';
        if (params[k + 1] === 2 && k + 4 < params.length) {
          state[key] = rgb(params.slice(k + 2, k + 5));
          k += 4;
        } else if (params[k + 1] === 5 && k + 2 < params.length) {
          state[key] = indexed(params[k + 2]);
          k += 2;
        }
      }
    }
  }
  span(text.slice(position));
  return output.join('');
}

const families = [
  ['presets', /^(dark|light|system|nocolor)$/],
  ['catppuccin', /^catppuccin-/],
  ['rose-pine-dracula', /^(rose-pine|dracula)/],
  ['tokyo-night', /^tokyo-night-/],
  ['gruvbox-solarized', /^(gruvbox|solarized)-/],
  ['nord-monokai', /^(nord|monokai)$/],
  ['composer', /^composer-/],
  ['other', /./],
];

export function gridPages(panels) {
  const buckets = new Map(families.map(([name]) => [name, []]));
  for (const panel of panels) {
    const [family] = families.find(([, pattern]) => pattern.test(panel.name));
    buckets.get(family).push(panel.body);
  }
  const pages = [];
  for (const [family, bodies] of buckets) {
    for (let offset = 0; offset < bodies.length; offset += 4) {
      pages.push({ name: family + (offset ? `-${offset / 4 + 1}` : ''), body: bodies.slice(offset, offset + 4).join('\n') });
    }
  }
  return pages;
}

function render(out, font, manifest) {
  const embedded = readFileSync(font).toString('base64');
  const css = `@font-face{font-family:Shot;src:url(data:font/ttf;base64,${embedded})}
*{box-sizing:border-box}body{margin:0;padding:16px;background:#333;color:#eee}
.grid{display:grid;grid-template-columns:repeat(2,max-content);gap:16px}
.panel{background:var(--bg);color:var(--fg)}
h2{margin:0;padding:8px 12px;font:16px/20px sans-serif}
pre{box-sizing:content-box;margin:0;padding:12px;width:calc(var(--cols)*1ch);height:560px;font:12px/14px Shot,monospace;white-space:pre}`;
  const page = (title, body) => `<!doctype html><meta charset="utf-8"><title>${escape(title)}</title><style>${css}</style>${body}`;
  const panels = readFileSync(manifest, 'utf8').trim().split('\n').map(line => {
    const { name, fg, bg, cols = 120 } = JSON.parse(line);
    const ansi = readFileSync(join(out, `${name}.ansi`), 'utf8').replace(/\n$/, '');
    const body = `<section class="panel" style="--fg:${fg};--bg:${bg};--cols:${cols}"><h2>${escape(name)}</h2><pre>${convert(ansi, fg, bg)}</pre></section>`;
    writeFileSync(join(out, `${name}.html`), page(name, body));
    return { name, body };
  });
  const grids = gridPages(panels);
  for (const grid of grids) {
    writeFileSync(join(out, `grid-${grid.name}.html`), page(grid.name, `<main class="grid">${grid.body}</main>`));
  }
  process.stdout.write(JSON.stringify(grids.map(({ name }) => `grid-${name}`)));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  render(...process.argv.slice(2));
}
