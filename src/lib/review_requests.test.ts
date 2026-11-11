import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { askForReview, finishReviewRequest, levelWords, loadReviewRequests, reviewRequests, type ReviewRequest } from './review_requests';

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

  it('says what the colleague can do', () => {
    expect(levelWords('drive')).toBe('can send notes to the agent');
    expect(levelWords('watch')).toBe('can read and draft notes');
  });
});
