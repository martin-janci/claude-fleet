import { describe, expect, it } from 'vitest';
import { fold, foldedIncludes } from './text_fold';
import { fuzzyScore } from './fuzzy';

describe('fold', () => {
  it('drops case and diacritics', () => {
    expect(fold('Úloha ŽLTÝ kôň')).toBe('uloha zlty kon');
    expect(fold('Łódź Straße Ærø')).toBe('lodz straße æro');
    expect(fold('Café')).toBe('cafe');
    expect(fold('plain-ascii_42')).toBe('plain-ascii_42');
  });

  it('keeps a precomposed string its length, so find can paint ranges', () => {
    const s = 'Príliš žluťoučký kůň';
    expect(fold(s)).toHaveLength(s.length);
  });

  it('matches either way round', () => {
    expect(foldedIncludes('Oprava prihlásenia', fold('prihlasenia'))).toBe(true);
    expect(foldedIncludes('Oprava prihlasenia', fold('prihlásenia'))).toBe(true);
    expect(foldedIncludes(null, 'x')).toBe(false);
  });

  it('lets the quick switcher match without accents', () => {
    expect(fuzzyScore('uloha', 'TASK-7 Úloha na zajtra')).not.toBeNull();
  });
});

describe('matchRanges', () => {
  it('marks each word where it matched, on the shown text', async () => {
    const { matchRanges } = await import('./fuzzy');
    expect(matchRanges('prihlasenia sso', 'Oprava prihlásenia cez SSO')).toEqual([
      [7, 18],
      [23, 26],
    ]);
    expect(matchRanges('bs', 'blue-sirius')).toEqual([
      [0, 1],
      [5, 6],
    ]);
    expect(matchRanges('zzz', 'blue')).toEqual([]);
  });
});
