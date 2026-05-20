/**
 * Tiny localStorage-backed key/value store for global UI prefs.
 *
 * Kept separate from session_ui because these are per-app prefs (sidebar
 * width, last-used filter) not tied to any specific tmux session.
 */

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
