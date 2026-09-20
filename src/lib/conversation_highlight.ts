// Painting the find query inside the conversation with the CSS Custom
// Highlight API. Split out of ConversationPanel: none of it is component
// state — it is DOM work the panel drives from two effects, and it is far
// easier to pin down on a hand-built tree than through a panel render.
//
// Where the API is missing (jsdom, older engines) every entry point is a
// no-op and the row outline is the only highlight the user gets.

/** The subset of `CSS.highlights` this module uses. */
export type HighlightRegistry = {
  set(name: string, highlight: unknown): void;
  delete(name: string): void;
};

/** Ranges painted before the walk gives up. A find that matches most of a
 *  long thread must not build an unbounded list. */
export const HL_MAX_RANGES = 2_000;

/** Text the conversation did not say: control labels (Copy, Show more, a
 *  tool row's chrome), timestamps, form fields, and hidden chrome such as a
 *  lone tool call's folded summary. */
const CHROME = 'button, time, input, textarea, select, [role="button"], [aria-hidden="true"]';

/** The two `::highlight()` names one panel paints under. Names cannot be
 *  dynamic inside a component's stylesheet, so each panel carries its own
 *  pair and its own rules. */
export function highlightNames(suffix: number | string): { all: string; current: string } {
  return { all: `conv-find-${suffix}`, current: `conv-find-current-${suffix}` };
}

/** The stylesheet text for one panel's pair of names. */
export function highlightCss(names: { all: string; current: string }): string {
  return (
    `::highlight(${names.all}) { background-color: color-mix(in srgb, var(--usage-warn) 35%, transparent); }\n` +
    `::highlight(${names.current}) { background-color: color-mix(in srgb, var(--usage-warn) 75%, transparent); color: var(--bg); }`
  );
}

/** `CSS.highlights`, or null where the API (or its `Highlight`
 *  constructor) is not there. Never throws. */
export function highlightRegistry(): HighlightRegistry | null {
  try {
    const reg = (globalThis.CSS as unknown as { highlights?: unknown } | undefined)?.highlights;
    if (!reg || typeof (globalThis as { Highlight?: unknown }).Highlight !== 'function') return null;
    return reg as HighlightRegistry;
  } catch {
    return null;
  }
}

/** Every occurrence of `query` in the conversation's own text inside the
 *  rows named by `keys`, plus the subset that falls in the `current` row.
 *  `query` is matched case-insensitively; an empty one matches nothing. */
export function collectMatchRanges(
  root: ParentNode,
  opts: { keys: ReadonlySet<string>; current: string | null; query: string },
): { all: Range[]; current: Range[] } {
  const q = opts.query.trim().toLowerCase();
  const all: Range[] = [];
  const current: Range[] = [];
  if (q === '' || opts.keys.size === 0) return { all, current };

  for (const el of Array.from(root.querySelectorAll<HTMLElement>('[data-row-key]'))) {
    const key = el.dataset.rowKey ?? '';
    if (!opts.keys.has(key)) continue;
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) =>
        n.parentElement?.closest(CHROME) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
    });
    for (let n = walker.nextNode(); n && all.length < HL_MAX_RANGES; n = walker.nextNode()) {
      const text = (n.textContent ?? '').toLowerCase();
      for (let at = text.indexOf(q); at !== -1 && all.length < HL_MAX_RANGES; at = text.indexOf(q, at + q.length)) {
        const r = document.createRange();
        r.setStart(n, at);
        r.setEnd(n, at + q.length);
        all.push(r);
        if (key === opts.current) current.push(r);
      }
    }
  }
  return { all, current };
}

/** Drop both of this panel's highlights. Safe to call when the API is
 *  absent or nothing was ever painted. */
export function clearHighlights(names: { all: string; current: string }): void {
  const reg = highlightRegistry();
  if (!reg) return;
  reg.delete(names.all);
  reg.delete(names.current);
}

/** Paint `query`'s matches under this panel's names. Anything the browser
 *  refuses leaves the panel with no highlights rather than stale ones. */
export function paintHighlights(
  root: ParentNode | null | undefined,
  names: { all: string; current: string },
  opts: { keys: ReadonlySet<string>; current: string | null; query: string },
): void {
  const reg = highlightRegistry();
  if (!reg || !root) {
    clearHighlights(names);
    return;
  }
  try {
    const { all, current } = collectMatchRanges(root, opts);
    if (all.length === 0) {
      clearHighlights(names);
      return;
    }
    const H = (globalThis as unknown as { Highlight: new (...r: Range[]) => unknown }).Highlight;
    reg.set(names.all, new H(...all));
    reg.set(names.current, new H(...current));
  } catch {
    clearHighlights(names);
  }
}
