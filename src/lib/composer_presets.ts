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
  /** A plain click sends at once instead of only filling the box. Served
   *  always; missing only in a cache written before the flag existed. */
  auto_send?: boolean;
}

export function isPresetArray(v: unknown): v is ComposerPreset[] {
  return (
    Array.isArray(v) &&
    v.every(
      (p) =>
        typeof p === 'object' &&
        p !== null &&
        typeof (p as ComposerPreset).label === 'string' &&
        typeof (p as ComposerPreset).text === 'string' &&
        ['boolean', 'undefined'].includes(typeof (p as ComposerPreset).auto_send),
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

/**
 * The list the backend last answered (a read or a write's echo): what a save
 * names as `expected`, so the backend refuses it with `E_CONFLICT` when
 * another device saved in between instead of overwriting that edit unseen.
 * `null` until the first answer — a save from the cache alone is
 * last-writer-wins, as every save was before.
 */
let served: ComposerPreset[] | null = null;

/**
 * True after a save was refused because another device changed the chips
 * first. The editor has been reloaded with that device's list; the Settings
 * dialog says so. Cleared by the next save that lands.
 */
export const presetsConflict = writable(false);

function take(list: ComposerPreset[]): void {
  composerPresets.set(list);
  served = list;
  cache(list);
}

/**
 * Read the fleet's chips. Called at startup (App.svelte) and after a save
 * refused with `E_CONFLICT`. Nothing announces a chip change made on another
 * device, so the other re-reads — a hub reconnect the hub could not replay,
 * and the Settings editor opening — go through
 * {@link refreshComposerPresetsIfIdle}, which leaves a pending edit alone.
 */
export async function loadComposerPresets(): Promise<Result<ComposerPreset[]>> {
  const r = await invokeCmd<ComposerPreset[]>('quick_replies');
  if (r.ok && isPresetArray(r.value)) take(r.value);
  return r;
}

/**
 * Re-read the chips unless an edit is pending (a debounced save or one on
 * the wire) — and drop the answer if an edit started while it was on its
 * way. A reload must never replace what the person is typing; that edit's
 * own save names the list it knew and is told about a newer one by
 * `E_CONFLICT`. Used by App's gap handler (a hub reconnect the hub could not
 * replay, `hub:resynced`) and by the Settings editor when it opens, so an
 * edit made there starts from what another device (the phone) saved since
 * launch instead of conflicting on its first keystroke. A reconnect the hub
 * did replay fires no gap handler, so chips changed on another device during
 * a replayed outage are picked up only when the editor opens.
 */
export async function refreshComposerPresetsIfIdle(): Promise<void> {
  if (timer !== null || inFlight !== null) return;
  const before = get(composerPresets);
  const r = await invokeCmd<ComposerPreset[]>('quick_replies');
  if (!r.ok || !isPresetArray(r.value)) return;
  if (timer !== null || inFlight !== null || get(composerPresets) !== before) return;
  take(r.value);
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
  const r = await invokeCmd<ComposerPreset[]>('set_quick_replies', { entries, expected: served });
  if (!r.ok && r.error.code === 'E_CONFLICT') {
    // Another device saved first. Its list wins, visibly: the editor shows
    // it, and the dialog says why the edit just made is gone.
    presetsConflict.set(true);
    await loadComposerPresets();
    return r;
  }
  if (r.ok && isPresetArray(r.value)) {
    served = r.value;
    presetsConflict.set(false);
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

/**
 * Saves run one after another: each names the previous one's answer as
 * `expected`, so a save started while the last is still on the wire would
 * name a list the backend no longer holds and conflict with itself.
 */
function saveAfterPrevious(): Promise<Result<ComposerPreset[]>> {
  const prev = inFlight;
  const next = prev ? prev.then(save, save) : save();
  inFlight = next;
  void next.finally(() => {
    if (inFlight === next) inFlight = null;
  });
  return next;
}

function schedule(): void {
  if (timer !== null) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    saveAfterPrevious();
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
    saveAfterPrevious();
  }
  await inFlight;
}

/** Restore the built-in list (the backend's, not a copy kept here). */
export function resetComposerPresets(): void {
  composerPresets.set([]);
  schedule();
}

export function addPreset(): void {
  composerPresets.update((list) => [...list, { label: '', text: '', auto_send: false }]);
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

/**
 * Move a chip one place up (`-1`) or down (`1`). The list's order is the chip
 * row's order on every client, so this is saved like any other edit.
 */
export function movePreset(index: number, dir: -1 | 1): void {
  const to = index + dir;
  const list = get(composerPresets);
  if (index < 0 || index >= list.length || to < 0 || to >= list.length) return;
  const next = [...list];
  [next[index], next[to]] = [next[to], next[index]];
  composerPresets.set(next);
  schedule();
}

/**
 * Whether a click on the chip sends it: the chip's own `auto_send`, inverted
 * by Shift so either behaviour stays one gesture away.
 */
export function presetSendsNow(p: ComposerPreset, shiftKey: boolean): boolean {
  return (p.auto_send === true) !== shiftKey;
}
