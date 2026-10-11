// Control's Views panel (Orbit Fleet redesign step 9.4, boards MissionControl
// and MCViews): the column beside the chat. Its views come from this chat's
// fleet (Needs you, the session in focus, Pull requests, Library, Today
// briefing); "+" toggles and reorders them. What lives in another place
// (Tasks and Missions in Work, Routines in Automation, Hosts and usage in
// Accounts) is a link there, not a copy. The layout is a local pref.
//
// Gap plan G3.10 (boards MissionControl, MCViews, MCTasks): Needs you
// gains a fleet line, a search and Running / Idle / Done today folds; the
// strip gains Tasks (☑, the Work tree's tasks by status, with bulk Start
// new, Assign and Done) and Routines (◷); Open elsewhere gains Routines in
// Automation.
import { derived, writable } from 'svelte/store';
import { inboxQueue, inboxRows } from './inbox';
import { attentionState, type AttentionOptions } from './attention';
import { attentionFacts } from './attention_facts';
import { effectiveHostFilter } from './hosts';
import { attentionIdleMinutes } from './notify';
import { effectiveScope, scopeOf } from './orgs';
import { sessions, showBgAgents, type SessionRow } from './sessions';
import { sessionVisible } from './sidebar_index';
import { localMidnight } from './today';
import { openRoutines } from './routines';
import type { WorkTask } from './work_view';
import { readPref, writePref } from './prefs';
import { goTo, leave } from './destination';
import { sidebarView } from './work_view';
import { sessionMatchesSearch } from './search';

export type ControlViewId = 'needs-you' | 'session' | 'tasks' | 'routines' | 'prs' | 'library' | 'today';

export interface ControlViewDef {
  id: ControlViewId;
  label: string;
  /** The strip's one-character glyph, from the board. */
  glyph: string;
  /** The plan step that brings it; hidden until `landed`. */
  step: string;
  landed: boolean;
}

export const CONTROL_VIEWS: readonly ControlViewDef[] = [
  { id: 'needs-you', label: 'Needs you', glyph: '▤', step: '9.4', landed: true },
  { id: 'session', label: 'Session in focus', glyph: '▢', step: '9.4', landed: true },
  { id: 'tasks', label: 'Tasks', glyph: '☑', step: 'G3.10', landed: true },
  { id: 'routines', label: 'Routines', glyph: '◷', step: 'G3.10', landed: true },
  { id: 'prs', label: 'Pull requests', glyph: '⑂', step: '9.4', landed: true },
  { id: 'library', label: 'Library', glyph: '◧', step: '9.7', landed: true },
  { id: 'today', label: 'Today briefing', glyph: '☀', step: '9.4', landed: true },
];

export type ElsewhereId = 'tasks' | 'missions' | 'routines' | 'hosts';

export interface ElsewhereLink {
  id: ElsewhereId;
  label: string;
}

/** "Not views here, open them where they live." */
export const ELSEWHERE: readonly ElsewhereLink[] = [
  { id: 'tasks', label: 'Tasks in Work' },
  { id: 'missions', label: 'Missions in Work' },
  { id: 'routines', label: 'Routines in Automation' },
  { id: 'hosts', label: 'Hosts and usage in Accounts' },
];

/** Leave Control for the place a link names. */
export function openElsewhere(id: ElsewhereId): void {
  if (id === 'hosts') {
    goTo('accounts');
    return;
  }
  if (id === 'routines') {
    openRoutines();
    return;
  }
  sidebarView.set('work');
  leave('control');
}

export interface ControlViewsLayout {
  /** Every landed view, in the strip's order. */
  order: ControlViewId[];
  /** Views turned off with "+". */
  hidden: ControlViewId[];
  /** The view on show. */
  active: ControlViewId;
  /** ✕ closes the column; the header's Views button opens it again. */
  open: boolean;
}

const landedIds = (): ControlViewId[] => CONTROL_VIEWS.filter((v) => v.landed).map((v) => v.id);

export function defaultLayout(): ControlViewsLayout {
  return { order: landedIds(), hidden: [], active: 'needs-you', open: true };
}

const isViewId = (v: unknown): v is ControlViewId => CONTROL_VIEWS.some((d) => d.id === v);

/** A stored layout made whole: unknown ids dropped, a view landed since it
 *  was stored appended, an active view that is hidden or gone replaced. */
