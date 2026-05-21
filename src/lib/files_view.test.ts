import { describe, it, expect } from 'vitest';
import { parseUnifiedDiff } from './DiffView.svelte';
import { buildTree } from './FileList.svelte';

describe('parseUnifiedDiff', () => {
  const sample = [
    'diff --git a/src/x.ts b/src/x.ts',
    'index 1111111..2222222 100644',
    '--- a/src/x.ts',
    '+++ b/src/x.ts',
    '@@ -1,3 +1,4 @@',
    ' const a = 1;',
    '-const b = 2;',
    '+const b = 3;',
    '+const c = 4;',
    ' const d = 5;',
    '',
  ].join('\n');

  it('tracks old/new line numbers across hunks', () => {
    const rows = parseUnifiedDiff(sample);
    const ctxFirst = rows.find((r) => r.kind === 'ctx');
    expect(ctxFirst?.oldNo).toBe(1);
    expect(ctxFirst?.newNo).toBe(1);

    const del = rows.find((r) => r.kind === 'del');
    expect(del?.text).toBe('const b = 2;');
    expect(del?.oldNo).toBe(2);
    expect(del?.newNo).toBe(null);

    const adds = rows.filter((r) => r.kind === 'add');
    expect(adds.map((r) => r.text)).toEqual(['const b = 3;', 'const c = 4;']);
    expect(adds[0].newNo).toBe(2);
    expect(adds[1].newNo).toBe(3);
  });

  it('classifies meta and hunk lines', () => {
    const rows = parseUnifiedDiff(sample);
    expect(rows.filter((r) => r.kind === 'meta').length).toBe(4); // git/index/---/+++
    expect(rows.filter((r) => r.kind === 'hunk').length).toBe(1);
  });

  it('does not emit a trailing blank context row', () => {
    const rows = parseUnifiedDiff(sample);
    const last = rows[rows.length - 1];
    expect(last.text).toBe('const d = 5;');
  });

  it('handles an empty diff', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
  });
});

describe('buildTree', () => {
  it('nests files under their folders', () => {
    const tree = buildTree(['src/lib/a.ts', 'src/lib/b.ts', 'README.md']);
    // Folders sort before files.
    expect(tree.map((n) => n.name)).toEqual(['src', 'README.md']);
    const src = tree[0];
    expect(src.isDir).toBe(true);
    expect(src.children.map((n) => n.name)).toEqual(['lib']);
    expect(src.children[0].children.map((n) => n.name)).toEqual(['a.ts', 'b.ts']);
  });

  it('carries the full path on every node', () => {
    const tree = buildTree(['src/lib/a.ts']);
    expect(tree[0].path).toBe('src');
    expect(tree[0].children[0].path).toBe('src/lib');
    expect(tree[0].children[0].children[0].path).toBe('src/lib/a.ts');
  });

  it('sorts folders before files, each group alphabetically', () => {
    const tree = buildTree(['z.txt', 'b/x.ts', 'a.txt', 'a/y.ts']);
    expect(tree.map((n) => `${n.isDir ? 'd:' : 'f:'}${n.name}`)).toEqual([
      'd:a',
      'd:b',
      'f:a.txt',
      'f:z.txt',
    ]);
  });

  it('returns an empty array for no entries', () => {
    expect(buildTree([])).toEqual([]);
  });
});
