import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RoutinesPanel from './RoutinesPanel.svelte';
import RoutineFailures from './RoutineFailures.svelte';
import { hosts } from '../hosts';
import { projects } from '../projects';
import { get } from 'svelte/store';
import { destination } from '../destination';
import { automationTab } from '../automation';
import { clearToasts, toasts } from '../toasts';
import { deviceOffsetMin, failing, nextRunLabel, zoneChoices, zoneOffsetMin, type RoutineRow, type RoutineRunRow } from '../routines';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const sweep: RoutineRow = {
  id: 3,
  name: 'Morning PR sweep',
  enabled: true,
  trigger: 'cron',
  cron: '30 7 * * 1-5',
  utc_offset_min: 120,
  host_alias: 'mac',
  project_id: 1,
  prompt: 'Review every open PR. Never push.',
  budget_run_micros: 2_000_000,
  overlap: 'skip',
  skip_next: false,
  created_at: 1,
  updated_at: 1,
};
const ok: RoutineRunRow = { id: 2, routine_id: 3, trigger: 'cron', state: 'done', outcome: 'did_work', cost_micros: 380_000, started_at: 1000, finished_at: 1340 };
const bad: RoutineRunRow = { id: 1, routine_id: 3, trigger: 'cron', state: 'failed', reason: 'gh token expired on mac', cost_micros: 10_000, started_at: 900, finished_at: 912 };

const argsOf = (action: string) =>
  inv.mock.calls.map((c) => c[1]?.args).filter((a) => a?.action === action).at(-1);

/** What `preview` answers in these tests (G2.3's dry run). */
let previewAnswer: Record<string, unknown> = { next_runs: [], utc_offset_min: 0, logins: [] };

function route(list: RoutineRow[] = [sweep], runs: RoutineRunRow[] = [ok, bad], failingNow: unknown[] = [], account?: unknown) {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a: { args: { action: string; routine?: { name: string } } }) => {
    if (cmd !== 'routines') return null;
    switch (a.args.action) {
      case 'list':
        return list;
      case 'get':
        return { routine: list[0], runs, may_change: true, account };
      case 'failing':
        return failingNow;
      case 'save':
        return { ...sweep, id: 8, name: a.args.routine!.name };
      case 'preview':
        return previewAnswer;
      default:
        return list[0];
    }
  });
}

