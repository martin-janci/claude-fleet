import { describe, expect, it } from 'vitest';
import { chipsThatFit } from './composer_chips';

describe('chipsThatFit', () => {
  it('shows every chip, and no ⋯, when the whole row fits', () => {
    // 3 × 100 + 2 gaps of 10 = 320.
    expect(chipsThatFit([100, 100, 100], 320, 40, 10)).toEqual([0, 1, 2]);
  });

  it('fills the line and reserves room for ⋯', () => {
    // ⋯ 40, then 10 + 100 per chip: 150, 260, 370 — 370 overflows 300.
    expect(chipsThatFit([100, 100, 100, 100], 300, 40, 10)).toEqual([0, 1]);
  });

  it('shows more than three when they fit', () => {
    const widths = [70, 100, 80, 110, 80, 90, 80, 90];
    expect(chipsThatFit(widths, 700, 40, 8)).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it('stops at the first chip that does not fit, keeping the order', () => {
    // A narrow chip after a wide one is not pulled forward.
    expect(chipsThatFit([100, 300, 50], 260, 40, 10)).toEqual([0]);
  });

  it('keeps a pinned chip whatever the room, and counts it first', () => {
    expect(chipsThatFit([100, 100, 100, 100], 300, 40, 10, [3])).toEqual([0, 3]);
    expect(chipsThatFit([100, 100, 100], 50, 40, 10, [2])).toEqual([2]);
  });

  it('ignores sub-pixel overflow', () => {
    expect(chipsThatFit([100, 100], 209.5, 40, 10)).toEqual([0, 1]);
  });

  it('handles no chips', () => {
    expect(chipsThatFit([], 300, 40, 10)).toEqual([]);
  });
});
