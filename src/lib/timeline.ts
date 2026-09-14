// Session event timeline (Q9): IPC wrapper plus the pure helpers the
// Timeline component renders with. The rows come from the append-only
// `session_events` table (migration 013) via the `session_history` command.

import { invokeCmd, type Result } from './result';

/** Mirrors `store::SessionEvent`. */
export interface SessionEvent {
  id: number;
  session_id: number;
  /** unix seconds */
  at: number;
  kind: string;
  detail: string | null;
}

/** Newest-first timeline for one session. */
export function sessionHistory(sessionId: number, limit?: number): Promise<Result<SessionEvent[]>> {
  return invokeCmd<SessionEvent[]>('session_history', {
    args: { session_id: sessionId, limit: limit ?? null },
  });
}

/** Filter-chip categories. `other` catches kinds added later. */
export type EventCategory = 'turns' | 'prompts' | 'errors' | 'ops' | 'other';

export const FILTER_CATEGORIES: { id: Exclude<EventCategory, 'other'>; label: string }[] = [
  { id: 'turns', label: 'Turns' },
  { id: 'prompts', label: 'Prompts' },
  { id: 'errors', label: 'Errors' },
  { id: 'ops', label: 'Ops' },
];

const OPS_KINDS = new Set([
  'killed',
  'recreated',
  'gc_killed',
  'playbook_applied',
  'mcp_call',
  'message_sent',
]);

/** Which chip an event kind belongs to. `status_change` into `failed` or
 *  `blocked` counts as an error; any other status change is a turn edge. */
export function eventCategory(e: Pick<SessionEvent, 'kind' | 'detail'>): EventCategory {
  const k = e.kind;
  if (k === 'stuck' || k.endsWith('_failed') || k.includes('error')) return 'errors';
  if (k === 'status_change') {
    const d = (e.detail ?? '').toLowerCase();
    return /\b(failed|blocked)\b/.test(d) ? 'errors' : 'turns';
  }
  if (k.startsWith('prompt')) return 'prompts';
  if (
    OPS_KINDS.has(k) ||
    k.startsWith('repair') ||
    k.startsWith('workspace') ||
    k.startsWith('safe_kill') ||
    k.startsWith('task_')
  ) {
    return 'ops';
  }
  return 'other';
}

/** Human label for a kind: `safe_kill_requested` → `safe kill requested`. */
export function kindLabel(kind: string): string {
  return kind.replace(/_/g, ' ');
}

/** One-line detail, trimmed to `max` characters. */
export function shortDetail(detail: string | null, max = 120): string {
  if (!detail) return '';
  const one = detail.replace(/\s+/g, ' ').trim();
  return one.length > max ? one.slice(0, max - 1) + '…' : one;
}

/** Clock time for today, date + time otherwise. */
export function eventTime(at: number, now: Date = new Date()): string {
  const d = new Date(at * 1000);
  const hm = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  if (d.toDateString() === now.toDateString()) return hm;
  return `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })} ${hm}`;
}

/** Events whose category is in `active`; an empty set means "all". */
export function filterEvents(
  events: SessionEvent[],
  active: ReadonlySet<EventCategory>,
): SessionEvent[] {
  if (active.size === 0) return events;
  return events.filter((e) => active.has(eventCategory(e)));
}
