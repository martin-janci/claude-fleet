import { describe, it, expect } from 'vitest';
import { replyActionsFor, quoteText } from './reply_actions';
import type { ConvTurn } from './conversation';

const turn = (prompt_uuid: string | null): ConvTurn => ({
  prompt: prompt_uuid ? 'hi' : null,
  at: null,
  ended_at: null,
  items: [],
  prompt_uuid,
});

describe('replyActionsFor', () => {
  it('forks on the NEXT later turn’s anchor, so this turn is kept', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, false, true);
    expect(v.forkAnchor).toBe('a2');
    expect(v.canFork).toBe(true);
  });

  it('forking the newest turn keeps the whole transcript', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, true);
    expect(v.canFork).toBe(true);
    expect(v.forkAnchor).toBeNull();
  });

  it('skips prompt-less turns when scanning forward for the fork anchor', () => {
    // A compact boundary between them has no anchor; fork must keep going and
    // keep MORE history rather than give up or keep less.
    const v = replyActionsFor([turn('a1'), turn(null), turn('a3')], 0, false, true);
    expect(v.forkAnchor).toBe('a3');
  });

  it('rewinds on this turn’s own anchor', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, true);
    expect(v.canRewind).toBe(true);
    expect(v.rewindAnchor).toBe('a2');
  });

  it('offers no rewind on the first turn of an untruncated conversation', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, false, true);
    expect(v.canRewind).toBe(false);
  });

  it('DOES offer rewind on index 0 when the window is truncated', () => {
    // Index 0 is only the conversation's first turn when nothing older was
    // dropped. Hiding it here would hide rewind on most of a long session.
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, true, true);
    expect(v.canRewind).toBe(true);
    expect(v.rewindAnchor).toBe('a1');
  });

  it('offers no rewind on a prompt-less turn', () => {
    const v = replyActionsFor([turn('a1'), turn(null)], 1, false, true);
    expect(v.canRewind).toBe(false);
  });

  it('offers nothing but copy and quote when the backend does not support it', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, false);
    expect(v.canFork).toBe(false);
    expect(v.canRewind).toBe(false);
  });
});

describe('quoteText', () => {
  it('prefixes every line, including blank ones, and ends with a blank line', () => {
    expect(quoteText('a\n\nb')).toBe('> a\n>\n> b\n\n');
  });
});
