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

// ─── Layout pref (redesign step 0.3) ─────────────────────────────────────

export type UiLayout = 'classic' | 'new';

const isUiLayout = (v: unknown): v is UiLayout => v === 'classic' || v === 'new';

/**
 * Which shell the app shows: Classic (0.5.x) or New (the Orbit Fleet
 * redesign, growing behind this switch until the parity sign-off in 7.5).
 * Classic stays the default until then. Nothing reads it yet besides the
 * Appearance section; the destination store in 3.1 is the first consumer.
 */
export const uiLayout = writable<UiLayout>(readPref<UiLayout>('ui.layout', 'classic', isUiLayout));
uiLayout.subscribe((v) => writePref('ui.layout', v));

// ─── Row density (redesign step 3.6) ─────────────────────────────────────

export type UiDensity = 'comfortable' | 'compact';

const isUiDensity = (v: unknown): v is UiDensity => v === 'comfortable' || v === 'compact';

/**
 * How tall a session row is. Comfortable is 0.5.4's row, every badge
 * included; Compact is the redesign's two-line row (a sans title, one meta
 * line, chips until hover) at {@link COMPACT_ROW_PX}.
 */
export const uiDensity = writable<UiDensity>(readPref<UiDensity>('ui.density', 'comfortable', isUiDensity));
uiDensity.subscribe((v) => writePref('ui.density', v));

/** A Compact row's height: 20 of them fit a 1080p window with 280 px to spare
 *  for the title bar, header and the list's own chrome. */
export const COMPACT_ROW_PX = 40;
