import { readFileSync, readdirSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// The focus ring (design manual, review r11): every focusable element shows
// a 2 px `--ring` when it has keyboard focus. app.css draws it at zero
// specificity (`:where(:focus-visible)`), so a component rule that says
// `outline: none` wins over it and the ring is gone. jsdom has no
// :focus-visible and no paint, so this reads the source: a rule that drops
// the outline on a focusable element must leave a ring of its own.

/** A declaration that removes the outline. */
const NO_OUTLINE = /(?<![-\w])outline\s*:\s*(none|0)\s*(;|$)/;
/** A declaration that draws a ring: a real outline or a box-shadow. A
 *  box-shadow built from a `*-soft` tint is a halo of about 1.2:1, not a
 *  ring (review w19-3). */
const DRAWS_RING = /(?<![-\w])outline\s*:(?!\s*(none|0)\b)[^;]+|box-shadow\s*:(?!\s*none\b)(?![^;]*-soft\b)[^;]+/;
/** Elements that take focus without a tabindex. */
const FOCUSABLE_TAG = /^(input|textarea|select|button|summary|a)$/;

interface Rule {
  selector: string;
  body: string;
  line: number;
}

function rulesOf(file: string, src: string): Rule[] {
  const style = file.endsWith('.svelte') ? /<style[^>]*>([\s\S]*?)<\/style>/.exec(src) : null;
  const css = style ? style[1] : file.endsWith('.css') ? src : '';
  const base = style ? src.slice(0, style.index + style[0].indexOf('>') + 1).split('\n').length - 1 : 0;
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, ' '));
  const out: Rule[] = [];
  for (const m of clean.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const lead = m[1].length - m[1].trimStart().length;
    out.push({
      selector: m[1].trim(),
      body: m[2],
      line: base + clean.slice(0, (m.index ?? 0) + lead).split('\n').length,
    });
  }
  return out;
}

/** The last class of a compound selector part (`.a .b:focus` → `b`). */
function lastClass(part: string): string | null {
  const all = [...part.matchAll(/\.([\w-]+)/g)];
  return all.length ? all[all.length - 1][1] : null;
}

