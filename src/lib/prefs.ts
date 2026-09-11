/**
 * Tiny localStorage-backed key/value store for global UI prefs.
 *
 * Kept separate from session_ui because these are per-app prefs (sidebar
 * width, last-used filter) not tied to any specific tmux session.
 */

import { writable } from 'svelte/store';
import { detectMac } from './terminal_keys';

const PREFIX = 'cf:pref:';

export function readPref<T>(key: string, fallback: T, isValid: (v: unknown) => v is T): T {
  if (typeof localStorage === 'undefined') return fallback;
  try {
    const raw = localStorage.getItem(PREFIX + key);
    if (raw === null) return fallback;
    const parsed = JSON.parse(raw);
    return isValid(parsed) ? parsed : fallback;
  } catch {
    return fallback;
  }
}

export function writePref<T>(key: string, value: T): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    /* quota — silently degrade */
  }
}

// ─── Terminal prefs ──────────────────────────────────────────────────────

const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

/**
 * Copy a drag-selection to the clipboard as soon as the mouse is released
 * (X11 "primary selection" habit). Defaults on for macOS to keep the
 * behaviour the app shipped with there, off elsewhere — on Linux/Windows a
 * drag would otherwise silently overwrite whatever the user just copied.
 * Explicit copy (Cmd+C / Ctrl+Shift+C / context menu) always works.
 */
export const copyOnSelect = writable<boolean>(
  readPref(
    'terminal.copyOnSelect',
    detectMac(typeof navigator === 'undefined' ? undefined : navigator),
    isBool,
  ),
);
copyOnSelect.subscribe((v) => writePref('terminal.copyOnSelect', v));
