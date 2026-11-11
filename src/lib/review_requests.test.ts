import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import {
  askForReview,
  askSessionName,
  finishReviewRequest,
  levelWords,
  loadReviewRequests,
  reviewRequestCount,
  reviewRequests,
  trackReviewRequests,
  type ReviewRequest,
} from './review_requests';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const mocked = vi.mocked(invoke);

const ask = (id: number): ReviewRequest => ({
  id,
  session_id: 7,
  from_person_id: 1,
  from_name: 'Martin',
  to_person_id: 2,
  to_name: 'Ada',
  level: 'drive',
  created_at: 100,
});

describe('review requests', () => {
  beforeEach(() => {
    mocked.mockReset();
    reviewRequests.set([]);
  });

  it('lists the asks to me and takes one off once done', async () => {
    mocked.mockResolvedValueOnce([ask(1), ask(2)]);
    await loadReviewRequests();
    expect(mocked).toHaveBeenCalledWith('review_request', { args: { action: 'list' } });
    expect(get(reviewRequests).map((r) => r.id)).toEqual([1, 2]);
    mocked.mockResolvedValueOnce({ ...ask(1), done_at: 200 });
    const r = await finishReviewRequest(1);
    expect(r.ok).toBe(true);
    expect(mocked).toHaveBeenLastCalledWith('review_request', { args: { action: 'done', id: 1, done: true } });
    expect(get(reviewRequests).map((x) => x.id)).toEqual([2]);
  });

  it('asks by name and leaves an empty note out', async () => {
    mocked.mockResolvedValueOnce(ask(3));
    await askForReview(7, '  Ada ', '   ');
    expect(mocked).toHaveBeenCalledWith('review_request', { args: { action: 'ask', session_id: 7, person: 'Ada' } });
    mocked.mockResolvedValueOnce(ask(3));
    await askForReview(7, 'Ada', ' the retry loop ');
    expect(mocked).toHaveBeenLastCalledWith('review_request', {
      args: { action: 'ask', session_id: 7, person: 'Ada', note: 'the retry loop' },
    });
  });

  it('is tracked while the app shows, and stops on a hub that refuses the read', async () => {
    const load = vi
      .fn()
      .mockResolvedValueOnce({ ok: true, value: [ask(1)] })
      .mockResolvedValueOnce({ ok: false, error: { code: 'E_INVALID', message: 'unknown tool' } });
    const doc = { visibilityState: 'visible', addEventListener: () => {}, removeEventListener: () => {} } as unknown as Document;
    const win = { addEventListener: () => {}, removeEventListener: () => {} } as unknown as Window;
    vi.useFakeTimers();
    try {
      const stop = trackReviewRequests({ load, doc, win, every: 1000 });
      await vi.advanceTimersByTimeAsync(0);
      expect(get(reviewRequests).map((r) => r.id)).toEqual([1]);
      expect(get(reviewRequestCount)).toBe(1);
      await vi.advanceTimersByTimeAsync(1000);
      // The refusal keeps what was read and stops asking.
      expect(get(reviewRequests)).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(5000);
      expect(load).toHaveBeenCalledTimes(2);
      stop();
    } finally {
      vi.useRealTimers();
    }
  });

  it('names the session from the row, else the name the hub sent', () => {
    expect(askSessionName(ask(1), { friendly_name: 'API', tmux_name: 'api' })).toBe('API');
    expect(askSessionName({ ...ask(1), session_name: 'pay-fix' }, undefined)).toBe('pay-fix');
    expect(askSessionName(ask(1), undefined)).toBe('Session 7');
  });

  it('says what the colleague can do', () => {
    expect(levelWords('drive')).toBe('can send notes to the agent');
    expect(levelWords('watch')).toBe('can read and draft notes');
  });
});
