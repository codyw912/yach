import assert from 'node:assert/strict';
import { test } from 'node:test';
import { convert, gridPages } from './ansi2html.mjs';

// A wrong SGR branch or a missing default reset changes reviewer-visible colors.
test('renders truecolor, indexed colors, resets and escaped text', () => {
  const result = convert('\x1b[38;2;10;20;30;48;5;196m<&>\x1b[39;49m│', '#abcdef', '#123456');
  assert.equal(result, '<span style="display:inline-block;color:#0a141e;background:#ff0000">&lt;&amp;&gt;</span><span style="display:inline-block;color:#abcdef;background:#123456">│</span>');
});

test('renders monochrome emphasis and reverse without inventing colors', () => {
  assert.equal(convert('\x1b[1;2;3;4;7mX\x1b[0mY', '#dddddd', '#111111'),
    '<span style="display:inline-block;color:#111111;background:#dddddd;font-weight:bold;font-style:italic;text-decoration:underline;opacity:.6">X</span><span style="display:inline-block;color:#dddddd;background:#111111">Y</span>');
});

test('renders tmux colon-form RGB and xterm grayscale', () => {
  assert.equal(convert('\x1b[38:2::10:20:30;48:5:232mX', '#fff', '#000'),
    '<span style="display:inline-block;color:#0a141e;background:#080808">X</span>');
});

// A styled run crossing a newline must not become one block spanning rows,
// or the panel background would be painted as a rectangle across lines.
test('splits a styled run at line breaks so each row is its own cell run', () => {
  assert.equal(convert('\x1b[48;5;196mA\nB\n\nC', '#fff', '#000'),
    ['A', 'B', '', 'C'].map(cell => (cell ? `<span style="display:inline-block;color:#fff;background:#ff0000">${cell}</span>` : '')).join('\n'));
});

test('includes new palettes automatically and splits overflowing families', () => {
  const panels = ['dark', 'light', 'system', 'nocolor', 'catppuccin-one', 'catppuccin-two', 'catppuccin-three', 'catppuccin-four', 'catppuccin-five', 'new-palette'];
  const pages = gridPages(panels.map(name => ({ name, body: `<pre>${name}</pre>` })));
  assert.equal(pages.length, 4);
  assert.deepEqual(pages.map(page => page.name), ['presets', 'catppuccin', 'catppuccin-2', 'other']);
  for (const name of panels) {
    assert.equal(pages.filter(page => page.body.includes(`<pre>${name}</pre>`)).length, 1);
  }
});
