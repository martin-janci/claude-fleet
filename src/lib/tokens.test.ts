import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { relativeLuminance, contrastRatio, THEME, CONTRAST_PAIRS } from './tokens';

describe('contrast maths', () => {
  it('matches known WCAG values', () => {
    expect(relativeLuminance('#ffffff')).toBeCloseTo(1, 5);
    expect(relativeLuminance('#000000')).toBeCloseTo(0, 5);
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 2);
    // The bug Task 1 fixed, kept as a regression witness.
    expect(contrastRatio('#50c86e', '#fafafa')).toBeCloseTo(2.05, 2);
  });
});

describe('every documented token pair clears its floor', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const pair of CONTRAST_PAIRS) {
      it(`${mode}: ${pair.fg} on ${pair.bg} >= ${pair.min}:1 (${pair.note})`, () => {
        const fg = THEME[mode][pair.fg];
        const bg = THEME[mode][pair.bg];
        expect(fg, `${pair.fg} missing from THEME.${mode}`).toBeTruthy();
        expect(bg, `${pair.bg} missing from THEME.${mode}`).toBeTruthy();
        expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(pair.min);
      });
    }
  }
});

// ---------------------------------------------------------------------------
// THEME against app.css itself.
//
// Without this, THEME is a copy nothing checks: a colour changed in app.css
// leaves every assertion above passing against the value the app no longer
// uses. `readFileSync`, not a Vite `?raw` import, for the reason
// `controls.test.ts` gives — Vitest strips `.css` module content either way.

// Comments stripped first: app.css's own prose names tokens ("--border is
// 1.26:1 against --bg: …"), which a declaration regex would otherwise read
// as declarations.
const appCss = readFileSync('src/app.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** Byte-mix two #rrggbb colours the way `color-mix(in srgb, a P%, b)` does. */
function mixSrgb(a: string, b: string, pct: number): string {
  const part = (hex: string, i: number) => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
  const ch = (i: number) => Math.round((part(a, i) * pct + part(b, i) * (100 - pct)) / 100);
  return `#${[0, 1, 2].map((i) => ch(i).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * The custom properties declared in the block that starts at `from`, with
 * `var(--x)` and `color-mix(in srgb, var(--x) N%, var(--y))` resolved against
 * the same block — the two forms app.css actually uses for a colour.
 */
function parseThemeBlock(css: string, from: number): Record<string, string> {
  const open = css.indexOf('{', from);
  let depth = 0;
  let end = open;
  for (let i = open; i < css.length; i++) {
    if (css[i] === '{') depth++;
    else if (css[i] === '}' && --depth === 0) {
      end = i;
      break;
    }
  }
  const body = css.slice(open + 1, end);
  const raw: Record<string, string> = {};
  for (const m of body.matchAll(/--([a-z0-9-]+)\s*:\s*([^;]+);/g)) raw[m[1]] = m[2].trim();

  const resolve = (v: string, seen = 0): string => {
    if (seen > 4) return v;
    const alias = v.match(/^var\(--([a-z0-9-]+)\)$/);
    if (alias) return resolve(raw[alias[1]] ?? '', seen + 1);
    const mix = v.match(/^color-mix\(in srgb,\s*var\(--([a-z0-9-]+)\)\s*(\d+)%,\s*var\(--([a-z0-9-]+)\)\)$/);
    if (mix) {
      return mixSrgb(resolve(raw[mix[1]] ?? '', seen + 1), resolve(raw[mix[3]] ?? '', seen + 1), Number(mix[2]));
    }
    return v;
  };
  return Object.fromEntries(Object.entries(raw).map(([k, v]) => [k, resolve(v)]));
}

/** The four blocks app.css declares a theme in, by mode. */
const BLOCKS: Record<'light' | 'dark', string[]> = {
  light: ['\n:root {', "\n:root[data-theme='light'] {"],
  dark: ['@media (prefers-color-scheme: dark) {\n  :root {', "\n:root[data-theme='dark'] {"],
};

describe('THEME is the palette app.css actually ships', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const marker of BLOCKS[mode]) {
      it(`${mode}: every THEME entry matches ${marker.trim().split('\n')[0]}`, () => {
        const at = appCss.indexOf(marker);
        expect(at, `block not found in app.css: ${marker}`).toBeGreaterThanOrEqual(0);
        const declared = parseThemeBlock(appCss, at + marker.lastIndexOf('{'));
        for (const [name, value] of Object.entries(THEME[mode])) {
          expect(declared[name], `--${name} is not declared in this block`).toBeTruthy();
          expect(declared[name]!.toLowerCase(), `--${name}`).toBe(value.toLowerCase());
        }
      });
    }
  }

  // app.css declares each theme twice (the media query and the explicit
  // data-theme override). A value changed in one and not the other is a
  // theme that silently differs by how it was chosen.
  for (const mode of ['light', 'dark'] as const) {
    it(`${mode}: the two blocks that declare it agree on every THEME token`, () => {
      const [a, b] = BLOCKS[mode].map((marker) => {
        const at = appCss.indexOf(marker);
        return parseThemeBlock(appCss, at + marker.lastIndexOf('{'));
      });
      for (const name of Object.keys(THEME[mode])) {
        expect(a[name]?.toLowerCase(), `--${name}`).toBe(b[name]?.toLowerCase());
      }
    });
  }

  it('--ring is an alias for --accent, not a second copy of the colour', () => {
    expect(appCss.match(/--ring:\s*var\(--accent\);/g)?.length).toBe(4);
  });
});
