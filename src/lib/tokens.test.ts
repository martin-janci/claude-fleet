import { describe, it, expect } from 'vitest';
import { relativeLuminance, contrastRatio, THEME, CONTRAST_PAIRS } from './tokens';

describe('contrast maths', () => {
  it('matches known WCAG values', () => {
    expect(relativeLuminance('#ffffff')).toBeCloseTo(1, 5);
    expect(relativeLuminance('#000000')).toBeCloseTo(0, 5);
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 2);
    // The bug Task 1 fixed, kept as a regression witness.
    expect(contrastRatio('#50c86e', '#fafafa')).toBeCloseTo(2.05, 2);
  });
});

describe('every documented token pair clears its floor', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const pair of CONTRAST_PAIRS) {
      it(`${mode}: ${pair.fg} on ${pair.bg} >= ${pair.min}:1 (${pair.note})`, () => {
        const fg = THEME[mode][pair.fg];
        const bg = THEME[mode][pair.bg];
        expect(fg, `${pair.fg} missing from THEME.${mode}`).toBeTruthy();
        expect(bg, `${pair.bg} missing from THEME.${mode}`).toBeTruthy();
        expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(pair.min);
      });
    }
  }
});
