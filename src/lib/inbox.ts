// The Inbox (Orbit Fleet redesign step 3.3): only the sessions that raise
// the Needs you badge, worst first, and one quiet line counting the rest.
//
// "Needs you" is the attention model's answer (step 0.4, `attention.ts`):
// Action required, Failed and Blocked. The badge, the rail's count, the
// Inbox and Today's "Needs you" (`service/work/today.rs`) all ask that one
// table, so they list the same sessions. A lost or ghost row (Paused) and a
// finished turn (Done) are not in it; the line below the list counts them.
//
// Gap plan G1.6 adds the model's two other classes: a mission waiting on a
// person (`mission_waits.ts`), listed and counted beside the sessions, and
// Jev's "probably waiting" (state Proposed), listed apart as "+N proposed"
// and never counted by the badge.
//
// Gap plan G3.1 (the Main board's Inbox): "Group: state" splits the queue
// into the model's own sections (`row_groups.ts::groupRows`, the Sessions
// list's state grouping), the footer says what finished today, and a mass
// loss shows as the Sessions list's "12 stopped on trn · Restore" line.
import { derived, writable } from 'svelte/store';
import {
  attentionState,
  byTriage,
  countNeedsYou,
  countsTowardBadge,
  type AttentionOptions,
} from './attention';
import { effectiveHostFilter } from './hosts';
import { attentionIdleMinutes } from './notify';
import { effectiveScope, scopeOf } from './orgs';
import { sessions, showBgAgents, type SessionRow } from './sessions';
import { sessionVisible } from './sidebar_index';
import { attentionFacts } from './attention_facts';
import { failingCount } from './routines';
import { waitingMissionCount } from './mission_waits';
import { groupRows } from './row_groups';
import { localMidnight } from './today';
import { readPref, writePref } from './prefs';

/** The rows that need you, worst first (`byTriage`). */
export function inboxRows(rows: readonly SessionRow[], opts: AttentionOptions): SessionRow[] {
  return byTriage(
    rows.filter((s) => countsTowardBadge(attentionState(s, opts))),
    opts,
  );
}

/** One Jev reading: the session and the turn it read. A later turn is a
 *  new reading, so "Not waiting" on an earlier one does not hide it. */
export function proposalKey(s: Pick<SessionRow, 'id' | 'last_stop_at'>): string {
  return `${s.id}:${s.last_stop_at ?? 0}`;
}

/** The readings the person set aside with "Not waiting" on this device.
 *  The next hook clears Jev's reading on the hub anyway. */
export const notWaitingSaid = writable<ReadonlySet<string>>(new Set());

export function sayNotWaiting(s: Pick<SessionRow, 'id' | 'last_stop_at'>): void {
  notWaitingSaid.update((set) => new Set([...set, proposalKey(s)]));
}

/** Jev's "probably waiting" rows (G1.6): shown apart from Needs you, the
 *  longest waiting first, never in the badge; one set aside is left out. */
export function proposedRows(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  setAside: ReadonlySet<string> = new Set(),
): SessionRow[] {
  return byTriage(
    rows.filter((s) => attentionState(s, opts) === 'proposed' && !setAside.has(proposalKey(s))),
    opts,
  );
}

/** "+1 proposed": the header's note for Jev's rows, or '' with none. */
export function proposedText(n: number): string {
  return n > 0 ? `+${n} proposed` : '';
}

/** Everything the Inbox leaves out, by state. `completedToday` is a
 *  finished turn (Done, or Idle after it) that ended since local midnight
 *  (`since`); `idle` and `done` hold the older ones. */
export interface NotWaiting {
  working: number;
  idle: number;
  completedToday: number;
  done: number;
  paused: number;
}

export function notWaiting(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  since: number = localMidnight(opts.now * 1000),
): NotWaiting {
  const n: NotWaiting = { working: 0, idle: 0, completedToday: 0, done: 0, paused: 0 };
  for (const s of rows) {
    const st = attentionState(s, opts);
    const today = s.kind !== 'shell' && s.kind !== 'external' && s.last_stop_at != null && s.last_stop_at >= since;
    if (st === 'working') n.working++;
    else if ((st === 'idle' || st === 'done') && today) n.completedToday++;
    else if (st === 'idle') n.idle++;
    else if (st === 'done') n.done++;
    else if (st === 'paused') n.paused++;
  }
  return n;
}

/** "6 running · 9 idle · 6 completed today": the parts that are not zero. */
export function notWaitingText(n: NotWaiting): string {
  const parts: string[] = [];
  if (n.working) parts.push(`${n.working} running`);
  if (n.idle) parts.push(`${n.idle} idle`);
  if (n.completedToday) parts.push(`${n.completedToday} completed today`);
  if (n.done) parts.push(`${n.done} done`);
  if (n.paused) parts.push(`${n.paused} paused`);
  return parts.join(' · ');
}

// ── state sections (G3.1) ──

/** How the Inbox lists its rows: by state (the board's "Group: state") or
 *  as one queue, worst first. */
