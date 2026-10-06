// What the person picks in the New session picker, decayed (project picker
// spec v2, D3): per device, like Raycast / Alfred. Not a server table —
// sessions started by agents or outside fleet say nothing about choice.
import { readPref, writePref } from './prefs';

export const FRECENCY_KEY = 'newsession.frecency';
export const HALF_LIFE_DAYS = 7;
export const FRECENCY_CAP = 200;
const DAY = 86_400;

export type FrecencyMap = Record<string, { score: number; at: number }>;

const isMap = (v: unknown): v is FrecencyMap =>
  typeof v === 'object' &&
  v !== null &&
  Object.values(v as Record<string, unknown>).every(
    (e) =>
      typeof e === 'object' &&
      e !== null &&
      typeof (e as { score: unknown }).score === 'number' &&
      typeof (e as { at: unknown }).at === 'number',
  );

export function readFrecency(): FrecencyMap {
  return readPref<FrecencyMap>(FRECENCY_KEY, {}, isMap);
}

export function decayed(entry: { score: number; at: number } | undefined, now: number): number {
  if (!entry) return 0;
  const days = Math.max(0, (now - entry.at) / DAY);
  return entry.score * Math.pow(2, -days / HALF_LIFE_DAYS);
}

export function recordPick(key: string, now: number = Math.floor(Date.now() / 1000)): void {
  const m = { ...readFrecency() };
  m[key] = { score: decayed(m[key], now) + 1, at: now };
  const keys = Object.keys(m);
  if (keys.length > FRECENCY_CAP) {
    keys
      .sort((a, b) => decayed(m[a], now) - decayed(m[b], now))
      .slice(0, keys.length - FRECENCY_CAP)
      .forEach((k) => delete m[k]);
  }
  writePref(FRECENCY_KEY, m);
}

export function recencyTerm(lastSessionAt: number | null, now: number): number {
  if (lastSessionAt == null) return 0;
  const days = Math.max(0, (now - lastSessionAt) / DAY);
  return 20 * Math.pow(2, -days / HALF_LIFE_DAYS);
}
