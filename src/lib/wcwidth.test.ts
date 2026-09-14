import { describe, it, expect } from 'vitest';
import { wcwidth, firstCharWidth, ZERO, WIDE } from './wcwidth';

function assertSortedDisjoint(table: readonly number[]) {
  expect(table.length % 2).toBe(0);
  for (let i = 0; i < table.length; i += 2) {
    expect(table[i]).toBeLessThanOrEqual(table[i + 1]);
    if (i > 0) expect(table[i]).toBeGreaterThan(table[i - 1]);
  }
}

describe('wcwidth tables', () => {
  it('ZERO is sorted and non-overlapping (binary search precondition)', () => {
    assertSortedDisjoint(ZERO);
  });
  it('WIDE is sorted and non-overlapping', () => {
    assertSortedDisjoint(WIDE);
  });
});

describe('wcwidth', () => {
  it('ASCII and Latin are width 1', () => {
    expect(wcwidth(0x41)).toBe(1); // A
    expect(wcwidth(0x20)).toBe(1); // space
    expect(wcwidth(0xe1)).toBe(1); // á
    expect(wcwidth(0xad)).toBe(1); // soft hyphen stays 1 (Kuhn convention)
  });

  it('C0 / DEL / C1 controls are width 0', () => {
    expect(wcwidth(0x00)).toBe(0);
    expect(wcwidth(0x1b)).toBe(0);
    expect(wcwidth(0x7f)).toBe(0);
    expect(wcwidth(0x9c)).toBe(0);
  });

  it('combining marks, ZWJ, variation selectors and format controls are width 0', () => {
    expect(wcwidth(0x0301)).toBe(0); // combining acute
    expect(wcwidth(0x200d)).toBe(0); // ZWJ
    expect(wcwidth(0x200b)).toBe(0); // ZWSP
    expect(wcwidth(0xfe0f)).toBe(0); // VS16
    expect(wcwidth(0x20e3)).toBe(0); // combining enclosing keycap
    expect(wcwidth(0xe0020)).toBe(0); // tag space
    expect(wcwidth(0x1160)).toBe(0); // Hangul jungseong filler
  });

  it('CJK, Hangul syllables, fullwidth forms and emoji are width 2', () => {
    expect(wcwidth(0x4e2d)).toBe(2); // 中
    expect(wcwidth(0x3042)).toBe(2); // あ
    expect(wcwidth(0xac00)).toBe(2); // 가
    expect(wcwidth(0xff21)).toBe(2); // Ａ
    expect(wcwidth(0x1f600)).toBe(2); // 😀
    expect(wcwidth(0x1f680)).toBe(2); // 🚀
    expect(wcwidth(0x2705)).toBe(2); // ✅
    expect(wcwidth(0x20000)).toBe(2); // CJK ext B
  });

  it('text-presentation symbols stay width 1', () => {
    expect(wcwidth(0x2764)).toBe(1); // ❤ (needs VS16 for emoji presentation)
    expect(wcwidth(0x2192)).toBe(1); // →
    expect(wcwidth(0x2500)).toBe(1); // ─ box drawing
  });

  it('firstCharWidth looks at the leading code point only', () => {
    expect(firstCharWidth('')).toBe(0);
    expect(firstCharWidth('é')).toBe(1);
    expect(firstCharWidth('😀️')).toBe(2);
  });
});
