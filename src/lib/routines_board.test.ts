import { describe, it, expect } from 'vitest';
import {
  averageRun,
  cronParts,
  lastRunByRoutine,
  nextRunWords,
  routineDot,
  routineLine,
  runDot,
  spentSince,
  type RoutineRow,
  type RoutineRunRow,
} from './routines';
import type { RunRow } from './runs';

const now = 1_000_000;
const r: RoutineRow = {
  id: 1,
  name: 'Morning PR sweep',
  enabled: true,
  trigger: 'cron',
  cron: '30 7 * * 1-5',
  utc_offset_min: 0,
  host_alias: 'mercury',
  project_id: 1,
  prompt: '',
  overlap: 'skip',
  skip_next: false,
  next_run_at: now + 18 * 3600,
  created_at: 1,
  updated_at: 1,
};
const run = (o: Partial<RunRow>): RunRow => ({ id: 'routine:1', source: 'routine', kind: 'routine', owner: 'x', started_at: 0, outcome: 'ok', session_ids: [], routine_id: 1, ...o });
const rr = (o: Partial<RoutineRunRow>): RoutineRunRow => ({ id: 1, routine_id: 1, trigger: 'cron', state: 'done', cost_micros: 0, started_at: 0, ...o });

describe('the Automation list (board Automation)', () => {
  it('splits a cron line into days and time', () => {
    expect(cronParts('30 7 * * 1-5')).toEqual({ days: 'Weekdays', at: '07:30' });
    expect(cronParts('0 16 * * 5')).toEqual({ days: 'Fridays', at: '16:00' });
    expect(cronParts('@hourly')).toEqual({ days: 'Hourly' });
  });

  it('keeps each routine’s newest run', () => {
    const m = lastRunByRoutine([run({ started_at: 5 }), run({ started_at: 9, outcome: 'failed' }), run({ routine_id: undefined })]);
    expect(m.size).toBe(1);
    expect(m.get(1)?.outcome).toBe('failed');
  });

  it('says the row line the way the board does', () => {
    expect(routineLine(r, run({ outcome: 'ok' }), now)).toBe('Weekdays · last run OK · next in 18h');
    expect(routineLine(r, run({ outcome: 'running' }), now)).toBe('Weekdays · running now on mercury');
    expect(routineLine({ ...r, enabled: false }, undefined, now)).toBe('Weekdays 07:30 · paused by you');
    expect(routineLine({ ...r, enabled: false, paused_reason: 'budget' }, undefined, now)).toBe('Weekdays 07:30 · paused by fleet');
    expect(routineLine({ ...r, skip_next: true }, undefined, now)).toBe('Weekdays · not run yet · next one skipped');
  });

  it('colours the dot from the newest run, then the switch', () => {
    expect(routineDot(r, { outcome: 'running' })).toBe('working');
    expect(routineDot(r, { outcome: 'failed' })).toBe('failed');
    expect(routineDot(r, { outcome: 'needs_person' })).toBe('waiting');
    expect(routineDot(r)).toBe('done');
    expect(routineDot({ enabled: false })).toBe('idle');
    expect(runDot(rr({ state: 'skipped' }))).toBe('idle');
    expect(runDot(rr({ outcome: 'failed' }))).toBe('failed');
  });

  it('averages the finished runs and says when it runs next', () => {
    expect(averageRun([rr({ cost_micros: 400_000, finished_at: 300 }), rr({ cost_micros: 440_000, finished_at: 420 }), rr({ state: 'running' })])).toBe(
      '$0.42 · 6 min',
    );
    expect(averageRun([])).toBeNull();
    expect(nextRunWords(r, now)).toBe('in 18h');
    expect(nextRunWords({ ...r, enabled: false }, now)).toBe('Paused');
    expect(nextRunWords({ ...r, skip_next: true }, now)).toBe('Next one skipped');
    expect(nextRunWords({ ...r, trigger: 'manual' }, now)).toBe('Only with Run now');
    expect(spentSince([rr({ cost_micros: 5, started_at: 10 }), rr({ cost_micros: 7, started_at: 1 })], 5)).toBe(5);
  });
});
