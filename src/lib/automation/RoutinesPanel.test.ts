import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RoutinesPanel from './RoutinesPanel.svelte';
import RoutineFailures from './RoutineFailures.svelte';
import RoutineMemory from './RoutineMemory.svelte';
import { hosts } from '../hosts';
import { projects } from '../projects';
import { get } from 'svelte/store';
import { destination } from '../destination';
import { automationTab } from '../automation';
import { clearToasts, toasts } from '../toasts';
import { fleetSettings, SETTING_DEFAULTS } from '../fleet_settings';
import { deviceOffsetMin, failing, openRoutines, nextRunLabel, zoneChoices, zoneOffsetMin, type RoutineRow, type RoutineRunRow } from '../routines';
import { stashDraft, zoneOfSaved } from './routine_draft';

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

/** What `explain` and `dry_run` answer (the Debug tab). */
const explainAnswer = {
  routine_id: 3,
  name: 'Morning PR sweep',
  trigger: 'cron',
  summary: 'Does not fire: Pause all is on (automation.paused): the scheduler starts nothing; Run now still works.',
  state: 'block',
  checks: [
    { name: 'enabled', state: 'ok', detail: 'it is on' },
    { name: 'automation', state: 'block', detail: 'Pause all is on (automation.paused): the scheduler starts nothing; Run now still works' },
  ],
  decisions: [{ id: 9, routine_id: 3, at: 1000, trigger: 'cron', verdict: 'skipped', reason: 'a person skipped it', run_id: 4 }],
};
const dryAnswer = {
  routine_id: 3,
  starts: true,
  session: { host_alias: 'mac', project_id: 1 },
  prompt: 'Review every open PR. Never push.',
  scheduled_only: [{ name: 'automation', state: 'block', detail: 'Pause all is on' }],
};

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
      case 'explain':
        return explainAnswer;
      case 'dry_run':
        return dryAnswer;
      default:
        return list[0];
    }
  });
}

