// G7.10: a review run's verdict line comes from the reviewer's own closing
// `Verdict:` line, and a run that wrote none shows nothing.
import { describe, it, expect } from 'vitest';
import { lastReplyText, parseReviewVerdict, reviewVerdictLine, reviewVerdictTone } from './review_verdict';
import { DEFAULT_REVIEW_PROMPT } from './sessions';
import type { Conversation } from './conversation';

describe('parseReviewVerdict', () => {
  it('reads the form the default prompt asks for', () => {
    expect(DEFAULT_REVIEW_PROMPT).toContain('Verdict: approve | approve-with-fixes | needs-rework');
    const v = parseReviewVerdict('Pass 1 …\n\n**Verdict: approve-with-fixes · 0 blocking · 2 nits**');
    expect(v).toEqual({ verdict: 'approve-with-fixes', blocking: 0, nits: 2 });
    expect(reviewVerdictLine(v!)).toBe('approve with fixes · no blocking findings · 2 nits');
    expect(reviewVerdictTone(v!)).toBe('warn');
  });

  it('takes the last verdict line and spelled-out words, counts optional', () => {
    const v = parseReviewVerdict('Verdict: approve\n…\nOverall verdict — needs rework (1 blocking)');
    expect(v).toEqual({ verdict: 'needs-rework', blocking: 1, nits: null });
    expect(reviewVerdictLine(v!)).toBe('needs rework · 1 blocking finding');
    expect(reviewVerdictTone(v!)).toBe('bad');
    expect(parseReviewVerdict('Verdict: approve · no blocking · no nits')).toEqual({
      verdict: 'approve',
      blocking: 0,
      nits: 0,
    });
  });

  it('no verdict line, no verdict', () => {
    expect(parseReviewVerdict('Looks fine to me.')).toBeNull();
    expect(parseReviewVerdict('Verdict: unsure')).toBeNull();
    expect(parseReviewVerdict('The verdict is approve')).toBeNull();
  });
});

describe('lastReplyText', () => {
  it('is the last text the reviewer wrote, past tools', () => {
    const conv = {
      turns: [
        { prompt: 'p', at: null, ended_at: null, items: [{ kind: 'text', text: 'first' }] },
        {
          prompt: 'q',
          at: null,
          ended_at: null,
          items: [
            { kind: 'text', text: 'Verdict: approve' },
            { kind: 'bash', command: 'ls', stdout: null, stderr: null },
          ],
        },
      ],
    } as unknown as Conversation;
    expect(lastReplyText(conv)).toBe('Verdict: approve');
    expect(lastReplyText(null)).toBeNull();
  });
});
