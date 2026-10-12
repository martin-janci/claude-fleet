import { describe, it, expect, beforeEach } from 'vitest';
import { decayed, FRECENCY_CAP, readFrecency, recencyTerm, recordPick } from './frecency';

const NOW = 1_800_000_000;
const DAY = 86_400;

beforeEach(() => localStorage.clear());

describe('frecency', () => {
  it('halves every 7 days', () => {
    expect(decayed({ score: 4, at: NOW - 7 * DAY }, NOW)).toBeCloseTo(2);
    expect(decayed(undefined, NOW)).toBe(0);
  });
  it('a pick decays the old score, then adds one', () => {
    recordPick('o/a', NOW - 7 * DAY);
    recordPick('o/a', NOW);
    expect(readFrecency()['o/a'].score).toBeCloseTo(1.5);
  });
  it('keeps at most FRECENCY_CAP keys, dropping the weakest', () => {
    for (let i = 0; i < FRECENCY_CAP + 5; i++) recordPick(`o/r${i}`, NOW - (FRECENCY_CAP + 5 - i) * DAY);
    const m = readFrecency();
    expect(Object.keys(m)).toHaveLength(FRECENCY_CAP);
    expect(m['o/r0']).toBeUndefined();
  });
  it('recency from last_session_at', () => {
    expect(recencyTerm(NOW, NOW)).toBeCloseTo(20);
    expect(recencyTerm(NOW - 7 * DAY, NOW)).toBeCloseTo(10);
    expect(recencyTerm(null, NOW)).toBe(0);
  });
});
