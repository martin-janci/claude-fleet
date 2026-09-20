import { describe, it, expect } from 'vitest';
import { describeMoveError } from './moveErrors';
import type { MoveStep } from './moveProgress';

const err = (code: string, details?: unknown, message = 'raw backend text') => ({ code, message, details });
/** The common case: a failure late enough to have left something behind. */
const failed = (code: string, details?: unknown, message?: string, reached: MoveStep | null = 'git') =>
  describeMoveError(err(code, details, message), 'failed', 'turanga', reached);

describe('describeMoveError', () => {
  it.each([
    ['E_MOVE_DIRTY', 'uncommitted'],
    ['E_MOVE_UNPUSHED', 'not pushed'],
    ['E_MOVE_MIDOP', 'in the middle of'],
    ['E_MOVE_TARGET_DIRTY', 'turanga already has uncommitted'],
    ['E_MOVE_TOO_LARGE', 'too large'],
    ['E_LOCAL_ONLY', 'hub'],
  ])('%s has its own sentence', (code, fragment) => {
    const d = failed(code);
    expect(d.what).toContain(fragment);
    expect(d.what).not.toContain('raw backend text');
  });

  it.each(['seed', 'haves', 'snapshot', 'download', 'upload', 'fetch', 'apply', 'verify', 'target'])(
    'E_MOVE_CARRY at %s has its own sentence',
    (step) => {
      const d = failed('E_MOVE_CARRY', { step });
      expect(d.what).not.toContain('raw backend text');
      expect(d.what.length).toBeGreaterThan(20);
    },
  );

  it('tells a timeout from a failure by the cause code', () => {
    for (const cause_code of ['E_SSH_TIMEOUT', 'E_TIMEOUT']) {
      const d = failed('E_MOVE_CARRY', { step: 'fetch', cause_code });
      expect(d.what).toBe('The target could not take in the carried commits. The host timed out.');
    }
  });

  it('tells busy from already-moving from anything else for E_INVALID_STATE', () => {
    expect(failed('E_INVALID_STATE', null, 'a move of session 5 is already in progress').what)
      .toBe('This session is already being moved.');
    expect(
      failed(
        'E_INVALID_STATE',
        null,
        'the source Claude is not idle (claude_status "working"); moving now would lose the turn in progress — wait until it finishes, then retry',
      ).what,
    ).toBe('The source Claude is in the middle of a turn. Wait for it to finish, then transfer.');
    expect(failed('E_INVALID_STATE', null, 'something else entirely').what).toBe('something else entirely');
  });

  it('falls back to the backend message for an unknown code and an unknown carry step', () => {
    expect(failed('E_WHATEVER').what).toBe('raw backend text');
    expect(failed('E_MOVE_CARRY', { step: 'novel' }).what).toBe('raw backend text');
  });

  // P-T4: `details` is whatever the backend put there. Anything that is not
  // an object with the keys this reads must fall back, never throw.
  it.each([['a string', 'oops'], ['an array', ['oops']], ['null', null], ['a number', 7]])(
    'survives details that are %s',
    (_name, details) => {
      const d = failed('E_MOVE_CARRY', details);
      expect(d.what).toBe('raw backend text');
      expect(d.standing.length).toBeGreaterThan(0);
    },
  );

  // F6: "anything copied was cleaned up" was not true. Only the transfer
  // scratch files are removed; the clone, the worktree and the copied files
  // stay on the target.
  describe('where things stand', () => {
    // A refusal the source itself produced: nothing over there was touched.
    it('says nothing was copied only while nothing had been', () => {
      const nothing = 'Nothing was copied to turanga. The source session was not touched.';
      for (const reached of [null, 'check', 'transcript'] as (MoveStep | null)[]) {
        expect(failed('E_MOVE_DIRTY', null, undefined, reached).standing).toBe(nothing);
        expect(failed('E_NO_TRANSCRIPT', null, undefined, reached).standing).toBe(nothing);
      }
    });

    it('owns up to what was left on the target once the move had started', () => {
      const left =
        'The source session was not touched. Temporary transfer files were removed; what was ' +
        'already set up on turanga — the clone, the worktree, copied files — was left there.';
      for (const reached of ['workspace', 'git', 'replay', 'handoff'] as MoveStep[]) {
        expect(failed('E_SHELL', null, undefined, reached).standing).toBe(left);
      }
    });

    // R1: "nothing was copied" is asserted from how far the EVENTS got, and
    // they may never have arrived at all (an old hub, a dropped stream, a
    // result that won the race) — every step is then pending and the sheet
    // reports `check`. Two codes cannot happen before the target is touched,
    // so for those the step the events reached proves nothing.
    it.each(['E_MOVE_CARRY', 'E_MOVE_TARGET_DIRTY'])(
      '%s always owns up, however far the events got',
      (code) => {
        const left =
          'The source session was not touched. Temporary transfer files were removed; what was ' +
          'already set up on turanga — the clone, the worktree, copied files — was left there.';
        for (const reached of [null, 'check', 'transcript', 'git'] as (MoveStep | null)[]) {
          expect(failed(code, { step: 'verify' }, undefined, reached).standing).toBe(left);
        }
      },
    );

    it('is neutral for a partial move: both sessions are alive', () => {
      expect(describeMoveError(err('E_MOVE_PARTIAL'), 'partial', 'turanga', 'handoff').standing)
        .toBe('A new session exists on turanga and the source is still there. Nothing was killed.');
    });
  });

  // F13: a partial move stops at one of five places, and what the user has
  // to do next is different at each.
  describe('a partial move names the step it stopped at', () => {
    const partial = (step?: string) =>
      describeMoveError(
        err('E_MOVE_PARTIAL', step === undefined ? {} : { step }),
        'partial',
        'turanga',
        'handoff',
      ).what;

    it('the confirm', () => {
      expect(partial('confirming the target is running'))
        .toBe('The new session on turanga did not confirm that it is running.');
    });

    it('a source that took a turn after the copy', () => {
      expect(partial('source transcript changed after copy')).toBe(
        'The source wrote to the conversation after it was copied, so turanga is missing the ' +
          'latest turn. Kill the session on turanga and transfer again.',
      );
    });

    it('the kill', () => {
      expect(partial('killing the source dev-foo on mefistos')).toBe(
        'The new session is running on turanga, but the source could not be stopped. Kill the source yourself.',
      );
    });

    it('reconciling the target host', () => {
      expect(partial('reconciling the target host'))
        .toBe('The new session was started on turanga, but fleet could not confirm it there.');
    });

    it('and falls back for a step it does not know', () => {
      const fallback = 'The new session started, but the last step of the move failed.';
      expect(partial('registering the target row')).toBe(fallback);
      expect(partial()).toBe(fallback);
      expect(partial('something a newer backend invented')).toBe(fallback);
    });
  });

  it('an observed failure has no error object', () => {
    const d = describeMoveError(null, 'failed', 'turanga', 'git');
    expect(d.what).toBe('The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.');
  });

  it.each([
    ['ours', ['src/lib.rs'], 'left behind', 'clean'],
    ['theirs', ['their_notes.md'], 'work of its own', null],
    ['unknown', [], 'already has uncommitted', 'retry'],
  ])('describes E_MOVE_TARGET_DIRTY(%s)', (leftovers, paths, phrase, action) => {
    const details = leftovers === 'theirs' ? { leftovers, theirs: paths } : { leftovers, ours: paths };
    const f = describeMoveError(
      { code: 'E_MOVE_TARGET_DIRTY', message: 'raw', details },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.what).toContain(phrase);
    expect(f.action?.kind ?? null).toBe(action);
    if (action === 'clean') expect((f.action as { paths: string[] }).paths).toEqual(paths);
  });

  it('names the paths it would remove, and turanga, for stale leftovers', () => {
    const f = describeMoveError(
      {
        code: 'E_MOVE_TARGET_DIRTY',
        message: 'raw',
        details: { leftovers: 'ours', ours: ['a.txt', 'b/c.txt'] },
      },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.what).toContain('turanga');
    expect(f.what).toContain('a.txt');
  });

  it('says nothing was overwritten when the target is holding its own work', () => {
    const f = describeMoveError(
      { code: 'E_MOVE_TARGET_DIRTY', message: 'raw', details: { leftovers: 'theirs', theirs: ['x'] } },
      'failed',
      'turanga',
      'replay',
    );
    expect(f.action).toBeNull();
    expect(f.standing).toContain('was not touched');
  });

  it('an undone partial reads as undone, not as a failure', () => {
    const f = describeMoveError(
      { code: 'E_MOVE_UNDONE', message: '', details: null },
      'failed',
      'turanga',
      'start',
    );
    expect(f.what).toContain('undid');
    expect(f.action).toBeNull();
  });
});
