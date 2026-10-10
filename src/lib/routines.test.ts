import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { destination } from './destination';
import { automationTab } from './automation';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import {
  autonomyWords,
  capSecsOf,
  capWords,
  fixLabel,
  hostWords,
  onFailureWords,
  startedByWords,
  routineDeleteLoss,
  ROUTINE_RUNS_SHOWN,
  clockChange,
  cronWords,
  dryRunLine,
  nextRunLabel,
  nextRoutineLine,
  soonestRoutine,
  offsetWords,
  scheduleCron,
  schedulePick,
  zoneOffsetMin,
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

describe('the schedule picker and its next run (G2.3)', () => {
  it('reads a cron line into days and time, and writes the line back', () => {
    expect(schedulePick('30 7 * * 1-5')).toEqual({ days: 'weekdays', time: '07:30' });
    expect(schedulePick('0 9 * * *')).toEqual({ days: 'daily', time: '09:00' });
    expect(schedulePick('0 16 * * 5')).toEqual({ days: '5', time: '16:00' });
    expect(schedulePick('0 10 * * 6,0')).toEqual({ days: 'weekends', time: '10:00' });
    expect(schedulePick('15 * * * *')).toEqual({ days: 'hourly', time: '15' });
    expect(schedulePick('0 */2 * * *').days).toBe('custom');
    expect(schedulePick('0 9 1 * *').days).toBe('custom');
    for (const line of ['30 7 * * 1-5', '0 9 * * *', '0 16 * * 5', '0 10 * * 0,6', '15 * * * *']) {
      const p = schedulePick(line);
      expect(scheduleCron(p.days, p.time)).toBe(line);
    }
    expect(scheduleCron('daily', '25:00')).toBeNull();
    expect(scheduleCron('custom', '09:00')).toBeNull();
  });

  // Europe/Bratislava leaves CEST (+02:00) for CET (+01:00) at 01:00 UTC
  // on Sunday 25 October 2026. The backend reads a line at the offset it
  // was saved at (fleet-core `routines::cron`), so a weekday 08:30 saved in
  // October fires at 06:30 UTC: 08:30 before the change, 07:30 after.
  const zone = 'Europe/Bratislava';
  const utc = (d: number, h: number, m: number) => Date.UTC(2026, 9, d, h, m) / 1000;
  const fires = [utc(23, 6, 30), utc(26, 6, 30), utc(27, 6, 30)];

  it('Next-run test across DST: names the first fire the clock change moves, and by how much', () => {
    expect(zoneOffsetMin(utc(23, 12, 0), zone)).toBe(120);
    expect(zoneOffsetMin(utc(26, 12, 0), zone)).toBe(60);
    expect(nextRunLabel(fires[0], zone)).toBe('Fri 23 Oct, 08:30');
    expect(nextRunLabel(fires[1], zone)).toBe('Mon 26 Oct, 07:30');
    expect(clockChange(fires, 120, zone)).toEqual({ at: fires[1], shiftMin: -60 });
    // Saved again after the change (+60): the same wall time, nothing moves.
    const winter = [utc(26, 7, 30), utc(27, 7, 30)];
    expect(winter.map((f) => nextRunLabel(f, zone))).toEqual(['Mon 26 Oct, 08:30', 'Tue 27 Oct, 08:30']);
    expect(clockChange(winter, 60, zone)).toBeNull();
    // And into summer time (29 March 2027): a line saved at +60 fires an hour later on the wall.
    const spring = [Date.UTC(2027, 2, 26, 7, 30) / 1000, Date.UTC(2027, 2, 29, 7, 30) / 1000];
    expect(clockChange(spring, 60, zone)).toEqual({ at: spring[1], shiftMin: 60 });
    expect(nextRunLabel(spring[1], zone)).toBe('Mon 29 Mar, 09:30');
  });

  it('says the offset a line is saved at, and the dry run in one line', () => {
    expect(offsetWords(120)).toBe('UTC+02:00');
    expect(offsetWords(-330)).toBe('UTC−05:30');
    expect(dryRunLine({ host_alias: 'mercury', budget_run_micros: 2_000_000 }, 'acme/web', 'me@x.com')).toBe(
      'Dry run: on mercury, in acme/web, as me@x.com, at most $2.00 a run.',
    );
    expect(dryRunLine({ host_alias: 'mac', profile: 'work' }, 'acme/web', null)).toBe(
      'Dry run: on mac, in acme/web, as profile work, no limit a run.',
    );
  });
});

describe('automation guards in words (G3.8)', () => {
  it('a time cap, a host with its fallback, autonomy and what started a retry', () => {
    expect(capWords(20 * 60)).toBe('20 min');
    expect(capWords(2 * 3600)).toBe('2 h');
    expect(capWords(90 * 60)).toBe('1 h 30 min');
    expect(capSecsOf('')).toBeUndefined();
    expect(capSecsOf('20')).toBe(1200);
    expect(capSecsOf('nope')).toBeUndefined();
    expect(hostWords({ host_alias: 'mac', fallback_host: 'nas' })).toBe('mac, else nas');
    expect(hostWords({ host_alias: 'mac' })).toBe('mac');
    expect(autonomyWords(0)).toBe('L0 · reports only');
    expect(autonomyWords(undefined)).toBe('L2 · pushes and opens PRs');
    expect(onFailureWords({ retry_once: true })).toMatch(/^Retry once/);
    expect(startedByWords({ trigger: 'cron', trigger_ref: 'retry:4' })).toBe('its schedule, retried once after run 4 failed');
    expect(startedByWords({ trigger: 'event', trigger_ref: 'pr:2:9' })).toBe('an event (pr:2:9)');
    expect(fixLabel(undefined)).toBe('Fix');
  });
});

describe('the calm Inbox names the next routine (G7.15)', () => {
  const at = (iso: string) => Date.parse(iso) / 1000;
  const row = (name: string, next: string | undefined, over: Record<string, unknown> = {}) => ({
    name,
    enabled: true,
    trigger: 'cron',
    skip_next: false,
    next_run_at: next ? at(next) : undefined,
    ...over,
  });

  it('picks the enabled, scheduled routine that runs first', () => {
    const now = at('2026-10-10T06:00:00Z');
    const rows = [
      row('Nightly', '2026-10-10T22:00:00Z'),
      row('Morning PR sweep', '2026-10-10T07:30:00Z'),
      row('Paused', '2026-10-10T06:30:00Z', { enabled: false }),
      row('Skipped', '2026-10-10T06:31:00Z', { skip_next: true }),
      row('On a PR', '2026-10-10T06:32:00Z', { trigger: 'event' }),
      row('Past', '2026-10-10T05:00:00Z'),
    ];
    expect(soonestRoutine(rows, now)?.name).toBe('Morning PR sweep');
    expect(soonestRoutine([row('Off', '2026-10-10T07:00:00Z', { enabled: false })], now)).toBeNull();
  });

  it('says the time for today, and the day for later', () => {
    const now = at('2026-10-10T06:00:00Z');
    expect(nextRoutineLine(row('Morning PR sweep', '2026-10-10T07:30:00Z'), now, 'UTC')).toBe(
      'The next routine is Morning PR sweep at 07:30.',
    );
    expect(nextRoutineLine(row('Weekly', '2026-10-12T07:30:00Z'), now, 'UTC')).toBe(
      'The next routine is Weekly on Mon 12 Oct, 07:30.',
    );
  });
});
