import { describe, it, expect } from 'vitest';
import { labelText, parseLabel, sameTags } from './session_label';

describe('the Label field (G2.7): labels are session tags', () => {
  it('splits on spaces and commas, trims and drops repeats in order', () => {
    expect(parseLabel(' release, wip  release ')).toEqual({ tags: ['release', 'wip'], error: null });
    expect(parseLabel('')).toEqual({ tags: [], error: null });
  });

  it('refuses what the backend refuses, naming the word', () => {
    expect(parseLabel('fix;now').error).toBe('"fix;now" may only use letters, digits and _ . : -');
    expect(parseLabel('x'.repeat(33)).error).toMatch(/longer than 32 characters/);
    expect(parseLabel(Array.from({ length: 17 }, (_, i) => `t${i}`).join(' ')).error).toBe('At most 16 labels per session.');
    expect(parseLabel('team:ui v1.2 a_b-c').error).toBeNull();
  });

  it('reads a tag list back as the field shows it', () => {
    expect(labelText(['release', 'wip'])).toBe('release wip');
    expect(labelText(null)).toBe('');
    expect(sameTags(['a', 'b'], ['a', 'b'])).toBe(true);
    expect(sameTags(['a', 'b'], ['b', 'a'])).toBe(false);
  });
});
