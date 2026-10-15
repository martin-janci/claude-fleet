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
    await fireEvent.click(await screen.findByTestId('automation-runs-list'));
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
    await fireEvent.click(await screen.findByTestId('automation-runs-list'));
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
    expect(await screen.findAllByTestId('run-bar')).toHaveLength(1);
    expect(screen.getAllByTestId('run-jev')).toHaveLength(1);
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

describe('Runs lanes and run timeline (automation runs boards A and B)', () => {
  const ROUTINE_RUN = {
    id: 'routine:418',
    source: 'routine',
    kind: 'routine',
    owner: 'Nightly dependency bump',
    started_at: now - 600,
    outcome: 'running',
    session_ids: [9],
    host: 'htz',
    routine_id: 3,
    summary: 'Bash: pnpm test',
  };
  const SKIPPED = { id: 'routine:417', source: 'routine', kind: 'routine', owner: 'Usage report', started_at: now - 900, outcome: 'nothing_to_do', session_ids: [] };
  const STEPS = [
    { seq: 1, attempt: 1, at: now - 600, phase: 'trigger', kind: 'trigger', actor: 'scheduler', summary: 'Fired by its schedule' },
    {
      seq: 2,
      attempt: 1,
      at: now - 600,
      phase: 'checks',
      kind: 'check',
      actor: 'scheduler',
      summary: '2 checks passed',
      data: { checks: [{ name: 'overlap', state: 'ok', detail: 'no run open' }, { name: 'day_budget', state: 'warn', detail: '$1.80 of $2.00' }] },
    },
    { seq: 3, attempt: 1, at: now - 596, phase: 'spawn', kind: 'spawn', actor: 'hub', summary: 'Session started on htz', duration_ms: 4200 },
    { seq: 4, attempt: 1, at: now - 590, phase: 'prompt', kind: 'prompt', actor: 'hub', summary: 'Prompt typed into the session' },
    { seq: 5, attempt: 1, at: now - 300, phase: 'working', kind: 'tool_call', actor: 'claude', summary: 'Bash: pnpm install', duration_ms: 168_000, data: { tool_use_id: 't1', tool: 'Bash' } },
  ];

  beforeEach(() => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === 'health_check') return { version: 'x', db_ready: true, schema_version: 1, loops: LOOPS, automation_paused: false };
      if (cmd === 'list_runs') return { runs: [ROUTINE_RUN, SKIPPED, ...RUNS], total: 4 };
      if (cmd === 'run_trace') return { run_id: 'routine:418', steps: STEPS, more: false };
      return null;
    });
  });

  it('lays every run in the window on lanes, with the window counts', async () => {
    render(AutomationView);
    automationTab.set('runs');
    const lanes = await screen.findAllByTestId('run-lane');
    expect(lanes.map((l) => l.dataset.lane)).toEqual([
      'Routines:Nightly dependency bump',
      'Routines:Usage report',
      'Jev:status_map',
      'Fleet:Morning PR sweep',
    ]);
    expect(screen.getAllByTestId('run-skip')).toHaveLength(1);
    const live = screen.getAllByTestId('run-bar').find((b) => b.dataset.run === 'routine:418')!;
    expect(live.className).toContain('running');
    const stats = screen.getByTestId('automation-runs-stats').textContent ?? '';
    expect(stats).toMatch(/Running now\s*1/);
    expect(stats).toMatch(/Skipped fires\s*1/);
    const call = invoke.mock.calls.find(([c]) => c === 'list_runs')!;
    expect(call[1].args.limit).toBe(200);
    expect(now - call[1].args.since).toBeGreaterThanOrEqual(3 * 3600 - 5);
  });

  it('a lane bar opens the run timeline: phases, steps, detail and checks', async () => {
    render(AutomationView);
    automationTab.set('runs');
    const bars = await screen.findAllByTestId('run-bar');
    await fireEvent.click(bars.find((b) => b.dataset.run === 'routine:418')!);
    await screen.findByTestId('run-timeline');
    await waitFor(() => expect(screen.getAllByTestId('run-step')).toHaveLength(5));
    expect(invoke).toHaveBeenCalledWith('run_trace', { args: { run_id: 'routine:418' } });
    expect(screen.getByTestId('run-phase-trigger').dataset.state).toBe('done');
    expect(screen.getByTestId('run-phase-working').dataset.state).toBe('live');
    expect(screen.getByTestId('run-phase-settle').dataset.state).toBe('todo');
    expect(screen.getByTestId('run-step-detail').textContent).toContain('Bash: pnpm install');
    expect(screen.getByTestId('run-checks').textContent).toContain('$1.80 of $2.00');
    await fireEvent.click(screen.getAllByTestId('run-step')[2]);
    expect(screen.getByTestId('run-step-detail').textContent).toContain('Session started on htz');
    await fireEvent.click(screen.getByTestId('run-timeline-close'));
    expect(await screen.findByTestId('run-lanes')).toBeInTheDocument();
  });

  it("the orbit map circles a comet round the live run's host, which opens its timeline", async () => {
    render(AutomationView);
    automationTab.set('runs');
    await fireEvent.click(await screen.findByTestId('automation-runs-orbit'));
    const comet = await screen.findByTestId('orbit-comet');
    expect(comet.dataset.run).toBe('routine:418');
    expect(screen.getAllByTestId('orbit-host').map((h) => h.dataset.host)).toEqual(['htz']);
    expect(screen.getByTestId('orbit-feed').textContent).toContain('Nightly dependency bump');
    expect(invoke).toHaveBeenCalledWith('routines', expect.anything());
    await fireEvent.click(comet);
    expect(await screen.findByTestId('run-timeline')).toBeInTheDocument();
  });

  it("Control's Orbit asks for Runs on one run, and Automation opens there", async () => {
    const { openRuns } = await import('./automation');
    openRuns('orbit', 'routine:418');
    render(AutomationView);
    expect(await screen.findByTestId('run-timeline')).toBeInTheDocument();
    await fireEvent.click(screen.getByTestId('run-timeline-close'));
    expect(await screen.findByTestId('orbit-map')).toBeInTheDocument();
  });

  it("the list opens a run's steps too, and an untraced run says so", async () => {
    render(AutomationView);
    automationTab.set('runs');
    await fireEvent.click(await screen.findByTestId('automation-runs-list'));
    const opens = await screen.findAllByTestId('automation-run-open');
    await fireEvent.click(opens[3]);
    expect(await screen.findByTestId('run-timeline-untraced')).toBeInTheDocument();
  });
});
