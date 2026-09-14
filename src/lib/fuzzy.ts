// Small fuzzy matcher for the quick switcher — VS Code quick-open style
// subsequence matching with bonuses for word starts and runs, no dependency.
//
// `fuzzyScore('bs', 'blue-sirius')` matches b…s across a word boundary;
// `fuzzyScore('sir', 'blue-sirius')` scores higher than `fuzzyScore('sir',
// 'serious-fix')` because the run is contiguous and starts a word.

const WORD_BREAK = new Set(['-', '_', ' ', '/', ':', '.', '@']);

/**
 * Score how well `query` matches `text` as an in-order subsequence.
 * Returns `null` when it does not match at all. Higher is better. Both
 * sides are compared case-insensitively.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (q.length === 0) return 0;
  if (q.length > t.length) return null;
  // Fast path: exact substring gets a big bonus (position-weighted).
  const idx = t.indexOf(q);
  if (idx !== -1) {
    const atWordStart = idx === 0 || WORD_BREAK.has(t[idx - 1]);
    return 100 + q.length * 10 + (atWordStart ? 30 : 0) - Math.min(idx, 20) - Math.min(t.length, 40) / 10;
  }
  let score = 0;
  let ti = 0;
  let prevMatch = -2;
  for (let qi = 0; qi < q.length; qi++) {
    const ch = q[qi];
    // Prefer a word-start occurrence when one exists ahead of the cursor.
    let found = -1;
    for (let k = ti; k < t.length; k++) {
      if (t[k] !== ch) continue;
      const wordStart = k === 0 || WORD_BREAK.has(t[k - 1]);
      if (wordStart || found === -1) {
        found = k;
        if (wordStart) break;
      }
      if (found !== -1 && k - found > 8) break; // don't scan forever for a word start
    }
    if (found === -1) return null;
    const wordStart = found === 0 || WORD_BREAK.has(t[found - 1]);
    score += 10;
    if (found === prevMatch + 1) score += 8; // contiguous run
    if (wordStart) score += 6;
    score -= Math.min(found - ti, 10); // gap penalty
    prevMatch = found;
    ti = found + 1;
  }
  // Shorter haystacks win ties.
  return score - Math.min(t.length, 40) / 10;
}

/**
 * Multi-token query against multiple fields: every whitespace-separated
 * token must match at least one field; the score is the sum of each token's
 * best field score. `null` when any token fails to match.
 */
export function fuzzyMatchFields(query: string, fields: readonly string[]): number | null {
  const tokens = query.split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return 0;
  let total = 0;
  for (const tok of tokens) {
    let best: number | null = null;
    for (const f of fields) {
      const s = fuzzyScore(tok, f);
      if (s !== null && (best === null || s > best)) best = s;
    }
    if (best === null) return null;
    total += best;
  }
  return total;
}
