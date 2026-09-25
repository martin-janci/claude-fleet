// Work graph M10.4: the work filters' rows and filter object over the one
// `rowMatches` — the combinations, the persistence, and "mine".
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { rowMatches } from './sidebar_index';
import { session } from './hosts_fixture';
import type { SessionRow, SessionWork } from './sessions';
import type { TrackerRow } from './trackers';
import {
  ASSIGNEE_MINE,
  DEFAULT_WORK_FILTERS,
  activeWorkFilterCount,
  bothPredicates,
  effectiveWorkFilters,
  isWorkFilters,
  loadMine,
  mineItemIds,
  pastWorkFields,
  sessionWorkRow,
  toRowFilters,
  workFilterPredicate,
  workFilters,
  type WorkFilters,
} from './work_filters';

const tracker = (id: number, prefixes: string[]): TrackerRow =>
  ({
    id,
    provider: 'jira_cloud',
    name: `T${id}`,
    site_url: `https://t${id}.example`,
    state: 'ok',
    created_at: 0,
    config: { key_prefixes: prefixes },
  }) as unknown as TrackerRow;
const TRACKERS = [tracker(1, ['PAY']), tracker(2, ['OPS'])];

const work = (key: string, item: number, status: string, archived: number | null = null): SessionWork => ({
  link_id: item,
  item_id: item,
  key,
  title: key,
  source: 'manual',
  status_category: status,
  archived_at: archived,
});

const s = (name: string, w: SessionWork | null, over: Partial<SessionRow> = {}) =>
  session('h', name, { work: w, ...over });

// PAY-1 mine / in progress, PAY-2 done / archived, OPS-3 todo, a session
// without work, and a background agent.
const rows = [
  s('pay1', work('PAY-1', 11, 'in_progress')),
  s('pay2', work('PAY-2', 12, 'done', 100)),
  s('ops3', work('OPS-3', 13, 'todo')),
  s('bare', null),
  s('bg', work('PAY-1', 11, 'in_progress'), { kind: 'bg' }),
];
const ctx = { trackers: TRACKERS, mine: new Set([11]) };

function visible(f: Partial<WorkFilters>, extra: Parameters<typeof rowMatches>[1] = {}): string[] {
  const rf = { ...toRowFilters({ ...DEFAULT_WORK_FILTERS, ...f }), ...extra };
  return rows.filter((r) => rowMatches(sessionWorkRow(r, ctx), rf)).map((r) => r.tmux_name);
}

