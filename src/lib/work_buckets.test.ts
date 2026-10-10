// Sprints and releases (sprints design 2026-09-28 §6b): the commands each
// helper calls, a move between sprints, and the roll-up text.
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  boardFilters,
  bucketIdOfGroup,
  liveScope,
  bucketSummary,
  closeSprint,
  createBucket,
  dayToUnix,
  openBuckets,
  planInto,
  unixToDay,
  type BucketRow,
} from './work_buckets';

const mock = invoke as ReturnType<typeof vi.fn>;
const bucket = (over: Partial<BucketRow> = {}): BucketRow => ({
  id: 1,
  kind: 'sprint',
  name: 'Sprint 24',
  state: 'active',
  created_at: 0,
  updated_at: 0,
  version: 3,
  ...over,
});

beforeEach(() => mock.mockReset());

describe('planInto', () => {
  it('adds each item and moves one out of the sprint that holds it', async () => {
    const seen: [string, unknown][] = [];
    mock.mockImplementation(async (cmd: string, args?: { args?: { bucket_id: number; item_id: number } }) => {
      if (!args?.args) return null;
      seen.push([cmd, args.args]);
      if (cmd === 'add_work_to_bucket' && args.args.item_id === 8 && !seen.some(([c]) => c === 'remove_work_from_bucket'))
        throw { code: 'E_CONFLICT', message: 'TASK-8 is already in sprint "Sprint 23"', details: { sprint_id: 5 } };
      return bucket();
    });
    const out = await planInto(bucket(), [7, 8]);
    expect(out).toEqual({ done: 2, failed: [] });
    expect(seen).toEqual([
      ['add_work_to_bucket', { bucket_id: 1, item_id: 7 }],
      ['add_work_to_bucket', { bucket_id: 1, item_id: 8 }],
      ['remove_work_from_bucket', { bucket_id: 5, item_id: 8 }],
      ['add_work_to_bucket', { bucket_id: 1, item_id: 8 }],
    ]);
  });

  it('reports a refusal and goes on with the rest', async () => {
    mock.mockImplementation(async (_cmd: string, args?: { args?: { item_id: number } }) => {
      if (args?.args?.item_id === 7) throw { code: 'E_FORBIDDEN', message: 'another organisation' };
      return bucket();
    });
    const out = await planInto(bucket(), [7, 9]);
    expect(out.done).toBe(1);
    expect(out.failed).toEqual([{ itemId: 7, error: { code: 'E_FORBIDDEN', message: 'another organisation' } }]);
  });
});

describe('admin', () => {
  it('creates a sprint with its dates as UTC noon, and a release without a start', async () => {
    mock.mockResolvedValue({ bucket: bucket() });
    await createBucket({ kind: 'sprint', name: ' Sprint 25 ', org_id: 2, starts_on: '2026-10-12', ends_on: '2026-10-23', goal: '' });
    expect(mock.mock.calls[0]).toEqual([
      'work_bucket_admin',
      {
        args: {
          action: 'bucket_create',
          kind: 'sprint',
          name: 'Sprint 25',
          org_id: 2,
          starts_at: dayToUnix('2026-10-12'),
          ends_at: dayToUnix('2026-10-23'),
        },
      },
    ]);
    await createBucket({ kind: 'release', name: '0.3.0', starts_on: '2026-10-12', ends_on: '' });
    expect(mock.mock.calls[1][1]).toEqual({ args: { action: 'bucket_create', kind: 'release', name: '0.3.0' } });
  });

  it('closes a sprint carrying only to a named sprint', async () => {
    mock.mockResolvedValue({ bucket: bucket(), ended: [], carried: [] });
    await closeSprint(bucket(), 4, [7]);
    await closeSprint(bucket(), null, [7]);
    expect(mock.mock.calls.map((c) => c[1])).toEqual([
      { args: { action: 'bucket_close', bucket_id: 1, expected_version: 3, carry_to: 4, carry: [7] } },
      { args: { action: 'bucket_close', bucket_id: 1, expected_version: 3 } },
    ]);
  });
});

describe('helpers', () => {
  it('reads a section id as its bucket', () => {
    expect(bucketIdOfGroup('sprint:12')).toBe(12);
    expect(bucketIdOfGroup('release:3')).toBe(3);
    expect(bucketIdOfGroup('none')).toBeNull();
    expect(bucketIdOfGroup('mission:3')).toBeNull();
  });

  it('round-trips a date', () => {
    expect(unixToDay(dayToUnix('2026-02-28'))).toBe('2026-02-28');
    expect(dayToUnix('')).toBeUndefined();
    expect(dayToUnix('28.2.2026')).toBeUndefined();
  });

  it('sums a bucket up', () => {
    expect(bucketSummary(bucket({ total: 9, done: 4 }))).toBe('Active · 4/9 done');
    expect(bucketSummary(bucket({ total: 2, done: 0, ends_at: dayToUnix('2026-10-20') }))).toMatch(/^Active · 0\/2 done · ends /);
    expect(bucketSummary(bucket({ kind: 'release', state: 'planned', ends_at: dayToUnix('2026-11-01') }))).toMatch(/^Planned · target /);
  });

  it('offers open buckets, active sprints first', () => {
    const all = [
      bucket({ id: 1, name: 'B', state: 'planned' }),
      bucket({ id: 2, name: 'A', state: 'closed' }),
      bucket({ id: 3, name: 'C', state: 'active' }),
      bucket({ id: 4, kind: 'release', name: '0.3.0', state: 'planned' }),
    ];
    expect(openBuckets(all, 'sprint').map((b) => b.id)).toEqual([3, 1]);
    expect(openBuckets(all, 'release').map((b) => b.id)).toEqual([4]);
  });
  it('narrows the board to one sprint, or to no sprint', () => {
    const view = { tracker: 1, status: 'open' as const, group_by: 'person' as const };
    expect(boardFilters(view, 'all')).toEqual({ tracker: 1, group_by: 'person', archived: true });
    expect(boardFilters(view, 4)).toEqual({ tracker: 1, archived: true, group_by: 'sprint', group: 'sprint:4' });
    expect(boardFilters(view, 'none')).toEqual({ tracker: 1, archived: true, group_by: 'sprint', group: 'none' });
  });

  it('keeps a sprint scope only while the sprint is open', () => {
    const all = [bucket({ id: 4 }), bucket({ id: 3, state: 'closed' })];
    expect(liveScope(4, all)).toBe(4);
    expect(liveScope(3, all)).toBe('all');
    expect(liveScope(9, all)).toBe('all');
    expect(liveScope(9, null)).toBe(9);
    expect(liveScope('none', all)).toBe('none');
  });
});
