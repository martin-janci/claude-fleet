// Workspaces (persona design 2026-10-11): one lens per kind of work —
// Developer, Tester, Project manager, Scrum master, Product owner. A
// workspace changes layout and defaults only: the rail's order, where a
// switch lands, and the Work view's filters. It never changes access,
// running sessions or Needs you: the Inbox and its badge are in every
// workspace, and no rail item is ever hidden, only moved down.
//
// The active workspace is a per-device pref (`ui.workspace`). Each
// workspace keeps its own Work filters: leaving one stores what the person
// had set, coming back puts it back, and the first visit starts from the
// preset's.
import { get, writable } from 'svelte/store';
import { readPref, writePref } from './prefs';
import type { RailId, RailItem } from './rail';
import {
  activeWorkViewId,
  normalizeFilters,
  workLayout,
  workViewFilters,
  type WorkLayout,
  type WorkTreeFilters,
} from './work_view';

export const WORKSPACE_IDS = ['developer', 'tester', 'pm', 'scrum', 'po'] as const;
export type WorkspaceId = (typeof WORKSPACE_IDS)[number];

export interface WorkspacePreset {
  id: WorkspaceId;
  name: string;
  /** The rail tile's word. */
  short: string;
  /** The rail tile's two letters. */
  mono: string;
  /** What it is for, in the menu. */
  need: string;
  /** Rail items first, in this order; the rest follow in the manual's. */
  rail: readonly RailId[];
  /** Where switching to it lands. */
  landing: RailId;
  /** The Work view's filters on a first visit. */
  work: WorkTreeFilters;
  layout: WorkLayout;
}

export const WORKSPACES: readonly WorkspacePreset[] = [
  {
    // Today's app, unchanged: the rail in the manual's order.
    id: 'developer',
    name: 'Developer',
    short: 'Dev',
    mono: 'Dv',
    need: 'Unblock my agents, ship PRs',
    rail: ['control', 'inbox', 'sessions', 'work', 'dashboards', 'automation', 'accounts', 'toolkit'],
    landing: 'inbox',
    work: {},
    layout: 'list',
  },
  {
    id: 'tester',
    name: 'Tester',
    short: 'QA',
    mono: 'QA',
    need: 'Verify agent work on real evidence',
    rail: ['inbox', 'dashboards', 'work', 'control', 'sessions'],
    landing: 'dashboards',
    work: { stages: ['in_review', 'blocked'], group_by: 'mission' },
    layout: 'list',
  },
  {
    id: 'pm',
    name: 'Project manager',
    short: 'PM',
    mono: 'PM',
    need: 'Dates, budget, who is blocking',
    rail: ['inbox', 'dashboards', 'control', 'work', 'automation'],
    landing: 'dashboards',
    work: { status: 'open', group_by: 'release' },
    layout: 'grouped',
  },
  {
    id: 'scrum',
    name: 'Scrum master',
    short: 'SM',
    mono: 'SM',
    need: 'Sprint flow, impediments, standup',
    rail: ['inbox', 'dashboards', 'work', 'control', 'sessions'],
    landing: 'dashboards',
    work: { iteration: 'current', group_by: 'person' },
    layout: 'grouped',
  },
  {
    id: 'po',
    name: 'Product owner',
    short: 'PO',
    mono: 'PO',
    need: 'Accept value, order the backlog',
    rail: ['inbox', 'dashboards', 'work', 'control', 'sessions'],
    landing: 'dashboards',
    work: { status: 'open', group_by: 'epic' },
    layout: 'grouped',
  },
];

const isWorkspaceId = (v: unknown): v is WorkspaceId => WORKSPACE_IDS.includes(v as WorkspaceId);

export function workspacePreset(id: WorkspaceId): WorkspacePreset {
  return WORKSPACES.find((w) => w.id === id) ?? WORKSPACES[0];
}

/** The active workspace. Developer, today's layout, until someone picks. */
export const workspace = writable<WorkspaceId>(readPref('ui.workspace', 'developer', isWorkspaceId));
workspace.subscribe((v) => writePref('ui.workspace', v));

/**
 * PURE: the rail in `ws`'s order. The preset's items first, then every
 * other item in the order given (the manual's), Settings last. Nothing is
 * dropped: an item a workspace does not feature only moves down.
 */
export function workspaceRailItems(ws: WorkspaceId, items: readonly RailItem[]): RailItem[] {
  const featured = workspacePreset(ws).rail;
  const rank = (i: RailItem) => {
    if (i.id === 'settings') return Number.MAX_SAFE_INTEGER;
    const at = featured.indexOf(i.id);
    return at >= 0 ? at : featured.length + items.indexOf(i);
  };
  return [...items].sort((a, b) => rank(a) - rank(b));
}

/** What one workspace had set on the Work view when it was left. */
interface WorkState {
  filters: WorkTreeFilters;
  viewId: number | null;
  layout: WorkLayout;
}
type Saved = Partial<Record<WorkspaceId, WorkState>>;

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isWorkState = (v: unknown): v is WorkState =>
  isObj(v) &&
  isObj(v.filters) &&
  (v.viewId === null || (typeof v.viewId === 'number' && Number.isInteger(v.viewId))) &&
  (v.layout === 'list' || v.layout === 'grouped');
const isSaved = (v: unknown): v is Saved =>
  isObj(v) && Object.entries(v).every(([k, s]) => isWorkspaceId(k) && isWorkState(s));

const SAVED_KEY = 'workspace.work';

/** A switch asks the shell to land here (App's rail handler does it). */
export const workspaceLanding = writable<{ rail: RailId; seq: number } | null>(null);
let seq = 0;

/** Land on a rail item as a click on it would (⌘K's Inbox and Toolkit
 *  rows, review w18-4); App's rail handler takes it. */
export function landOn(rail: RailId): void {
  workspaceLanding.set({ rail, seq: ++seq });
}

/**
 * Switch to `to`: store the Work view's state under the workspace being
 * left, put back `to`'s (or its preset's on a first visit), and ask the
 * shell to land on `to`'s landing item. False when `to` is already active.
 */
export function switchWorkspace(to: WorkspaceId): boolean {
  const from = get(workspace);
  if (from === to) return false;
  const saved = readPref<Saved>(SAVED_KEY, {}, isSaved);
  saved[from] = { filters: normalizeFilters(get(workViewFilters)), viewId: get(activeWorkViewId), layout: get(workLayout) };
  writePref(SAVED_KEY, saved);
  const preset = workspacePreset(to);
  const back = saved[to] ?? { filters: preset.work, viewId: null, layout: preset.layout };
  const { group: _g, ...filters } = normalizeFilters(back.filters);
  workViewFilters.set(filters);
  activeWorkViewId.set(back.viewId);
  workLayout.set(back.layout);
  workspace.set(to);
  workspaceLanding.set({ rail: preset.landing, seq: ++seq });
  return true;
}
