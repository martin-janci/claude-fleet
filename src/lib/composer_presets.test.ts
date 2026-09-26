import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  composerPresets,
  isPresetArray,
  loadComposerPresets,
  flushComposerPresets,
  resetComposerPresets,
  addPreset,
  updatePreset,
  removePreset,
  PRESETS_PREF,
} from './composer_presets';

const invoked = () => mockedInvoke as ReturnType<typeof vi.fn>;

/** What the backend answered last, so a write's echo is the list it stored. */
function backendHolds(list: { label: string; text: string }[]) {
  invoked().mockImplementation((cmd: string, args?: { entries?: unknown }) => {
    if (cmd === 'quick_replies') return Promise.resolve(list);
    if (cmd === 'set_quick_replies') return Promise.resolve(args?.entries);
    throw new Error(`unexpected command ${cmd}`);
  });
}

const SERVED = [
  { label: 'Clear', text: '/clear' },
  { label: 'Review', text: 'review this' },
];

beforeEach(() => {
  invoked().mockReset();
  localStorage.clear();
  composerPresets.set([]);
  backendHolds(SERVED);
});

describe('composer presets', () => {
  it('validates a stored value shape and rejects garbage', () => {
    expect(isPresetArray([{ label: 'a', text: 'b' }])).toBe(true);
    expect(isPresetArray([])).toBe(true);
    expect(isPresetArray([{ label: 'a' }])).toBe(false);
    expect(isPresetArray([{ label: 1, text: 'b' }])).toBe(false);
    expect(isPresetArray('nope')).toBe(false);
    expect(isPresetArray(null)).toBe(false);
  });

  it('reads the fleet list from the backend and caches it for the next launch', async () => {
    await loadComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('quick_replies', undefined);
    expect(get(composerPresets)).toEqual(SERVED);
    expect(JSON.parse(localStorage.getItem('cf:pref:' + PRESETS_PREF)!)).toEqual(SERVED);
  });

  it('keeps the cached list when the backend read fails, rather than emptying the row', async () => {
    await loadComposerPresets();
    invoked().mockRejectedValue({ code: 'E_HUB_OFFLINE', message: 'no hub' });
    const r = await loadComposerPresets();
    expect(r.ok).toBe(false);
    expect(get(composerPresets)).toEqual(SERVED);
  });

  it('saves an edit through the backend, debounced, and takes its answer as the list', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    updatePreset(1, { label: 'Ship', text: 'ship it' });
    // Nothing sent yet: the editor writes on every keystroke and this is one.
    expect(invoked()).not.toHaveBeenCalled();
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', {
      entries: [SERVED[0], { label: 'Ship', text: 'ship it' }],
    });
    expect(get(composerPresets)[1]).toEqual({ label: 'Ship', text: 'ship it' });
  });

  it('coalesces a typed label into one write', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    for (const label of ['S', 'Sh', 'Shi', 'Ship']) updatePreset(1, { label });
    await flushComposerPresets();
    expect(invoked().mock.calls.filter((c) => c[0] === 'set_quick_replies')).toHaveLength(1);
  });

  it('does not send a blank new row — the backend refuses a chip with no text', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    addPreset();
    await flushComposerPresets();
    expect(invoked()).not.toHaveBeenCalled();
    expect(get(composerPresets)).toHaveLength(SERVED.length + 1);
  });

  it('keeps a half-written new chip out of the write, and on screen', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    addPreset();
    updatePreset(2, { label: 'Ship' }); // a label typed before the prompt
    await flushComposerPresets();
    // The backend refuses a chip with no text, so it is not sent…
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: SERVED });
    // …and the row being typed into is still there.
    expect(get(composerPresets)).toHaveLength(SERVED.length + 1);
    expect(get(composerPresets)[2]).toEqual({ label: 'Ship', text: '' });
  });

  it('removes a chip and saves the shortened list', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    removePreset(0);
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: [SERVED[1]] });
  });

  it('reset asks the backend for the built-ins by storing nothing', async () => {
    await loadComposerPresets();
    // The backend answers an empty `set` with its defaults; mimic that.
    invoked().mockImplementation((cmd: string) => {
      if (cmd === 'set_quick_replies') return Promise.resolve(SERVED);
      return Promise.resolve(SERVED);
    });
    resetComposerPresets();
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: [] });
    expect(get(composerPresets)).toEqual(SERVED);
  });
});
