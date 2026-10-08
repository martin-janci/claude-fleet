import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';
import { relativeLuminance, contrastRatio, composite, THEME, CONTRAST_PAIRS } from './tokens';

describe('contrast maths', () => {
  it('matches known WCAG values', () => {
    expect(relativeLuminance('#ffffff')).toBeCloseTo(1, 5);
    expect(relativeLuminance('#000000')).toBeCloseTo(0, 5);
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 2);
    // The bug Task 1 fixed, kept as a regression witness.
    expect(contrastRatio('#50c86e', '#fafafa')).toBeCloseTo(2.05, 2);
  });

  it('composites a tint the way the browser does', () => {
    expect(composite('rgba(0,0,0,0.5)', '#ffffff')).toBe('#808080');
    expect(composite('rgba(210,155,74,0.13)', '#161616')).toBe('#2e271d');
  });
});

describe('every documented token pair clears its floor', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const pair of CONTRAST_PAIRS) {
      const on = pair.tint ? `${pair.tint} over ${pair.bg}` : pair.bg;
      it(`${mode}: ${pair.fg} on ${on} >= ${pair.min}:1 (${pair.note})`, () => {
        const fg = THEME[mode][pair.fg];
        const ground = THEME[mode][pair.bg];
        expect(fg, `${pair.fg} missing from THEME.${mode}`).toBeTruthy();
        expect(ground, `${pair.bg} missing from THEME.${mode}`).toBeTruthy();
        const bg = pair.tint ? composite(THEME[mode][pair.tint]!, ground) : ground;
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

// ---------------------------------------------------------------------------
// THEME and app.css against the design manual's snapshot.
//
// docs/design/tokens.json is the Orbit Fleet manual's tokens, the source of
// truth (ground rule 8 of the redesign plan). Every token it names ships in
// app.css under the same name and value, and every colour is also in THEME,
// so the contrast suite above measures the manual's palette.

interface Tok<V> {
  name: string;
  value: V;
}
interface Snapshot {
  color: { tokens: Tok<Record<'light' | 'dark', string>>[] };
  type: {
    families: Record<'sans' | 'mono', string>;
    groups: { styles: { name: string; fontSize: string; lineHeight: string; fontWeight: number }[] }[];
  };
  spacing: { tokens: Tok<string>[] };
  radius: { tokens: Tok<string>[] };
  shadow: { tokens: Tok<Record<'light' | 'dark', string>>[] };
  duration: { tokens: Tok<string>[] };
}

const snapshot: Snapshot = JSON.parse(readFileSync('docs/design/tokens.json', 'utf8'));
const norm = (v: string) => v.toLowerCase().replace(/\s+/g, '');

/** A snapshot colour with its `{name}` references resolved, as THEME holds it. */
function snapshotColour(mode: 'light' | 'dark', name: string, seen = 0): string {
  const tok = snapshot.color.tokens.find((t) => t.name === name);
  if (!tok) throw new Error(`{${name}} is not a token in the snapshot`);
  const ref = tok.value[mode].match(/^\{([a-z0-9-]+)\}$/);
  return ref && seen < 4 ? snapshotColour(mode, ref[1], seen + 1) : tok.value[mode];
}

describe('app.css and THEME follow the design manual (docs/design/tokens.json)', () => {
  for (const mode of ['light', 'dark'] as const) {
    it(`${mode}: every snapshot colour is in THEME with the same value`, () => {
      for (const { name } of snapshot.color.tokens) {
        expect(THEME[mode][name], `${name} missing from THEME.${mode}`).toBeTruthy();
        expect(norm(THEME[mode][name]!), name).toBe(norm(snapshotColour(mode, name)));
      }
    });

    for (const marker of BLOCKS[mode]) {
      it(`${mode}: every snapshot colour and shadow is declared in ${marker.trim().split('\n')[0]}`, () => {
        const at = appCss.indexOf(marker);
        const declared = parseThemeBlock(appCss, at + marker.lastIndexOf('{'));
        for (const { name } of snapshot.color.tokens) {
          expect(declared[name], `--${name} is not declared in this block`).toBeTruthy();
          expect(norm(declared[name]!), `--${name}`).toBe(norm(snapshotColour(mode, name)));
        }
        for (const { name, value } of snapshot.shadow.tokens) {
          expect(norm(declared[name] ?? ''), `--${name}`).toBe(norm(value[mode]));
        }
      });
    }
  }

  it('a reference in the snapshot stays a var() in app.css, not a copied value', () => {
    for (const { name, value } of snapshot.color.tokens) {
      for (const v of Object.values(value)) {
        const ref = v.match(/^\{([a-z0-9-]+)\}$/);
        if (!ref) continue;
        const decl = new RegExp(`--${name}:\\s*var\\(--${ref[1]}\\);`, 'g');
        expect(appCss.match(decl)?.length ?? 0, `--${name}: var(--${ref[1]})`).toBeGreaterThanOrEqual(2);
      }
    }
  });

  it('type, spacing, radius and duration tokens are declared in :root', () => {
    const at = appCss.indexOf(BLOCKS.light[0]);
    const root = parseThemeBlock(appCss, at + BLOCKS.light[0].lastIndexOf('{'));
    const expected: Record<string, string> = {
      'font-sans': snapshot.type.families.sans,
      'font-mono': snapshot.type.families.mono,
    };
    for (const g of snapshot.type.groups) {
      for (const s of g.styles) {
        const n = s.name.startsWith('text-') ? s.name : `text-${s.name}`;
        expected[n] = s.fontSize;
        expected[`${n}-lh`] = s.lineHeight;
        expected[`${n}-weight`] = String(s.fontWeight);
      }
    }
    for (const t of [...snapshot.spacing.tokens, ...snapshot.radius.tokens, ...snapshot.duration.tokens]) {
      expected[t.name] = t.value;
    }
    for (const [name, value] of Object.entries(expected)) {
      expect(root[name], `--${name} is not declared in :root`).toBeTruthy();
      expect(norm(root[name]!), `--${name}`).toBe(norm(value));
    }
  });

  it('every focusable element gets the ring, at zero specificity', () => {
    expect(appCss).toMatch(
      /:where\(:focus-visible\)\s*\{\s*outline:\s*var\(--ring-w\) solid var\(--ring\);\s*outline-offset:\s*var\(--ring-offset\);/,
    );
  });
});
