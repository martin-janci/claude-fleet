import { describe, it, expect } from 'vitest';
import { needsMore, OVERFLOW_SLACK } from './composer_overflow';

describe('needsMore', () => {
  it('is false when the chips fit', () => {
    expect(needsMore(300, 300)).toBe(false);
    expect(needsMore(280, 300)).toBe(false);
  });
  it('ignores sub-pixel rounding', () => {
    expect(needsMore(300 + OVERFLOW_SLACK, 300)).toBe(false);
  });
  it('is true when they do not fit', () => {
    expect(needsMore(420, 300)).toBe(true);
  });
  it('treats an unmeasured row as fitting', () => {
    expect(needsMore(0, 0)).toBe(false);
  });
});
