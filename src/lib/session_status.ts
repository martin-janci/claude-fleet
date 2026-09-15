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
  const ageSec = Math.floor((nowMs - unixSecs * 1000) / 1000);
  if (ageSec < 60) return 'just now';
  if (ageSec < 3600) return `${Math.floor(ageSec / 60)}m ago`;
  if (ageSec < 86400) return `${Math.floor(ageSec / 3600)}h ago`;
  return `${Math.floor(ageSec / 86400)}d ago`;
}

/** Elapsed since the session started ("3h 5m"), or '' before it started. */
export function rowElapsed(sess: SessionRow, nowSec: number): string {
  return sess.started_at !== null ? formatElapsed(sessionStart(sess), nowSec) : '';
}

/** First line of the last prompt, truncated for the row; '' when none. */
export function rowPrompt(sess: SessionRow): string {
  return promptPreview(sess.last_prompt, 48);
}
