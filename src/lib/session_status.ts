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

export function timeAgo(unixSecs: number): string {
  const diffMs = Date.now() - unixSecs * 1000;
  const diffMins = Math.floor(diffMs / 60_000);
  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}

/** Secondary row text: elapsed since start + the last prompt's first line. */
export function rowMeta(sess: SessionRow, nowSec: number): string {
  const parts: string[] = [];
  if (sess.started_at !== null) parts.push(formatElapsed(sessionStart(sess), nowSec));
  const preview = promptPreview(sess.last_prompt, 48);
  if (preview) parts.push(preview);
  return parts.join(' · ');
}
