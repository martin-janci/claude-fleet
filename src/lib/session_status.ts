// Pure helpers shared by the Sidebar and its session rows (moved out of
// Sidebar.svelte, F5a).
import type { ProjectTreeRow } from './projects';
import type { SessionRow } from './sessions';
import { formatElapsed, promptPreview, sessionStart } from './attention';

export type Recency = 'all' | '8h' | '1d' | '3d' | '7d' | '30d';
export const RECENCY_VALUES: readonly Recency[] = ['all', '8h', '1d', '3d', '7d', '30d'];
export function isRecency(v: unknown): v is Recency {
  return typeof v === 'string' && (RECENCY_VALUES as readonly string[]).includes(v);
}

const RECENCY_WINDOW: Record<Recency, number | null> = {
  all: null,
  '8h': 60 * 60 * 8,
  '1d': 60 * 60 * 24,
  '3d': 60 * 60 * 24 * 3,
  '7d': 60 * 60 * 24 * 7,
  '30d': 60 * 60 * 24 * 30,
};

/** A unix-seconds timestamp falls in the recency window (`null`: never,
 *  unless the window is `all`). One rule for every row the sidebar lists —
 *  a session by its last activity, a past link by when it ended — so the
 *  Last active filter narrows every section, not only the project tree. */
export function withinRecency(ts: number | null | undefined, r: Recency, nowSec: number = Math.floor(Date.now() / 1000)): boolean {
  const window = RECENCY_WINDOW[r];
  if (window === null) return true;
  if (ts == null) return false;
  const ageSec = nowSec - ts;
  return ageSec <= window;
}

export function matchesRecency(p: ProjectTreeRow, r: Recency): boolean {
  const window = RECENCY_WINDOW[r];
  if (window === null) return true;
  if (p.project.last_session_at === null) return false;
  const ageSec = Math.floor(Date.now() / 1000) - p.project.last_session_at;
  return ageSec >= 0 && ageSec <= window;
}

/** "just now" / "5m ago" / "3h ago" / "2d ago" for a unix-seconds timestamp.
 *  `nowMs` is injectable so callers with a shared clock (and tests) stay
 *  deterministic. */
export function timeAgo(unixSecs: number, nowMs: number = Date.now()): string {
  // An unreadable timestamp (a transcript's free-form `timestamp`) is NaN
  // here; every comparison below is false for NaN, so it read "NaNd ago".
  if (!Number.isFinite(unixSecs)) return '';
  const ageSec = Math.floor((nowMs - unixSecs * 1000) / 1000);
  if (ageSec < 60) return 'just now';
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
  return `${Math.floor(ageSec / 86400)}d ago`;
}

/** Elapsed since the session started ("3h 5m"), or '' before it started. */
/** The short age the redesign's rows use (step 3.6, the manual's content
 *  rules): `now`, `2m`, `3h`, `4d`. */
export function shortAge(unixSecs: number, nowSec: number): string {
  const t = timeAgo(unixSecs, nowSec * 1000);
  return t === 'just now' ? 'now' : t.replace(/ ago$/, '');
}

export function rowElapsed(sess: SessionRow, nowSec: number): string {
  return sess.started_at !== null ? formatElapsed(sessionStart(sess), nowSec) : '';
}

/** First line of the last prompt, truncated for the row; '' when none. */
export function rowPrompt(sess: SessionRow): string {
  return promptPreview(sess.last_prompt, 48);
}
