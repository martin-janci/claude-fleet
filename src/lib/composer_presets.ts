/**
 * Quick-action presets for the Conversation composer: the row of chips above
 * the prompt box, and the same list fleet-mobile draws above its own.
 *
 * The list lives on the backend (`service::quick_replies`, one row in the
 * `settings` table) and is routed to the hub when this desktop is a window
 * onto one — so a chip added here shows up on the phone, and the other way
 * round. It used to be per-app `localStorage`, which meant the desktop and
 * the phone each had their own and neither could see the other's.
 *
 * `localStorage` is still used, for exactly one thing: a cache of the last
 * list the backend served, so the composer draws the chips you know
 * *immediately* on launch and while a hub round trip is in flight, instead of
 * flashing the built-in defaults. It is never the source of truth, and an
 * edit is not saved by writing it.
 */
import { writable, get } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { invokeCmd } from './result';
import type { Result } from './result';

export interface ComposerPreset {
  label: string;
  text: string;
}

export function isPresetArray(v: unknown): v is ComposerPreset[] {
  return (
    Array.isArray(v) &&
    v.every(
      (p) =>
        typeof p === 'object' &&
        p !== null &&
        typeof (p as ComposerPreset).label === 'string' &&
        typeof (p as ComposerPreset).text === 'string',
    )
  );
}

/** Where the cache of the last served list lives. */
export const PRESETS_PREF = 'composer-presets';

/**
 * The chips as last read from (or written to) the backend.
 *
 * Seeded from the cache so the first paint has the right buttons; replaced by
 * {@link loadComposerPresets} as soon as the backend answers. Empty only on a
 * device that has never reached a backend — the built-in defaults come from
 * there, deliberately, so a later release can improve a built-in chip without
 * every installed client pinning its own copy.
 */
export const composerPresets = writable<ComposerPreset[]>(
  readPref<ComposerPreset[]>(PRESETS_PREF, [], isPresetArray),
);

function cache(list: ComposerPreset[]): void {
  writePref(PRESETS_PREF, list);
}

/** Read the fleet's chips. Called at startup and after a hub reconnect. */
export async function loadComposerPresets(): Promise<Result<ComposerPreset[]>> {
  const r = await invokeCmd<ComposerPreset[]>('quick_replies');
  if (r.ok && isPresetArray(r.value)) {
    composerPresets.set(r.value);
    cache(r.value);
  }
  return r;
}

/**
 * How long an edit sits before it is sent.
 *
 * The Settings editor writes on every keystroke (`oninput`, so that the chip
 * row previews as you type), and each save is a routed hub call. Debounced,
 * a typed label is one write instead of one per character; the delay is short
 * enough that closing the dialog straight after typing still lands inside it,
 * and {@link flushComposerPresets} exists for the cases that must not wait.
 */
export const SAVE_DEBOUNCE_MS = 400;

let timer: ReturnType<typeof setTimeout> | null = null;
let inFlight: Promise<Result<ComposerPreset[]>> | null = null;

async function save(): Promise<Result<ComposerPreset[]>> {
  const local = get(composerPresets);
  // A chip with no text yet is the editor's blank "new chip" row, not a chip:
  // the backend refuses one (it would send nothing), so it is left out of the
  // write and stays in the editor until it has a prompt.
  const entries = local.filter((p) => p.text.trim().length > 0);
  const r = await invokeCmd<ComposerPreset[]>('set_quick_replies', { entries });
  if (r.ok && isPresetArray(r.value)) {
    cache(r.value);
    // The backend normalises (trims, drops duplicate prompts, restores the
    // defaults for an empty list), so what it answers — not what was sent —
    // is the list. Taking it back is what makes "remove them all" show the
    // defaults returning rather than an empty row.
    //
    // Except while the editor holds a row the write left out, or while a
    // later edit has already moved on: the answer cannot carry either, and
    // assigning it would delete a half-typed chip under the person typing it.
    if (entries.length === local.length && get(composerPresets) === local) {
      composerPresets.set(r.value);
    }
  }
  return r;
}

function schedule(): void {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    inFlight = save();
  }, SAVE_DEBOUNCE_MS);
}

/**
 * Send a pending edit now and wait for it. Safe to call when nothing is
 * pending. Used by the Settings dialog when it closes, and by tests.
 */
export async function flushComposerPresets(): Promise<void> {
  if (timer !== null) {
    clearTimeout(timer);
    timer = null;
    inFlight = save();
  }
  await inFlight;
}

/** Restore the built-in list (the backend's, not a copy kept here). */
export function resetComposerPresets(): void {
  composerPresets.set([]);
  schedule();
}

export function addPreset(): void {
  composerPresets.update((list) => [...list, { label: '', text: '' }]);
  // Not scheduled: a blank row is the editor's "new chip" placeholder, and
  // the backend refuses a chip with no text. It saves on the first keystroke
  // in it, through updatePreset.
}

export function updatePreset(index: number, patch: Partial<ComposerPreset>): void {
  composerPresets.update((list) => list.map((p, i) => (i === index ? { ...p, ...patch } : p)));
  schedule();
}

export function removePreset(index: number): void {
  composerPresets.update((list) => list.filter((_, i) => i !== index));
  schedule();
}
