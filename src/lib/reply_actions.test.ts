import { describe, it, expect } from 'vitest';
import {
  replyActionsFor,
  quoteText,
  waitForReplQuiet,
  RETRY_READY_TIMEOUT_MS,
  RETRY_READY_POLL_MS,
} from './reply_actions';
import type { ActivityProbe, ConvTurn } from './conversation';
import type { Result } from './result';

const turn = (prompt_uuid: string | null): ConvTurn => ({
  prompt: prompt_uuid ? 'hi' : null,
  at: null,
  ended_at: null,
  items: [],
  prompt_uuid,
});

/** A turn that HAS an anchor but no prompt text — an image-only prompt.
 *  `transcript.rs:1166` sets `prompt` only when the text is non-empty while
 *  always setting `prompt_uuid`. */
const imageOnlyTurn = (prompt_uuid: string): ConvTurn => ({
  prompt: null,
  at: null,
  ended_at: null,
  items: [],
  prompt_uuid,
});

const probe = (claude_status: ActivityProbe['claude_status']): Result<ActivityProbe> => ({
  ok: true,
  value: {
    claude_status,
    current_activity: null,
    stuck_kind: null,
    waiting_for: null,
    spinner: null,
    pending_input: null,
  },
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

  it('offers Rewind but NOT Retry on an anchored turn with no prompt text', () => {
    // An image-only prompt yields `prompt: null, prompt_uuid: Some(_)`. Retry
    // promises "the same prompt is then sent again" and there is nothing to
    // send, so it must not be offered; a bare Rewind is still valid.
    const v = replyActionsFor([turn('a1'), imageOnlyTurn('a2')], 1, false, true);
    expect(v.canRewind).toBe(true);
    expect(v.rewindAnchor).toBe('a2');
    expect(v.canRetry).toBe(false);
  });

  it('offers Rewind but NOT Retry when the prompt is partial, and says why', () => {
    const v = replyActionsFor([turn('a1'), { ...turn('a2'), prompt_partial: true }], 1, false, true);
    expect(v.canRewind).toBe(true);
    expect(v.canRetry).toBe(false);
    expect(v.retryUnavailable).toMatch(/not the whole prompt/);
  });

  it('gives no Retry reason where Rewind is not offered either', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 0, false, true);
    expect(v.canRetry).toBe(false);
    expect(v.retryUnavailable).toBeNull();
  });

  it('offers Retry when the turn does have prompt text', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, true);
    expect(v.canRetry).toBe(true);
  });

  it('offers nothing but copy and quote when the backend does not support it', () => {
    const v = replyActionsFor([turn('a1'), turn('a2')], 1, false, false);
    expect(v.canFork).toBe(false);
    expect(v.canRewind).toBe(false);
    expect(v.canRetry).toBe(false);
  });
});

describe('quoteText', () => {
  it('prefixes every line, including blank ones, and ends with a blank line', () => {
    expect(quoteText('a\n\nb')).toBe('> a\n>\n> b\n\n');
  });
});

describe('waitForReplQuiet', () => {
  it('returns true as soon as the REPL reports a quiet status', async () => {
    const seen: number[] = [];
    let n = 0;
    const ok = await waitForReplQuiet(7, {
      probe: async (id) => {
        seen.push(id);
        return probe(++n < 3 ? 'working' : 'idle');
      },
      sleep: async () => {},
      pollMs: 10,
      timeoutMs: 1_000,
    });
    expect(ok).toBe(true);
    expect(seen).toEqual([7, 7, 7]);
  });

  it('gives up — bounded — when the REPL never goes quiet', async () => {
    let polls = 0;
    const ok = await waitForReplQuiet(7, {
      probe: async () => {
        polls++;
        return probe('working');
      },
      sleep: async () => {},
      pollMs: 10,
      timeoutMs: 50,
    });
    expect(ok).toBe(false);
    expect(polls).toBe(5);
  });

  it('treats a probe that cannot answer as “not yet”, never as ready', async () => {
    let polls = 0;
    const ok = await waitForReplQuiet(7, {
      probe: async () => {
        polls++;
        return { ok: false, error: { code: 'E_FORBIDDEN', message: 'no' } } as Result<ActivityProbe>;
      },
      sleep: async () => {},
      pollMs: 10,
      timeoutMs: 30,
    });
    expect(ok).toBe(false);
    expect(polls).toBe(3);
  });

  it('has a bound that is a whole number of polls', () => {
    expect(RETRY_READY_TIMEOUT_MS % RETRY_READY_POLL_MS).toBe(0);
  });
});
