import { describe, it, expect } from 'vitest';
import { isWorktreeGone, hasDiff } from './files';
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
