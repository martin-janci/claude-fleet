// G1.6: missions waiting on a person, as the Inbox and Today read them.
import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import { loadWaitingMissions, waitingMissions, waitingOf, waitWords, missionInScope } from './mission_waits';
import type { Mission } from './missions';

const m = (id: number, over: Partial<Mission> = {}): Mission => ({
  id,
  name: `m${id}`,
  goal: 'g',
  mode: 'finite',
  state: 'active',
  level: 2,
  plan_version: 1,
  created_at: 1,
  updated_at: 1,
  version: 1,
  ...over,
});

describe('mission waits', () => {
  it('keeps the missions that wait, the longest waiting first', () => {
    const list = [
      m(1, { waiting_on: { reason: 'confirm', since: 50, open_cards: 3 } }),
      m(2),
      m(3, { waiting_on: { reason: 'question', since: 20, open_cards: 1 } }),
      // Review 23b w09-1: a brake paused it and asks a question.
      m(4, { state: 'paused', waiting_on: { reason: 'question', since: 5, open_cards: 1 } }),
      m(5, { state: 'paused' }),
    ];
    expect(waitingOf(list).map((x) => x.id)).toEqual([4, 3, 1]);
  });

  it('says why in words', () => {
    expect(waitWords({ reason: 'sign_grant', since: 0, open_cards: 0 })).toBe('sign the autonomy grant');
    expect(waitWords({ reason: 'question', since: 0, open_cards: 1 })).toBe('answer its question');
    expect(waitWords({ reason: 'confirm', since: 0, open_cards: 3 })).toBe('3 to confirm');
  });

  it('w11-9: a read that fails in passing keeps the last list', async () => {
    waitingMissions.set(waitingOf([m(1, { waiting_on: { reason: 'confirm', since: 1, open_cards: 1 } })]));
    await loadWaitingMissions(async () => ({ ok: false, error: { code: 'E_NETWORK', message: 'timed out' } }) as never);
    expect(get(waitingMissions).map((x) => x.id)).toEqual([1]);
    waitingMissions.set([]);
  });

  it('reads nothing from a hub that refuses, rather than keeping a stale list', async () => {
    waitingMissions.set(waitingOf([m(1, { waiting_on: { reason: 'confirm', since: 1, open_cards: 1 } })]));
    await loadWaitingMissions(async () => ({ ok: false, error: { code: 'E_INVALID', message: 'no' } }) as never);
    expect(get(waitingMissions)).toEqual([]);
  });
});

describe('missionInScope (review w11-9)', () => {
  it('scopes a waiting mission by its org, as Today does', () => {
    expect(missionInScope({ org_id: 1 }, 'all')).toBe(true);
    expect(missionInScope({ org_id: 1 }, 'org:1')).toBe(true);
    expect(missionInScope({ org_id: 1 }, 'org:2')).toBe(false);
    expect(missionInScope({ org_id: null }, 'org:2')).toBe(false);
    expect(missionInScope({ org_id: null }, 'unassigned')).toBe(true);
    expect(missionInScope({ org_id: 1 }, 'unassigned')).toBe(false);
  });
});
