// The Debug devices board's pure parts: filters, grouping, the header's
// counts, the claim line and which actions a card offers.
import { describe, it, expect } from 'vitest';
import { DESKTOP_HOLDER, type DebugDevice } from './debug_devices';
import {
  MIRROR_NOTE,
  cardActions,
  claimLine,
  countLine,
  filterDevices,
  groupByHost,
  matchesQuery,
  stateTone,
} from './debug_devices_board';

const dev = (over: Partial<DebugDevice>): DebugDevice => ({
  id: 1,
  title: 'Pixel 9',
  host: 'mac',
  platform: 'android',
  kind: 'physical',
  key: 'k',
  name: 'Pixel 9',
  state: 'online',
  ready: true,
  shared: false,
  first_seen_at: 1,
  last_seen_at: 1,
  ...over,
});

describe('filters', () => {
  const ds = [
    dev({ id: 1 }),
    dev({ id: 2, claimed_by: 'host:mercury#4' }),
    dev({ id: 3, state: 'offline', ready: false }),
    dev({ id: 4, platform: 'ios', kind: 'simulator', host: 'mini', title: 'iPhone 16', model: 'iPhone17,3' }),
  ];
  const ids = (state: 'all' | 'ready' | 'in-use' | 'not-ready', platform: 'any' | 'android' | 'ios' = 'any', query = '') =>
    filterDevices(ds, { state, platform, query }).map((d) => d.id);

  it('keeps by state: ready means ready and free', () => {
    expect(ids('all')).toEqual([1, 2, 3, 4]);
    expect(ids('ready')).toEqual([1, 4]);
    expect(ids('in-use')).toEqual([2]);
    expect(ids('not-ready')).toEqual([3]);
  });

  it('keeps by platform and search words', () => {
    expect(ids('all', 'ios')).toEqual([4]);
    expect(ids('all', 'android')).toEqual([1, 2, 3]);
    expect(ids('all', 'any', 'MINI')).toEqual([4]);
    expect(ids('all', 'any', 'iphone17 mini')).toEqual([4]);
    expect(matchesQuery(ds[0], '  ')).toBe(true);
  });
});

describe('groupByHost', () => {
  it('groups by host name, ready first then by title, with the newest sighting', () => {
    const g = groupByHost([
      dev({ id: 1, host: 'mercury', title: 'b', last_seen_at: 5 }),
      dev({ id: 2, host: 'mac', title: 'z' }),
      dev({ id: 3, host: 'mercury', title: 'a', ready: false, state: 'missing', last_seen_at: 9 }),
      dev({ id: 4, host: 'mercury', title: 'c' }),
    ]);
    expect(g.map((x) => [x.host, x.devices.map((d) => d.id), x.lastSeen])).toEqual([
      ['mac', [2], 1],
      ['mercury', [1, 4, 3], 9],
    ]);
  });
});

describe('countLine and claimLine', () => {
  it('counts devices, those in use and who waits', () => {
    expect(countLine([dev({})])).toBe('1 device');
    expect(
      countLine([
        dev({ claimed_by: 'x', waiting: 1 }),
        dev({ claimed_by: 'y', queue: [{ holder: 'a', queued_at: 1 }, { holder: 'b', queued_at: 2 }] }),
        dev({}),
      ]),
    ).toBe('3 devices · 2 in use · 3 waiting');
  });

  it('says who holds it, what for and for how long', () => {
    expect(claimLine(dev({}), 0)).toBe('Free');
    expect(claimLine(dev({ claimed_by: 'host:mercury#42', claim_note: 'login flow', claimed_until: 600 }), 0)).toBe(
      'In use by session #42 on mercury · login flow · 10 min left',
    );
    expect(claimLine(dev({ claimed_by: DESKTOP_HOLDER }), 0)).toBe('In use by you');
  });
});

describe('cardActions', () => {
  it('offers claim on a free ready device', () => {
    expect(cardActions(dev({}))).toEqual(['claim', 'mirror', 'logs']);
  });

  it('offers wait in line and release on one someone else holds', () => {
    expect(cardActions(dev({ claimed_by: 'host:m#1' }))).toEqual(['queue', 'release', 'mirror', 'logs']);
  });

  it('offers only leave line from inside it', () => {
    expect(cardActions(dev({ claimed_by: 'host:m#1', queue_place: 2 }))).toEqual(['leave', 'mirror', 'logs']);
  });

  it('offers release, not wait, on your own claim, and stop mirror on a mirror’s', () => {
    expect(cardActions(dev({ claimed_by: DESKTOP_HOLDER }))).toEqual(['release', 'mirror', 'logs']);
    expect(cardActions(dev({ claimed_by: DESKTOP_HOLDER, claim_note: MIRROR_NOTE }))).toEqual([
      'release',
      'stop-mirror',
      'logs',
    ]);
    expect(cardActions(dev({}), true)).toEqual(['claim', 'stop-mirror', 'logs']);
  });

  it('starts a stopped simulator and stops a running one; nothing else on a stopped one', () => {
    const sim = { platform: 'ios', kind: 'simulator' };
    expect(cardActions(dev({ ...sim, state: 'shutdown', ready: false }))).toEqual(['boot']);
    expect(cardActions(dev({ ...sim, state: 'booted' }))).toEqual(['claim', 'logs', 'shutdown']);
    expect(cardActions(dev({ ...sim, state: 'booting', ready: false }))).toEqual([]);
    expect(cardActions(dev({ state: 'unauthorized', ready: false }))).toEqual([]);
  });
});

describe('stateTone', () => {
  it('reads ready green, starting busy, stopped quiet and the rest red', () => {
    expect(stateTone(dev({}))).toBe('done');
    expect(stateTone(dev({ state: 'booting', ready: false }))).toBe('working');
    expect(stateTone(dev({ state: 'shutdown', ready: false }))).toBe('idle');
    expect(stateTone(dev({ state: 'missing', ready: false }))).toBe('failed');
  });
});
