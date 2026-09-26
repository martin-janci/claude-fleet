import { describe, it, expect } from 'vitest';
import { needsMore, wrapsPastOneLine, OVERFLOW_SLACK } from './composer_overflow';

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

describe('wrapsPastOneLine', () => {
  it('is false for one line of chips', () => {
    expect(wrapsPastOneLine([0, 0, 0])).toBe(false);
    expect(wrapsPastOneLine([4, 4 + OVERFLOW_SLACK])).toBe(false);
  });
  it('is true once a chip wrapped to a second line', () => {
    expect(wrapsPastOneLine([0, 0, 30])).toBe(true);
  });
  it('treats zero or one chip as fitting', () => {
    expect(wrapsPastOneLine([])).toBe(false);
    expect(wrapsPastOneLine([12])).toBe(false);
  });
});
