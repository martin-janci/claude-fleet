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
  refreshComposerPresetsIfIdle,
  flushComposerPresets,
  resetComposerPresets,
  addPreset,
  updatePreset,
  removePreset,
  movePreset,
  presetSendsNow,
  presetsConflict,
  PRESETS_PREF,
  SAVE_DEBOUNCE_MS,
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
      expected: SERVED,
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
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: SERVED, expected: SERVED });
    // …and the row being typed into is still there.
    expect(get(composerPresets)).toHaveLength(SERVED.length + 1);
    expect(get(composerPresets)[2]).toEqual({ label: 'Ship', text: '', auto_send: false });
  });

  it('moves a chip and saves the list in its new order', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    movePreset(1, -1);
    await flushComposerPresets();
    const swapped = [SERVED[1], SERVED[0]];
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: swapped, expected: SERVED });
    expect(get(composerPresets)).toEqual(swapped);
  });

  it('does not move a chip past either end', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    movePreset(0, -1);
    movePreset(SERVED.length - 1, 1);
    await flushComposerPresets();
    expect(invoked()).not.toHaveBeenCalled();
    expect(get(composerPresets)).toEqual(SERVED);
  });

  it('saves the auto-send flag with the chip', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    updatePreset(0, { auto_send: true });
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', {
      entries: [{ ...SERVED[0], auto_send: true }, SERVED[1]],
      expected: SERVED,
    });
  });

  it('an auto-send chip sends on a click and fills on Shift+click; the rest the other way', () => {
    expect(presetSendsNow({ label: 'a', text: 'b', auto_send: true }, false)).toBe(true);
    expect(presetSendsNow({ label: 'a', text: 'b', auto_send: true }, true)).toBe(false);
    expect(presetSendsNow({ label: 'a', text: 'b', auto_send: false }, false)).toBe(false);
    expect(presetSendsNow({ label: 'a', text: 'b' }, true)).toBe(true);
    expect(isPresetArray([{ label: 'a', text: 'b', auto_send: 'yes' }])).toBe(false);
  });

  it('removes a chip and saves the shortened list', async () => {
    await loadComposerPresets();
    invoked().mockClear();
    removePreset(0);
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: [SERVED[1]], expected: SERVED });
  });

  it('names the list it last saw, so a save after another device’s is a conflict it shows', async () => {
    await loadComposerPresets();
    const theirs = [{ label: 'Theirs', text: 'their prompt', auto_send: false }];
    invoked().mockImplementation((cmd: string) => {
      if (cmd === 'quick_replies') return Promise.resolve(theirs);
      if (cmd === 'set_quick_replies') return Promise.reject({ code: 'E_CONFLICT', message: 'changed elsewhere' });
      throw new Error(`unexpected command ${cmd}`);
    });
    updatePreset(0, { label: 'Mine' });
    await flushComposerPresets();
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', {
      entries: [{ ...SERVED[0], label: 'Mine' }, SERVED[1]],
      expected: SERVED,
    });
    // The other device's list is what the editor now shows, and it says why.
    expect(get(composerPresets)).toEqual(theirs);
    expect(get(presetsConflict)).toBe(true);
    // The next save names that list, and landing clears the note.
    backendHolds(theirs);
    updatePreset(0, { label: 'Mine again' });
    await flushComposerPresets();
    expect(invoked()).toHaveBeenLastCalledWith('set_quick_replies', {
      entries: [{ ...theirs[0], label: 'Mine again' }],
      expected: theirs,
    });
    expect(get(presetsConflict)).toBe(false);
  });

  it('a save waits for the one on the wire, so it names that one’s answer', async () => {
    await loadComposerPresets();
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const calls: unknown[] = [];
    invoked().mockImplementation(async (cmd: string, args?: { entries?: unknown }) => {
      if (cmd !== 'set_quick_replies') throw new Error(cmd);
      calls.push(args);
      if (calls.length === 1) await gate;
      return args?.entries;
    });
    updatePreset(0, { label: 'One' });
    const first = flushComposerPresets();
    updatePreset(0, { label: 'Two' });
    const second = flushComposerPresets();
    await Promise.resolve();
    expect(calls).toHaveLength(1);
    release();
    await first;
    await second;
    expect(calls).toHaveLength(2);
    expect((calls[1] as { expected: unknown }).expected).toEqual([{ ...SERVED[0], label: 'One' }, SERVED[1]]);
  });

  it('an idle refresh re-reads the list and names it in the next save', async () => {
    await loadComposerPresets();
    const theirs = [{ label: 'Phone', text: 'from the phone', auto_send: false }];
    backendHolds(theirs);
    await refreshComposerPresetsIfIdle();
    expect(get(composerPresets)).toEqual(theirs);
    expect(JSON.parse(localStorage.getItem('cf:pref:' + PRESETS_PREF)!)).toEqual(theirs);
    // `served` moved too: the next edit names the phone's list, not a stale one.
    updatePreset(0, { label: 'Desk' });
    await flushComposerPresets();
    expect(invoked()).toHaveBeenLastCalledWith('set_quick_replies', {
      entries: [{ ...theirs[0], label: 'Desk' }],
      expected: theirs,
    });
  });

  it('a refresh never replaces the list while a debounced edit is pending', async () => {
    vi.useFakeTimers();
    try {
      await loadComposerPresets();
      backendHolds([{ label: 'Phone', text: 'from the phone' }]);
      invoked().mockClear();
      updatePreset(0, { label: 'Typing' });
      await refreshComposerPresetsIfIdle();
      expect(invoked()).not.toHaveBeenCalledWith('quick_replies', undefined);
      expect(get(composerPresets)[0].label).toBe('Typing');
      await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS);
      await flushComposerPresets();
      expect(invoked()).toHaveBeenCalledWith('set_quick_replies', {
        entries: [{ ...SERVED[0], label: 'Typing' }, SERVED[1]],
        expected: SERVED,
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it('a refresh drops its answer when an edit started while it was on the wire', async () => {
    await loadComposerPresets();
    let answer!: (v: unknown) => void;
    invoked().mockImplementation((cmd: string, args?: { entries?: unknown }) => {
      if (cmd === 'quick_replies') return new Promise((r) => (answer = r));
      if (cmd === 'set_quick_replies') return Promise.resolve(args?.entries);
      throw new Error(`unexpected command ${cmd}`);
    });
    const pending = refreshComposerPresetsIfIdle();
    updatePreset(0, { label: 'Typing' });
    answer([{ label: 'Phone', text: 'from the phone' }]);
    await pending;
    expect(get(composerPresets)[0].label).toBe('Typing');
    await flushComposerPresets();
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
    expect(invoked()).toHaveBeenCalledWith('set_quick_replies', { entries: [], expected: SERVED });
    expect(get(composerPresets)).toEqual(SERVED);
  });
});