/** Whether markup puts class `c` on an element that takes keyboard focus. */
function focusableInMarkup(src: string, c: string): boolean {
  const markup = src.replace(/<style[\s\S]*?<\/style>/, '');
  const re = new RegExp(`<([a-zA-Z]+)\\b[^>]*?class(?:=["'{][^"'}]*\\b|:)${c}\\b[^>]*>`, 'g');
  for (const m of markup.matchAll(re)) {
    if (/tabindex=["{]?-1/.test(m[0])) continue;
    if (FOCUSABLE_TAG.test(m[1]) || /tabindex=/.test(m[0])) return true;
  }
  return false;
}

/** Every rule that hides keyboard focus, as `file:line selector`. */
export function ringless(file: string, src: string): string[] {
  const rules = rulesOf(file, src);
  const hasRing = (c: string) =>
    rules.some(
      (r) =>
        DRAWS_RING.test(r.body) &&
        r.selector.split(',').some((p) => new RegExp(`\\.${c}\\b[^,]*:focus(-visible|-within)\\b`).test(p) || new RegExp(`:focus(-visible|-within)\\b[^,]*\\.${c}\\b`).test(p)),
    );
  const out: string[] = [];
  for (const r of rules) {
    if (!NO_OUTLINE.test(r.body) || DRAWS_RING.test(r.body)) continue;
    for (const part of r.selector.split(',').map((p) => p.trim())) {
      if (/:focus-within/.test(part)) continue;
      const c = lastClass(part);
      if (/:focus-visible/.test(part)) {
        out.push(`${file}:${r.line} ${part}`);
      } else if (/:focus\b/.test(part)) {
        if (!c || !hasRing(c)) out.push(`${file}:${r.line} ${part}`);
      } else if (c && !/:/.test(part.split(' ').pop() ?? '') && focusableInMarkup(src, c) && !hasRing(c)) {
        out.push(`${file}:${r.line} ${part}`);
      }
    }
  }
  return out;
}

/** An outline thinner than the house `--ring-w` (2 px). */
const THIN_OUTLINE = /(?<![-\w])outline(-width)?\s*:\s*(0?\.\d+|1)px\b/;

/** Every focus rule that swaps the 2 px ring for a thinner one (review
 *  w19-4: a 1 px accent hairline on every tool line and screenshot). */
export function thinRings(file: string, src: string): string[] {
  return rulesOf(file, src)
    .filter((r) => THIN_OUTLINE.test(r.body))
    .flatMap((r) =>
      r.selector
        .split(',')
        .map((p) => p.trim())
        .filter((p) => /:focus(-visible)?\b/.test(p))
        .map((p) => `${file}:${r.line} ${p}`),
    );
}

/**
 * Rules that drop the outline on purpose, each with where its focus shows
 * instead. Keyed `file selector`; a row nothing matches any more fails, so
 * the list cannot go stale.
 */
const RING_ELSEWHERE: Record<string, string> = {
  'src/lib/ConversationPanel.svelte .composer-input:focus': 'the composer shell draws the ring (.composer-shell:focus-within)',
  'src/lib/QueryInput.svelte .field': 'the query box draws the ring (.query:focus-within)',
  'src/lib/TerminalView.svelte .ime-proxy': 'the terminal grid draws the ring (.grid.kb-focus, keyboard focus only)',
};

function appStyleFiles(): string[] {
  return readdirSync('src', { recursive: true })
    .filter((n) => /\.(svelte|css)$/.test(n))
    .map((n) => `src/${n.replaceAll('\\', '/')}`);
}

describe('focus ring', () => {
  it('reads a rule that drops the ring on keyboard focus', () => {
    const src = '<button class="mi">x</button>\n<style>\n  .mi:focus-visible { background: red; outline: none; }\n</style>';
    expect(ringless('src/lib/X.svelte', src)).toEqual(['src/lib/X.svelte:3 .mi:focus-visible']);
  });

  it('accepts a :focus rule whose :focus-visible twin draws the ring', () => {
    const src = '<input class="q" />\n<style>\n  .q:focus { outline: none; }\n  .q:focus-visible { outline: 2px solid; }\n</style>';
    expect(ringless('src/lib/X.svelte', src)).toEqual([]);
    expect(ringless('src/lib/X.svelte', src.replace(/\n  \.q:focus-visible[^\n]*/, ''))).toEqual(['src/lib/X.svelte:3 .q:focus']);
  });

  it('flags a focusable element whose base rule drops the outline', () => {
    const src = '<div class="list" tabindex="0"></div>\n<style>\n  .list { outline: none; }\n</style>';
    expect(ringless('src/lib/X.svelte', src)).toEqual(['src/lib/X.svelte:3 .list']);
    expect(ringless('src/lib/X.svelte', src.replace('tabindex="0"', 'tabindex="-1"'))).toEqual([]);
  });

  it('does not count a *-soft box-shadow halo as a ring (review w19-3)', () => {
    const src = '<input class="inp" />\n<style>\n  .inp:focus { outline: none; box-shadow: 0 0 0 2px var(--accent-soft); }\n</style>';
    expect(ringless('src/lib/X.svelte', src)).toEqual(['src/lib/X.svelte:3 .inp:focus']);
    expect(ringless('src/lib/X.svelte', src.replace('--accent-soft', '--ring'))).toEqual([]);
  });

  it('reads a focus rule that draws a ring thinner than --ring-w (review w19-4)', () => {
    const src = '<button class="t">x</button>\n<style>\n  .t:focus-visible { outline: 1px solid var(--accent); }\n  .t { outline: 1px solid red; }\n</style>';
    expect(thinRings('src/lib/X.svelte', src)).toEqual(['src/lib/X.svelte:3 .t:focus-visible']);
  });

  it('no focus rule draws a ring thinner than --ring-w', () => {
    expect(appStyleFiles().flatMap((f) => thinRings(f, readFileSync(f, 'utf8')))).toEqual([]);
  });

  it('no component hides keyboard focus without a ring of its own', () => {
    const found = appStyleFiles().flatMap((f) => ringless(f, readFileSync(f, 'utf8')));
    const key = (hit: string) => hit.replace(/:\d+ /, ' ');
    expect(found.filter((h) => !(key(h) in RING_ELSEWHERE))).toEqual([]);
    const live = new Set(found.map(key));
    expect(Object.keys(RING_ELSEWHERE).filter((k) => !live.has(k))).toEqual([]);
  });
});
