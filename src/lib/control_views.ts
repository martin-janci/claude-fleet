// Control's Views panel (Orbit Fleet redesign step 9.4, boards MissionControl
// and MCViews): the column beside the chat. Its views come from this chat's
// fleet (Needs you, the session in focus, Pull requests, Library, Today
// briefing); "+" toggles and reorders them. What lives in another place
// (Tasks and Missions in Work, Routines in Automation, Hosts and usage in
// Accounts) is a link there, not a copy. The layout is a local pref.
import { writable } from 'svelte/store';
import { inboxQueue } from './inbox';
import { readPref, writePref } from './prefs';
import { goTo, leave } from './destination';
import { sidebarView } from './work_view';

export type ControlViewId = 'needs-you' | 'session' | 'prs' | 'library' | 'today';

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
  { id: 'prs', label: 'Pull requests', glyph: '⑂', step: '9.4', landed: true },
  { id: 'library', label: 'Library', glyph: '◧', step: '9.7', landed: false },
  { id: 'today', label: 'Today briefing', glyph: '☀', step: '9.4', landed: true },
];

export type ElsewhereId = 'tasks' | 'missions' | 'hosts';

export interface ElsewhereLink {
  id: ElsewhereId;
  label: string;
}

/** "Not views here, open them where they live." Routines in Automation
 *  joins them with the Automation rail item (8.4). */
export const ELSEWHERE: readonly ElsewhereLink[] = [
  { id: 'tasks', label: 'Tasks in Work' },
  { id: 'missions', label: 'Missions in Work' },
  { id: 'hosts', label: 'Hosts and usage in Accounts' },
];

/** Leave Control for the place a link names. */
export function openElsewhere(id: ElsewhereId): void {
  if (id === 'hosts') {
    goTo('accounts');
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
