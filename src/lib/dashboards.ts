// Dashboards (persona design 2026-10-11, step 4): a page of widgets, each a
// view of `work_stats` — the Work view's own tasks, counted under the
// person's access (`service/work/stats.rs`). The numbers are the app's,
// never a model's: a spec only names filters and what to show of the
// answer.
//
// Every workspace has a built-in dashboard. A widget reads one query; the
// queries of one dashboard are deduplicated, so two widgets over the same
// filters cost one read.
//
// Step 5: an agent proposes a dashboard as a `dashboard` chat block
// (docs/chat-blocks.md), the card draws it live, and the person pins it.
// Step 6: pinned dashboards are kept on the hub, per person, so every
// device of theirs shows them and the agent reads them. A change
// goes back through the agent: "Change" puts the dashboard as it is into
// the agent's composer, and its answer is a new block with the same id.
import { writable } from 'svelte/store';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { DASHBOARDS_CHANGED_EVENT } from './events';
import { readPref, writePref } from './prefs';
import { checkUiBlock, UI_SPEC } from './rich_blocks';
import { invokeCmd, type Result } from './result';
import type { WorkTreeFilters, WorkGroupBy } from './work_view';
import type { WorkspaceId } from './workspace';

// ---------------------------------------------------------------------------
// Wire types (`WorkStats`)

export const STAGE_KEYS = ['backlog', 'in_progress', 'in_review', 'blocked', 'done'] as const;
export type StageKey = (typeof STAGE_KEYS)[number];
export type StageCounts = Record<StageKey, number>;

export const STAGE_LABELS: Record<StageKey, string> = {
  backlog: 'Backlog',
  in_progress: 'In progress',
  in_review: 'In review',
  blocked: 'Needs you',
  done: 'Done',
};

export interface BucketHealth {
  bucket_id: number;
  kind: 'sprint' | 'release' | string;
  state: string;
  starts_at?: number | null;
  ends_at?: number | null;
  goal?: string | null;
  pct_time_elapsed?: number | null;
  added_after_start: number;
  removed_after_start: number;
}

export interface StatsGroup {
  org_id?: number | null;
  group: { id: string; label: string; source: string };
  total: number;
  stages: StageCounts;
  pct_done: number;
  needs_you: number;
  cost_micros: number;
  bucket?: BucketHealth | null;
}

export interface WorkStats {
  group_by: string;
  total: number;
  stages: StageCounts;
  pct_done: number;
  needs_you: number;
  cost_micros: number;
  groups: StatsGroup[];
  groups_more?: number;
  generated_at: number;
}

export function workStats(filters: WorkTreeFilters): Promise<Result<WorkStats>> {
  return invokeCmd<WorkStats>('work_stats', { args: { filters } });
}

// ---------------------------------------------------------------------------
// The spec

/** What a stat shows of an answer. */
export type StatValue = 'total' | 'open' | 'needs_you' | 'pct_done' | 'cost' | StageKey;

export type Widget =
  | {
      kind: 'stat';
      title: string;
      query: WorkTreeFilters;
      value: StatValue;
      /** Drawn in the warning colour when above zero. */
      warn?: boolean;
    }
  | {
      /** One row per group: a bar split by stage, or by done share. */
      kind: 'groups';
      title: string;
      query: WorkTreeFilters & { group_by: WorkGroupBy };
      show: 'stages' | 'done';
      limit?: number;
      /** Leave the "No …" group out. */
      hide_none?: boolean;
    }
  | {
      /** Sprints or releases with their health. */
      kind: 'buckets';
      title: string;
      query: WorkTreeFilters & { group_by: 'sprint' | 'release' };
      /** Only buckets in these states; absent: all. */
      states?: string[];
    };

export interface Dashboard {
  id: string;
  title: string;
  /** One line under the title: what it answers. */
  intro: string;
  widgets: Widget[];
}

const OPEN: WorkTreeFilters = { status: 'open' };

/** The built-in dashboard of each workspace. Developer's leads with what
 *  waits on the person; the others with their role's first question. */
