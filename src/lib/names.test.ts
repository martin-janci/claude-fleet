import { describe, it, expect } from 'vitest';
import raw from './names.json?raw';
import {
  ADJECTIVES,
  NOUNS,
  MAX_TRIES,
  generateName,
  nameWords,
  isGeneratedName,
  tmuxNameSuffix,
} from './names';

// Deterministic rng: cycles through the given fractions.
function seq(values: number[]): () => number {
  let i = 0;
  return () => values[i++ % values.length];
}

describe('word lists', () => {
  it('are non-trivial and match the sizes the Rust twin asserts', () => {
    // Keep in sync with `service/names.rs::lists_match_frontend`. Both sides
    // read the same JSON, so this only fails if someone edits one list and
    // forgets the other assertion — which is the point.
    expect(ADJECTIVES.length).toBe(68);
    expect(NOUNS.length).toBe(124);
  });

  it('every word is lowercase ASCII, 3-8 letters, unique across both lists', () => {
    const all = [...ADJECTIVES, ...NOUNS];
    for (const w of all) {
      expect(w, w).toMatch(/^[a-z]{3,8}$/);
    }
    expect(new Set(all).size).toBe(all.length);
  });

  it('the JSON on disk is what the module loaded (Rust reads the same file)', () => {
    const parsed = JSON.parse(raw);
    expect(parsed.adjectives).toEqual(ADJECTIVES);
    expect(parsed.nouns).toEqual(NOUNS);
  });
});

describe('generateName', () => {
  it('returns adjective-noun from the lists', () => {
    const n = generateName(new Set(), seq([0, 0]));
    expect(n).toBe(`${ADJECTIVES[0]}-${NOUNS[0]}`);
    expect(isGeneratedName(n)).toBe(true);
  });

  it('skips names already taken (case-insensitively)', () => {
    const first = `${ADJECTIVES[0]}-${NOUNS[0]}`;
    const second = `${ADJECTIVES[0]}-${NOUNS[1]}`;
    const rng = seq([0, 0, 0, 1 / NOUNS.length]);
    expect(generateName(new Set([first.toUpperCase()]), rng)).toBe(second);
  });

  it('falls back to a numeric suffix after MAX_TRIES collisions', () => {
    const only = `${ADJECTIVES[0]}-${NOUNS[0]}`;
    const rng = seq([0]); // every draw yields `only`
    expect(generateName(new Set([only]), rng)).toBe(`${only}-2`);
    expect(generateName(new Set([only, `${only}-2`]), rng)).toBe(`${only}-3`);
  });

  it('never returns a name in the existing set (random rng, many rounds)', () => {
    const existing = new Set<string>();
    for (let i = 0; i < 500; i++) {
      const n = generateName(existing);
      expect(existing.has(n)).toBe(false);
      existing.add(n);
    }
  });

  it('honours MAX_TRIES as the retry budget', () => {
    let calls = 0;
    const rng = () => {
      calls++;
      return 0;
    };
    generateName(new Set([`${ADJECTIVES[0]}-${NOUNS[0]}`]), rng);
    // Two draws per try (adjective + noun).
    expect(calls).toBe(MAX_TRIES * 2);
  });
});

describe('helpers', () => {
  it('nameWords turns the slug into words', () => {
    expect(nameWords('blue-sirius')).toBe('blue sirius');
    expect(nameWords('blue-sirius-2')).toBe('blue sirius 2');
  });

  it('isGeneratedName recognises only list pairs', () => {
    expect(isGeneratedName('blue-sirius')).toBe(true);
    expect(isGeneratedName('blue-sirius-7')).toBe(true);
    expect(isGeneratedName('fix-login')).toBe(false);
    expect(isGeneratedName('blue')).toBe(false);
    expect(isGeneratedName('blue-sirius-x')).toBe(false);
  });

  it('tmuxNameSuffix strips the project prefix and -term', () => {
    expect(tmuxNameSuffix('dev-o-r--blue-sirius', 'o', 'r')).toBe('blue-sirius');
    expect(tmuxNameSuffix('dev-o-r--blue-sirius-term', 'o', 'r')).toBe('blue-sirius');
    expect(tmuxNameSuffix('dev-o-r', 'o', 'r')).toBeNull();
    expect(tmuxNameSuffix('other', 'o', 'r')).toBeNull();
  });
});
