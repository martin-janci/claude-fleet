import { writable, get } from 'svelte/store';
import { readPref, writePref } from './prefs';
import { ACCENTS, defaultPlace, orbitPalette, sunHeight, sunTimes, type AccentId, type ContrastLevel } from './palette';

/**
 * The theme (dynamic day/night, "Mission HUD" look). `sun` follows the sun
 * in your time zone (palette.ts `defaultPlace`: the zone's meridian at a fixed
 * latitude, not your location): light while it is up, dark while it is down, drifting
 * through golden hour, blue hour and night in between (palette.ts). `auto`
 * follows the system's light/dark setting; `light` and `dark` hold noon and
 * a mid-night. Every mode is painted by the palette engine, so Contrast and
 * Accent apply to all of them; app.css's blocks are the first paint before
 * this runs, and the fallback if it never does.
 */
export type Theme = 'sun' | 'auto' | 'light' | 'dark';
const KEY = 'cf:theme';
const ORDER: Theme[] = ['sun', 'auto', 'light', 'dark'];
const isTheme = (v: unknown): v is Theme => ORDER.includes(v as Theme);

/** Each theme's name as Settings and the command palette show it. */
export const THEMES = [
  { id: 'sun', label: 'Sun' },
  { id: 'auto', label: 'System' },
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
] as const satisfies readonly { id: Theme; label: string }[];

export const themeLabel = (t: Theme): string => THEMES.find((x) => x.id === t)!.label;

/**
 * The last palette `paint()` wrote, which public/theme-boot.js replays in
 * <head> before any stylesheet paints, so a launch opens on the engine's
 * colours instead of flashing app.css's first. Keep the key and shape in
 * step with that file.
 */
export const BOOT_KEY = 'cf:theme-paint';

export type ContrastPref = 'system' | ContrastLevel;
export const CONTRAST_PREFS: readonly ContrastPref[] = ['system', 'soft', 'std', 'high'];
const isContrastPref = (v: unknown): v is ContrastPref => CONTRAST_PREFS.includes(v as ContrastPref);
const isAccent = (v: unknown): v is AccentId => ACCENTS.some((a) => a.id === v);
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

function read(): Theme {
  try {
    const v = localStorage.getItem(KEY);
    return isTheme(v) ? v : 'sun';
  } catch {
    return 'sun';
  }
}

export const theme = writable<Theme>(read());
export const contrastPref = writable<ContrastPref>(readPref<ContrastPref>('ui.contrast', 'system', isContrastPref));
export const accentPref = writable<AccentId>(readPref<AccentId>('ui.accent', 'cyan', isAccent));
/** The fleet's mood as a thin glowing line along the top of the window (MoodLine.svelte). Off by default. */
export const moodLine = writable<boolean>(readPref('ui.moodLine', false, isBool));
contrastPref.subscribe((v) => writePref('ui.contrast', v));
accentPref.subscribe((v) => writePref('ui.accent', v));
moodLine.subscribe((v) => writePref('ui.moodLine', v));

/** Where Light and Dark sit on the sun's curve: noon, and a mid-night. */
const LIGHT_SUN = 1;
const DARK_SUN = -0.6;

const media = (q: string) => (typeof window !== 'undefined' && typeof window.matchMedia === 'function' ? window.matchMedia(q) : null);

/** The sun height a mode paints at `now`. */
export function sunFor(mode: Theme, now: Date, systemDark: boolean): number {
  if (mode === 'light') return LIGHT_SUN;
  if (mode === 'dark') return DARK_SUN;
  if (mode === 'auto') return systemDark ? DARK_SUN : LIGHT_SUN;
  const { lat, lon } = defaultPlace(now);
  return sunHeight(now.getHours() + now.getMinutes() / 60, sunTimes(now, lat, lon));
}

export function resolveContrast(pref: ContrastPref, systemMore: boolean): ContrastLevel {
  return pref === 'system' ? (systemMore ? 'high' : 'std') : pref;
}

let painted: Record<string, string> = {};

/** The tokens the boot snapshot holds (what theme-boot.js may have set). */
function bootVars(): Record<string, string> {
  try {
    const v = JSON.parse(localStorage.getItem(BOOT_KEY) ?? '{}')?.vars;
    return v && typeof v === 'object' ? v : {};
  } catch {
    return {};
  }
}
let lastDark: boolean | null = null;
let fadeTimer: ReturnType<typeof setTimeout> | null = null;

/** Paints the engine's tokens onto <html> for the current prefs and time. */
export function paint(now: Date = new Date()): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  const sun = sunFor(get(theme), now, media('(prefers-color-scheme: dark)')?.matches ?? false);
  const contrast = resolveContrast(get(contrastPref), media('(prefers-contrast: more)')?.matches ?? false);
  const hue = ACCENTS.find((a) => a.id === get(accentPref))!.hue;
  const next = orbitPalette({ sun, contrast, hue });
  const dark = sun < 0;
  // A flip between light and dark crossfades (app.css, `data-theme-fade`);
  // the minute-by-minute drift is too small to need it.
  if (lastDark !== null && lastDark !== dark) {
    root.setAttribute('data-theme-fade', '');
    if (fadeTimer) clearTimeout(fadeTimer);
    fadeTimer = setTimeout(() => root.removeAttribute('data-theme-fade'), 1200);
  }
  // The first paint also clears what theme-boot.js replayed from an older
  // engine that this one no longer writes.
  const before = lastDark === null ? { ...bootVars(), ...painted } : painted;
  lastDark = dark;
  root.setAttribute('data-theme', dark ? 'dark' : 'light');
  for (const name of Object.keys(before)) if (!(name in next)) root.style.removeProperty(`--${name}`);
  for (const [name, value] of Object.entries(next)) {
    if (painted[name] !== value) root.style.setProperty(`--${name}`, value);
  }
  painted = next;
  try {
    const snap = JSON.stringify({ theme: dark ? 'dark' : 'light', vars: next });
    if (localStorage.getItem(BOOT_KEY) !== snap) localStorage.setItem(BOOT_KEY, snap);
  } catch {
    /* no storage: the next launch paints app.css first, as before */
  }
}

export function applyTheme(next: Theme): void {
  theme.set(next);
  try {
    localStorage.setItem(KEY, next);
  } catch {
    /* quota — the choice still applies for this run */
  }
  paint();
}

export function cycleTheme(): void {
  const idx = ORDER.indexOf(get(theme));
  applyTheme(ORDER[(idx + 1) % ORDER.length]);
}

// A Contrast or Accent change repaints at once (the first call, on import,
// is the earliest paint the engine can give).
contrastPref.subscribe(() => paint());
accentPref.subscribe(() => paint());

let started = false;

/** Paints once, then keeps the theme on the clock and the system settings. */
export function initTheme(): void {
  theme.set(read());
  paint();
  if (started) return;
  started = true;
  for (const q of ['(prefers-color-scheme: dark)', '(prefers-contrast: more)']) media(q)?.addEventListener?.('change', () => paint());
  // The sun moves slowly: once a minute is plenty, and a window coming back
  // from sleep repaints at once.
  setInterval(() => {
    if (get(theme) === 'sun') paint();
  }, 60_000);
  if (typeof document !== 'undefined') document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && paint());
}
