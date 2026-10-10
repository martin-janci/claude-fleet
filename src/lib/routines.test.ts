import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { destination } from './destination';
import { automationTab } from './automation';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  routineDeleteLoss,
  ROUTINE_RUNS_SHOWN,
  cronWords,
  failing,
  fixRoutine,
  microsOf,
  morningPrSweep,
  pauseRoutine,
  retryRoutine,
  routineAccountLabel,
  routineStateWords,
  routinesRequest,
  runSourceHint,
  runWords,
  trackFailingRoutines,
  type FailingRoutine,
  type RoutineRow,
} from './routines';
import { inboxCount } from './inbox';
import { sessions } from './sessions';
import { selectedSession } from './selection';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const routine = (over: Partial<RoutineRow> = {}): RoutineRow => ({
  id: 3,
  name: 'Morning PR sweep',
  enabled: true,
  trigger: 'cron',
  cron: '30 7 * * 1-5',
  utc_offset_min: 120,
  host_alias: 'mac',
  project_id: 1,
  prompt: 'Review',
  overlap: 'skip',
  skip_next: false,
  created_at: 1,
  updated_at: 1,
  ...over,
});
const failed = (over: Partial<FailingRoutine> = {}): FailingRoutine => ({
  routine: routine(),
  run: { id: 9, routine_id: 3, trigger: 'cron', state: 'failed', reason: 'host unreachable', cost_micros: 10_000, started_at: 100, finished_at: 112 },
  may_change: true,
  ...over,
});

beforeEach(() => {
  inv.mockReset();
  failing.set([]);
  sessions.set([]);
  routinesRequest.set(null);
  destination.set('session');
});

describe('words', () => {
  it('says a cron line in words, and shows one it does not know as it is', () => {
    expect(cronWords('30 7 * * 1-5')).toBe('Weekdays 07:30');
    expect(cronWords('0 2 * * *')).toBe('Daily 02:00');
    expect(cronWords('0 16 * * 5')).toBe('Fridays 16:00');
    expect(cronWords('15 * * * *')).toBe('Hourly at :15');
    expect(cronWords('*/5 * * * *')).toBe('*/5 * * * *');
  });

  it('says a run in the status words and a routine by its switch', () => {
    expect(runWords({ state: 'failed', reason: 'gh token expired on mac' })).toBe('Failed: gh token expired on mac');
    expect(runWords({ state: 'done', outcome: 'needs_person' })).toBe('Needs you');
    expect(runWords({ state: 'done', outcome: 'nothing' })).toBe('Nothing to do');
    expect(runWords({ state: 'running' })).toBe('Working');
    expect(runSourceHint({ outcome: 'nothing', outcome_source: 'jev' })).toMatch(/^Jev read this/);
    expect(runSourceHint({ outcome: 'did_work', outcome_source: 'rule' })).toBeUndefined();
    expect(runSourceHint({ outcome_source: 'jev' })).toBeUndefined();
    expect(routineStateWords({ enabled: false, paused_reason: 'over its budget' })).toBe('Paused by fleet');
    expect(routineStateWords({ enabled: false })).toBe('Paused');
  });

  it('names the account a routine runs as from the flat LoginAccount (review r05 F4)', () => {
    const a = { host_alias: 'mac', account_uuid: 'abcdef1234567', email: 'me@x.com', over: false };
    expect(routineAccountLabel({ ...a, profile: 'work' })).toBe('work');
    expect(routineAccountLabel({ ...a, profile: null }, { nickname: 'Silvester', email: null })).toBe('Silvester');
    expect(routineAccountLabel({ ...a, profile: null })).toBe('me@x.com');
    expect(routineAccountLabel({ ...a, email: undefined })).toBe('abcdef12');
  });

  it('reads dollars a person types, and nothing as no limit', () => {
    expect(microsOf('2')).toBe(2_000_000);
    expect(microsOf('$0.50')).toBe(500_000);
    expect(microsOf('')).toBeUndefined();
    expect(microsOf('lots')).toBeUndefined();
  });
});

