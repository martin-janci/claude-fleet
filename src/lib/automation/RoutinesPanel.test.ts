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
import { failing, type RoutineRow, type RoutineRunRow } from '../routines';

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
      default:
        return list[0];
    }
  });
}

beforeEach(() => {
  hosts.set([{ alias: 'mac', hidden: false } as never]);
  projects.set([{ project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', system: false }, worktrees: [] } as never]);
  failing.set([]);
  route();
});

describe('Routines (8.6)', () => {
  it('lists the routines and shows one with its runs, a failed one carrying Fix, Retry and Pause', async () => {
    render(RoutinesPanel);
    expect((await screen.findByTestId('routine-title')).textContent).toBe('Morning PR sweep');
    expect(screen.getByTestId('routine-row').textContent).toContain('Weekdays 07:30');
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

  it('starts a new routine from the Morning PR sweep template and saves it whole', async () => {
    route([]);
    render(RoutinesPanel);
    expect(await screen.findByTestId('routines-empty')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-morning-pr-sweep'));
    expect((screen.getByTestId('routine-name') as HTMLInputElement).value).toBe('Morning PR sweep');
    expect((screen.getByTestId('routine-cron') as HTMLInputElement).value).toBe('30 7 * * 1-5');
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