export const BUILTIN_DASHBOARDS: Record<WorkspaceId, Dashboard> = {
  developer: {
    id: 'dash.developer',
    title: 'My work',
    intro: 'Your tasks: what needs you, what is in review, where it stands per repo.',
    widgets: [
      { kind: 'stat', title: 'Needs you', query: { mine: true }, value: 'needs_you', warn: true },
      { kind: 'stat', title: 'In progress', query: { mine: true }, value: 'in_progress' },
      { kind: 'stat', title: 'In review', query: { mine: true }, value: 'in_review' },
      { kind: 'stat', title: 'Spent on them', query: { mine: true }, value: 'cost' },
      { kind: 'groups', title: 'By repo', query: { mine: true, ...OPEN, group_by: 'repo' }, show: 'stages', limit: 8 },
    ],
  },
  tester: {
    id: 'dash.tester',
    title: 'Verification',
    intro: 'Work waiting to be verified, and where it is stuck.',
    widgets: [
      { kind: 'stat', title: 'In review', query: OPEN, value: 'in_review' },
      { kind: 'stat', title: 'Waiting on other work', query: OPEN, value: 'blocked', warn: true },
      { kind: 'stat', title: 'Open', query: OPEN, value: 'open' },
      { kind: 'stat', title: 'Done', query: {}, value: 'pct_done' },
      { kind: 'groups', title: 'In review by mission', query: { stages: ['in_review', 'blocked'], group_by: 'mission' }, show: 'stages', limit: 8 },
      { kind: 'groups', title: 'In review by person', query: { stages: ['in_review'], group_by: 'person' }, show: 'stages', limit: 8 },
    ],
  },
  pm: {
    id: 'dash.pm',
    title: 'Delivery',
    intro: 'Releases against their dates, missions against their spend, and what is blocked.',
    widgets: [
      { kind: 'stat', title: 'Open', query: OPEN, value: 'open' },
      { kind: 'stat', title: 'Waiting on other work', query: OPEN, value: 'blocked', warn: true },
      { kind: 'stat', title: 'Needs a person', query: OPEN, value: 'needs_you', warn: true },
      { kind: 'stat', title: 'Spent', query: {}, value: 'cost' },
      { kind: 'buckets', title: 'Releases', query: { group_by: 'release' }, states: ['planned'] },
      { kind: 'groups', title: 'Missions', query: { group_by: 'mission' }, show: 'done', limit: 8, hide_none: true },
    ],
  },
  scrum: {
    id: 'dash.scrum',
    title: 'Sprint health',
    intro: 'The active sprint: done against time gone, scope change, and who is blocked.',
    widgets: [
      { kind: 'buckets', title: 'Active sprint', query: { group_by: 'sprint' }, states: ['active'] },
      { kind: 'stat', title: 'Waiting on other work', query: OPEN, value: 'blocked', warn: true },
      { kind: 'stat', title: 'In review', query: OPEN, value: 'in_review' },
      { kind: 'stat', title: 'Needs a person', query: OPEN, value: 'needs_you', warn: true },
      { kind: 'stat', title: 'In progress', query: OPEN, value: 'in_progress' },
      { kind: 'groups', title: 'Open work by person', query: { ...OPEN, group_by: 'person' }, show: 'stages', limit: 10 },
    ],
  },
  po: {
    id: 'dash.po',
    title: 'Epics and releases',
    intro: 'How far each epic is, what each release still holds, and what waits for acceptance.',
    widgets: [
      { kind: 'stat', title: 'Waiting in review', query: OPEN, value: 'in_review' },
      { kind: 'stat', title: 'Backlog', query: OPEN, value: 'backlog' },
      { kind: 'stat', title: 'Waiting on other work', query: OPEN, value: 'blocked', warn: true },
      { kind: 'stat', title: 'Done', query: {}, value: 'pct_done' },
      { kind: 'groups', title: 'Epics', query: { group_by: 'epic' }, show: 'done', limit: 10, hide_none: true },
      { kind: 'buckets', title: 'Releases', query: { group_by: 'release' }, states: ['planned'] },
    ],
  },
};

