import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  composerPresets,
  DEFAULT_PRESETS,
  isPresetArray,
  resetComposerPresets,
  addPreset,
  updatePreset,
  removePreset,
} from './composer_presets';
import { DEFAULT_REVIEW_PROMPT } from './sessions';

beforeEach(() => resetComposerPresets());

describe('composer presets', () => {
  it('ship with the basic commands plus the review prompt', () => {
    const texts = DEFAULT_PRESETS.map((p) => p.text);
    expect(texts).toContain('/clear');
    expect(texts).toContain('/compact');
    expect(texts).toContain('/status');
    expect(texts).toContain(DEFAULT_REVIEW_PROMPT);
    expect(get(composerPresets)).toEqual(DEFAULT_PRESETS);
  });

  it('validates a stored value shape and rejects garbage', () => {
    expect(isPresetArray([{ label: 'a', text: 'b' }])).toBe(true);
    expect(isPresetArray([])).toBe(true);
    expect(isPresetArray([{ label: 'a' }])).toBe(false);
    expect(isPresetArray([{ label: 1, text: 'b' }])).toBe(false);
    expect(isPresetArray('nope')).toBe(false);
    expect(isPresetArray(null)).toBe(false);
  });

  it('add / update / remove edit the list in order; reset restores the defaults', () => {
    addPreset();
    let list = get(composerPresets);
    expect(list).toHaveLength(DEFAULT_PRESETS.length + 1);
    expect(list[list.length - 1]).toEqual({ label: '', text: '' });
    updatePreset(list.length - 1, { label: 'Tests', text: 'run the tests' });
    list = get(composerPresets);
    expect(list[list.length - 1]).toEqual({ label: 'Tests', text: 'run the tests' });
    removePreset(0);
    list = get(composerPresets);
    expect(list[0]).toEqual(DEFAULT_PRESETS[1]);
    resetComposerPresets();
    expect(get(composerPresets)).toEqual(DEFAULT_PRESETS);
  });
});
