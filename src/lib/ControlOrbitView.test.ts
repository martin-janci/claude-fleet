// Automation runs board C in Control's right panel: the orbit, compact.
import { render, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { get } from 'svelte/store';

const now = Math.floor(Date.now() / 1000);
const RUNS = [
  { id: 'routine:7', source: 'routine', kind: 'routine', owner: 'Nightly bump', started_at: now - 120, outcome: 'running', session_ids: [41], host: 'mac' },
  { id: 'routine:6', source: 'routine', kind: 'routine', owner: 'PR sweep', started_at: now - 900, ended_at: now - 60, outcome: 'failed', error: 'time cap', session_ids: [], host: 'mac' },
];
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd === 'list_runs') return { runs: RUNS, total: RUNS.length };
    if (cmd === 'routines') return [];
    if (cmd === 'run_trace') return { steps: [], more: false };
    throw { code: 'E_TEST', message: `no ${cmd}` };
  }),
}));
import ControlOrbitView from './ControlOrbitView.svelte';
import ControlViewsPanel from './ControlViewsPanel.svelte';
import { controlViews, defaultLayout, selectView } from './control_views';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { destination } from './destination';
import { automationTab, runsRequest } from './automation';
import { expectAccessible } from './a11y_check';

const live = { ...session('mac', 'bump', { claude_status: 'working' }), id: 41 };

beforeEach(() => {
  controlViews.set(defaultLayout());
  sessions.set([live]);
});
afterEach(() => {
  sessions.set([]);
  destination.set('session');
});

describe('ControlOrbitView', () => {
  it('draws the last day compact: a comet on its host, a spark, the live feed, no legend', async () => {
    const { findByTestId, getByTestId, queryByText, container } = render(ControlOrbitView);
    const comet = await findByTestId('orbit-comet');
    expect(comet.dataset.run).toBe('routine:7');
    expect(getByTestId('orbit-spark')).toBeInTheDocument();
    expect(getByTestId('orbit-feed').textContent).toContain('failed: time cap');
    expect(queryByText('How to read it')).toBeNull();
    await expectAccessible(container);
  });

  it("a comet opens the run's steps in the panel, and Open session focuses its session", async () => {
    const onfocus = vi.fn();
    const { findByTestId, getByTestId } = render(ControlOrbitView, { onfocus });
    await fireEvent.click(await findByTestId('orbit-comet'));
    expect(await findByTestId('run-timeline')).toBeInTheDocument();
    await fireEvent.click(getByTestId('run-timeline-session'));
    expect(onfocus).toHaveBeenCalledWith(expect.objectContaining({ id: 41 }));
    await fireEvent.click(getByTestId('run-timeline-close'));
    expect(await findByTestId('orbit-map')).toBeInTheDocument();
  });

  it('is a view in the "+" menu, and ⤢ opens the full orbit in Automation', async () => {
    selectView('orbit');
    const { findByTestId, getByTestId } = render(ControlViewsPanel);
    expect(await findByTestId('control-orbit')).toBeInTheDocument();
    await fireEvent.click(getByTestId('control-views-expand'));
    expect(get(destination)).toBe('automation');
    expect(get(automationTab)).toBe('runs');
    expect(get(runsRequest)?.view).toBe('orbit');
  });
});
