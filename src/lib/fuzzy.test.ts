import { describe, it, expect } from 'vitest';
import { fuzzyScore, fuzzyMatchFields } from './fuzzy';

describe('fuzzyScore', () => {
  it('returns null when the query is not a subsequence', () => {
    expect(fuzzyScore('xyz', 'blue-sirius')).toBeNull();
    expect(fuzzyScore('sirius', 'sir')).toBeNull();
  });

  it('empty query matches everything with a neutral score', () => {
    expect(fuzzyScore('', 'anything')).toBe(0);
  });

  it('is case-insensitive', () => {
    expect(fuzzyScore('BLUE', 'blue-sirius')).not.toBeNull();
    expect(fuzzyScore('blue', 'Blue Sirius')).not.toBeNull();
  });

  it('ranks a substring above a scattered subsequence', () => {
    const sub = fuzzyScore('sir', 'blue-sirius')!;
    const scattered = fuzzyScore('sir', 'serious-fixer')!;
    expect(sub).toBeGreaterThan(scattered);
  });

  it('ranks a word-start match above a mid-word match', () => {
    const wordStart = fuzzyScore('fix', 'feat/fix-login')!;
    const midWord = fuzzyScore('fix', 'prefixes')!;
    expect(wordStart).toBeGreaterThan(midWord);
  });

  it('matches initials across word boundaries', () => {
    expect(fuzzyScore('bs', 'blue-sirius')).not.toBeNull();
    expect(fuzzyScore('cf', 'claude-fleet')).not.toBeNull();
  });

  it('prefers the shorter haystack on otherwise equal matches', () => {
    expect(fuzzyScore('vega', 'vega')!).toBeGreaterThan(fuzzyScore('vega', 'vega-extended')!);
  });
});

describe('fuzzyMatchFields', () => {
  it('requires every token to hit some field', () => {
    const fields = ['blue-sirius', 'martin-janci/claude-fleet', 'mefistos'];
    expect(fuzzyMatchFields('blue mef', fields)).not.toBeNull();
    expect(fuzzyMatchFields('blue nope', fields)).toBeNull();
  });

  it('sums the best score per token', () => {
    const fields = ['blue-sirius', 'local'];
    const one = fuzzyMatchFields('blue', fields)!;
    const two = fuzzyMatchFields('blue local', fields)!;
    expect(two).toBeGreaterThan(one);
  });

  it('blank query matches with score 0', () => {
    expect(fuzzyMatchFields('   ', ['x'])).toBe(0);
  });
});