beforeEach(() => {
  // A changed draft an earlier test left open must not come back (review w05-2).
  stashDraft(null);
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

describe('the Debug tab (migration 186)', () => {
  it('says why a routine does not fire, lists its checks and trace, and dry-runs it', async () => {
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-tab-debug'));
    expect((await screen.findByTestId('routine-debug-summary')).textContent).toContain('Pause all is on');
    expect(argsOf('explain')).toEqual({ action: 'explain', routine_id: 3, limit: 50 });
    expect(screen.getByTestId('routine-check-automation').textContent).toContain('Pause all');
    expect(screen.getByTestId('routine-debug-decisions').textContent).toContain('Skipped · a person skipped it');
    await fireEvent.click(screen.getByTestId('routine-debug-dry-run'));
    expect((await screen.findByTestId('routine-dry-run-prompt')).textContent).toBe('Review every open PR. Never push.');
    expect(argsOf('dry_run')).toEqual({ action: 'dry_run', routine_id: 3 });
    const dry = screen.getByTestId('routine-dry-run').textContent ?? '';
    expect(dry).toContain('starts a session on mac');
    expect(dry).toContain('A scheduled fire also needs pause all: Pause all is on.');
    expect(argsOf('run_now')).toBeUndefined();
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

  it('the Filters menu closes on Esc, keeping the Esc from the page, and on a press outside (review w18-11)', async () => {
    render(RoutinesPanel, { props: { loops, nowSec: now } });
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-filters'));
    const item = screen.getByTestId('routine-filter-failed');
    item.focus();
    const esc = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    item.dispatchEvent(esc);
    await tick();
    expect(esc.defaultPrevented).toBe(true);
    expect(screen.queryByTestId('routine-filters-menu')).toBeNull();
    expect(document.activeElement).toBe(screen.getByTestId('routine-filters'));
    await fireEvent.click(screen.getByTestId('routine-filters'));
    expect(screen.getByTestId('routine-filters-menu')).toBeTruthy();
    await fireEvent.pointerDown(document.body);
    expect(screen.queryByTestId('routine-filters-menu')).toBeNull();
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

  it('a routine with many runs keeps them (G7.4), so it asks with the red verb alone', async () => {
    route([sweep], many);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-delete'));
    const dialog = await screen.findByTestId('destructive-confirm');
    expect(dialog.textContent).toContain('Delete routine "Morning PR sweep"?');
    expect(dialog.textContent).toContain('Its 6 runs stay in Runs under its name. Sessions it started keep running.');
    expect(screen.queryByTestId('destructive-typed-name')).toBeNull();
    const go = screen.getByTestId('routine-delete-confirm') as HTMLButtonElement;
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
    expect((await screen.findByTestId('destructive-confirm')).textContent).toContain('Its run stays in Runs under its name.');
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

  it('tunes the prompt on the routine host, unsaved, and writes it back only on Use this', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.prompt_tune': 'true' });
    try {
      route([]);
      const base = inv.getMockImplementation() as (cmd: string, a: unknown) => Promise<unknown>;
      inv.mockImplementation(async (cmd: string, a: { args: { action: string } }) =>
        cmd === 'routines' && a.args.action === 'tune_prompt'
          ? { prompt: 'Tidy up the repo. Done when CI is green.', notes: 'Said when done.', unchanged: false, model: 'haiku', host_alias: 'mac', at: 1 }
          : base(cmd, a),
      );
      render(RoutinesPanel);
      await screen.findByTestId('routines-empty');
      await fireEvent.click(screen.getByTestId('routine-new'));
      await fireEvent.click(screen.getByTestId('routine-template-blank'));
      await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
      await fireEvent.click(screen.getByTestId('routine-tune-open'));
      await fireEvent.keyDown(screen.getByTestId('routine-tune-ask'), { key: 'Enter' });
      await waitFor(() => expect(argsOf('tune_prompt')?.routine.prompt).toBe('Tidy up.'));
      expect(argsOf('tune_prompt')?.routine.host_alias).toBe('mac');
      await screen.findByTestId('routine-tune-accept');
      expect((screen.getByTestId('routine-prompt') as HTMLTextAreaElement).value).toBe('Tidy up.');
      await fireEvent.click(screen.getByTestId('routine-tune-accept'));
      expect((screen.getByTestId('routine-prompt') as HTMLTextAreaElement).value).toBe('Tidy up the repo. Done when CI is green.');
      expect(argsOf('save')).toBeUndefined();
      // Focus goes back to the prompt it tuned, not to the page.
      await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('routine-prompt')));
    } finally {
      fleetSettings.set({ ...SETTING_DEFAULTS });
    }
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
    await waitFor(() => expect(Array.from(acct.options).map((o) => o.textContent)).toEqual(['me@x.com · own login', 'work@x.com · profile work']));
    expect(acct.options[0].title).toBe("me@x.com · the host's own login");
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
    // A routine saved before zones were stored opens in the zone of its
    // offset, so saving does not move it (review w05-3).
    expect(select.value).toBe(zoneOfSaved({ utc_offset_min: 120 }).zone);
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

describe('the routine form (FormsAutomation board)', () => {
  async function openBlank() {
    route([]);
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routines-empty-new'));
  }

  it('says under the disabled verb what is still missing, and checks a field on blur', async () => {
    await openBlank();
    const save = screen.getByTestId('routine-save') as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    expect(screen.getByTestId('routine-save-why').textContent).toBe('Name the routine.');
    await fireEvent.blur(screen.getByTestId('routine-name'));
    expect(screen.getByText('A routine needs a name.')).toBeInTheDocument();
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    expect(screen.getByTestId('routine-save-why').textContent).toBe('Write the prompt.');
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    expect(screen.queryByTestId('routine-save-why')).toBeNull();
    expect(save.disabled).toBe(false);
  });

  it('Starts picks the trigger: a schedule, an event or Run now only', async () => {
    await openBlank();
    expect(screen.getByTestId('routine-days')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('routine-trigger-event'));
    expect(screen.getByTestId('routine-event')).toBeInTheDocument();
    expect(screen.queryByTestId('routine-days')).toBeNull();
    await fireEvent.click(screen.getByTestId('routine-trigger-manual'));
    expect(screen.getByTestId('routine-manual-note')).toBeInTheDocument();
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'By hand' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')?.routine).toMatchObject({ trigger: 'manual', cron: undefined, event: undefined }));
  });

  it('⌘↵ saves; a refused save stays open with the failure as its banner', async () => {
    await openBlank();
    inv.mockImplementation(async (cmd: string, a: { args: { action: string } }) => {
      if (cmd !== 'routines') return null;
      if (a.args.action === 'save') throw { code: 'E_FORBIDDEN', message: 'you are a Viewer in 32bit' };
      if (a.args.action === 'list') return [];
      if (a.args.action === 'preview') return previewAnswer;
      return [];
    });
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.keyDown(screen.getByTestId('routine-prompt'), { key: 'Enter', metaKey: true });
    expect(await screen.findByTestId('routine-save-error')).toBeInTheDocument();
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Nightly');
  });

  it('Cancel on a changed form asks "Discard changes?" once; an untouched one just closes', async () => {
    await openBlank();
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    expect(screen.queryByTestId('routine-editor')).toBeNull();

    await fireEvent.click(screen.getByTestId('routines-empty-new'));
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.keyDown(screen.getByTestId('routine-name'), { key: 'Escape' });
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-keep-editing'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Nightly');
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    await fireEvent.click(screen.getByTestId('form-discard'));
    expect(screen.queryByTestId('routine-editor')).toBeNull();
  });

  it('Use a template… fills a new routine from the template; an edit does not offer it', async () => {
    await openBlank();
    await fireEvent.click(screen.getByTestId('routine-use-template'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep');
  });

  it('Esc on the Use a template… menu closes the menu, not the editor', async () => {
    await openBlank();
    await fireEvent.click(screen.getByTestId('routine-use-template'));
    expect(screen.getByTestId('routine-new-menu')).toBeInTheDocument();
    await fireEvent.keyDown(screen.getByTestId('routine-template-blank'), { key: 'Escape' });
    expect(screen.queryByTestId('routine-new-menu')).toBeNull();
    expect(screen.getByTestId('routine-editor')).toBeInTheDocument();
    expect(screen.queryByTestId('form-discard-ask')).toBeNull();
  });

  it('Esc in the tuner closes only the tuner, and a template picked while it is open closes it', async () => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.prompt_tune': 'true' });
    try {
      await openBlank();
      await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
      await fireEvent.click(screen.getByTestId('routine-tune-open'));
      await fireEvent.keyDown(screen.getByTestId('routine-tune-close'), { key: 'Escape' });
      expect(screen.queryByTestId('routine-tune')).toBeNull();
      expect(screen.getByTestId('routine-editor')).toBeInTheDocument();
      expect(screen.queryByTestId('form-discard-ask')).toBeNull();

      await fireEvent.click(screen.getByTestId('routine-tune-open'));
      expect(screen.getByTestId('routine-tune')).toBeInTheDocument();
      await fireEvent.click(screen.getByTestId('routine-use-template'));
      await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
      // The typed prompt would go: it asks first (review w05-1).
      await fireEvent.click(screen.getByTestId('form-discard'));
      expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep');
      expect(screen.queryByTestId('routine-tune')).toBeNull();
    } finally {
      fleetSettings.set({ ...SETTING_DEFAULTS });
    }
  });
});

describe('the routine editor (review w05)', () => {
  async function openBlankOver(list: RoutineRow[] = [sweep]) {
    route(list);
    const v = render(RoutinesPanel);
    if (list.length) await screen.findByTestId('routine-title');
    else await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-blank'));
    return v;
  }

  it('w05-1: picking another routine, New or a request from elsewhere asks before a changed draft goes', async () => {
    await openBlankOver();
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.click(screen.getByTestId('routine-row'));
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-keep-editing'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Nightly');

    // A template over it asks too.
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Nightly');
    await fireEvent.click(screen.getByTestId('form-keep-editing'));

    // The Inbox's Fix (a request to select a routine) asks as well.
    openRoutines({ select: 3 });
    await tick();
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-discard'));
    expect(screen.queryByTestId('routine-editor')).toBeNull();
    expect(screen.getByTestId('routine-title').textContent).toBe('Morning PR sweep');
  });

  it('w05-1: an untouched draft goes without asking', async () => {
    await openBlankOver();
    await fireEvent.click(screen.getByTestId('routine-row'));
    expect(screen.queryByTestId('routine-editor')).toBeNull();
  });

  it('w05-2: a changed draft survives the tab closing under it; an untouched one does not', async () => {
    const v = await openBlankOver();
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    v.unmount();
    render(RoutinesPanel);
    expect(((await screen.findByTestId('routine-name')) as HTMLInputElement).value).toBe('Nightly');
    // Still a changed draft: Cancel asks.
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
  });

  it('w05-3: a routine saved without a zone at UTC is saved back at UTC', async () => {
    route([{ ...sweep, utc_offset_min: 0 }]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Evening sweep' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    expect(argsOf('save').routine.utc_offset_min).toBe(0);
  });

  it('w05-4: a refused save scrolls to the banner and focuses it, and the footer says it did not save', async () => {
    await openBlankOver([]);
    inv.mockImplementation(async (cmd: string, a: { args: { action: string } }) => {
      if (cmd !== 'routines') return null;
      if (a.args.action === 'save') throw { code: 'E_INVALID', message: 'the fallback host nas is in another organisation than mac' };
      if (a.args.action === 'preview') return previewAnswer;
      return [];
    });
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    const banner = await screen.findByTestId('routine-save-error');
    await waitFor(() => expect(document.activeElement?.contains(banner)).toBe(true));
    expect(screen.getByTestId('routine-save-why').textContent).toContain('Not saved');
  });

  it('w05-5/6: a budget or cap it cannot read is said on blur and blocks Save', async () => {
    await openBlankOver([]);
    await fireEvent.input(screen.getByTestId('routine-name'), { target: { value: 'Nightly' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Tidy up.' } });
    await fireEvent.input(screen.getByTestId('routine-budget-run'), { target: { value: '1.255' } });
    await fireEvent.blur(screen.getByTestId('routine-budget-run'));
    expect(screen.getByTestId('routine-budget-run-error').textContent).toContain('amount like 5');
    expect((screen.getByTestId('routine-save') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('routine-save-why').textContent).toBe('Fix the budget per run.');
    await fireEvent.input(screen.getByTestId('routine-budget-run'), { target: { value: '5,50' } });
    await fireEvent.input(screen.getByTestId('routine-cap'), { target: { value: '400' } });
    await fireEvent.blur(screen.getByTestId('routine-cap'));
    expect(screen.getByTestId('routine-cap-error').textContent).toContain('1 to 360');
    await fireEvent.input(screen.getByTestId('routine-cap'), { target: { value: '30' } });
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')?.routine).toMatchObject({ budget_run_micros: 5_500_000, run_max_secs: 1800 }));
  });

  it('w05-8: a cron line that does not read says why under it, not "never runs"', async () => {
    previewAnswer = { next_runs: [], utc_offset_min: 0, problem: 'hour: "25" is not a number from 0 to 23', logins: [] };
    await openBlankOver([]);
    await fireEvent.change(screen.getByTestId('routine-days'), { target: { value: 'custom' } });
    await fireEvent.input(screen.getByTestId('routine-cron'), { target: { value: '0 25 * * *' } });
    expect((await screen.findByTestId('routine-cron-error')).textContent).toContain('hour');
    expect(screen.getByTestId('routine-cron').getAttribute('aria-invalid')).toBe('true');
    expect(screen.getByTestId('routine-next-run').textContent).not.toContain('never runs');
  });

  it('w05-9: a dry run that fails says so with Retry, not "checking…" forever', async () => {
    route([]);
    const base = inv.getMockImplementation() as (cmd: string, a: unknown) => Promise<unknown>;
    let down = true;
    inv.mockImplementation(async (cmd: string, a: { args: { action: string } }) => {
      if (cmd === 'routines' && a.args.action === 'preview' && down) throw { code: 'E_UNREACHABLE', message: 'the hub is not reachable' };
      return base(cmd, a);
    });
    render(RoutinesPanel);
    await screen.findByTestId('routines-empty');
    await fireEvent.click(screen.getByTestId('routines-empty-new'));
    await waitFor(() => expect(screen.getByTestId('routine-dry-run').textContent).toContain('Dry run unavailable'));
    down = false;
    await fireEvent.click(screen.getByTestId('routine-dry-run-retry'));
    await waitFor(() => expect(screen.getByTestId('routine-dry-run').textContent).not.toContain('unavailable'));
  });

  it('w05-11: a hidden host, a deleted project and another organisation are shown as such', async () => {
    hosts.set([
      { alias: 'mac', hidden: true, org_id: 1 } as never,
      { alias: 'pi', hidden: false, org_id: 1 } as never,
      { alias: 'nas', hidden: false, org_id: 2 } as never,
    ]);
    route([{ ...sweep, project_id: 9 }]);
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    const host = screen.getByTestId('routine-host') as HTMLSelectElement;
    expect(host.value).toBe('mac');
    expect(host.selectedOptions[0].textContent).toBe('mac (hidden)');
    const project = screen.getByTestId('routine-project') as HTMLSelectElement;
    expect(project.selectedOptions[0].textContent).toBe('project 9 (deleted)');
    expect(screen.getByTestId('routine-save-why').textContent).toBe('Pick a project: its project is gone.');
    const fallback = screen.getByTestId('routine-fallback') as HTMLSelectElement;
    expect(Array.from(fallback.options).map((o) => o.value)).toEqual(['', 'pi']);
  });

  it('w05-12: the form takes focus on open, and Cancel gives it back to the routine', async () => {
    await openBlankOver();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('routine-name')));
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByTestId('routine-row')));
  });

  it('w05-13: Autonomy reads short, and the prompt counts toward the hub limit', async () => {
    await openBlankOver([]);
    const auto = screen.getByTestId('routine-autonomy') as HTMLSelectElement;
    expect(auto.selectedOptions[0].textContent).toBe('L2 · pushes and opens PRs');
    const prompt = screen.getByTestId('routine-prompt') as HTMLTextAreaElement;
    expect(prompt.maxLength).toBe(4000);
    expect(screen.queryByTestId('routine-prompt-count')).toBeNull();
    await fireEvent.input(prompt, { target: { value: 'x'.repeat(3600) } });
    expect(screen.getByTestId('routine-prompt-count').textContent).toBe('3600 / 4000 characters');
  });
});

describe('the routine notebooks (migration 204)', () => {
  const withMemory = { ...sweep, memory: 'routine' } as RoutineRow;
  const notebook = (body: string) => ({
    routine_id: 3,
    memory: 'routine',
    memory_auto: false,
    routine: { scope: 'routine', owner_id: 3, body, updated_at: 1, updated_by: 'run', updated_run_id: 2 },
    max_chars: 4000,
    may_change: true,
  });

  /** `route`, with `memory` answered by `answer` (a hub without notebooks throws). */
  function routeMemory(answer: (id: number) => unknown, list: RoutineRow[] = [withMemory]) {
    route(list);
    const base = inv.getMockImplementation() as (cmd: string, a: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a: { args: { action: string; routine_id?: number } }) => {
      if (cmd === 'routines' && a.args.action === 'memory') return answer(a.args.routine_id!);
      return base(cmd, a);
    });
  }
  const notFound = () => {
    throw { code: 'E_NOT_FOUND', message: 'no routine 0' };
  };

  async function openSaved() {
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
  }

  it('a notebook edit survives a change to the form, read-only, and leaving asks first', async () => {
    routeMemory((id) => (id === 3 ? notebook('old notes') : notFound()));
    await openSaved();
    const box = (await screen.findByTestId('routine-memory-routine')) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toBe('old notes'));
    await fireEvent.input(box, { target: { value: 'new notes' } });

    // An unsaved notebook alone makes Cancel ask.
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('form-keep-editing'));

    // A form edit keeps the box and its text, read-only until the form is saved.
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Only CI.' } });
    const kept = screen.getByTestId('routine-memory-routine') as HTMLTextAreaElement;
    expect(kept.value).toBe('new notes');
    expect(kept.readOnly).toBe(true);
    expect(screen.getByTestId('routine-memory-locked')).toBeInTheDocument();
    expect((screen.getByTestId('routine-memory-save-routine') as HTMLButtonElement).disabled).toBe(true);
  });

  /** `routeMemory`, with `save` answering the routine saved in place. */
  function routeSaveInPlace() {
    routeMemory((id) => (id === 3 ? notebook('old notes') : notFound()));
    const base = inv.getMockImplementation() as (cmd: string, a: unknown) => Promise<unknown>;
    inv.mockImplementation(async (cmd: string, a: { args: { action: string; routine?: Partial<RoutineRow> } }) => {
      if (cmd === 'routines' && a.args.action === 'save') return { ...withMemory, ...a.args.routine, id: 3 };
      return base(cmd, a);
    });
  }

  it('saving the form keeps the editor open with the unsaved notebook text, unlocked', async () => {
    routeSaveInPlace();
    await openSaved();
    const box = (await screen.findByTestId('routine-memory-routine')) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toBe('old notes'));
    await fireEvent.input(box, { target: { value: 'new notes' } });
    await fireEvent.input(screen.getByTestId('routine-prompt'), { target: { value: 'Only CI.' } });
    expect(screen.getByTestId('routine-memory-locked')).toBeInTheDocument();

    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')?.routine?.prompt).toBe('Only CI.'));
    await waitFor(() => expect(screen.queryByTestId('routine-memory-locked')).toBeNull());
    const kept = screen.getByTestId('routine-memory-routine') as HTMLTextAreaElement;
    await waitFor(() => expect(kept.readOnly).toBe(false));
    expect(kept.value).toBe('new notes');
    expect((screen.getByTestId('routine-memory-save-routine') as HTMLButtonElement).disabled).toBe(false);
    // Leaving still asks: the notebook text is not saved yet.
    await fireEvent.click(screen.getByTestId('routine-cancel'));
    expect(screen.getByTestId('form-discard-ask')).toBeInTheDocument();
  });

  it('Run once now with only a notebook edit runs and keeps the editor and its text', async () => {
    routeSaveInPlace();
    await openSaved();
    const box = (await screen.findByTestId('routine-memory-routine')) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toBe('old notes'));
    await fireEvent.input(box, { target: { value: 'new notes' } });

    await fireEvent.click(screen.getByTestId('routine-run-once'));
    await waitFor(() => expect(argsOf('run_now')).toEqual({ action: 'run_now', routine_id: 3 }));
    expect((screen.getByTestId('routine-memory-routine') as HTMLTextAreaElement).value).toBe('new notes');
  });

  it('counts characters as the fleet does: an emoji is one', async () => {
    const body = '🙂'.repeat(20) + 'x'.repeat(3970);
    routeMemory((id) => (id === 3 ? notebook(body) : notFound()));
    await openSaved();
    const box = (await screen.findByTestId('routine-memory-routine')) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toBe(body));
    await fireEvent.input(box, { target: { value: body + 'y' } });
    expect(box.getAttribute('aria-invalid')).toBeNull();
    expect(box.parentElement!.textContent).toContain('3991 / 4000 characters');
    expect((screen.getByTestId('routine-memory-save-routine') as HTMLButtonElement).disabled).toBe(false);
  });

  it('against a hub without notebooks the form offers none, so a save never names one', async () => {
    routeMemory(() => {
      throw { code: 'E_INVALID', message: 'action must be list | get | runs, got "memory"' };
    }, [sweep]);
    await openSaved();
    expect(await screen.findByTestId('routine-memory-unserved')).toBeInTheDocument();
    expect(screen.queryByTestId('routine-memory-mode')).toBeNull();
  });

  it('a stale notebook reply for the routine left behind is not shown under the next one', async () => {
    let releaseA: () => void = () => {};
    routeMemory((id) => {
      if (id === 3) return new Promise((res) => (releaseA = () => res(notebook('A notes'))));
      if (id === 4) return { ...notebook('B notes'), routine_id: 4 };
      return notFound();
    });
    const v = render(RoutineMemory, { routineId: 3 });
    await v.rerender({ routineId: 4 });
    const box = (await screen.findByTestId('routine-memory-routine')) as HTMLTextAreaElement;
    await waitFor(() => expect(box.value).toBe('B notes'));
    releaseA();
    await new Promise((r) => setTimeout(r, 0));
    await tick();
    expect((screen.getByTestId('routine-memory-routine') as HTMLTextAreaElement).value).toBe('B notes');
  });
});