export function normalizeLayout(raw: unknown): ControlViewsLayout {
  const d = defaultLayout();
  if (!raw || typeof raw !== 'object') return d;
  const r = raw as Partial<Record<keyof ControlViewsLayout, unknown>>;
  const landed = new Set(landedIds());
  const order = (Array.isArray(r.order) ? r.order : []).filter((v): v is ControlViewId => isViewId(v) && landed.has(v));
  const uniq = [...new Set(order)];
  for (const id of landedIds()) if (!uniq.includes(id)) uniq.push(id);
  const hidden = [...new Set((Array.isArray(r.hidden) ? r.hidden : []).filter((v): v is ControlViewId => isViewId(v) && landed.has(v)))];
  const shown = uniq.filter((v) => !hidden.includes(v));
  const active = isViewId(r.active) && shown.includes(r.active) ? r.active : (shown[0] ?? d.active);
  return { order: uniq, hidden, active, open: typeof r.open === 'boolean' ? r.open : true };
}

const isAnyObject = (v: unknown): v is object => v !== null && typeof v === 'object';

export const controlViews = writable<ControlViewsLayout>(normalizeLayout(readPref<object | null>('ui.controlViews', null, isAnyObject)));
controlViews.subscribe((v) => writePref('ui.controlViews', v));

/** The views the strip shows, in order. */
export function shownViews(l: ControlViewsLayout): ControlViewDef[] {
  return l.order.filter((id) => !l.hidden.includes(id)).map((id) => CONTROL_VIEWS.find((v) => v.id === id)!);
}

export function selectView(id: ControlViewId): void {
  controlViews.update((l) => ({ ...l, active: id, open: true, hidden: l.hidden.filter((h) => h !== id) }));
}

/** "+": a view on or off. The last shown view stays on. */
export function toggleView(id: ControlViewId): void {
  controlViews.update((l) => {
    const hiding = !l.hidden.includes(id);
    if (hiding && shownViews(l).length <= 1) return l;
    return normalizeLayout({ ...l, hidden: hiding ? [...l.hidden, id] : l.hidden.filter((h) => h !== id) });
  });
}

/** "+": move a view one place earlier (-1) or later (1) in the strip. */
export function moveView(id: ControlViewId, by: -1 | 1): void {
  controlViews.update((l) => {
    const i = l.order.indexOf(id);
    const j = i + by;
    if (i < 0 || j < 0 || j >= l.order.length) return l;
    const order = [...l.order];
    [order[i], order[j]] = [order[j], order[i]];
    return { ...l, order };
  });
}

export function setViewsOpen(open: boolean): void {
  controlViews.update((l) => ({ ...l, open }));
}

/** Needs you: the Inbox's own rows (`inboxQueue`), so the panel, the Inbox
 *  and the rail's badge ask one attention query. */
export const needsYouList = inboxQueue;

// ── Needs you folds and search (G3.10, board MissionControl) ──

/** The fleet as the Inbox sees it: the rail count's filters, with the
 *  attention options the model reads. */
export const visibleFleet = derived(
  [sessions, effectiveHostFilter, showBgAgents, effectiveScope, scopeOf, attentionIdleMinutes, attentionFacts],
  ([$sessions, $host, $bg, $scope, $of, $idle, $facts]) => {
    const scope = $scope === 'all' ? null : { id: $scope, of: $of };
    const rows = $sessions.filter((s) => sessionVisible(s, $host, $bg, null, scope));
    const opts: AttentionOptions = { idleSecs: $idle * 60, now: Math.floor(Date.now() / 1000), facts: $facts };
    return { rows, opts };
  },
);

/** Does a row match the panel's search: the Sessions list's rule
 *  (`search.ts`), every word in its names, host, branch, tags, last prompt
 *  or task. An empty query matches everything. */
export function matchesQuery(s: SessionRow, query: string): boolean {
  return sessionMatchesSearch(s, query);
}

export interface FleetFolds {
  needs: SessionRow[];
  running: SessionRow[];
  idle: SessionRow[];
  doneToday: SessionRow[];
}

/** Needs you and its folds: the Inbox's rows (worst first), then what is
 *  working, idle, and what finished a turn since local midnight, the same
 *  split as the Inbox's quiet line (`notWaiting`). */
