import { describe, it, expect } from 'vitest';
import { isWorktreeGone, hasDiff, blameGutter, canBlame, type BlameHunk } from './files';
import type { Result } from './result';

describe('isWorktreeGone', () => {
  it('is true for an E_NO_WORKTREE failure', () => {
    const r: Result<unknown> = {
      ok: false,
      error: { code: 'E_NO_WORKTREE', message: 'worktree directory no longer exists' },
    };
    expect(isWorktreeGone(r)).toBe(true);
  });

  it('is false for other failures', () => {
    const r: Result<unknown> = {
      ok: false,
      error: { code: 'E_REPO', message: 'fatal: not a git repository' },
    };
    expect(isWorktreeGone(r)).toBe(false);
  });

  it('is false for a successful result', () => {
    const r: Result<unknown> = { ok: true, value: [] };
    expect(isWorktreeGone(r)).toBe(false);
  });
});

describe('hasDiff', () => {
  it('is false for untracked and undefined, true otherwise', () => {
    expect(hasDiff('untracked')).toBe(false);
    expect(hasDiff(undefined)).toBe(false);
    expect(hasDiff('modified')).toBe(true);
  });
});

describe('blameGutter', () => {
  const h = (start: number, lines: number, hash: string): BlameHunk => ({
    start, lines, hash, author: 'a', time: 0, summary: '', uncommitted: false,
  });

  it('maps every line to its run and marks only the first line of each', () => {
    const g = blameGutter([h(1, 2, 'x'), h(3, 1, 'y')], 4);
    expect(g.map((e) => (e ? `${e.hunk.hash}${e.first ? '*' : ''}` : null))).toEqual(['x*', 'x', 'y*', null]);
  });

  it('ignores runs past the end of the file', () => {
    expect(blameGutter([h(2, 5, 'x')], 3).filter(Boolean)).toHaveLength(2);
  });
});

describe('canBlame', () => {
  it('is false only for an untracked file', () => {
    expect(canBlame('untracked')).toBe(false);
    expect(canBlame('modified')).toBe(true);
    expect(canBlame(undefined)).toBe(true);
  });
});
