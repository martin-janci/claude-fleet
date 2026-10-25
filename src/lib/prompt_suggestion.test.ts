import { describe, expect, it } from 'vitest';
import { offeredSuggestion } from './prompt_suggestion';

describe('offeredSuggestion', () => {
  it('offers the proposal of an idle session with an empty box', () => {
    expect(offeredSuggestion({ prompt_suggestion: ' run the tests ' }, 'idle', '')).toBe('run the tests');
  });

  it('offers nothing while a turn runs, over a draft, or without a proposal', () => {
    expect(offeredSuggestion({ prompt_suggestion: 'run the tests' }, 'working', '')).toBeNull();
    expect(offeredSuggestion({ prompt_suggestion: 'run the tests' }, 'idle', 'fix')).toBeNull();
    expect(offeredSuggestion({ prompt_suggestion: null }, 'idle', '')).toBeNull();
    expect(offeredSuggestion({}, 'idle', '')).toBeNull();
    expect(offeredSuggestion({ prompt_suggestion: '  ' }, 'idle', '')).toBeNull();
  });

  it('offers nothing while a prompt of our own is in flight', () => {
    expect(offeredSuggestion({ prompt_suggestion: 'run the tests' }, 'idle', '', true)).toBeNull();
  });
});