export function fleetFolds(
  rows: readonly SessionRow[],
  opts: AttentionOptions,
  query = '',
  since: number = localMidnight(opts.now * 1000),
): FleetFolds {
  const hit = rows.filter((s) => matchesQuery(s, query));
  const out: FleetFolds = { needs: inboxRows(hit, opts), running: [], idle: [], doneToday: [] };
  const recent = (a: SessionRow, b: SessionRow) => (b.last_activity_at ?? 0) - (a.last_activity_at ?? 0);
  for (const s of hit) {
    const st = attentionState(s, opts);
    const today = s.kind !== 'shell' && s.kind !== 'external' && s.last_stop_at != null && s.last_stop_at >= since;
    if (st === 'working') out.running.push(s);
    else if ((st === 'idle' || st === 'done') && today) out.doneToday.push(s);
    else if (st === 'idle') out.idle.push(s);
  }
  out.running.sort(recent);
  out.idle.sort(recent);
  out.doneToday.sort((a, b) => (b.last_stop_at ?? 0) - (a.last_stop_at ?? 0));
  return out;
}

/** "4 need you · 6 running · 9 idle": the folds counted, zeros left out;
 *  "Nothing running" for an empty fleet. */
export function fleetLine(f: FleetFolds): string {
  const parts: string[] = [];
  if (f.needs.length) parts.push(`${f.needs.length} need${f.needs.length === 1 ? 's' : ''} you`);
  if (f.running.length) parts.push(`${f.running.length} running`);
  if (f.idle.length) parts.push(`${f.idle.length} idle`);
  if (f.doneToday.length) parts.push(`${f.doneToday.length} done today`);
  return parts.length ? parts.join(' · ') : 'Nothing running';
}

// ── Tasks view (G3.10, board MCTasks) ──

export type TaskSection = 'needs_you' | 'in_progress' | 'up_next' | 'done_week';

export const TASK_SECTION_LABELS: Record<TaskSection, string> = {
  needs_you: 'Needs you',
  in_progress: 'In progress',
  up_next: 'Up next',
  done_week: 'Done this week',
};

export const TASK_SECTIONS: readonly TaskSection[] = ['needs_you', 'in_progress', 'up_next', 'done_week'];

const WEEK_SECS = 7 * 86_400;

/** Where a task sits in the Tasks view: a session waiting on a person
 *  first, then done (shown only for a week), working, and the rest up next.
 *  `null` drops it (done longer ago than a week). */
export function taskSection(t: WorkTask, nowSec: number): TaskSection | null {
  const done = t.stage === 'done' || t.status_category === 'done';
  if (done) return (t.last_activity_at ?? 0) >= nowSec - WEEK_SECS ? 'done_week' : null;
  if (t.needs_you) return 'needs_you';
  if (t.stage === 'in_progress' || t.stage === 'in_review' || t.stage === 'blocked' || t.status_category === 'in_progress' || (t.counts?.active ?? 0) > 0)
    return 'in_progress';
  return 'up_next';
}

/** The Tasks view's filter chips: Mine (assigned to me in its tracker, the
 *  hub's own filter) and Claude (an agent session is on it now). */
export interface TaskChips {
  mine: boolean;
  claude: boolean;
}

export function tasksBySection(tasks: readonly WorkTask[], chips: TaskChips, nowSec: number): Record<TaskSection, WorkTask[]> {
  const out: Record<TaskSection, WorkTask[]> = { needs_you: [], in_progress: [], up_next: [], done_week: [] };
  for (const t of tasks) {
    if (t.parent_task_id) continue;
    if (chips.claude && (t.counts?.active ?? 0) === 0) continue;
    const s = taskSection(t, nowSec);
    if (s) out[s].push(t);
  }
  return out;
}

/** A native item a person may change from here (status, assignees); a
 *  tracker's ticket changes in its tracker. */
export function isNativeTask<T extends Pick<WorkTask, 'kind' | 'item_id'>>(t: T): t is T & { item_id: number } {
  return t.kind === 'local' && t.item_id != null;
}

/** Start new: the tasks of a selection with no live session on them. */
export function startable(tasks: readonly WorkTask[]): WorkTask[] {
  return tasks.filter((t) => (t.counts?.active ?? 0) === 0 && (t.item_id != null || !!t.key) && t.stage !== 'done');
}
