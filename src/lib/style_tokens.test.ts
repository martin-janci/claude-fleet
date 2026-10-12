import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// Review round 10, design-token conformance: a component sizes its type,
// rounds its corners and picks its font through the tokens in app.css
// (docs/ux/…/design-system/tokens.json), never with a literal. A literal
// drifts from the scale the day a token moves, and an off-scale size or
// radius is one the design system never drew. light_mode.test.ts holds the
// same rule for colour; this adds what it leaves out, a named colour.
//
// `em` font sizes stay: they size an inline part against its own text
// (a kbd in a sentence), which a fixed token cannot.

const FILES = readdirSync('src', { recursive: true })
  .filter((p) => p.endsWith('.svelte'))
  .map((p) => `src/${p.replaceAll('\\', '/')}`);

/** Lines of a file's <style> blocks, numbered as in the file, comments blanked. */
function styleLines(src: string): [number, string][] {
  const out: [number, string][] = [];
  for (const m of src.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)) {
    const first = src.slice(0, (m.index ?? 0) + m[0].indexOf('>') + 1).split('\n').length - 1;
    const body = m[1].replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ' '));
    body.split('\n').forEach((line, i) => out.push([first + i + 1, line]));
  }
  return out;
}

/** What each rule refuses, as `file:line text`. */
export function tokenFindings(file: string, src: string): string[] {
  const out: string[] = [];
  for (const [n, line] of styleLines(src)) {
    const hit = (what: string) => out.push(`${file}:${n} ${what}: ${line.trim()}`);
    if (/font-size:\s*[0-9.]+(px|rem)\b/.test(line)) hit('font-size');
    const radius = /border(?:-(?:top|bottom)-(?:left|right))?-radius:\s*([^;}]+)/.exec(line);
    if (radius && /\b(?!0(?:px)?\b)[0-9.]+(px|rem)\b/.test(radius[1])) hit('radius');
    const family = /font-family:\s*([^;}]+)/.exec(line);
    if (family && !/var\(--(?:font-|mono\b)|inherit/.test(family[1]) && !FAMILY_EXEMPT.has(`${file} ${family[1].trim()}`)) hit('font-family');
    if (/(?:^|[\s;{])(?:color|background(?:-color)?|border(?:-[a-z]+)?|outline(?:-color)?|fill|stroke):[^;]*(?<![\w-])(white|black|red|green|blue|gray|grey|orange|yellow|purple)(?![\w-])/.test(line)) hit('named colour');
  }
  return out;
}

/** Families right as literals, with the reason. */
const FAMILY_EXEMPT = new Set([
  // The terminal grid draws Menlo first and measures its cells from that
  // font (the .measure probe); --font-mono puts SF Mono first.
  'src/lib/TerminalView.svelte Menlo, ui-monospace, SFMono-Regular, monospace',
]);

describe('style tokens (review r10)', () => {
  it('reads each kind of literal, and lets tokens, em sizes and zero radii by', () => {
    const src = [
      '<div></div>',
      '<style>',
      '  .a { font-size: 11px; border-radius: 4px; }',
      '  .b { font-size: var(--text-2xs); border-radius: var(--radius-sm) 0 0 var(--radius-sm); }',
      '  .c { font-size: 0.85em; border-radius: 0; font-family: var(--font-mono); }',
      '  .d { font-family: Menlo, monospace; color: white; }',
      '  .e { border-radius: 50%; color: var(--accent-fg); } /* white */',
      '</style>',
    ].join('\n');
    expect(tokenFindings('x.svelte', src).map((l) => l.slice(0, l.indexOf(':', 'x.svelte:'.length)))).toEqual([
      'x.svelte:3 font-size',
      'x.svelte:3 radius',
      'x.svelte:6 font-family',
      'x.svelte:6 named colour',
    ]);
  });

  it('no component uses a literal type size, radius, font or named colour', () => {
    expect(FILES.flatMap((f) => tokenFindings(f, readFileSync(f, 'utf8')))).toEqual([]);
  });
});
