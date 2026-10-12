// Automation guards (Orbit Fleet M15 step G3.8): the editor writes the time
// cap, fallback host, retry once and autonomy; the inspector and the runs
// say them back; a failed run carries its named fix; the Automation foot
// shows the routines' spend against the fleet's daily budget.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { get } from 'svelte/store';
import RoutinesPanel from './RoutinesPanel.svelte';
import RoutineFailures from './RoutineFailures.svelte';
import AutomationView from '../AutomationView.svelte';
import { hosts } from '../hosts';
import { projects } from '../projects';
import { destination } from '../destination';
import { automationTab } from '../automation';
import { failing, type RoutineRow, type RoutineRunRow, type RunFix } from '../routines';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;

const guarded: RoutineRow = {
  id: 3,
  name: 'Morning PR sweep',
  enabled: true,
  trigger: 'cron',
  cron: '30 7 * * 1-5',
  utc_offset_min: 120,
  time_zone: 'Europe/Bratislava',
  host_alias: 'mac',
  fallback_host: 'nas',
  project_id: 1,
  prompt: 'Review every open PR.',
  budget_run_micros: 2_000_000,
  run_max_secs: 1200,
  retry_once: true,
  autonomy: 1,
  overlap: 'skip',
  skip_next: false,
  created_at: 1,
  updated_at: 1,
};
const bad: RoutineRunRow = {
  id: 1,
  routine_id: 3,
  trigger: 'run_now',
  trigger_ref: 'retry:7',
  state: 'failed',
  reason: 'its turn ended in an error: auth (authentication_error): HTTP 401',
  error_code: 'E_TURN_AUTH',
  host_alias: 'nas',
  cost_micros: 10_000,
  started_at: 900,
  finished_at: 912,
};
const fix: RunFix = { run_id: 1, code: 'E_TURN_AUTH', label: 'Log in again on nas', action: 'host', host: 'nas' };

const argsOf = (action: string) =>
  inv.mock.calls.map((c) => c[1]?.args).filter((a) => a?.action === action).at(-1);

function route(r: RoutineRow = guarded, fixes: RunFix[] = [fix]) {
  inv.mockReset();
  inv.mockImplementation(async (cmd: string, a: { args: { action: string; routine?: { name: string } } }) => {
    if (cmd !== 'routines') return null;
    switch (a.args.action) {
      case 'list':
        return [r];
      case 'get':
        return { routine: r, runs: [bad], may_change: true, fixes };
      case 'failing':
        return [];
      case 'preview':
        return { next_runs: [], utc_offset_min: 0, logins: [] };
      case 'save':
        return { ...r, id: 8 };
      default:
        return r;
    }
  });
}

