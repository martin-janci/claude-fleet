// G1.6: missions waiting on a person, as the Inbox and Today read them.
import { describe, it, expect } from 'vitest';
import { get } from 'svelte/store';
import { loadWaitingMissions, waitingMissions, waitingOf, waitWords } from './mission_waits';
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
  it('keeps the active missions that wait, the longest waiting first', () => {
    const list = [
      m(1, { waiting_on: { reason: 'confirm', since: 50, open_cards: 3 } }),
      m(2),
      m(3, { waiting_on: { reason: 'question', since: 20, open_cards: 1 } }),
      m(4, { state: 'paused', waiting_on: { reason: 'sign_grant', since: 5, open_cards: 0 } }),
    ];
    expect(waitingOf(list).map((x) => x.id)).toEqual([3, 1]);
  });

  it('says why in words', () => {
    expect(waitWords({ reason: 'sign_grant', since: 0, open_cards: 0 })).toBe('sign the autonomy grant');
    expect(waitWords({ reason: 'question', since: 0, open_cards: 1 })).toBe('answer its question');
    expect(waitWords({ reason: 'confirm', since: 0, open_cards: 3 })).toBe('3 to confirm');
  });

  it('reads nothing from a hub that refuses, rather than keeping a stale list', async () => {
    waitingMissions.set(waitingOf([m(1, { waiting_on: { reason: 'confirm', since: 1, open_cards: 1 } })]));
    await loadWaitingMissions(async () => ({ ok: false, error: { code: 'E_INVALID', message: 'no' } }) as never);
    expect(get(waitingMissions)).toEqual([]);
  });
});
