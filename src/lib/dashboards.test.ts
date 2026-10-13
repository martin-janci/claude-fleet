// Persona dashboards (step 4): the built-ins, the shared queries and what
// each widget draws of a `work_stats` answer.
import { describe, it, expect } from 'vitest';
import {
  BUILTIN_DASHBOARDS,
  bucketsShown,
  dashboardQueries,
  groupsShown,
  paceOf,
  queryKey,
  segmentsOf,
  statOf,
  type StatsGroup,
  type WorkStats,
} from './dashboards';
import { WORKSPACE_IDS } from './workspace';

const stages = (b = 0, p = 0, r = 0, x = 0, d = 0) => ({ backlog: b, in_progress: p, in_review: r, blocked: x, done: d });

const group = (id: string, over: Partial<StatsGroup> = {}): StatsGroup => ({
  group: { id, label: id, source: 'x' },
  total: 4,
  stages: stages(1, 1, 0, 1, 1),
  pct_done: 25,
  needs_you: 1,
  cost_micros: 0,
  ...over,
});

const answer = (over: Partial<WorkStats> = {}): WorkStats => ({
  group_by: 'group',
  total: 10,
  stages: stages(3, 2, 1, 2, 2),
  pct_done: 20,
  needs_you: 3,
  cost_micros: 12_340_000,
  groups: [],
  generated_at: 1,
  ...over,
});

describe('dashboards', () => {
  it('every workspace has a built-in dashboard with widgets', () => {
    for (const id of WORKSPACE_IDS) {
      expect(BUILTIN_DASHBOARDS[id].widgets.length, id).toBeGreaterThan(0);
      expect(BUILTIN_DASHBOARDS[id].id).toBe(`dash.${id}`);
    }
  });

  it('two widgets over the same filters share one read, archived work counted', () => {
    const qs = dashboardQueries(BUILTIN_DASHBOARDS.scrum);
    const opens = BUILTIN_DASHBOARDS.scrum.widgets.filter((w) => queryKey(w.query) === queryKey({ status: 'open' }));
    expect(opens.length).toBeGreaterThan(1);
    expect(qs.size).toBeLessThan(BUILTIN_DASHBOARDS.scrum.widgets.length);
    for (const q of qs.values()) expect(q.filters.archived).toBe(true);
    expect(queryKey({ status: 'open', mine: true })).toBe(queryKey({ mine: true, status: 'open' }));
  });

  it('a stat reads its value and says what it counts', () => {
    const s = answer();
    expect(statOf(s, 'open')).toEqual({ value: '8', raw: 8, note: 'of 10' });
    expect(statOf(s, 'blocked').value).toBe('2');
    expect(statOf(s, 'pct_done').note).toBe('2 of 10 done');
    expect(statOf(s, 'cost').value).toBe('$12.34');
    expect(statOf(s, 'needs_you').raw).toBe(3);
  });

  it('a stacked bar has one segment per stage present, adding up to the whole', () => {
    const segs = segmentsOf(group('a'));
    expect(segs.map((x) => x.stage)).toEqual(['backlog', 'in_progress', 'blocked', 'done']);
    expect(segs.reduce((n, x) => n + x.pct, 0)).toBeCloseTo(100);
    expect(segmentsOf({ total: 0, stages: stages() })).toEqual([]);
  });

  it('groups can leave out the "No …" group and are capped', () => {
    const s = answer({ groups: [group('a'), group('none'), group('b'), group('c')] });
    const w = { kind: 'groups' as const, title: 'x', query: { group_by: 'epic' as const }, show: 'done' as const, limit: 2, hide_none: true };
    expect(groupsShown(s, w).map((g) => g.group.id)).toEqual(['a', 'b']);
  });

  it('a bucket is behind when done trails time gone by more than ten points', () => {
    const health = (pct_time_elapsed: number | null) => ({
      bucket_id: 1, kind: 'sprint', state: 'active', pct_time_elapsed, added_after_start: 0, removed_after_start: 0,
    });
    expect(paceOf(group('s', { pct_done: 30, bucket: health(70) }))).toBe('behind');
    expect(paceOf(group('s', { pct_done: 65, bucket: health(70) }))).toBe('on_track');
    expect(paceOf(group('s', { pct_done: 90, bucket: health(50) }))).toBe('ahead');
    expect(paceOf(group('s', { bucket: health(null) }))).toBe('unknown');
    const s = answer({ groups: [group('s', { bucket: health(10) }), group('t', { bucket: { ...health(1), state: 'closed' } }), group('none')] });
    const w = { kind: 'buckets' as const, title: 'x', query: { group_by: 'sprint' as const }, states: ['active'] };
    expect(bucketsShown(s, w).map((g) => g.group.id)).toEqual(['s']);
  });
});