// ---------------------------------------------------------------------------
// Reading and deriving (pure)

/** The key two widgets over the same filters share. */
export function queryKey(q: WorkTreeFilters): string {
  return JSON.stringify(Object.keys(q).sort().map((k) => [k, q[k as keyof WorkTreeFilters]]));
}

/** Every distinct query of `d`, by key. */
export function dashboardQueries(d: Dashboard): Map<string, WorkTreeFilters> {
  const out = new Map<string, WorkTreeFilters>();
  for (const w of d.widgets) {
    // Archived work counts: a done task is still done.
    const q: WorkTreeFilters = { archived: true, ...w.query };
    out.set(queryKey(w.query), q);
  }
  return out;
}

/** Micro-USD as dollars, as the Work view writes spend. */
export function dollars(micros: number): string {
  const d = micros / 1_000_000;
  return d >= 100 ? `$${Math.round(d)}` : `$${d.toFixed(2)}`;
}

/** A stat's number and its words. */
export function statOf(s: WorkStats, v: StatValue): { value: string; raw: number; note: string } {
  const open = s.total - s.stages.done;
  switch (v) {
    case 'total':
      return { value: String(s.total), raw: s.total, note: 'tasks' };
    case 'open':
      return { value: String(open), raw: open, note: `of ${s.total}` };
    case 'needs_you':
      return { value: String(s.needs_you), raw: s.needs_you, note: 'wait on a person' };
    case 'pct_done':
      return { value: `${s.pct_done}%`, raw: s.pct_done, note: `${s.stages.done} of ${s.total} done` };
    case 'cost':
      return { value: dollars(s.cost_micros), raw: s.cost_micros, note: 'agent spend' };
    default:
      return { value: String(s.stages[v]), raw: s.stages[v], note: `of ${s.total}` };
  }
}

/** One stage's slice of a stacked bar, in percent of the group. */
export interface Segment {
  stage: StageKey;
  count: number;
  pct: number;
}

export function segmentsOf(g: Pick<StatsGroup, 'total' | 'stages'>): Segment[] {
  if (g.total === 0) return [];
  return STAGE_KEYS.filter((k) => g.stages[k] > 0).map((k) => ({
    stage: k,
    count: g.stages[k],
    pct: (g.stages[k] * 100) / g.total,
  }));
}

/** The groups a `groups` widget draws. */
export function groupsShown(s: WorkStats, w: Extract<Widget, { kind: 'groups' }>): StatsGroup[] {
  const rows = w.hide_none ? s.groups.filter((g) => g.group.id !== 'none') : s.groups;
  return rows.slice(0, w.limit ?? 10);
}

/** How a sprint or release stands: done against time gone. */
export type Pace = 'ahead' | 'on_track' | 'behind' | 'unknown';

export function paceOf(g: StatsGroup): Pace {
  const t = g.bucket?.pct_time_elapsed;
  if (t == null || g.total === 0) return 'unknown';
  // Within ten points of the time gone reads as on track.
  if (g.pct_done + 10 < t) return 'behind';
  if (g.pct_done > t + 10) return 'ahead';
  return 'on_track';
}

export const PACE_LABELS: Record<Pace, string> = {
  ahead: 'Ahead',
  on_track: 'On track',
  behind: 'Behind',
  unknown: 'No dates',
};

/** The buckets a `buckets` widget draws. */
export function bucketsShown(s: WorkStats, w: Extract<Widget, { kind: 'buckets' }>): StatsGroup[] {
  return s.groups.filter((g) => g.bucket && (!w.states || w.states.includes(g.bucket.state)));
}

/** A unix day as "Mon 3 Nov". */
export function shortDate(unix: number | null | undefined): string {
  if (unix == null) return '—';
  return new Date(unix * 1000).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
}

// ---------------------------------------------------------------------------
// Pinned dashboards (steps 5 and 6)

/** A dashboard the person pinned from an agent's proposal. */
export type PinnedDashboard = Dashboard & {
  /** When it was first pinned, unix seconds. */
  pinned_at: number;
  /** The agent's one sentence on why, as the block gave it. */
  why?: string;
};

