import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// The 11 px type floor (Orbit Fleet redesign step 7.2): no font-size in the
// app's styles renders under 11 px. jsdom has no layout, so this reads the
// source. The root is 14 px (`html { font-size: 14px }` in app.css), so a rem
// is 14 px; an em is read against the same 14 px, the body size, which is
// what it resolves to unless a parent is larger.

const ROOT_PX = 14;
const FLOOR_PX = 11;

/** Every `font-size: N(px|rem|em)` under the floor, as `file:line value`. */
export function underFloor(file: string, src: string): string[] {
  const out: string[] = [];
  src.split('\n').forEach((line, i) => {
    for (const m of line.matchAll(/font-size:\s*([0-9.]+)(px|rem|em)\b/g)) {
      const px = m[2] === 'px' ? Number(m[1]) : Number(m[1]) * ROOT_PX;
      if (px < FLOOR_PX - 1e-9) out.push(`${file}:${i + 1} ${m[1]}${m[2]}`);
    }
  });
  return out;
}

/** Selectors that style a control a pointer aims at. */
const CONTROL_SELECTOR = /\bbutton\b|btn|\[role=["']?(button|tab|menuitem)|\bselect\b|\bsummary\b/;

/**
 * Every rule on a control that caps or floors it under the 24 px target:
 * `min-*` or `max-*` height or width below 24 px. A plain `height: 18px`
 * is fine, because app.css's min-block-size wins over it.
 */
export function underTarget(file: string, src: string): string[] {
  const out: string[] = [];
  const style = file.endsWith('.svelte') ? /<style[^>]*>([\s\S]*?)<\/style>/.exec(src) : null;
  const css = style ? style[1] : file.endsWith('.css') ? src : '';
  const base = style ? src.slice(0, style.index + style[0].indexOf('>') + 1).split('\n').length - 1 : 0;
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ' '));
  for (const rule of clean.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = rule[1].trim();
    // Pseudo-elements and parts inside a control are not the target.
    const own = selector.split(',').filter((s) => CONTROL_SELECTOR.test(s) && !/::|\s(svg|span|img|i)\b/.test(s));
    if (own.length === 0) continue;
    for (const d of rule[2].matchAll(/(?<![-\w])((?:min|max)-(?:height|width|block-size|inline-size)):\s*([0-9.]+)(px|rem)\b/g)) {
      const px = d[3] === 'px' ? Number(d[2]) : Number(d[2]) * ROOT_PX;
      if (px < TARGET_PX - 1e-9) {
        const line = base + clean.slice(0, (rule.index ?? 0) + rule[1].length + 1 + (d.index ?? 0)).split('\n').length;
        out.push(`${file}:${line} ${own[0].trim()} ${d[1]}: ${d[2]}${d[3]}`);
      }
    }
  }
  return out;
}

const TARGET_PX = 24;

function appStyleFiles(): string[] {
  return readdirSync('src', { recursive: true })
    .filter((n) => /\.(svelte|css)$/.test(n))
    .map((n) => `src/${n.replaceAll('\\', '/')}`);
}

describe('24 px targets', () => {
  it('app.css floors every control at --control-h, which is 24 px', () => {
    const css = readFileSync('src/app.css', 'utf8');
    expect(css).toMatch(/--control-h: 24px;/);
    expect(css).toMatch(/:where\(button, \[role='button'\][^{]*\{\s*min-block-size: var\(--control-h\);\s*min-inline-size: var\(--control-h\);/);
  });

  it('reads a cap or floor under 24 px on a control, and not on its parts', () => {
    const css = '.icon-btn { min-width: 1.2rem; } .row button { max-height: 20px; } .btn::before { min-width: 4px; } .btn svg { max-width: 12px; } .ok-btn { height: 18px; min-height: 24px; }';
    expect(underTarget('x.css', css)).toEqual(['x.css:1 .icon-btn min-width: 1.2rem', 'x.css:1 .row button max-height: 20px']);
  });

  it('no component shrinks a control under 24 px', () => {
    expect(appStyleFiles().flatMap((f) => underTarget(f, readFileSync(f, 'utf8')))).toEqual([]);
  });
});

describe('11 px type floor', () => {
  it('reads px, rem and em against the 14 px root', () => {
    const css = '.a { font-size: 10px; } .b { font-size: 0.75rem; } .c { font-size: 0.8rem; } .d { font-size: 11px; } .e{font-size:0.7em}';
    expect(underFloor('x.css', css)).toEqual(['x.css:1 10px', 'x.css:1 0.75rem', 'x.css:1 0.7em']);
  });

  it('the root is still 14 px', () => {
    expect(readFileSync('src/app.css', 'utf8')).toMatch(/html, body, #app \{[^}]*font-size: 14px;/);
  });

  it('no style in the app sets text under 11 px', () => {
    const files = readdirSync('src', { recursive: true })
      .filter((n) => /\.(svelte|css)$/.test(n))
      .map((n) => `src/${n.replaceAll('\\', '/')}`);
    expect(files.length).toBeGreaterThan(50);
    expect(files.flatMap((f) => underFloor(f, readFileSync(f, 'utf8')))).toEqual([]);
  });
});
