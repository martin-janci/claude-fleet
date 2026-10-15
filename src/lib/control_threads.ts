// Control as a Thread room (direction A, 2026-10-14; parity map
// control-chat-first/2026-10-14/control-parity.md): the left column inside
// Control lists Main chat, then the threads Control handed out — the
// sessions its agent started (`origin = 'operator'`), the sessions and the
// missions its handoff receipts name (`control_handoffs`) — grouped Needs
// you / Working / Done today, with filter chips All · Needs you · Missions.
//
// Pure: the column reads the live rows, the receipts and the missions list
// and asks this module how to group and word them. Nothing here is inferred
// from the transcript's text.
import { attentionState, claudeStatusLabel, countsTowardBadge, displayName, type AttentionOptions } from './attention';
import { sessionState, workState, type ControlHandoff } from './handoffs';
import type { Mission } from './missions';
import type { SessionRow } from './sessions';
import type { OfState } from './kit/status';
import { localMidnight } from './today';

export type ThreadGroup = 'needs' | 'working' | 'done' | 'earlier';

export const THREAD_GROUPS: readonly { id: ThreadGroup; label: string }[] = [
  { id: 'needs', label: 'Needs you' },
  { id: 'working', label: 'Working' },
  { id: 'done', label: 'Done today' },
  { id: 'earlier', label: 'Earlier' },
];

interface ThreadBase {
  /** `s:<id>` or `m:<id>`, unique across the list. */
  key: string;
  id: number;
  group: ThreadGroup;
  state: OfState;
  title: string;
  /** The second line: what it is doing or waiting on. */
  line: string;
  /** Its last change, unix seconds: the row's age and the sort. */
  at: number;
}

export interface SessionThread extends ThreadBase {
  kind: 'session';
  row: SessionRow;
}

export interface MissionThread extends ThreadBase {
  kind: 'mission';
}

export type ControlThread = SessionThread | MissionThread;

export type ThreadFilter = 'all' | 'needs' | 'missions';

export interface ThreadInput {
  rows: readonly SessionRow[];
  handoffs: readonly ControlHandoff[];
  /** The live missions (`work_missions`), for state and waits; a receipt's
   *  own name and state stand in for a mission the list does not have. */
  missions: readonly Mission[];
  /** Control's own session, never a thread of itself. */
  operatorId: number | null;
  opts: AttentionOptions;
  /** Local midnight, unix seconds: what "Done today" means. */
  since?: number;
}

function sessionGroup(s: SessionRow, opts: AttentionOptions, since: number): ThreadGroup {
  const st = attentionState(s, opts);
  if (countsTowardBadge(st)) return 'needs';
  if (st === 'working') return 'working';
  if ((s.last_stop_at ?? 0) >= since || (s.claude_status === 'completed' && (s.last_activity_at ?? 0) >= since)) return 'done';
  return 'earlier';
}

function sessionLine(s: SessionRow): string {
  const status = claudeStatusLabel(s.claude_status);
  return [status, s.host_alias].filter(Boolean).join(' · ');
}

/** Why a mission waits on a person, in words (`MissionWait.reason`). */
export const WAIT_WORDS: Record<string, string> = {
  question: 'Asks a question',
  sign_grant: 'Grant needs signing',
  confirm: 'A step waits for your OK',
};

function missionGroup(state: string, waiting: boolean, finishedAt: number | null, since: number): ThreadGroup {
  if (waiting) return 'needs';
  const w = workState(state);
  if (w === 'working') return 'working';
  if (w === 'failed') return 'needs';
  if ((finishedAt ?? 0) >= since) return 'done';
  return 'earlier';
}

