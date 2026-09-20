import { describe, it, expect, afterEach } from 'vitest';
import {
  HL_MAX_RANGES,
  highlightNames,
  highlightCss,
  highlightRegistry,
  collectMatchRanges,
  clearHighlights,
  paintHighlights,
} from './conversation_highlight';

/** A thread fragment: rows keyed the way the panel keys them. */
function thread(html: string): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = html;
  document.body.appendChild(root);
  return root;
}

afterEach(() => {
  document.body.innerHTML = '';
  const g = globalThis as { CSS?: unknown; Highlight?: unknown };
  delete g.CSS;
  delete g.Highlight;
});

/** jsdom ships neither CSS.highlights nor Highlight; stand both up. */
function withHighlightApi() {
  const store = new Map<string, unknown>();
  const g = globalThis as unknown as { CSS: unknown; Highlight: unknown };
  g.CSS = { highlights: { set: (n: string, h: unknown) => store.set(n, h), delete: (n: string) => store.delete(n) } };
  g.Highlight = class {
    ranges: Range[];
    constructor(...r: Range[]) {
      this.ranges = r;
    }
  };
  return store;
}

describe('highlightNames / highlightCss', () => {
  it('keeps two panels off each other’s names', () => {
    expect(highlightNames(1)).not.toEqual(highlightNames(2));
    const n = highlightNames(7);
    expect(n.all).not.toBe(n.current);
  });

  it('states both rules through the theme token', () => {
    const css = highlightCss(highlightNames(1));
    expect(css).toContain('::highlight(conv-find-1)');
    expect(css).toContain('::highlight(conv-find-current-1)');
    expect(css).toContain('var(--usage-warn)');
    expect(css).not.toMatch(/#[0-9a-f]{6}/i);
  });
});

describe('highlightRegistry', () => {
  it('is null without the API', () => {
    expect(highlightRegistry()).toBeNull();
  });

  it('is null when Highlight is missing even though CSS.highlights is not', () => {
    (globalThis as unknown as { CSS: unknown }).CSS = { highlights: {} };
    expect(highlightRegistry()).toBeNull();
  });

  it('is the registry once both are there', () => {
    withHighlightApi();
    expect(highlightRegistry()).not.toBeNull();
  });
});

describe('collectMatchRanges', () => {
  const opts = (over: Partial<{ keys: Set<string>; current: string | null; query: string }> = {}) => ({
    keys: new Set(['t0']),
    current: 't0' as string | null,
    query: 'bug',
    ...over,
  });

  it('finds every occurrence in a matching row', () => {
    const root = thread('<section data-row-key="t0"><p>a bug and another bug</p></section>');
    const { all, current } = collectMatchRanges(root, opts());
    expect(all).toHaveLength(2);
    expect(current).toHaveLength(2);
    expect(all[0].toString()).toBe('bug');
  });

  it('ignores rows the caller did not name', () => {
    const root = thread(
      '<section data-row-key="t0"><p>bug</p></section><section data-row-key="t1"><p>bug</p></section>',
    );
    expect(collectMatchRanges(root, opts()).all).toHaveLength(1);
  });

  it('separates the current row from the rest', () => {
    const root = thread(
      '<section data-row-key="t0"><p>bug</p></section><section data-row-key="t1"><p>bug bug</p></section>',
    );
    const { all, current } = collectMatchRanges(root, opts({ keys: new Set(['t0', 't1']), current: 't1' }));
    expect(all).toHaveLength(3);
    expect(current).toHaveLength(2);
  });

  it('matches without regard to case', () => {
    const root = thread('<section data-row-key="t0"><p>A BUG</p></section>');
    expect(collectMatchRanges(root, opts()).all).toHaveLength(1);
  });

  it('skips chrome: controls, times and hidden text', () => {
    const root = thread(`
      <section data-row-key="t0">
        <button>bug</button>
        <time datetime="x">bug</time>
        <span aria-hidden="true">bug</span>
        <span role="button">bug</span>
        <p>bug</p>
      </section>`);
    // Only the paragraph is the conversation's own text.
    const { all } = collectMatchRanges(root, opts());
    expect(all).toHaveLength(1);
    expect(all[0].startContainer.parentElement?.tagName).toBe('P');
  });

  it('matches nothing for an empty or blank query, or with no keys', () => {
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    expect(collectMatchRanges(root, opts({ query: '' })).all).toHaveLength(0);
    expect(collectMatchRanges(root, opts({ query: '   ' })).all).toHaveLength(0);
    expect(collectMatchRanges(root, opts({ keys: new Set() })).all).toHaveLength(0);
  });

  it('stops at the range cap rather than building an unbounded list', () => {
    const root = thread(`<section data-row-key="t0"><p>${'bug '.repeat(HL_MAX_RANGES + 50)}</p></section>`);
    expect(collectMatchRanges(root, opts()).all).toHaveLength(HL_MAX_RANGES);
  });
});

describe('paintHighlights / clearHighlights', () => {
  const names = highlightNames('p');
  const opts = { keys: new Set(['t0']), current: 't0', query: 'bug' };

  it('does nothing at all without the API', () => {
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    expect(() => paintHighlights(root, names, opts)).not.toThrow();
    expect(() => clearHighlights(names)).not.toThrow();
  });

  it('registers both names when there is something to paint', () => {
    const store = withHighlightApi();
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    paintHighlights(root, names, opts);
    expect(store.has(names.all)).toBe(true);
    expect(store.has(names.current)).toBe(true);
  });

  it('clears rather than painting an empty highlight when nothing matches', () => {
    const store = withHighlightApi();
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    paintHighlights(root, names, opts);
    paintHighlights(root, names, { ...opts, query: 'nothing like this' });
    expect(store.has(names.all)).toBe(false);
    expect(store.has(names.current)).toBe(false);
  });

  it('clears when there is no root to search', () => {
    const store = withHighlightApi();
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    paintHighlights(root, names, opts);
    paintHighlights(null, names, opts);
    expect(store.has(names.all)).toBe(false);
  });

  it('leaves no stale highlight when the browser refuses one', () => {
    const store = withHighlightApi();
    store.set(names.all, 'stale');
    (globalThis as unknown as { Highlight: unknown }).Highlight = function () {
      throw new Error('nope');
    };
    const root = thread('<section data-row-key="t0"><p>bug</p></section>');
    paintHighlights(root, names, opts);
    expect(store.has(names.all)).toBe(false);
  });
});
