// Missions → Compare (lanes design L6): rows per mission and medians per
// execution mode.
import { describe, it, expect } from 'vitest';
import type { Mission, MissionDetail, MissionStats } from './missions';
import { compareRow, missionsToCompare, summarize } from './mission_compare';

function mission(over: Partial<Mission> = {}): Mission {
  return {
    id: 1,
    name: 'M',
    goal: 'g',
    mode: 'finite',
    state: 'completed',
    level: 2,
    plan_version: 1,
    created_at: 1,
    updated_at: 1,
    version: 1,
    ...over,
  } as Mission;
}

function stats(over: Partial<MissionStats> = {}): MissionStats {
  return {
    items: 4,
    items_done: 4,
    runs_by_role: { implement: 6, review: 4 },
    session_runs: 10,
    sessions: 8,
    checkouts: 8,
    failed_runs: 2,
    retries: 2,
    runs_per_done_item_x100: 250,
    item_secs_p50: 1800,
    span_secs: 10_000,
    idle_secs: 2500,
    ...over,
  };
}

const detail = (m: Partial<Mission>, s: Partial<MissionStats> | null): MissionDetail => ({
  mission: mission(m),
  stats: s ? stats(s) : null,
});

describe('a mission\'s row', () => {
  it('reads its mode, its rates per done task and its idle share', () => {
    const r = compareRow(detail({ id: 7, name: 'Pay', policy: { execution: 'lane' }, cost_micros: 8_000_000 }, {}))!;
    expect(r).toMatchObject({
      id: 7,
      name: 'Pay',
      execution: 'lane',
      done: 4,
      runsPerDone: 2.5,
      coldStartsPerDone: 2,
      taskSecs: 1800,
      idlePct: 25,
      costPerDoneMicros: 2_000_000,
    });
  });

  it('is item by default and has no rates before a task is done', () => {
    const r = compareRow(detail({}, { items_done: 0, runs_per_done_item_x100: null }))!;
    expect(r.execution).toBe('item');
    expect(r.runsPerDone).toBeNull();
    expect(r.coldStartsPerDone).toBeNull();
    expect(r.costPerDoneMicros).toBeNull();
  });

  it('is nothing for a mission that never ran', () => {
    expect(compareRow(detail({}, null))).toBeNull();
    expect(compareRow(detail({}, { span_secs: 0 }))).toBeNull();
  });
});

describe('the summary per mode', () => {
  it('takes each metric\'s median, item first, and leaves an empty mode out', () => {
    const rows = [
      compareRow(detail({ id: 1 }, { runs_per_done_item_x100: 300, item_secs_p50: 3000 }))!,
      compareRow(detail({ id: 2 }, { runs_per_done_item_x100: 200, item_secs_p50: 1000 }))!,
      compareRow(detail({ id: 3, policy: { execution: 'lane' } }, { runs_per_done_item_x100: 120, sessions: 2 }))!,
    ];
    const s = summarize(rows);
    expect(s.map((x) => x.execution)).toEqual(['item', 'lane']);
    expect(s[0]).toMatchObject({ missions: 2, doneTasks: 8, runsPerDone: 2.5, taskSecs: 2000 });
    expect(s[1]).toMatchObject({ missions: 1, runsPerDone: 1.2, coldStartsPerDone: 0.5 });
    expect(summarize(rows.slice(0, 2)).map((x) => x.execution)).toEqual(['item']);
  });
});

describe('which missions are compared', () => {
  it('those that ran and are no plan, newest first, up to the limit', () => {
    const list = [
      mission({ id: 1, state: 'draft', updated_at: 9 }),
      mission({ id: 2, state: 'completed', updated_at: 5 }),
      mission({ id: 3, state: 'active', updated_at: 8 }),
      mission({ id: 4, state: 'active', mode: 'plan', updated_at: 7 }),
      mission({ id: 5, state: 'failed', updated_at: 6 }),
    ];
    expect(missionsToCompare(list).map((m) => m.id)).toEqual([3, 5, 2]);
    expect(missionsToCompare(list, 2).map((m) => m.id)).toEqual([3, 5]);
  });
});
