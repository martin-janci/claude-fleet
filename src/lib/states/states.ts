// The states kit (redesign step 10.6): what every empty, loading, offline and
// no-results state shares. A loading state appears only after
// LOADING_DELAY_MS, so a quick load never flashes; the loaders (0.8, 10.10)
// wait the same.

/** How long a load runs before its skeleton (or loader) shows. */
export const LOADING_DELAY_MS = 400;

/** A way out of an empty or no-results state: one button. */
export interface StateAction {
  label: string;
  onclick: () => void;
  primary?: boolean;
  testid?: string;
}

/** "6 m", "2 h", "3 d": how long ago, at most two characters of unit. */
export function sinceWords(at: number | null | undefined, now: number): string | null {
  if (at === null || at === undefined) return null;
  const secs = Math.max(0, now - at);
  if (secs < 60) return `${secs} s`;
  if (secs < 3600) return `${Math.floor(secs / 60)} m`;
  if (secs < 86_400) return `${Math.floor(secs / 3600)} h`;
  return `${Math.floor(secs / 86_400)} d`;
}
