// Session event timeline (Q9): IPC wrapper plus the pure helpers the
// Timeline component renders with. The rows come from the append-only
// `session_events` table (migration 013) via the `session_history` command.
//
// `moveOrigin` and `unresolvedPartial` are also pure over that same
// `SessionEvent[]`: they are what makes "Move back to {host}" and the
// finish/undo of a partial move available after the app restarts and the
// in-memory move-run store is gone. The durable record lives on the
// session's own timeline (the `session_moved` / `session_move_partial` /
// `session_move_undone` events), not in memory, so reading it back needs no
// new IPC — hence pure helpers living beside the other timeline helpers.

import { invokeCmd, type Result } from './result';

/** Mirrors `store::SessionEvent`. */
export interface SessionEvent {
  id: number;
  session_id: number;
  /** unix seconds */
  at: number;
  kind: string;
  detail: string | null;
  /** The conversation the event belongs to (migration 037). */
  claude_session_id: string | null;
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

const TURN_KINDS = new Set([
  'conversation_started',
  'conversation_ended',
  'compact_started',
  'compact_done',
  'turn_done',
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
  if (TURN_KINDS.has(k)) return 'turns';
  // `keys_sent` (a key press via send_prompt { keys }) is the same kind of
  // "we told the session something" event as `prompt_sent`, just without a
  // typed body — same chip.
  if (k.startsWith('prompt') || k === 'keys_sent') return 'prompts';
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

/** Where a session came from, for a "Move back to {host}" offer. */
export interface MoveOrigin {
  fromHost: string;
  claudeSessionId: string | null;
}

/** A partial move (both sessions still alive) that nothing has resolved yet. */
export interface UnresolvedPartial {
  targetSessionId: number;
  sourceSessionId: number | null;
  fromHost: string;
  toHost: string;
  step: string | null;
  /** What the transfer was told to do with the source (`keep_source`), or
   *  `null` when the record does not say — an event written before the
   *  backend carried the fact. A run rebuilt from this decides whether a
   *  later retry kills the source, so "unknown" must not read as `false`. */
  keptSource: boolean | null;
}

const MOVE_KINDS = new Set(['session_moved', 'session_move_partial', 'session_move_undone']);

/** Parses `detail` defensively: `null`, malformed JSON, or a non-object
 *  value all mean "no usable record" rather than a thrown error. */
function detailOf(e: SessionEvent): Record<string, unknown> | null {
  if (e.detail === null) return null;
  try {
    const v: unknown = JSON.parse(e.detail);
    return typeof v === 'object' && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** The most recent host a session was moved from, from its own
 *  `session_moved` events (newest first, per `sessionHistory`). `null` when
 *  the session was never moved or the record is unusable. */
export function moveOrigin(events: SessionEvent[]): MoveOrigin | null {
  for (const e of events) {
    if (e.kind !== 'session_moved') continue;
    const d = detailOf(e);
    if (!d) continue;
    const fromHost = d.from_host;
    if (typeof fromHost !== 'string') continue;
    const claudeSessionId = typeof d.claude_session_id === 'string' ? d.claude_session_id : null;
    return { fromHost, claudeSessionId };
  }
  return null;
}

/** The most recent partial move nothing has resolved yet: scans newest-first
 *  over the move-related kinds only, and stops at the first one that
 *  decides — a `session_moved`/`session_move_undone` means resolved
 *  (`null`), a `session_move_partial` is the answer, provided it names a
 *  target to act on. */
export function unresolvedPartial(events: SessionEvent[]): UnresolvedPartial | null {
  for (const e of events) {
    if (!MOVE_KINDS.has(e.kind)) continue;
    if (e.kind === 'session_moved' || e.kind === 'session_move_undone') return null;
    // e.kind === 'session_move_partial'
    const d = detailOf(e);
    if (!d) return null;
    const targetSessionId = d.to_session_id;
    if (typeof targetSessionId !== 'number') return null;
    const fromHost = typeof d.from_host === 'string' ? d.from_host : '';
    const toHost = typeof d.to_host === 'string' ? d.to_host : '';
    const sourceSessionId = typeof d.from_session_id === 'number' ? d.from_session_id : null;
    const step = typeof d.step === 'string' ? d.step : null;
    const keptSource = typeof d.kept_source === 'boolean' ? d.kept_source : null;
    return { targetSessionId, sourceSessionId, fromHost, toHost, step, keptSource };
  }
  return null;
}
