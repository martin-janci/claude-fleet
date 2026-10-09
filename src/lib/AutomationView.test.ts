import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import AutomationView from './AutomationView.svelte';
import { automationTab } from './automation';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const now = Math.floor(Date.now() / 1000);

let paused = false;
const LOOPS = [
  { name: 'reconcile', label: 'Reconcile', pausable: false, keeps_running: 'Only reads what each host runs; it starts and stops nothing.', last_run_at: now - 30, next_run_at: now + 30, result: 'ok', runs: 9, failures: 0 },
  { name: 'catalog_scan', label: 'Catalog sync', pausable: true, last_run_at: now - 120, result: 'error', last_error: 'git: auth', runs: 2, failures: 1 },
];
const RUNS = [
  { id: 'aux:2', source: 'aux', kind: 'planner', owner: 'Morning PR sweep', started_at: now - 300, duration_ms: 340_000, outcome: 'ok', cost_micros: 380_000, session_ids: [7], summary: 'Reviewed 4 PRs' },
  { id: 'jev:1', source: 'jev', kind: 'jev', owner: 'status_map', started_at: now - 60, outcome: 'failed', error: 'gh token expired', cost_micros: 10_000, session_ids: [] },
];

function route() {
  invoke.mockImplementation(async (cmd: string, a?: { key?: string; value?: string }) => {
    if (cmd === 'health_check') return { version: 'x', db_ready: true, schema_version: 1, loops: LOOPS, automation_paused: paused };
    if (cmd === 'list_runs') return { runs: RUNS, total: RUNS.length };
    if (cmd === 'start_rules') return [];
    if (cmd === 'get_fleet_settings') return { 'automation.paused': String(paused) };
    if (cmd === 'set_fleet_setting') {
      paused = a?.value === 'true';
      return { 'automation.paused': String(paused) };
    }
    return null;
  });
}

beforeEach(() => {
  invoke.mockReset();
  paused = false;
  automationTab.set('routines');
  route();
});

describe('Automation (redesign step 8.4)', () => {
  it('lists the built-in routines with their last run, and a failure in words', async () => {
    render(AutomationView);
    const rows = await screen.findAllByTestId('automation-loop');
    expect(rows.map((r) => r.dataset.loop)).toEqual(['reconcile', 'catalog_scan']);
    expect(rows[0].textContent).toContain('keeps running on Pause all');
    expect(rows[0].querySelector('[data-testid="automation-loop-why"]')?.textContent).toContain('starts and stops nothing');
    expect(rows[1].querySelector('[data-testid="automation-loop-why"]')).toBeNull();
    expect(rows[1].textContent).toContain('failed 2m ago: git: auth');
    expect(screen.getByTestId('automation-today').textContent).toBe('Today $0.39');
  });

  it('lists runs with outcome, cost and a link to the session', async () => {
    render(AutomationView);
    automationTab.set('runs');
    const rows = await screen.findAllByTestId('automation-run');
    expect(rows[0].textContent).toContain('Morning PR sweep');
    expect(rows[0].textContent).toContain('5m 40s');
    expect(rows[0].textContent).toContain('$0.38');
    expect(rows[1].textContent).toContain('Failed: gh token expired');
    expect(screen.getAllByTestId('automation-run-session')).toHaveLength(1);
  });

  it('the Runs column filters by outcome', async () => {
    render(AutomationView);
    automationTab.set('runs');
    await screen.findAllByTestId('automation-run');
    await fireEvent.click(screen.getByTestId('automation-runs-failed'));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('list_runs', { args: { limit: 50, outcome: 'failed' } }));
    expect(screen.getByTestId('automation-runs-failed').getAttribute('aria-selected')).toBe('true');
  });

  it('+ New on any tab opens the Routines with the template in the editor', async () => {
    render(AutomationView);
    automationTab.set('agents');
    await screen.findByTestId('automation-agent-operator');
    await fireEvent.click(screen.getByTestId('routine-new'));
    await fireEvent.click(screen.getByTestId('routine-template-blank'));
    expect(await screen.findByTestId('routine-editor')).toBeInTheDocument();
    expect(screen.getByTestId('automation-tab-routines').getAttribute('aria-selected')).toBe('true');
  });

  it('the Rules tab holds the start rules (8.11)', async () => {
    render(AutomationView);
    automationTab.set('rules');
    expect(await screen.findByTestId('automation-rules')).toBeInTheDocument();
    expect(await screen.findByTestId('start-rules')).toBeInTheDocument();
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('start_rules', { args: { action: 'list' } }));
  });

  it('names the three built-in agents', async () => {
    render(AutomationView);
    automationTab.set('agents');
    expect(await screen.findByTestId('automation-agent-operator')).toBeInTheDocument();
    expect(screen.getByTestId('automation-agent-orchestrator')).toBeInTheDocument();
    expect(screen.getByTestId('automation-agent-jev').textContent).toContain('off');
  });

  it('Pause all sets automation.paused, says what stands still, and Resume clears it', async () => {
    render(AutomationView);
    await screen.findAllByTestId('automation-loop');
    await fireEvent.click(screen.getByTestId('automation-pause'));
    await waitFor(() => expect(screen.getByTestId('automation-paused')).toBeInTheDocument());
    expect(invoke).toHaveBeenCalledWith('set_fleet_setting', { key: 'automation.paused', value: 'true' });
    expect(screen.getByTestId('automation-pause').textContent).toContain('Resume');
    await fireEvent.click(screen.getByTestId('automation-pause'));
    await waitFor(() => expect(screen.queryByTestId('automation-paused')).toBeNull());
    expect(invoke).toHaveBeenCalledWith('set_fleet_setting', { key: 'automation.paused', value: 'false' });
  });
});

// Review round 13: a failed read says so with Retry, and a later good read of
// the routines never leaves the Runs tab blank.
describe('Automation when a read fails (review r13)', () => {
  it('a failed runs read shows an error with Retry, which reads the runs again', async () => {
    let runsFail = true;
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'health_check') return { version: 'x', db_ready: true, schema_version: 1, loops: LOOPS, automation_paused: false };
      if (cmd === 'list_runs') {
        if (runsFail) throw { code: 'E_HUB_TIMEOUT', message: 'deadline' };
        return { runs: RUNS, total: RUNS.length };
      }
      return null;
    });
    render(AutomationView);
    automationTab.set('runs');
    const err = await screen.findByTestId('automation-runs-error');
    expect(err.textContent).toContain('The hub took too long to answer');
    runsFail = false;
    await fireEvent.click(screen.getByTestId('automation-runs-error-retry'));
    expect(await screen.findAllByTestId('automation-run')).toHaveLength(2);
  });

  it('a failed first read of the routines is said, not left blank', async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'health_check') throw { code: 'E_HUB_UNREACHABLE', message: 'refused' };
      if (cmd === 'list_runs') return { runs: [], total: 0 };
      return null;
    });
    render(AutomationView);
    const err = await screen.findByTestId('automation-load-error');
    expect(err.textContent).toContain("Couldn't load automation");
    expect(err.textContent).not.toMatch(/^E_/);
    expect(screen.getByTestId('automation-load-error-retry')).toBeInTheDocument();
  });
});
