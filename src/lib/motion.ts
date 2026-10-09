import { derived, readable, writable, get, type Readable } from 'svelte/store';
import { readPref, writePref } from './prefs';

/**
 * Motion (redesign step 0.6). The Motion pref picks how much the UI moves;
 * `effectiveMotion` resolves it against the OS and is written to
 * `<html data-motion>`, where app.css retimes the duration tokens:
 *
 * - **full**: the manual's 80 to 280 ms (`--dur-fast`, `--dur-base`, `--dur-slow`).
 * - **reduced**: only short fades; every UI duration is `--dur-fast` (80 ms),
 *   and loaders (0.8) and every other loop (`--loop-*`: a pulsing step, a
 *   skeleton, a stream of dots) turn into one `--loader-reduced` fade.
 * - **off**: no UI motion; every duration is 0 ms and other loops rest.
 *   Loaders still fade, so a wait never looks frozen.
 *
 * `system` (the default) is Full, or Reduced when the OS asks for less motion.
 * Every transition in the app reads a `--dur-*` token, never a raw time
 * (`motion.test.ts`), so this one attribute governs all of them.
 */

export type MotionPref = 'system' | 'full' | 'reduced' | 'off';
export type Motion = Exclude<MotionPref, 'system'>;

export const MOTION_PREFS: readonly MotionPref[] = ['system', 'full', 'reduced', 'off'];

const isMotionPref = (v: unknown): v is MotionPref => MOTION_PREFS.includes(v as MotionPref);

export const motionPref = writable<MotionPref>(readPref<MotionPref>('ui.motion', 'system', isMotionPref));
motionPref.subscribe((v) => writePref('ui.motion', v));

const REDUCE_QUERY = '(prefers-reduced-motion: reduce)';

/** Whether the OS asks for reduced motion, kept live. */
export const osPrefersReduced: Readable<boolean> = readable(false, (set) => {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
  const mq = window.matchMedia(REDUCE_QUERY);
  set(mq.matches);
  const on = (e: MediaQueryListEvent) => set(e.matches);
  mq.addEventListener?.('change', on);
  return () => mq.removeEventListener?.('change', on);
});

export function resolveMotion(pref: MotionPref, osReduced: boolean): Motion {
  if (pref === 'system') return osReduced ? 'reduced' : 'full';
  return pref;
}

export const effectiveMotion: Readable<Motion> = derived([motionPref, osPrefersReduced], ([p, os]) =>
  resolveMotion(p, os),
);

/** The duration tokens in ms for each motion level, as app.css sets them. */
export const DURATIONS: Record<Motion, Record<'fast' | 'base' | 'slow', number>> = {
  full: { fast: 80, base: 160, slow: 280 },
  reduced: { fast: 80, base: 80, slow: 80 },
  off: { fast: 0, base: 0, slow: 0 },
};

/** A duration token in ms under the current motion level, for JS-driven motion. */
export function durationMs(token: 'fast' | 'base' | 'slow'): number {
  return DURATIONS[get(effectiveMotion)][token];
}

/** Mirrors the effective level onto `<html data-motion>`; call once at startup. */
export function initMotion(): () => void {
  return effectiveMotion.subscribe((m) => document.documentElement.setAttribute('data-motion', m));
}