export type InboxGroupBy = 'state' | 'none';
const isInboxGroupBy = (v: unknown): v is InboxGroupBy => v === 'state' || v === 'none';
export const inboxGroupBy = writable<InboxGroupBy>(readPref('inbox.group', 'state', isInboxGroupBy));
inboxGroupBy.subscribe((v) => writePref('inbox.group', v));

/** One Inbox section. The sessions come from the attention model; the
 *  section also says which of the model's other rows it carries: missions
 *  waiting on a person and Jev's proposals sit in Needs you, routines whose
 *  newest run failed in Failed. */
export interface InboxSection {
  key: 'needs_you' | 'failed' | 'all';
  /** The header ("Needs you"); null for the one-queue list, which has none. */
  label: string | null;
  rows: SessionRow[];
  /** The header's count: its sessions plus the missions or routines it holds.
   *  Jev's proposals are never counted ("+1 proposed" beside it instead). */
  count: number;
  missions: boolean;
  routines: boolean;
  proposed: boolean;
}

export interface InboxExtras {
  missions: number;
  failingRoutines: number;
  proposed: number;
}

/**
 * The Inbox's sections. By state: Needs you (Action required and Blocked,
 * the Sessions list's one status word for both), then Failed, each left
 * out when it holds nothing. As one queue: every row worst first. Either
 * way the rows are `inboxRows`, so the sections add up to the badge.
 */
export function inboxSections(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  by: InboxGroupBy,
  extra: InboxExtras,
): InboxSection[] {
  const queue = inboxRows(rows, opts);
  if (by === 'none') {
    const count = queue.length + extra.missions + extra.failingRoutines;
    if (count === 0 && extra.proposed === 0) return [];
    return [{ key: 'all', label: null, rows: queue, count, missions: true, routines: true, proposed: true }];
  }
  const groups = groupRows(queue, 'state', opts, new Set());
  const of = (state: string) => groups.find((g) => g.key === `state:${state}`)?.rows ?? [];
  const out: InboxSection[] = [];
  const needs = of('action_required');
  if (needs.length + extra.missions + extra.proposed > 0) {
    out.push({
      key: 'needs_you',
      label: 'Needs you',
      rows: needs,
      count: needs.length + extra.missions,
      missions: true,
      routines: false,
      proposed: true,
    });
  }
  const failed = of('failed');
  if (failed.length + extra.failingRoutines > 0) {
    out.push({
      key: 'failed',
      label: 'Failed',
      rows: failed,
      count: failed.length + extra.failingRoutines,
      missions: false,
      routines: true,
      proposed: false,
    });
  }
  return out;
}

/** The rail's Inbox count: the Needs you pill's number, under the same host,
 *  background-agent and organisation filters, plus the routines whose
 *  newest run failed (redesign 8.6: a failed run raises the badge) and the
 *  missions waiting on a person (G1.6). Jev's proposed rows never count. */
export const inboxCount = derived(
  [
    sessions,
    effectiveHostFilter,
    showBgAgents,
    effectiveScope,
    scopeOf,
    attentionIdleMinutes,
    failingCount,
    waitingMissionCount,
  ],
  ([$sessions, $host, $bg, $scope, $of, $idle, $failing, $missions]) => {
    const scope = $scope === 'all' ? null : { id: $scope, of: $of };
    const visible = $sessions.filter((s) => sessionVisible(s, $host, $bg, null, scope));
    return countNeedsYou(visible, { idleSecs: $idle * 60, now: Math.floor(Date.now() / 1000) }) + $failing + $missions;
  },
);

/** The Inbox's rows as a store, under the rail count's filters and with the
 *  attention facts (step 2.4), for code that walks the queue rather than
 *  drawing it: the Conversation's "moved to next" after an answer (5.9). */
export const inboxQueue = derived(
  [sessions, effectiveHostFilter, showBgAgents, effectiveScope, scopeOf, attentionIdleMinutes, attentionFacts],
  ([$sessions, $host, $bg, $scope, $of, $idle, $facts]) => {
    const scope = $scope === 'all' ? null : { id: $scope, of: $of };
    const visible = $sessions.filter((s) => sessionVisible(s, $host, $bg, null, scope));
    return inboxRows(visible, { idleSecs: $idle * 60, now: Math.floor(Date.now() / 1000), facts: $facts });
  },
);

/**
 * The session to move to after answering `currentId` (redesign 5.9): the
 * next one in the Inbox's order, wrapping round, never `currentId` itself
 * (its row says it is waiting until the next tick reads the pane). `null`
 * when nothing else needs you.
 */
export function nextInInbox(queue: readonly SessionRow[], currentId: number): SessionRow | null {
  const others = queue.filter((s) => s.id !== currentId);
  if (others.length === 0) return null;
  const at = queue.findIndex((s) => s.id === currentId);
  if (at === -1) return others[0];
  return queue.slice(at + 1).find((s) => s.id !== currentId) ?? others[0];
}
