import { describe, it, expect } from 'vitest';
import { describeMoveError } from './moveErrors';

const err = (code: string, details?: unknown, message = 'raw backend text') => ({ code, message, details });

describe('describeMoveError', () => {
  it.each([
    ['E_MOVE_DIRTY', 'uncommitted'],
    ['E_MOVE_UNPUSHED', 'not pushed'],
    ['E_MOVE_MIDOP', 'in the middle of'],
    ['E_MOVE_TARGET_DIRTY', 'turanga already has uncommitted'],
    ['E_MOVE_TOO_LARGE', 'too large'],
    ['E_LOCAL_ONLY', 'hub'],
  ])('%s has its own sentence', (code, fragment) => {
    const d = describeMoveError(err(code), 'failed', 'turanga');
    expect(d.what).toContain(fragment);
    expect(d.what).not.toContain('raw backend text');
  });

  it.each(['seed', 'haves', 'snapshot', 'download', 'upload', 'fetch', 'apply', 'verify', 'target'])(
    'E_MOVE_CARRY at %s has its own sentence',
    (step) => {
      const d = describeMoveError(err('E_MOVE_CARRY', { step }), 'failed', 'turanga');
      expect(d.what).not.toContain('raw backend text');
      expect(d.what.length).toBeGreaterThan(20);
    },
  );

  it('tells a timeout from a failure by the cause code', () => {
    for (const cause_code of ['E_SSH_TIMEOUT', 'E_TIMEOUT']) {
      const d = describeMoveError(err('E_MOVE_CARRY', { step: 'fetch', cause_code }), 'failed', 'turanga');
      expect(d.what).toBe('The target could not take in the carried commits. The host timed out.');
    }
  });

  it('tells busy from already-moving for E_INVALID_STATE', () => {
    expect(describeMoveError(err('E_INVALID_STATE', null, 'a move of session 5 is already in progress'), 'failed', 't').what)
      .toBe('This session is already being moved.');
    expect(describeMoveError(err('E_INVALID_STATE', null, 'the source Claude is working'), 'failed', 't').what)
      .toBe('the source Claude is working');
  });

  it('falls back to the backend message for an unknown code and an unknown carry step', () => {
    expect(describeMoveError(err('E_WHATEVER'), 'failed', 't').what).toBe('raw backend text');
    expect(describeMoveError(err('E_MOVE_CARRY', { step: 'novel' }), 'failed', 't').what).toBe('raw backend text');
  });

  it('says where things stand', () => {
    expect(describeMoveError(err('E_MOVE_CARRY', { step: 'fetch' }), 'failed', 'turanga').standing)
      .toBe('The source session was not touched. Anything copied to turanga was cleaned up.');
    expect(describeMoveError(err('E_MOVE_PARTIAL'), 'partial', 'turanga').standing)
      .toBe('The session is running on turanga, but the source could not be retired. Both sessions were left as they are.');
  });

  it('an observed failure has no error object', () => {
    const d = describeMoveError(null, 'failed', 'turanga');
    expect(d.what).toBe('The move failed. It was started elsewhere, so the reason is in that window or in the session timeline.');
  });
});