beforeEach(() => {
  hosts.set([{ alias: 'mac', hidden: false } as never, { alias: 'nas', hidden: false } as never]);
  projects.set([{ project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', system: false }, worktrees: [] } as never]);
  failing.set([]);
  destination.set('automation');
  route();
});

describe('Automation guards (G3.8)', () => {
  it('the inspector says the time cap, fallback host, retry, autonomy and where outcomes go', async () => {
    render(RoutinesPanel);
    const inspector = await screen.findByTestId('routine-inspector');
    const text = inspector.textContent ?? '';
    expect(text).toContain('$2.00 · 20 min');
    expect(text).toContain('mac, else nas');
    expect(text).toContain('Retry once, then Inbox as Failed');
    expect(text).toContain('L1 · asks before push');
    expect(text).toContain('Needs you goes to the Inbox; Nothing to do stays in Runs');
    expect(screen.getByTestId('routine-autonomy-stat').textContent).toBe('L1 · asks before push');
  });

  it("a failed run's Fix is its named fix, with the code and host in Details, and opens the host", async () => {
    render(RoutinesPanel);
    const fixBtn = await screen.findByTestId('routine-run-fix');
    expect(fixBtn.textContent).toBe('Log in again on nas');
    expect(screen.getByTestId('routine-run-code').textContent).toBe('E_TURN_AUTH · on nas');
    expect(screen.getByTestId('routine-run').textContent).toContain('retried once after run 7 failed');
    await fireEvent.click(fixBtn);
    expect(get(destination)).toBe('hosts');
  });

  it('a run from an older hub, with no fix named, still says Fix', async () => {
    route(guarded, []);
    render(RoutinesPanel);
    expect((await screen.findByTestId('routine-run-fix')).textContent).toBe('Fix');
  });

  it('the editor writes the guards back whole, in the zone the routine was saved in', async () => {
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    expect((screen.getByTestId('routine-cap') as HTMLInputElement).value).toBe('20');
    expect((screen.getByTestId('routine-fallback') as HTMLSelectElement).value).toBe('nas');
    expect((screen.getByTestId('routine-retry-once') as HTMLInputElement).checked).toBe(true);
    expect((screen.getByTestId('routine-autonomy') as HTMLSelectElement).value).toBe('1');
    // The Time zone select (G7.4) opens on the routine's own zone.
    expect((screen.getByTestId('routine-zone-select') as HTMLSelectElement).value).toBe('Europe/Bratislava');
    await fireEvent.input(screen.getByTestId('routine-cap'), { target: { value: '45' } });
    await fireEvent.change(screen.getByTestId('routine-autonomy'), { target: { value: '0' } });
    await fireEvent.click(screen.getByTestId('routine-retry-once'));
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    const saved = argsOf('save').routine;
    expect(saved).toMatchObject({ run_max_secs: 2700, fallback_host: 'nas', retry_once: false, autonomy: 0 });
    expect(saved.time_zone).toBe('Europe/Bratislava');
  });

  it('push and open PRs (L2) and no cap are saved as nothing', async () => {
    route({ ...guarded, autonomy: undefined, run_max_secs: undefined, fallback_host: undefined, retry_once: false });
    render(RoutinesPanel);
    await screen.findByTestId('routine-title');
    await fireEvent.click(screen.getByTestId('routine-edit'));
    expect((screen.getByTestId('routine-autonomy') as HTMLSelectElement).value).toBe('2');
    await fireEvent.click(screen.getByTestId('routine-save'));
    await waitFor(() => expect(argsOf('save')).toBeDefined());
    const saved = argsOf('save').routine;
    expect(saved.autonomy).toBeUndefined();
    expect(saved.run_max_secs).toBeUndefined();
    expect(saved.fallback_host).toBeUndefined();
  });

  it("the Inbox's Fix is the named fix, and an accounts fix opens the accounts", async () => {
    failing.set([{ routine: guarded, run: bad, may_change: true, fix: { ...fix, label: 'Switch account or wait for the limit', action: 'accounts' } }]);
    render(RoutineFailures);
    const btn = screen.getByTestId('routine-failure-fix');
    expect(btn.textContent).toBe('Switch account or wait for the limit');
    await fireEvent.click(btn);
    expect(get(destination)).toBe('accounts');
  });

  it("Automation's foot shows the routines' spend against the fleet's daily budget", async () => {
    automationTab.set('routines');
    inv.mockReset();
    inv.mockImplementation(async (cmd: string, a?: { args?: { action: string } }) => {
      if (cmd === 'health_check') return { version: 'x', db_ready: true, schema_version: 1, loops: [], automation_paused: false };
      if (cmd === 'list_runs') return { runs: [], total: 0 };
      if (cmd === 'start_rules') return [];
      if (cmd === 'get_fleet_settings') return {};
      if (cmd === 'routines' && a?.args?.action === 'budget') return { spent_micros: 4_100_000, budget_micros: 15_000_000, since: 0 };
      if (cmd === 'routines' && a?.args?.action === 'list') return [];
      return null;
    });
    render(AutomationView);
    expect((await screen.findByTestId('automation-budget')).textContent).toBe('Routines $4.10 of $15.00 budget');
  });
});
