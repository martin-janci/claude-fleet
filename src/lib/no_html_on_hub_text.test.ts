// A gate, not a reminder: hub text (turns, tool output, prompts, session
// state) must never reach Svelte's `{@html}`. Everything the panel renders
// goes through `markdown.ts` + the template, which escape by construction.
//
// The scan is a glob over EVERY component, not a hand-maintained list — a
// list only covers the components someone remembered to add, and the next
// one to render hub text is exactly the one that would be missing.
//
// Vite's `import.meta.glob` (the pattern NewBgSessionDialog.test.ts uses)
// rather than `node:fs`: the project ships no Node types, and the pattern is
// resolved against THIS module's URL, not the directory vitest was started
// from.
import { describe, it, expect } from 'vitest';
import { parseMarkdown, type Block, type Inline } from './markdown';

/** Every `.svelte` file under `src/`, keyed by its path relative to this
 *  module — `./Foo.svelte` for a sibling in `src/lib/`, `../Foo.svelte` for
 *  one in `src/`. */
const SOURCES = import.meta.glob('../**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** Components allowed to use `{@html}` because they demonstrably render no
 *  hub-provided text. Empty, and adding an entry needs that argument made in
 *  a comment beside it. */
const ALLOWED: string[] = [];

describe('Hub text never reaches {@html}', () => {
  const files = Object.keys(SOURCES).sort();

  it('finds the components to scan', () => {
    // A broken glob would make the assertion below vacuously green.
    expect(files.length).toBeGreaterThan(30);
    expect(files).toContain('./ConversationPanel.svelte');
    expect(files).toContain('./MarkdownView.svelte');
    expect(files).toContain('../App.svelte');
  });

  it('no component uses {@html}', () => {
    const offenders = files.filter((f) => !ALLOWED.includes(f) && SOURCES[f].includes('{@html'));
    expect(
      offenders,
      `{@html} found in ${offenders.join(', ')}. Hub data is rendered through MarkdownView.svelte ` +
        'or plain template syntax — both escape. If a component truly renders no hub text, argue ' +
        'it in a comment and add it to ALLOWED.',
    ).toEqual([]);
  });

  it('the markdown parser has no html node type — tags survive as literal text', () => {
    // Shape check: the parser produces a paragraph of inline TEXT carrying
    // the markup verbatim. Nothing downstream can re-interpret it as HTML,
    // because no node ever says "this is HTML".
    expect(parseMarkdown('<img src=x onerror=alert(1)>')).toEqual([
      { t: 'para', c: [{ t: 'text', v: '<img src=x onerror=alert(1)>' }] },
    ]);

    const kinds = new Set<string>();
    const walkInline = (nodes: Inline[]) => {
      for (const n of nodes) {
        kinds.add(n.t);
        if ('c' in n) walkInline(n.c);
      }
    };
    const walk = (bs: Block[]) => {
      for (const b of bs) {
        kinds.add(b.t);
        if (b.t === 'quote') walk(b.c);
        else if (b.t === 'list') for (const item of b.items) walk(item.c);
        else if (b.t === 'table') for (const cells of [b.head, ...b.rows]) for (const c of cells) walkInline(c);
        else if ('c' in b) walkInline(b.c);
      }
    };
    walk(
      parseMarkdown(
        '# <b>h</b>\n\n<img src=x onerror=alert(1)>\n\n> <script>alert(1)</script>\n\n' +
          '- <iframe src=javascript:alert(1)>\n\n| a |\n|---|\n| <svg onload=alert(1)> |\n',
      ),
    );
    expect(kinds.has('html')).toBe(false);
  });
});