beforeEach(() => {
  hosts.set([{ alias: 'mac', hidden: false } as never]);
  projects.set([{ project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', system: false }, worktrees: [] } as never]);
  failing.set([]);
  previewAnswer = { next_runs: [], utc_offset_min: 0, logins: [] };
  route();
});

describe('Routines (8.6)', () => {
  it('lists the routines and shows one with its runs, a failed one carrying Fix, Retry and Pause', async () => {
    render(RoutinesPanel);
    expect((await screen.findByTestId('routine-title')).textContent).toBe('Morning PR sweep');
    const row = screen.getByTestId('routine-row');
    expect(row.textContent).toContain('07:30');
    expect(row.textContent).toContain('Weekdays');
    const runs = screen.getAllByTestId('routine-run');
    expect(runs.map((r) => r.dataset.state)).toEqual(['done', 'failed']);
    expect(runs[1].textContent).toContain('Failed: gh token expired on mac');
    expect(screen.getAllByTestId('routine-run-fix')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('routine-run-retry'));
    await waitFor(() => expect(argsOf('run_now')).toEqual({ action: 'run_now', routine_id: 3 }));
    await fireEvent.click(screen.getByTestId('routine-run-pause'));
    await waitFor(() => expect(argsOf('set_enabled')).toEqual({ action: 'set_enabled', routine_id: 3, enabled: false }));
  });

  it('says which account the routine runs as, from the flat wire shape (review r05 F4)', async () => {
    // fleet-core `LoginAccount` flattens its `HostLogin`: no nested `login`.
    route([sweep], [ok], [], { host_alias: 'mac', profile: null, account_uuid: 'abcdef1234', used_pct: 12, email: 'me@x.com', over: false });
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    expect(document.querySelector('.kicker')?.textContent).toContain('runs as me@x.com on mac');
  });

  it("marks a run outcome Jev read from the screen, and only that one (8.10)", async () => {
    const quiet: RoutineRunRow = { ...ok, id: 4, outcome: 'nothing', outcome_source: 'jev' };
    route([sweep], [quiet, { ...ok, outcome_source: 'rule' }]);
    render(RoutinesPanel);
    const runs = await screen.findAllByTestId('routine-run');
    expect(runs[0].textContent).toContain('Nothing to do');
    const tags = screen.getAllByTestId('routine-run-jev');
    expect(tags).toHaveLength(1);
    expect(runs[0].contains(tags[0])).toBe(true);
    expect(tags[0].getAttribute('title')).toMatch(/last screen/);
  });

  it('a run Jev read as nothing to do is kept out of Inbox, with Change and Send to Inbox (G7.9)', async () => {
    const quiet: RoutineRunRow = { ...ok, id: 4, outcome: 'nothing', outcome_source: 'jev' };
    route([sweep], [quiet]);
    render(RoutinesPanel);
    const line = await screen.findByTestId('routine-run-nothing');
    expect(line.textContent).toContain('kept out of Inbox');
    expect(line.textContent).toContain('Proposed by Jev');
    await fireEvent.click(screen.getByTestId('routine-run-to-inbox'));
    await waitFor(() =>
      expect(argsOf('set_run_outcome')).toEqual({ action: 'set_run_outcome', routine_id: 3, run_id: 4, outcome: 'needs_person' }),
    );
    await fireEvent.click(screen.getByTestId('routine-run-change'));
    const pick = screen.getByTestId('routine-run-outcome') as HTMLSelectElement;
    expect(Array.from(pick.options).map((o) => o.textContent)).toEqual(['Did work', 'Nothing to do', 'Needs you']);
    await fireEvent.change(pick, { target: { value: 'did_work' } });
    await waitFor(() => expect(argsOf('set_run_outcome').outcome).toBe('did_work'));
  });

  it('starts a new routine from the Morning PR sweep template and saves it whole', async () => {
    route([]);
    render(RoutinesPanel);
    expect(await screen.findByTestId('routines-empty')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep');
    // The schedule is a picker now (G2.3): Weekdays at 07:30.
    expect((screen.getByTestId('routine-days') as HTMLSelectElement).value).toBe('weekdays');
    expect((screen.getByTestId('routine-time') as HTMLInputElement).value).toBe('07:30');
    expect((screen.getByTestId('routine-prompt') as HTMLTextAreaElement).value).toContain('martin-janci/claude-fleet');
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    expect(argsOf('save').routine).toMatchObject({
      name: 'Morning PR sweep',
      trigger: 'cron',
      cron: '30 7 * * 1-5',
      host_alias: 'mac',
      project_id: 1,
      budget_run_micros: 2_000_000,
      budget_day_micros: 5_000_000,
      overlap: 'skip',
      enabled: true,
    });
  });

  it('shows a routine the scheduler paused with why, and its limits', async () => {
    route([{ ...sweep, enabled: false, paused_reason: 'over its budget: $2.40 of the run’s $2.00' }]);
    render(RoutinesPanel);
    expect((await screen.findByTestId('routine-state')).textContent).toBe('Paused by fleet');
    expect(screen.getByTestId('routine-paused-reason').textContent).toContain('over its budget');
    expect(screen.queryByTestId('routine-run-pause')).toBeNull();
    await fireEvent.click(screen.getByTestId('routine-tab-limits'));
    expect(screen.getByTestId('routine-limits').textContent).toContain('$2.00');
  });
});

describe('the Automation board (list, detail, inspector)', () => {
  const now = Math.floor(Date.now() / 1000);
  const loops = [
    { name: 'catalog_scan', label: 'Catalog refresh', pausable: true, last_run_at: now - 60, next_run_at: now + 3600, result: 'ok', runs: 9, failures: 0 },
    { name: 'reconcile', label: 'Reconcile', pausable: false, last_run_at: now - 120, result: 'error', last_error: 'ssh: refused', runs: 4, failures: 1 },
  ];

  it('lists yours, then the built-in routines, and opens a built-in one', async () => {
    render(RoutinesPanel, { props: { loops, nowSec: now } });
    await screen.findByTestId('routine-title');
    expect(screen.getByTestId('routine-section-yours').textContent).toContain('Yours');
    expect(screen.getByTestId('routine-section-built-in').textContent).toContain('Built in');
    const builtIn = screen.getAllByTestId('automation-loop');
    expect(builtIn.map((r) => r.dataset.loop)).toEqual(['catalog_scan', 'reconcile']);
    await fireEvent.click(builtIn[1]);
    const d = await screen.findByTestId('automation-loop-detail');
    expect(d.textContent).toContain('Reconcile');
    expect(d.textContent).toContain('keeps running on Pause all');
    expect(screen.getByTestId('automation-loop-error').textContent).toBe('ssh: refused');
  });

  it('Filters narrow the list, and Group by state puts the failed first', async () => {
    render(RoutinesPanel, { props: { loops, nowSec: now } });
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-filters'));
    await fireEvent.click(screen.getByTestId('routine-filter-failed'));
    expect(screen.queryAllByTestId('routine-row')).toHaveLength(0);
    expect(screen.getAllByTestId('automation-loop').map((r) => r.dataset.loop)).toEqual(['reconcile']);
    await fireEvent.click(screen.getByTestId('routine-filter-failed'));
    await fireEvent.click(screen.getByTestId('routine-group'));
    expect(screen.getByTestId('routine-group').textContent).toContain('state');
    expect(document.querySelector('[data-testid^="routine-section-"]')?.textContent).toContain('Failed');
  });

  it('the inspector says the limits, with the day spent against its budget, and Duplicate saves a copy', async () => {
    route([{ ...sweep, budget_day_micros: 5_000_000 }], [{ ...ok, started_at: now - 60, finished_at: now }]);
    render(RoutinesPanel, { props: { nowSec: now } });
    const ins = await screen.findByTestId('routine-inspector');
    expect(ins.textContent).toContain('$2.00');
    expect(ins.textContent).toContain('$0.38 of $5.00');
    expect(ins.textContent).toContain('Skip if the last run is still going');
    await fireEvent.click(screen.getByTestId('routine-duplicate'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep copy');
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    expect(argsOf('save').routine_id).toBeUndefined();
    expect(argsOf('save').routine.name).toBe('Morning PR sweep copy');
  });

  it('a run reads its state in words: needs you and nothing to do', async () => {
    route([sweep], [
      { ...ok, id: 5, outcome: 'needs_person' },
      { ...ok, id: 6, outcome: 'nothing' },
    ]);
    render(RoutinesPanel, { props: { nowSec: now } });
    const runs = await screen.findAllByTestId('routine-run');
    expect(runs[0].textContent).toContain('Needs you');
    expect(runs[0].textContent).toContain('waits for you in the Inbox');
    expect(runs[1].textContent).toContain('Nothing to do');
    expect(screen.getByTestId('routine-stats').textContent).toContain('2 OK');
  });
});

describe('the Inbox block', () => {
  it('lists each failed routine with Fix, Retry and Pause, and opens the Routines', async () => {
    const f = { routine: sweep, run: bad, may_change: true };
    route([sweep], [ok, bad], [f]);
    failing.set([f]);
    render(RoutineFailures);
    const item = await screen.findByTestId('routine-failure');
    expect(item.textContent).toContain('Morning PR sweep');
    expect(item.textContent).toContain('Failed: gh token expired on mac');
    expect(screen.getByTestId('routine-failure-pause')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('routine-failure-retry'));
    await waitFor(() => expect(argsOf('run_now')).toEqual({ action: 'run_now', routine_id: 3 }));
    // Fix on a run with no session opens the routine's definition, in
    // Automation's Routines tab (8.4).
    await fireEvent.click(screen.getByTestId('routine-failure-fix'));
    expect(get(destination)).toBe('automation');
    expect(get(automationTab)).toBe('routines');
    render(RoutinesPanel);
    expect(await screen.findByTestId('routine-definition')).toBeInTheDocument();
  });

  it('shows nothing when no routine failed', () => {
    render(RoutineFailures);
    expect(screen.queryByTestId('routine-failures')).toBeNull();
  });
});

describe('deleting a routine (G1.4, the destructive confirm)', () => {
  const many: RoutineRunRow[] = Array.from({ length: 6 }, (_, i) => ({ ...ok, id: 10 + i }));

  it('a routine with many runs asks for its name, and Delete routine sends delete only then', async () => {
    route([sweep], many);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    const dialog = await screen.findByTestId('destructive-confirm');
    expect(dialog.textContent).toContain('Delete routine "Morning PR sweep"?');
    expect(dialog.textContent).toContain('Its 6 runs go with it. Sessions it started keep running.');
    const go = screen.getByTestId('routine-delete-confirm') as HTMLButtonElement;
    expect(go.disabled).toBe(true);
    await fireEvent.input(screen.getByTestId('destructive-typed-name'), { target: { value: 'Morning PR sweep' } });
    expect(go.disabled).toBe(false);
    await fireEvent.click(go);
    await waitFor(() => expect(argsOf('delete')).toEqual({ action: 'delete', routine_id: 3 }));
    await waitFor(() => expect(screen.queryByTestId('destructive-confirm')).toBeNull());
  });

  it('a routine with few runs asks with the red verb alone', async () => {
    route([sweep], [ok]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    expect((await screen.findByTestId('destructive-confirm')).textContent).toContain('Its run goes with it.');
    expect(screen.queryByTestId('destructive-typed-name')).toBeNull();
    await fireEvent.click(screen.getByTestId('routine-delete-confirm'));
    await waitFor(() => expect(argsOf('delete')).toEqual({ action: 'delete', routine_id: 3 }));
  });

  it('offers Pause it instead on the left for a routine that is on, and pauses without deleting', async () => {
    route([sweep], many);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    await fireEvent.click(await screen.findByTestId('routine-delete-pause'));
    await waitFor(() => expect(argsOf('set_enabled')).toEqual({ action: 'set_enabled', routine_id: 3, enabled: false }));
    expect(argsOf('delete')).toBeUndefined();
    expect(screen.queryByTestId('destructive-confirm')).toBeNull();
  });

  it('offers no Pause for a routine already paused', async () => {
    route([{ ...sweep, enabled: false }], [ok]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    await screen.findByTestId('destructive-confirm');
    expect(screen.queryByTestId('routine-delete-pause')).toBeNull();
  });

  it('a failed delete stays open with the failure as its banner', async () => {
    route([sweep], [ok]);
    const base = inv.getMockImplementation() as (cmd: string, a: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a: { args: { action: string } }) => {
      if (cmd === 'routines' && a.args.action === 'delete') throw { code: 'E_FORBIDDEN', message: 'you are a Viewer in 32bit' };
      return base(cmd, a);
    });
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    await fireEvent.click(await screen.findByTestId('routine-delete-confirm'));
    expect((await screen.findByTestId('destructive-confirm-error')).textContent).toContain('The hub refused this');
    expect(screen.getByTestId('destructive-confirm')).toBeTruthy();
  });
});

describe('no routines yet (G3.13)', () => {
  it('the empty state starts a blank routine or the template, right there', async () => {
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routines-empty-template'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep');
  });

  it('+ New routine opens a blank one', async () => {
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routines-empty-new'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('');
    expect(screen.getByTestId('routine-editor')).toBeTruthy();
  });
});

describe('the routine editor (G2.3: schedule picker, next run, account, dry run, Run once now)', () => {
  // Monday 12 October 2026, 06:30 UTC.
  const mon = Date.UTC(2026, 9, 12, 6, 30) / 1000;

  it('picks a schedule in words, shows the next run from the dry run, and saves the cron line it means', async () => {
    previewAnswer = { next_runs: [mon, mon + 86_400], utc_offset_min: deviceOffsetMin(), logins: [] };
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-blank'));
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.change(screen.getByTestId('routine-days'), { target: { value: '5' } });
    await fireEvent.input(screen.getByTestId('routine-time'), { target: { value: '16:15' } });
    await waitFor(() => expect(argsOf('preview')?.routine.cron).toBe('15 16 * * 5'));
    await waitFor(() => expect(screen.getByTestId('routine-next-run').textContent).toContain(`next run ${nextRunLabel(mon)}`));
    expect(screen.getByTestId('routine-next-run').textContent).toContain('Fridays 16:15');
    expect(screen.getByTestId('routine-zone').textContent).toContain('Time zone');
    // Custom shows the line itself.
    await fireEvent.change(screen.getByTestId('routine-days'), { target: { value: 'custom' } });
    await fireEvent.input(screen.getByTestId('routine-cron'), { target: { value: '0 */2 * * *' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')?.routine.cron).toBe('0 */2 * * *'));
  });

  it('picks the Account from the host logins, apart from the Profile, and the dry run names it', async () => {
    previewAnswer = {
      next_runs: [mon],
      utc_offset_min: deviceOffsetMin(),
      account: { host_alias: 'mac', profile: 'work', account_uuid: 'w-1', email: 'work@x.com', over: false },
      logins: [
        { host_alias: 'mac', profile: null, account_uuid: 'o-1', email: 'me@x.com', over: false },
        { host_alias: 'mac', profile: 'work', account_uuid: 'w-1', email: 'work@x.com', over: false },
      ],
    };
    route([sweep]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    const acct = screen.getByTestId('routine-account') as HTMLSelectElement;
    await waitFor(() => expect(Array.from(acct.options).map((o) => o.textContent)).toEqual(["me@x.com · the host's own login", 'work@x.com · profile work']));
    await fireEvent.change(acct, { target: { value: 'work' } });
    expect((screen.getByTestId('routine-profile') as HTMLInputElement).value).toBe('work');
    await waitFor(() => expect(argsOf('preview')?.routine.profile).toBe('work'));
    await waitFor(() =>
      expect(screen.getByTestId('routine-dry-run').textContent).toContain('Dry run: on mac, in martin-janci/claude-fleet, as work@x.com, at most $2.00 a run.'),
    );
    expect(argsOf('preview').routine_id).toBe(3);
  });

  it('the dry run says what save would refuse, and Run once now waits for a routine that would save', async () => {
    previewAnswer = { next_runs: [], utc_offset_min: 0, problem: 'host gone not found', logins: [] };
    route([sweep]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    await waitFor(() => expect(screen.getByTestId('routine-dry-run').textContent).toContain('it would not save: host gone not found'));
    expect((screen.getByTestId('routine-run-once') as HTMLButtonElement).disabled).toBe(true);
  });

  it('Run once now runs the saved routine unchanged; a changed one is saved first', async () => {
    previewAnswer = { next_runs: [mon], utc_offset_min: deviceOffsetMin(), logins: [] };
    route([sweep]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    const once = screen.getByTestId('routine-run-once') as HTMLButtonElement;
    await waitFor(() => expect(once.disabled).toBe(false));
    expect(once.textContent).toBe('Run once now');
    await fireEvent.click(once);
    await waitFor(() => expect(argsOf('run_now')).toEqual({ action: 'run_now', routine_id: 3 }));
    expect(argsOf('save')).toBeUndefined();

    await fireEvent.click(screen.getByTestId('routine-edit'));
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Only CI.' } });
    const again = screen.getByTestId('routine-run-once') as HTMLButtonElement;
    expect(again.textContent).toBe('Save and run once');
    await waitFor(() => expect(again.disabled).toBe(false));
    inv.mockClear();
    await fireEvent.click(again);
    await waitFor(() => expect(argsOf('run_now')).toEqual({ action: 'run_now', routine_id: 8 }));
    expect(argsOf('save')).toMatchObject({ routine_id: 3, routine: { prompt: 'Only CI.' } });
  });
});

describe('the routine editor (G7.4: time zone select, Undo on save)', () => {
  it('a zone picked in the select is saved with its own offset', async () => {
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    const select = screen.getByTestId('routine-zone-select') as HTMLSelectElement;
    // A routine saved before zones were stored opens on this device's.
    expect(select.value).toBe('');
    await fireEvent.change(select, { target: { value: 'Asia/Tokyo' } });
    expect(screen.getByTestId('routine-zone').textContent).toContain('Asia/Tokyo · saved as UTC+09:00');
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    expect(argsOf('save').routine).toMatchObject({ time_zone: 'Asia/Tokyo', utc_offset_min: zoneOffsetMin(Date.now() / 1000, 'Asia/Tokyo') });
  });

  it('zoneChoices always holds UTC, the device zone and the one to keep', () => {
    const z = zoneChoices('Mars/Olympus');
    expect(z).toContain('UTC');
    expect(z).toContain('Mars/Olympus');
    expect(z).toEqual([...z].sort((a, b) => a.localeCompare(b)));
  });

  it('Undo after an edit saves the routine back as it was', async () => {
    clearToasts();
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Evening sweep' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(get(toasts).some((t) => t.action?.label === 'Undo')).toBe(true));
    get(toasts).find((t) => t.action?.label === 'Undo')!.action!.run();
    await waitFor(() => expect(argsOf('save').routine.name).toBe('Morning PR sweep'));
    expect(argsOf('save')).toMatchObject({ routine_id: 3, routine: { cron: '30 7 * * 1-5', utc_offset_min: 120 } });
  });

  it('Undo after creating one deletes it', async () => {
    clearToasts();
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-blank'));
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(get(toasts).some((t) => t.action?.label === 'Undo')).toBe(true));
    get(toasts).find((t) => t.action?.label === 'Undo')!.action!.run();
    await waitFor(() => expect(argsOf('delete')).toMatchObject({ routine_id: 8 }));
  });
});
