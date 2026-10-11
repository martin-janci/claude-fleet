import { describe, it, expect } from 'vitest';
import { addTally, andList, localDay, tallyLine, tallyToday } from './review_tally';

describe('review tally (G7.7)', () => {
  const day = new Date(2026, 9, 10, 12);
  const next = new Date(2026, 9, 11, 9);
  it('counts the day, starts again the next, and never goes below zero', () => {
    let t = addTally(null, { confirmed: 1 }, day);
    t = addTally(t, { rejected: 1 }, day);
    expect(tallyLine(t, day)).toBe('Done today: 1 confirmed · 1 rejected');
    expect(tallyLine(t, next)).toBeNull();
    expect(tallyToday(t, next)).toEqual({ day: localDay(next), confirmed: 0, rejected: 0 });
    expect(addTally(t, { rejected: -3 }, day).rejected).toBe(0);
  });
  it('joins names with and', () => {
    expect(andList(['A'])).toBe('A');
    expect(andList(['A', 'B'])).toBe('A and B');
    expect(andList(['A', 'B', 'C'])).toBe('A, B and C');
  });
});