/** The most dashboards one person keeps pinned (`MAX_PINNED` in
 *  service/dashboards.rs). */
export const MAX_PINNED = 20;

/** The block a dashboard is written as: the agent's own format. */
export function dashboardBlock(d: Dashboard): Record<string, unknown> {
  return { spec: UI_SPEC, kind: 'dashboard', id: d.id, title: d.title, intro: d.intro, widgets: d.widgets };
}

/** A pinned dashboard as the hub answers it (`PinnedDashboardRow`). */
interface PinnedRow {
  id: string;
  title: string;
  spec: unknown;
  why?: string | null;
  pinned_at: number;
}

/** A block checked again as the card checks it: a row a newer build wrote,
 *  or a pref edited by hand, never reaches the page unchecked. */
function fromBlock(spec: unknown, pinned_at: number, why?: string | null): PinnedDashboard | null {
  const c = checkUiBlock(JSON.stringify(spec));
  if (!c.ok || c.block.kind !== 'dashboard') return null;
  const { kind: _k, why: _w, ...d } = c.block;
  return { ...d, pinned_at, ...(why ? { why } : {}) };
}

/** The person's pinned dashboards, as the last read or write left them.
 *  Kept on the hub (`dashboards` tool), so every device of theirs shows
 *  the same; a standalone desktop keeps them in its own store. */
export const pinnedDashboards = writable<PinnedDashboard[]>([]);

/** Step 5 kept pins in this device's local storage: the first read moves
 *  them to the hub, then forgets them here. */
const LOCAL_KEY = 'ui.dashboards.pinned';

function isList(v: unknown): v is unknown[] {
  return Array.isArray(v);
}

function localPins(): PinnedDashboard[] {
  return readPref<unknown[]>(LOCAL_KEY, [], isList).flatMap((raw) => {
    if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return [];
    const x = raw as PinnedDashboard;
    const d = fromBlock(dashboardBlock(x), typeof x.pinned_at === 'number' ? x.pinned_at : 0, x.why);
    return d ? [d] : [];
  });
}

function call<T>(args: Record<string, unknown>): Promise<Result<T>> {
  return invokeCmd<T>('dashboards', { args });
}

/** Read the pinned dashboards again; moves step 5's local pins first. */
export async function loadPinned(): Promise<void> {
  const r = await call<PinnedRow[]>({ action: 'list' });
  if (!r.ok || !Array.isArray(r.value)) return;
  let rows = r.value;
  const local = localPins().filter((d) => !rows.some((x) => x.id === d.id));
  for (const d of local) {
    const p = await call<PinnedRow>({ action: 'pin', dashboard: dashboardBlock(d), ...(d.why ? { why: d.why } : {}) });
    if (p.ok) rows = [...rows, p.value];
  }
  if (local.length || readPref<unknown[]>(LOCAL_KEY, [], isList).length) writePref(LOCAL_KEY, []);
  pinnedDashboards.set(rows.flatMap((x) => fromBlock(x.spec, x.pinned_at, x.why) ?? []));
}

let followers = 0;
let stopFollowing: (() => void) | null = null;

/**
 * Keep `pinnedDashboards` current while a page or a card shows it: read
 * now, and again on `dashboards:changed`, which the hub sends when the
 * person pins or unpins on any device (step 7). Returns the release; the
 * last release stops listening.
 */
export function followPinned(): () => void {
  followers += 1;
  if (followers === 1) {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void loadPinned();
    void listen(DASHBOARDS_CHANGED_EVENT, () => void loadPinned()).then(
      (u) => {
        if (disposed) u();
        else unlisten = u;
      },
      () => {},
    );
    stopFollowing = () => {
      disposed = true;
      unlisten?.();
    };
  }
  let released = false;
  return () => {
    if (released) return;
    released = true;
    followers -= 1;
    if (followers === 0) {
      stopFollowing?.();
      stopFollowing = null;
    }
  };
}

/** The id of every built-in: a proposal may not take one over. */
export const BUILTIN_IDS: ReadonlySet<string> = new Set(Object.values(BUILTIN_DASHBOARDS).map((d) => d.id));

