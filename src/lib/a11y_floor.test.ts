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
