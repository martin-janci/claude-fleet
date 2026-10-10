import { describe, it, expect } from 'vitest';
import { hubHeaderFacts, othersText } from './hub_header';
import type { DeviceSummary } from './devices';

const dev = (name: string, over: Partial<DeviceSummary> = {}): DeviceSummary => ({
  name,
  mode: 'full',
  trusted: true,
  created_at: 1,
  catalogs: [],
  ...over,
});

describe('the Hub & sync status header (M15 G7.13)', () => {
  it('reads this device\'s last contact and counts the same person\'s other devices', () => {
    const facts = hubHeaderFacts(
      [
        dev('mac', { this_device: true, person_id: 1, last_seen_at: 996 }),
        dev('phone', { person_id: 1 }),
        dev('tablet', { person_id: 1 }),
        dev('alice-phone', { person_id: 2 }),
      ],
      1000,
    );
    expect(facts).toEqual({ lastSync: 'last sync 4 s ago', others: 2 });
    expect(othersText(facts.others)).toBe('2 more of your devices');
    expect(othersText(1)).toBe('1 more of your device');
    expect(othersText(0)).toBeNull();
  });

  it('says nothing it cannot read', () => {
    expect(hubHeaderFacts([dev('phone', { person_id: 1 })], 1000)).toEqual({ lastSync: null, others: null });
    expect(hubHeaderFacts([dev('mac', { this_device: true, last_seen_at: 1000 - 7200 })], 1000).lastSync).toBe('last sync 2 h ago');
  });
});
