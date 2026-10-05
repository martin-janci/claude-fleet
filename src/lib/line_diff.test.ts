import { describe, it, expect } from 'vitest';
import { diffLines, unifiedDiff } from './line_diff';
import { parseUnifiedDiff } from './DiffView.svelte';

describe('diffLines', () => {
  it('keeps equal lines and marks the edit', () => {
    const ops = diffLines(['a', 'b', 'c'], ['a', 'x', 'c']);
    expect(ops.map((o) => o.t + o.line)).toEqual([' a', '-b', '+x', ' c']);
  });
  it('handles empty sides', () => {
    expect(diffLines([], ['a']).map((o) => o.t + o.line)).toEqual(['+a']);
    expect(diffLines(['a'], []).map((o) => o.t + o.line)).toEqual(['-a']);
    expect(diffLines([], [])).toEqual([]);
  });
  it('is minimal on a moved block', () => {
    const ops = diffLines(['1', '2', '3', '4'], ['1', '3', '4', '2']);
    expect(ops.filter((o) => o.t !== ' ')).toHaveLength(2);
  });
  it('falls back to replace-all past the edit budget', () => {
    const a = Array.from({ length: 50 }, (_, i) => `a${i}`);
    const b = Array.from({ length: 50 }, (_, i) => `b${i}`);
    const ops = diffLines(a, b, 10);
    expect(ops.filter((o) => o.t === '-')).toHaveLength(50);
    expect(ops.filter((o) => o.t === '+')).toHaveLength(50);
  });
});

describe('unifiedDiff', () => {
  it('emits hunks with three lines of context that DiffView parses', () => {
    const a = 'l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n';
    const b = 'l1\nl2\nl3\nl4\nX\nl6\nl7\nl8\n';
    const u = unifiedDiff(a, b, 'catalog/SKILL.md', 'oci/SKILL.md');
    expect(u).toContain('--- catalog/SKILL.md');
    expect(u).toContain('+++ oci/SKILL.md');
    expect(u).toContain('@@ -2,7 +2,7 @@');
    const rows = parseUnifiedDiff(u);
    expect(rows.filter((r) => r.kind === 'del').map((r) => r.text)).toEqual(['l5']);
    expect(rows.filter((r) => r.kind === 'add').map((r) => r.text)).toEqual(['X']);
  });
  it('is empty when both sides are equal', () => {
    expect(unifiedDiff('a\n', 'a\n', 'x', 'y')).toBe('');
  });
  it('treats a missing side as empty', () => {
    expect(unifiedDiff(null, 'a\n', 'c', 'h')).toContain('@@ -0,0 +1,1 @@');
  });
});