describe('Morning PR sweep', () => {
  it('runs weekdays at 07:30 on the device clock, with limits, and never pushes', () => {
    const t = morningPrSweep('mac', 4, 'martin-janci/claude-fleet');
    expect(t).toMatchObject({ name: 'Morning PR sweep', trigger: 'cron', cron: '30 7 * * 1-5', host_alias: 'mac', project_id: 4, overlap: 'skip' });
    expect(t.utc_offset_min).toBe(-new Date().getTimezoneOffset());
    expect(t.prompt).toContain('martin-janci/claude-fleet');
    expect(t.prompt).toContain('Never push');
    expect([t.budget_run_micros, t.budget_day_micros]).toEqual([2_000_000, 5_000_000]);
  });
});

describe('the Inbox', () => {
  it('a failed run raises the badge', () => {
    expect(get(inboxCount)).toBe(0);
    failing.set([failed()]);
    expect(get(inboxCount)).toBe(1);
  });

  it('Retry runs it now and Pause switches it off, each re-reading the list', async () => {
    inv.mockImplementation(async (_c: string, a: { args: { action: string } }) => (a.args.action === 'failing' ? [] : {}));
    failing.set([failed()]);
    await retryRoutine(failed());
    expect(inv.mock.calls.map((c) => c[1].args)).toEqual([{ action: 'run_now', routine_id: 3 }, { action: 'failing' }]);
    expect(get(failing)).toEqual([]);
    inv.mockClear();
    await pauseRoutine(failed());
    expect(inv.mock.calls[0][1].args).toEqual({ action: 'set_enabled', routine_id: 3, enabled: false });
  });

  it('Fix opens the run\'s session, or the definition when it never started one', () => {
    sessions.set([{ id: 77, host_alias: 'mac', tmux_name: 'r-1', status: 'running' } as never]);
    expect(fixRoutine(failed({ run: { ...failed().run, session_id: 77 } }))).toBe('session');
    expect(get(selectedSession)?.id).toBe(77);
    expect(fixRoutine(failed())).toBe('definition');
    expect(get(routinesRequest)).toMatchObject({ select: 3, tab: 'definition' });
    // The definition opens in Automation's Routines tab (8.4).
    expect(get(destination)).toBe('automation');
    expect(get(automationTab)).toBe('routines');
  });

  it('keeps the list fresh, and stops asking an older hub', async () => {
    vi.useFakeTimers();
    const load = vi.fn().mockResolvedValue({ ok: true, value: [failed()] });
    const stop = trackFailingRoutines({ load, doc: null, every: 1000 });
    await vi.advanceTimersByTimeAsync(0);
    expect(get(failing)).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(load).toHaveBeenCalledTimes(2);
    stop();
    const old = vi.fn().mockResolvedValue({ ok: false, error: { code: 'E_INVALID', message: 'action must be …' } });
    trackFailingRoutines({ load: old, doc: null, every: 1000 });
    await vi.advanceTimersByTimeAsync(5000);
    expect(old).toHaveBeenCalledTimes(1);
    expect(get(failing)).toEqual([]);
    vi.useRealTimers();
  });
});

describe('routineDeleteLoss (G1.4)', () => {
  const run = { id: 1, routine_id: 1, trigger: 'cron', state: 'done', cost_micros: 0, started_at: 1 } as const;
  it('counts the runs that go with it and says the sessions stay', () => {
    expect(routineDeleteLoss({ runs: [] })).toEqual({ loss: 0, lead: "It has no runs yet. Sessions it started keep running. This can't be undone." });
    expect(routineDeleteLoss({ runs: [run] }).lead).toMatch(/^Its run goes with it\./);
    expect(routineDeleteLoss({ runs: [run, run, run] })).toMatchObject({ loss: 3, lead: expect.stringMatching(/^Its 3 runs go with it\./) });
  });
  it('a full page of runs may be more: says "or more"', () => {
    const page = Array.from({ length: ROUTINE_RUNS_SHOWN }, () => run);
    expect(routineDeleteLoss({ runs: page }).lead).toMatch(/^Its 20 or more runs go with it\./);
  });
});
