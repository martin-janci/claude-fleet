import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// Redesign step 7.3, the light-mode pass: every component colours itself
// through the theme tokens in app.css (docs/design/tokens.json), so the
// Light board holds without a component-by-component audit. A hex or rgb()
// literal is one colour for both themes: dark-tuned greens and reds sit near
// 2:1 on the light ground. A `var(--name)` nothing declares falls through to
// its fallback in both themes, which is the same bug spelled differently.
// assets_tokens.test.ts and conversation_theme.test.ts hold narrower versions
// of this rule; this one covers every component.

// Paths are relative to the repo root, where Vitest runs (as tokens.test.ts
// reads app.css).
const FILES = readdirSync('src', { recursive: true })
  .filter((p) => p.endsWith('.svelte'))
  .map((path) => ({ path, source: readFileSync(`src/${path}`, 'utf8') }));

const stylesOf = (src: string) =>
  [...src.matchAll(/<style[^>]*>([\s\S]*?)<\/style>/g)]
    .map((m) => m[1])
    .join('\n')
    .replace(/\/\*[\s\S]*?\*\//g, '');

/** Files whose literals are right in both themes, with the reason. */
const EXEMPT: Record<string, string> = {
  'lib/TerminalView.svelte': "the agent's terminal is a dark surface in both themes, as on the Light board",
};

/**
 * Serial files (redesign plan, rules for parallel work: one open PR at a
 * time) that 7.3 left alone so it would not collide with the lane holding
 * them. The lines are pinned exactly: the debt can shrink, never grow. The
 * next PR that edits one of these files moves its lines to tokens and
 * deletes its entry here.
 */
const PENDING: Record<string, { literals: string[]; undeclared: string[] }> = {
  'App.svelte': { literals: ['.status .err { color: #e64a4a; }'], undeclared: [] },
};

/** Black and white used as shade, never as a surface or text colour. */
const SHADE = /^(?:#000(?:0{3})?|rgba?\(0,\s*0,\s*0[,\s/][^)]*\)|rgb\(0 0 0 \/ [^)]*\))$/;

const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\([^)]*\)/g;

/** Lines that colour something with a literal, minus shade in shadows,
 *  scrims and masks. */
function literalLines(css: string): string[] {
  return css.split('\n').flatMap((line) => {
    const hits = (line.match(COLOR_LITERAL) ?? []).filter((lit) => {
      if (!SHADE.test(lit)) return true;
      // Shade is fine where it darkens: a shadow, a modal scrim, a mask.
      return !/box-shadow|mask-image|background:\s*rgba\(0,\s*0,\s*0|color-mix\(in srgb, #000/.test(line);
    });
    return hits.length ? [line.trim()] : [];
  });
}

const DECLARED = new Set<string>();
for (const src of [
  readFileSync('src/app.css', 'utf8'),
  ...readdirSync('src/lib')
    .filter((f) => f.endsWith('.css'))
    .map((f) => readFileSync(`src/lib/${f}`, 'utf8')),
  ...FILES.map((f) => f.source),
]) {
  // `--name:` in a stylesheet or a style attribute, `style:--name=` in markup,
  // and `setProperty('--name'` in script all declare it.
  for (const m of src.matchAll(/(?:^|[\s;{"'`(])(--[a-z0-9-]+)\s*[:=]/gm)) DECLARED.add(m[1]);
  for (const m of src.matchAll(/style:(--[a-z0-9-]+)/g)) DECLARED.add(m[1]);
  for (const m of src.matchAll(/setProperty\(\s*['"`](--[a-z0-9-]+)/g)) DECLARED.add(m[1]);
}

describe('light mode: components colour only through tokens', () => {
  it('finds the components', () => {
    expect(FILES.length).toBeGreaterThan(100);
  });

  for (const { path, source } of FILES) {
    const css = stylesOf(source);
    if (!css) continue;
    const pending = PENDING[path];
    it(`${path} has no colour literal`, () => {
      const lines = path in EXEMPT ? [] : literalLines(css);
      expect(lines).toEqual(pending?.literals ?? []);
    });
    it(`${path} uses only declared custom properties`, () => {
      const undeclared = [...css.matchAll(/var\((--[a-z0-9-]+)/g)]
        .map((m) => m[1])
        .filter((name) => !DECLARED.has(name));
      expect([...new Set(undeclared)]).toEqual(pending?.undeclared ?? []);
    });
  }
});
