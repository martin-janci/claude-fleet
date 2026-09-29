import { describe, it, expect } from 'vitest';
import { lineDiff, splitPath, parseNumbered, parseTodos, parseFileList, inputField, detailKind } from './tool_view';

const kinds = (d: ReturnType<typeof lineDiff>) => d.rows.map((r) => (r.kind === 'gap' ? `…${r.hidden}` : `${r.kind[0]}:${r.text}`));

describe('lineDiff', () => {
  it('interleaves a replaced line with what replaced it', () => {
    const d = lineDiff('a\nb\nc\nd', 'a\nB\nc\nD');
    expect(kinds(d)).toEqual(['c:a', 'd:b', 'a:B', 'c:c', 'd:d', 'a:D']);
    expect(d.added).toBe(2);
    expect(d.removed).toBe(2);
  });

  it('numbers old and new lines separately', () => {
    const d = lineDiff('x\ny', 'x\nnew\ny');
    expect(d.rows.map((r) => [r.oldNo, r.newNo])).toEqual([
      [1, 1],
      [null, 2],
      [2, 3],
    ]);
  });

  it('folds long unchanged runs and keeps three lines of context', () => {
    const old = Array.from({ length: 20 }, (_, i) => `l${i}`);
    const next = [...old];
    next[10] = 'changed';
    const d = lineDiff(old.join('\n'), next.join('\n'));
    expect(kinds(d)).toEqual(['…7', 'c:l7', 'c:l8', 'c:l9', 'd:l10', 'a:changed', 'c:l11', 'c:l12', 'c:l13', '…6']);
    // Line numbers survive the fold.
    expect(d.rows.find((r) => r.kind === 'del')?.oldNo).toBe(11);
  });

  it('a new file is all additions', () => {
    const d = lineDiff('', 'one\ntwo');
    expect(kinds(d)).toEqual(['a:one', 'a:two']);
    expect(d.removed).toBe(0);
  });

  it('an unchanged text stays whole', () => {
    expect(kinds(lineDiff('a\nb', 'a\nb'))).toEqual(['c:a', 'c:b']);
  });

  it('falls back to removed-then-added past the LCS budget', () => {
    const old = Array.from({ length: 600 }, (_, i) => `o${i}`).join('\n');
    const next = Array.from({ length: 600 }, (_, i) => `n${i}`).join('\n');
    const d = lineDiff(old, next);
    expect(d.removed).toBe(600);
    expect(d.added).toBe(600);
    expect(d.rows[0].kind).toBe('del');
    expect(d.rows[600].kind).toBe('add');
  });
});

describe('tool detail parsers', () => {
  it('splitPath', () => {
    expect(splitPath('/r/src/a.ts')).toEqual({ dir: '/r/src/', base: 'a.ts' });
    expect(splitPath('a.ts')).toEqual({ dir: '', base: 'a.ts' });
  });

  it('parseNumbered reads both cat -n shapes and refuses anything else', () => {
    expect(parseNumbered('     1\tfoo\n     2\t  bar\n')).toEqual([
      { no: 1, text: 'foo' },
      { no: 2, text: '  bar' },
    ]);
    expect(parseNumbered('    10→x')).toEqual([{ no: 10, text: 'x' }]);
    expect(parseNumbered('File does not exist.')).toBeNull();
    expect(parseNumbered('')).toBeNull();
  });

  it('parseTodos reads the todo list, defaulting unknown statuses to pending', () => {
    const input = JSON.stringify({ todos: [{ content: 'a', status: 'completed' }, { content: 'b', status: 'in_progress' }, { content: 'c', status: '??' }] });
    expect(parseTodos(input)).toEqual([
      { content: 'a', status: 'completed' },
      { content: 'b', status: 'in_progress' },
      { content: 'c', status: 'pending' },
    ]);
    expect(parseTodos('{"todos": [{"content": "cut…')).toBeNull();
    expect(parseTodos('{}')).toBeNull();
  });

  it('parseFileList drops the header and refuses content-mode output', () => {
    expect(parseFileList('Found 2 files\nsrc/a.ts\nsrc/b.ts')).toEqual(['src/a.ts', 'src/b.ts']);
    expect(parseFileList('src/a.ts:12:  let x = 1')).toBeNull();
    expect(parseFileList('No files found')).toBeNull();
  });

  it('inputField and detailKind', () => {
    expect(inputField('{"file_path": "/a"}', 'file_path')).toBe('/a');
    expect(inputField('not json', 'file_path')).toBeNull();
    expect(detailKind('Edit', true, false)).toBe('edit');
    expect(detailKind('Bash', false, true)).toBe('bash');
    expect(detailKind('Read', false, false)).toBe('read');
    expect(detailKind('Glob', false, false)).toBe('files');
    expect(detailKind('TodoWrite', false, false)).toBe('todos');
    expect(detailKind('WebFetch', false, false)).toBe('raw');
  });
});