describe('work filters over rowMatches (M10.4)', () => {
  it('the defaults filter nothing', () => {
    expect(visible({})).toEqual(['pay1', 'pay2', 'ops3', 'bare', 'bg']);
    expect(activeWorkFilterCount(DEFAULT_WORK_FILTERS)).toBe(0);
    expect(workFilterPredicate(DEFAULT_WORK_FILTERS, ctx)).toBeNull();
  });

  it('each filter alone', () => {
    expect(visible({ tracker: 1 })).toEqual(['pay1', 'pay2', 'bg']);
    expect(visible({ tracker: 2 })).toEqual(['ops3']);
    expect(visible({ status: 'in_progress' })).toEqual(['pay1', 'bg']);
    expect(visible({ status: 'done' })).toEqual(['pay2']);
    expect(visible({ assignee: 'mine' })).toEqual(['pay1', 'bg']);
    expect(visible({ archived: false })).toEqual(['pay1', 'ops3', 'bare', 'bg']);
    // Every session row is live.
    expect(visible({ hasSession: 'yes' })).toHaveLength(5);
    expect(visible({ hasSession: 'no' })).toEqual([]);
  });

  it('filters compose: every clause must hold', () => {
    expect(visible({ tracker: 1, status: 'done' })).toEqual(['pay2']);
    expect(visible({ tracker: 1, status: 'done', archived: false })).toEqual([]);
    expect(visible({ tracker: 2, assignee: 'mine' })).toEqual([]);
    expect(visible({ assignee: 'mine', status: 'in_progress' })).toEqual(['pay1', 'bg']);
    // … and with the host / bg filters and the needs-you predicate.
    expect(visible({ assignee: 'mine' }, { showBgAgents: false })).toEqual(['pay1']);
    expect(visible({ tracker: 1 }, { predicate: (r) => r.tmux_name !== 'pay2' })).toEqual(['pay1', 'bg']);
    expect(visible({ status: 'todo' }, { host: 'other' })).toEqual([]);
  });

  it('a key two trackers claim, or no key, has no tracker', () => {
    const both = [tracker(1, ['PAY']), tracker(3, ['PAY'])];
    expect(sessionWorkRow(rows[0], { trackers: both, mine: new Set() }).trackerId).toBeNull();
    expect(sessionWorkRow(rows[3], ctx).trackerId).toBeNull();
    expect(sessionWorkRow(rows[0], ctx).assignees).toEqual([ASSIGNEE_MINE]);
    expect(sessionWorkRow(rows[2], ctx).assignees).toEqual([]);
  });

  it('past links: archived, never live, tracker by key, no status', () => {
    const past = (f: Partial<WorkFilters>) =>
      rowMatches(
        { host: 'h', scope: 'all', ...pastWorkFields('PAY-9', 11, ctx) },
        toRowFilters({ ...DEFAULT_WORK_FILTERS, ...f }),
      );
    expect(past({})).toBe(true);
    expect(past({ hasSession: 'no' })).toBe(true);
    expect(past({ hasSession: 'yes' })).toBe(false);
    expect(past({ archived: false })).toBe(false);
    expect(past({ tracker: 1, assignee: 'mine' })).toBe(true);
    expect(past({ tracker: 2 })).toBe(false);
    expect(past({ status: 'done' })).toBe(false);
  });

  it('a removed tracker and has-session outside work mode do not filter', () => {
    const f: WorkFilters = { ...DEFAULT_WORK_FILTERS, tracker: 7, hasSession: 'no' };
    expect(effectiveWorkFilters(f, TRACKERS, false)).toEqual(DEFAULT_WORK_FILTERS);
    expect(effectiveWorkFilters(f, [...TRACKERS, tracker(7, [])], true)).toEqual(f);
    expect(activeWorkFilterCount(f)).toBe(2);
  });

  it('bothPredicates', () => {
    const a = (r: SessionRow) => r.id % 2 === 0;
    const b = (r: SessionRow) => r.id % 3 === 0;
    expect(bothPredicates(null, null)).toBeNull();
    expect(bothPredicates(a, null)).toBe(a);
    expect(bothPredicates(null, b)).toBe(b);
    const both = bothPredicates(a, b)!;
    expect([6, 4, 3].map((id) => both({ id } as SessionRow))).toEqual([true, false, false]);
  });

  it('validates what it reads back', () => {
    expect(isWorkFilters(DEFAULT_WORK_FILTERS)).toBe(true);
    expect(isWorkFilters({ ...DEFAULT_WORK_FILTERS, tracker: 3 })).toBe(true);
    expect(isWorkFilters({ ...DEFAULT_WORK_FILTERS, tracker: '3' })).toBe(false);
    expect(isWorkFilters({ ...DEFAULT_WORK_FILTERS, status: 'blocked' })).toBe(false);
    expect(isWorkFilters({ ...DEFAULT_WORK_FILTERS, assignee: 'alice' })).toBe(false);
    expect(isWorkFilters({ ...DEFAULT_WORK_FILTERS, archived: 'no' })).toBe(false);
    expect(isWorkFilters(null)).toBe(false);
  });
});

describe('work filter state', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(invoke).mockReset();
  });

  it('persists like the host and scope filters', async () => {
    workFilters.set({ ...DEFAULT_WORK_FILTERS, status: 'todo', archived: false });
    expect(JSON.parse(localStorage.getItem('cf:pref:sidebar.work-filters') ?? 'null')).toMatchObject({
      status: 'todo',
      archived: false,
    });
    vi.resetModules();
    const fresh = await import('./work_filters');
    expect(get(fresh.workFilters)).toMatchObject({ status: 'todo', archived: false });
    workFilters.set({ ...DEFAULT_WORK_FILTERS });
  });

  it('a corrupt pref falls back to the defaults', async () => {
    localStorage.setItem('cf:pref:sidebar.work-filters', JSON.stringify({ status: 'nope' }));
    vi.resetModules();
    const fresh = await import('./work_filters');
    expect(get(fresh.workFilters)).toEqual(DEFAULT_WORK_FILTERS);
  });

  it('"mine" is the hub\'s mine view', async () => {
    vi.mocked(invoke).mockResolvedValue([{ id: 11 }, { id: 14 }]);
    await loadMine();
    expect(invoke).toHaveBeenCalledWith('work_tickets', { args: { view: 'mine', limit: 200 } });
    expect([...get(mineItemIds)]).toEqual([11, 14]);
    // A failure keeps the last set.
    vi.mocked(invoke).mockRejectedValue({ code: 'E_HUB', message: 'down' });
    await loadMine();
    expect([...get(mineItemIds)]).toEqual([11, 14]);
  });
});
