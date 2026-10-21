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
import { isFinal, WAIT_WORDS, type Mission } from './missions';
import { missionRowLine } from './mission_room';
import type { FollowMark } from './control_journal';
import type { SessionRow } from './sessions';
import type { OfState } from './kit/status';
import { localMidnight } from './today';

export type ThreadGroup = 'needs' | 'working' | 'paused' | 'done' | 'earlier';

export const THREAD_GROUPS: readonly { id: ThreadGroup; label: string }[] = [
  { id: 'needs', label: 'Needs you' },
  { id: 'working', label: 'Working' },
  { id: 'paused', label: 'Draft and paused' },
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
  /** Muted: fleet stops telling Control about it (Control review F10). */
  muted: boolean;
  /** Why Control no longer follows it, in words ("went quiet"), or null
   *  while it does (or the hub did not say). Muted is `muted`'s. */
  unfollowed: string | null;
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
  /** The muted threads' keys (`s:<id>`, `m:<id>`, `control_journal`). */
  muted?: ReadonlySet<string>;
  /** What the hub follows (`threadFollows`, the journal's `follows`): every
   *  thread Control started or was handed, from up to 500 receipts — the
   *  column's list (review w04-1), where `handoffs` (the chips' newest 20)
   *  only adds receipt times and names. `null` from an older hub. */
  follows?: ReadonlyMap<string, FollowMark> | null;
}

/** Why a thread is no longer followed, by the journal's state. */
const UNFOLLOWED_WORDS: Record<string, string> = {
  quiet: 'went quiet',
  done: 'done',
  merged: 'PR merged',
  killed: 'ended',
};

function unfollowedWhy(mark: FollowMark | undefined): string | null {
  if (!mark || mark.muted || mark.state === 'following') return null;
  return UNFOLLOWED_WORDS[mark.state] ?? mark.state;
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
export { WAIT_WORDS };

/** A mission's group, as the Mission room groups it (`roomGroupOf`), with
 *  today for its week: one that ended goes by when it did — a failure today
 *  needs you, a cancel never does, and any of them older is Earlier
 *  (review w04-2); a draft or a paused one is Draft and paused (w04-9). */
function missionGroup(state: string, waiting: boolean, finishedAt: number | null, since: number): ThreadGroup {
  if (isFinal(state)) {
    const recent = (finishedAt ?? 0) >= since;
    if (!recent) return 'earlier';
    return state === 'failed' ? 'needs' : 'done';
  }
  if (waiting) return 'needs';
  if (workState(state) === 'working') return 'working';
  return 'paused';
}

/** A mission's dot: cancelled, draft and paused are idle, not failed. */
function missionDot(state: string, waiting: boolean): OfState {
  if (waiting && !isFinal(state)) return 'waiting';
  if (state === 'cancelled' || state === 'draft' || state === 'paused') return 'idle';
  return workState(state);
}

/** A mission's second line: what it waits on, its progress while it works,
 *  else the Mission room's row line ("Draft · not planned yet", "Paused",
 *  "Finished · 6 tasks"). */
function missionLine(m: Mission | undefined, state: string): string {
  if (m?.waiting_on && !isFinal(state)) return WAIT_WORDS[m.waiting_on.reason] ?? 'Waits on you';
  const total = m?.total ?? 0;
  if (state === 'active' && total > 0) return `Mission · ${m?.done ?? 0} of ${total} done`;
  if (m) return missionRowLine(m);
  if (state === 'draft') return 'Draft · not planned yet';
  if (state === 'paused') return 'Paused';
  return 'Mission';
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
  // The hub's follows: every thread it knows, past the newest receipts.
  const missionIds = new Set<number>(missionReceipts.keys());
  for (const key of input.follows?.keys() ?? []) {
    const id = Number(key.slice(2));
    if (!Number.isFinite(id)) continue;
    if (key.startsWith('s:') && id !== input.operatorId && byId.has(id)) sessionIds.add(id);
    else if (key.startsWith('m:')) missionIds.add(id);
  }
  const out: ControlThread[] = [];
  const muted = input.muted ?? new Set<string>();
  const missionById = new Map(input.missions.map((m) => [m.id, m]));
  for (const id of sessionIds) {
    const row = byId.get(id)!;
    // A mission's worker belongs to the mission's thread, not its own.
    if (row.origin === 'mission' && row.origin_ref && missionIds.has(Number(row.origin_ref))) continue;
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
      muted: muted.has(`s:${id}`) || !!input.follows?.get(`s:${id}`)?.muted,
      unfollowed: unfollowedWhy(input.follows?.get(`s:${id}`)),
    });
  }
  for (const id of missionIds) {
    const h = missionReceipts.get(id);
    const m = missionById.get(id);
    const mark = input.follows?.get(`m:${id}`);
    const state = m?.state ?? h?.mission_state ?? 'draft';
    const waiting = !!m?.waiting_on && !isFinal(state);
    out.push({
      kind: 'mission',
      key: `m:${id}`,
      id,
      group: missionGroup(state, waiting, m ? (m.finished_at ?? m.updated_at) : null, since),
      state: missionDot(state, waiting),
      title: m?.name ?? h?.mission_name ?? mark?.name ?? `Mission ${id}`,
      line: missionLine(m, state),
      at: Math.max(m?.updated_at ?? 0, h?.at ?? 0),
      muted: muted.has(`m:${id}`) || !!mark?.muted,
      // A finished mission is not followed because it finished: no news.
      unfollowed: isFinal(state) ? null : unfollowedWhy(mark),
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
