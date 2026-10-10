// The destructive confirm's rules (G1.4, FormsAnatomy): a typed name only
// when the loss is large, and the name must match.
import { describe, it, expect } from 'vitest';
import { LARGE_LOSS, countOf, needsTypedName, typedNameMatches, typedNameWhy } from './destructive';

describe('destructive confirm rules', () => {
  it('asks for the typed name from LARGE_LOSS things lost, not below', () => {
    expect(needsTypedName(0)).toBe(false);
    expect(needsTypedName(LARGE_LOSS - 1)).toBe(false);
    expect(needsTypedName(LARGE_LOSS)).toBe(true);
    expect(needsTypedName(41)).toBe(true);
    expect(needsTypedName(Number.NaN)).toBe(false);
  });

  it('matches the exact name, spaces around it trimmed; an empty name never matches', () => {
    expect(typedNameMatches('Morning PR sweep', 'Morning PR sweep')).toBe(true);
    expect(typedNameMatches('  Morning PR sweep ', 'Morning PR sweep')).toBe(true);
    expect(typedNameMatches('morning pr sweep', 'Morning PR sweep')).toBe(false);
    expect(typedNameMatches('Morning PR', 'Morning PR sweep')).toBe(false);
    expect(typedNameMatches('', '')).toBe(false);
  });

  it('says why the verb is off, and counts with the right noun', () => {
    expect(typedNameWhy('routine')).toBe('Type the routine name to confirm.');
    expect(countOf(1, 'run')).toBe('1 run');
    expect(countOf(41, 'run')).toBe('41 runs');
  });
});
