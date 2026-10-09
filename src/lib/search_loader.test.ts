import { describe, expect, it } from 'vitest';
import { LONG_SEARCH_MS, searchLoader } from './search_loader';

describe('searchLoader (step 9.12)', () => {
  it('is the Dot wave while a search is short and Comet trails once it runs long', () => {
    expect(searchLoader(0)).toBe('dot-wave');
    expect(searchLoader(LONG_SEARCH_MS - 1)).toBe('dot-wave');
    expect(searchLoader(LONG_SEARCH_MS)).toBe('comet-trails');
    expect(searchLoader(60_000)).toBe('comet-trails');
  });
});
