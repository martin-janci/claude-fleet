// Dashboards (persona design 2026-10-11, step 4): a page of widgets, each a
// view of `work_stats` — the Work view's own tasks, counted under the
// person's access (`service/work/stats.rs`). The numbers are the app's,
// never a model's: a spec only names filters and what to show of the
// answer, which is the shape an agent will later write (step 5).
//
// Every workspace has a built-in dashboard. A widget reads one query; the
// queries of one dashboard are deduplicated, so two widgets over the same
// filters cost one read.
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
      hideNone?: boolean;
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
      { kind: 'groups', title: 'Missions', query: { group_by: 'mission' }, show: 'done', limit: 8, hideNone: true },
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
      { kind: 'groups', title: 'Epics', query: { group_by: 'epic' }, show: 'done', limit: 10, hideNone: true },
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
  const rows = w.hideNone ? s.groups.filter((g) => g.group.id !== 'none') : s.groups;
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
