// Review w18-7: a key hint written into markup as a Mac chord (`⌘P`, `⌘↵`)
// shows ⌘ on Windows and Linux too. Hints are drawn from the shortcut
// registry (`shortcutLabel`, `Kbd`) or through `platformChord`, so no text
// node of a component's markup may carry ⌘ or ⌥ literally.
import { describe, it, expect } from 'vitest';

const SOURCES = import.meta.glob('/src/**/*.svelte', { query: '?raw', import: 'default', eager: true }) as Record<
  string,
  string
>;

/** The text nodes of `src`'s markup, without script, style, comments or
 *  `{…}` expressions (an expression may pick the Mac form on a Mac). */
function markupText(src: string): string[] {
  const markup = src
    .replace(/<script[\s\S]*?<\/script>/g, '')
    .replace(/<style[\s\S]*?<\/style>/g, '')
    .replace(/<!--[\s\S]*?-->/g, '');
  const out: string[] = [];
  for (const m of markup.matchAll(/>([^<>]*)</g)) {
    const text = m[1].replace(/\{[^{}]*\}/g, '').trim();
    if (text) out.push(text);
  }
  return out;
}

describe('no Mac-only key hint in markup (review w18-7)', () => {
  it('reads the components at all', () => {
    expect(Object.keys(SOURCES).length).toBeGreaterThan(100);
    expect(markupText('<p>a {x} ⌘K</p>')).toEqual(['a  ⌘K']);
  });

  it('no text node carries ⌘ or ⌥', () => {
    const found = Object.entries(SOURCES).flatMap(([path, src]) =>
      markupText(src)
        .filter((t) => /[⌘⌥]/.test(t))
        .map((t) => `${path}: ${t.slice(0, 60)}`),
    );
    expect(found).toEqual([]);
  });
});