/** Same widgets, title and intro: pinning it again would change nothing. */
export function sameDashboard(a: Dashboard, b: Dashboard): boolean {
  return JSON.stringify(dashboardBlock(a)) === JSON.stringify(dashboardBlock(b));
}

/** What pinning `d` would do: add it, replace the pinned one of its id, or
 *  nothing (`same`). `full` when the person already keeps MAX_PINNED. */
export type PinState = 'new' | 'replace' | 'same' | 'full' | 'builtin';

export function pinStateOf(d: Dashboard, pinned: readonly PinnedDashboard[]): PinState {
  if (BUILTIN_IDS.has(d.id)) return 'builtin';
  const had = pinned.find((p) => p.id === d.id);
  if (had) return sameDashboard(had, d) ? 'same' : 'replace';
  return pinned.length >= MAX_PINNED ? 'full' : 'new';
}

/** Pin `d` on the hub, or replace the pinned one of its id in place. The
 *  hub checks the block again and refuses a built-in's id or a full list. */
export async function pinDashboard(d: Dashboard, why?: string): Promise<Result<PinnedDashboard>> {
  const r = await call<PinnedRow>({ action: 'pin', dashboard: dashboardBlock(d), ...(why ? { why } : {}) });
  if (!r.ok) return r;
  const entry = fromBlock(r.value.spec, r.value.pinned_at, r.value.why);
  if (!entry) return { ok: false, error: { code: 'E_INVALID', message: 'the hub kept a dashboard this app cannot draw' } };
  pinnedDashboards.update((list) =>
    list.some((p) => p.id === entry.id) ? list.map((p) => (p.id === entry.id ? entry : p)) : [...list, entry],
  );
  return { ok: true, value: entry };
}

export async function unpinDashboard(id: string): Promise<Result<boolean>> {
  const r = await call<{ removed: boolean }>({ action: 'unpin', id });
  if (!r.ok) return r;
  pinnedDashboards.update((list) => list.filter((p) => p.id !== id));
  return { ok: true, value: r.value.removed };
}

/** In words, what a replacement changes: widgets added, removed and
 *  changed, by title. Empty when only the title or intro moved. */
export function dashboardChanges(from: Dashboard, to: Dashboard): string[] {
  const before = new Map(from.widgets.map((w) => [w.title, JSON.stringify(w)]));
  const after = new Map(to.widgets.map((w) => [w.title, JSON.stringify(w)]));
  const out: string[] = [];
  for (const [t, w] of after) {
    if (!before.has(t)) out.push(`adds “${t}”`);
    else if (before.get(t) !== w) out.push(`changes “${t}”`);
  }
  for (const t of before.keys()) if (!after.has(t)) out.push(`removes “${t}”`);
  if (out.length === 0 && (from.title !== to.title || from.intro !== to.intro)) out.push('renames it');
  return out;
}

/** What "Change" puts in the agent's composer: the dashboard as it is, as
 *  the block the agent answers with, then the start of the person's ask.
 *  A built-in is offered as a copy, under an id of its own. */
export function changePrompt(d: Dashboard, widget?: string): string {
  const builtin = BUILTIN_IDS.has(d.id);
  const base = builtin ? { ...d, id: `${d.id}.mine` } : d;
  const json = JSON.stringify(dashboardBlock(base), null, 2);
  const head = builtin
    ? `The built-in dashboard "${d.title}", to make my own copy of (keep the id ${base.id}):`
    : `My pinned dashboard "${d.title}", as it is now:`;
  const ask = widget ? `Change the widget "${widget}": ` : 'Change it: ';
  return `${head}\n\n\`\`\`fleet-ui\n${json}\n\`\`\`\n\nAnswer with a \`dashboard\` block with the same id. ${ask}`;
}

/** The dashboard the page shows instead of the workspace's own, and the
 *  workspace it was picked in: a pick holds only while that workspace is
 *  active, so switching workspace lands on the new one's dashboard. */
export const shownDashboard = writable<{ id: string; workspace: WorkspaceId } | null>(null);
