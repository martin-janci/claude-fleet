/**
 * Tiny localStorage-backed key/value store for global UI prefs.
 *
 * Kept separate from session_ui because these are per-app prefs (sidebar
 * width, last-used filter) not tied to any specific tmux session.
 */

import { writable } from 'svelte/store';
import { detectMac } from './terminal_keys';
import type { SessionView } from './session_view';

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

/** Forget a pref so the next read falls back to its default. */
export function clearPref(key: string): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.removeItem(PREFIX + key);
  } catch {
    /* ignore */
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

// ─── Right-panel prefs ───────────────────────────────────────────────────

const isSessionView = (v: unknown): v is SessionView =>
  v === 'conversation' || v === 'terminal';

/**
 * Which sub-view the Session tab shows. One choice for the whole app, kept
 * across restarts — picking a session should not decide for you which of
 * its two views you get. Rows that can only offer one view override this
 * without writing to it; see `resolveSessionView`.
 */
export const sessionView = writable<SessionView>(
  readPref<SessionView>('ui.sessionView', 'conversation', isSessionView),
);
sessionView.subscribe((v) => writePref('ui.sessionView', v));
