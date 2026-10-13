// A `dashboard` block in the chat (persona design, step 5): drawn live from
// work stats, pinned only on a click, offered as a replacement when its id
// is pinned already.
import { fireEvent, render, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import UiBlockCard from './UiBlockCard.svelte';
import { checkUiBlock } from '../rich_blocks';
import { dashboardBlock, pinnedDashboards, shownDashboard, type Dashboard } from '../dashboards';
import { destination } from '../destination';
import { expectAccessible } from '../a11y_check';

const mock = invoke as ReturnType<typeof vi.fn>;
const stages = { backlog: 1, in_progress: 2, in_review: 1, blocked: 0, done: 4 };

const raw = JSON.stringify({
  spec: 'fleet.ui/1',
  kind: 'dashboard',
  id: 'dash.release-42',
  title: 'Release 4.2',
  why: 'You asked how 4.2 is going.',
  widgets: [
    { kind: 'stat', title: 'Open', query: { status: 'open' }, value: 'open' },
    { kind: 'groups', title: 'By person', query: { group_by: 'person' } },
  ],
});
const c = checkUiBlock(raw);
if (!c.ok) throw new Error(c.problems.join('; '));
const block = c.block;

/** The hub's `dashboards` tool, over the rows the test seeds. */
function fakeDashboards(rows: Map<string, Record<string, unknown>>, a: { args: { action: string; id?: string; dashboard?: Record<string, unknown>; why?: string } }) {
  const { action, id, dashboard, why } = a.args;
  if (action === 'list') return [...rows.values()];
  if (action === 'pin') {
    const row = { id: dashboard!.id, title: dashboard!.title, spec: dashboard!, why, pinned_at: 100 };
    rows.set(dashboard!.id as string, row);
    return row;
  }
  return { removed: rows.delete(id!) };
}

let hub: Map<string, Record<string, unknown>>;
const seed = (d: Dashboard) => hub.set(d.id, { id: d.id, title: d.title, spec: dashboardBlock(d), pinned_at: 1 });

beforeEach(() => {
  pinnedDashboards.set([]);
  hub = new Map();
  destination.set('session');
  mock.mockReset();
  mock.mockImplementation(async (cmd: string, a?: { args?: { filters?: { group_by?: string } } }) => {
    if (cmd === 'dashboards') return fakeDashboards(hub, a as never);
    expect(cmd).toBe('work_stats');
    const by = a?.args?.filters?.group_by;
    return {
      group_by: by ?? 'group', total: 8, stages, pct_done: 50, needs_you: 0, cost_micros: 0, generated_at: 1,
      groups: by ? [{ group: { id: 'person:ana', label: 'Ana', source: 'person' }, total: 8, stages, pct_done: 50, needs_you: 0, cost_micros: 0 }] : [],
    };
  });
});

describe('DashboardCard', () => {
  it('draws the proposal from work stats and pins it on a click', async () => {
    const r = render(UiBlockCard, { block, raw, sessionId: 7 });
    await waitFor(() => expect(r.getByText('Ana')).toBeTruthy());
    expect(r.getByText('You asked how 4.2 is going.')).toBeTruthy();
    // The open count is the app's: 8 tasks less 4 done.
    expect(r.getByTestId('widget-stat').textContent).toContain('4');
    expect(get(pinnedDashboards)).toEqual([]);
    await expectAccessible(r.container);

    await fireEvent.click(r.getByTestId('rich-dashboard-pin'));
    await waitFor(() => expect(hub.has('dash.release-42')).toBe(true));
    expect(get(pinnedDashboards).map((p) => [p.id, p.why])).toEqual([['dash.release-42', 'You asked how 4.2 is going.']]);
    expect(get(destination)).toBe('dashboard');
    expect(get(shownDashboard)?.id).toBe('dash.release-42');
    expect(r.getByTestId('rich-dashboard-state').dataset.state).toBe('same');
  });

  it('offers a block with a pinned id as its replacement, saying what changes', async () => {
    const old: Dashboard = { id: 'dash.release-42', title: 'Release 4.2', intro: '', widgets: [{ kind: 'stat', title: 'Spent', query: {}, value: 'cost' }] };
    seed(old);
    pinnedDashboards.set([{ ...old, pinned_at: 1 }]);
    const r = render(UiBlockCard, { block, raw, sessionId: null });
    expect(r.getByTestId('rich-dashboard-state').dataset.state).toBe('replace');
    expect(r.getByText(/Replaces your pinned “Release 4.2”: adds “Open”, adds “By person”, removes “Spent”/)).toBeTruthy();
    await fireEvent.click(r.getByTestId('rich-dashboard-pin'));
    await waitFor(() => expect(get(pinnedDashboards)[0].widgets.map((w) => w.title)).toEqual(['Open', 'By person']));
  });

  it('reads once: a card in the transcript does not re-read on every work change', async () => {
    const { bumpWorkChanged } = await import('../work');
    render(UiBlockCard, { block, raw, sessionId: 7 });
    const reads = () => mock.mock.calls.filter((c) => c[0] === 'work_stats').length;
    await waitFor(() => expect(reads()).toBe(2));
    vi.useFakeTimers();
    bumpWorkChanged('updated' as never);
    vi.advanceTimersByTime(5000);
    vi.useRealTimers();
    expect(reads()).toBe(2);
  });

  it('says why the hub would not pin it', async () => {
    mock.mockImplementation(async (cmd: string, a?: { args?: { action?: string; filters?: { group_by?: string } } }) => {
      if (cmd === 'dashboards' && a?.args?.action === 'pin') throw { code: 'E_INVALID', message: '20 dashboards are pinned; unpin one first' };
      if (cmd === 'dashboards') return [];
      return { group_by: 'group', total: 0, stages, pct_done: 0, needs_you: 0, cost_micros: 0, generated_at: 1, groups: [] };
    });
    const r = render(UiBlockCard, { block, raw, sessionId: 7 });
    await fireEvent.click(r.getByTestId('rich-dashboard-pin'));
    await waitFor(() => expect(r.getByTestId('rich-dashboard-error').textContent).toContain('unpin one first'));
    expect(get(destination)).toBe('session');
  });
});