/** Every thread Control handed out, newest first within its group. */
export function buildThreads(input: ThreadInput): ControlThread[] {
  const since = input.since ?? localMidnight(input.opts.now * 1000);
  const byId = new Map(input.rows.map((s) => [s.id, s]));
  const sessionIds = new Set<number>();
  for (const s of input.rows) if (s.origin === 'operator' && s.id !== input.operatorId) sessionIds.add(s.id);
  const handedAt = new Map<number, number>();
  const missionReceipts = new Map<number, ControlHandoff>();
  for (const h of input.handoffs) {
    if (h.session_id != null && h.session_id !== input.operatorId && (h.kind === 'session' || h.kind === 'task')) {
      if (byId.has(h.session_id)) {
        sessionIds.add(h.session_id);
        handedAt.set(h.session_id, Math.max(handedAt.get(h.session_id) ?? 0, h.at));
      }
    }
    if (h.mission_id != null) {
      const prev = missionReceipts.get(h.mission_id);
      if (!prev || h.at > prev.at) missionReceipts.set(h.mission_id, h);
    }
  }
  const out: ControlThread[] = [];
  const missionById = new Map(input.missions.map((m) => [m.id, m]));
  for (const id of sessionIds) {
    const row = byId.get(id)!;
    // A mission's worker belongs to the mission's thread, not its own.
    if (row.origin === 'mission' && row.origin_ref && missionReceipts.has(Number(row.origin_ref))) continue;
    out.push({
      kind: 'session',
      key: `s:${id}`,
      id,
      row,
      group: sessionGroup(row, input.opts, since),
      state: sessionState(row),
      title: displayName(row, true),
      line: sessionLine(row),
      at: Math.max(row.last_activity_at ?? 0, handedAt.get(id) ?? 0, row.created_at ?? 0),
    });
  }
  for (const [id, h] of missionReceipts) {
    const m = missionById.get(id);
    const state = m?.state ?? h.mission_state ?? 'draft';
    const waiting = !!m?.waiting_on;
    const done = m?.done ?? 0;
    const total = m?.total ?? 0;
    const line = waiting
      ? (WAIT_WORDS[m!.waiting_on!.reason] ?? 'Waits on you')
      : total > 0
        ? `Mission · ${done} of ${total} done`
        : 'Mission';
    out.push({
      kind: 'mission',
      key: `m:${id}`,
      id,
      group: missionGroup(state, waiting, m?.finished_at ?? null, since),
      state: waiting ? 'waiting' : workState(state),
      title: m?.name ?? h.mission_name ?? `Mission ${id}`,
      line,
      at: Math.max(m?.updated_at ?? 0, h.at),
    });
  }
  out.sort((a, b) => b.at - a.at);
  return out;
}

/** The threads a chip leaves. */
export function filterThreads(list: readonly ControlThread[], f: ThreadFilter): ControlThread[] {
  if (f === 'needs') return list.filter((t) => t.group === 'needs');
  if (f === 'missions') return list.filter((t) => t.kind === 'mission');
  return [...list];
}

/** The chips' counts. */
export function threadCounts(list: readonly ControlThread[]): Record<ThreadFilter, number> {
  return {
    all: list.length,
    needs: list.filter((t) => t.group === 'needs').length,
    missions: list.filter((t) => t.kind === 'mission').length,
  };
}

/** The list by group, in the column's order, empty groups left out. */
export function groupThreads(list: readonly ControlThread[]): { id: ThreadGroup; label: string; threads: ControlThread[] }[] {
  return THREAD_GROUPS.map((g) => ({ ...g, threads: list.filter((t) => t.group === g.id) })).filter((g) => g.threads.length > 0);
}

/**
 * The sessions beside `row` in its thread, itself included, for the Session
 * tab's strip: a mission's workers (`origin = 'mission'`, the same mission),
 * else the sessions on the same work item, else the sessions the same
 * parent started. Empty when it stands alone.
 */
export function threadSiblings(row: SessionRow, rows: readonly SessionRow[]): SessionRow[] {
  const live = rows.filter((s) => s.status !== 'ghost');
  let group: SessionRow[] = [];
  if (row.origin === 'mission' && row.origin_ref) {
    group = live.filter((s) => s.origin === 'mission' && s.origin_ref === row.origin_ref);
  } else if (row.work?.item_id != null) {
    group = live.filter((s) => s.work?.item_id === row.work!.item_id);
  } else if (row.origin_ref && row.origin !== 'person') {
    group = live.filter((s) => s.origin === row.origin && s.origin_ref === row.origin_ref);
  }
  if (!group.some((s) => s.id === row.id)) group = [row, ...group];
  return group.length > 1 ? group : [];
}

/** What the strip above the Session tab calls the thread. */
export function siblingsLabel(row: SessionRow, missionName: (id: number) => string | null): string {
  if (row.origin === 'mission' && row.origin_ref) {
    const name = missionName(Number(row.origin_ref));
    return name ? `In this mission · ${name}` : 'In this mission';
  }
  if (row.work?.key) return `On ${row.work.key}`;
  return 'In this thread';
}
