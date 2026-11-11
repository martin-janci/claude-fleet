import { render, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import DashboardView from './DashboardView.svelte';
import { workspace } from './workspace';
import { expectAccessible } from './a11y_check';
import { dashboardBlock, pinnedDashboards, shownDashboard, type Dashboard } from './dashboards';
import { operatorSession } from './operator';
import { composerDrafts } from './conversation';
import { destination } from './destination';
import { fireEvent } from '@testing-library/svelte';
import type { SessionRow } from './sessions';
import { hosts, type HostRow } from './hosts';

const mock = invoke as ReturnType<typeof vi.fn>;

/** The hub's `dashboards` tool, over the rows the test seeds. */
function fakeDashboards(rows: Map<string, Record<string, unknown>>, a: { args: { action: string; id?: string; dashboard?: Record<string, unknown>; why?: string } }) {
  const { action, id, dashboard, why } = a.args;
  if (action === 'list') return [...rows.values()];
  if (action === 'pin') {
    const row = { id: dashboard!.id, title: dashboard!.title, spec: dashboard!, why, pinned_at: 100 };
    rows.set(dashboard!.id as string, row);
    return row;
  }
  if (action === 'undo') {
    const row = rows.get(id!) as { previous?: Record<string, unknown> } | undefined;
    if (row?.previous) {
      rows.set(id!, row.previous);
      return { undone: true, removed: false };
    }
    return { undone: rows.delete(id!), removed: true };
  }
  return { removed: rows.delete(id!) };
}

let hub = new Map<string, Record<string, unknown>>();
const seed = (d: Dashboard) => hub.set(d.id, { id: d.id, title: d.title, spec: dashboardBlock(d), pinned_at: 1_760_000_000 });
const stages = { backlog: 2, in_progress: 1, in_review: 1, blocked: 1, done: 3 };

beforeEach(() => {
  mock.mockReset();
  hub = new Map();
  mock.mockImplementation(async (cmd: string, a?: { args?: { filters?: { group_by?: string } } }) => {
    if (cmd === 'dashboards') return fakeDashboards(hub, a as never);
    expect(cmd).toBe('work_stats');
    const by = a?.args?.filters?.group_by;
    const since = (a?.args as { since?: number } | undefined)?.since;
    return {
      ...(since !== undefined ? { burnup: [{ day: since, total: 6, done: 2 }, { day: since + 86_400, total: 8, done: 3 }] } : {}),
      group_by: by ?? 'group',
      total: 8,
      stages,
      pct_done: 37,
      needs_you: 2,
      cost_micros: 5_000_000,
      generated_at: 1_760_000_000,
      groups:
        by === 'sprint'
          ? [
              {
                group: { id: 'sprint:4', label: 'Sprint 42', source: 'sprint' },
                total: 8, stages, pct_done: 37, needs_you: 2, cost_micros: 0,
                bucket: { bucket_id: 4, kind: 'sprint', state: 'active', goal: 'Checkout', pct_time_elapsed: 70,
                          starts_at: 1_759_000_000, ends_at: 1_761_000_000, added_after_start: 3, removed_after_start: 1 },
              },
            ]
          : by
            ? [{ group: { id: 'person:ana', label: 'Ana', source: 'person' }, total: 8, stages, pct_done: 37, needs_you: 2, cost_micros: 0 }]
            : [],
    };
  });
});
afterEach(() => {
  workspace.set('developer');
  pinnedDashboards.set([]);
  shownDashboard.set(null);
  operatorSession.set(null);
  composerDrafts.clear();
});

describe('DashboardView', () => {
  it('draws a feed as written and the fleet’s own rows', async () => {
    workspace.set('pm');
    seed({
      id: 'dash.ops',
      title: 'Ops',
      intro: '',
      widgets: [
        { kind: 'feed', title: 'Deploys today', key: 'deploys', show: 'stat' },
        { kind: 'feed', title: 'Notes', key: 'notes' },
        { kind: 'fleet', title: 'Hosts', source: 'hosts', show: 'list' },
      ],
    });
    shownDashboard.set({ id: 'dash.ops', workspace: 'pm' });
    hosts.set([{ alias: 'mac', reachable: true, hidden: false } as HostRow, { alias: 'htz', reachable: false, hidden: false } as HostRow]);
    const base = mock.getMockImplementation() as (cmd: string, a?: unknown) => Promise<unknown>;
    mock.mockImplementation(async (cmd: string, a?: { args?: { action?: string } }) => {
      if (cmd === 'dashboards' && a?.args?.action === 'feeds')
        return [{ key: 'deploys', value: '3', by: 'routine', by_ref: 'Nightly', updated_at: 1_760_000_000 }];
      return base(cmd, a);
    });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getAllByTestId('feed-value')[0].textContent).toBe('3'));
    const metas = r.getAllByTestId('feed-meta').map((m) => m.textContent ?? '');
    expect(metas[0]).toContain('by routine Nightly');
    expect(metas[1]).toContain('Nothing written to “notes” yet');
    const rows = r.getByTestId('fleet-rows').textContent ?? '';
    expect(rows).toContain('mac');
    expect(rows).toContain('offline');
    hosts.set([]);
  });

  it('draws the scrum master’s sprint health from work stats', async () => {
    workspace.set('scrum');
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByText('Sprint 42')).toBeTruthy());
    expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Sprint health');
    expect(r.getByText('Behind')).toBeTruthy();
    expect(r.getByText(/scope \+3\/−1 since start/)).toBeTruthy();
    expect(r.getByText('Ana')).toBeTruthy();
    // The sprint's burn-up asks for its days and draws them.
    expect(r.getByTestId('burnup').textContent).toContain('Done');
    const burnupAsk = mock.mock.calls.find((c) => c[1]?.args?.since !== undefined);
    expect(burnupAsk?.[1].args.filters).toMatchObject({ iteration: 'current' });
    // One read per distinct query, not per widget.
    const asked = new Set(mock.mock.calls.map((c) => JSON.stringify(c[1])));
    expect(mock.mock.calls.length).toBe(asked.size);
    await expectAccessible(r.container);
  });

  it('says what failed in the widget, and keeps the others', async () => {
    workspace.set('pm');
    mock.mockImplementation(async (_cmd: string, a?: { args?: { filters?: { group_by?: string } } }) => {
      if (a?.args?.filters?.group_by === 'mission') throw { code: 'E_INVALID', message: 'needs a newer hub' };
      return { group_by: 'group', total: 3, stages, pct_done: 0, needs_you: 0, cost_micros: 0, generated_at: 1, groups: [] };
    });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByText(/Could not read/)).toBeTruthy());
    expect(r.getAllByTestId('widget-stat').length).toBe(4);
  });

  it('shows a pinned dashboard, and hands it to the agent to change a widget', async () => {
    workspace.set('pm');
    seed({
      id: 'dash.release-42',
      title: 'Release 4.2',
      intro: 'What the release still holds.',
      widgets: [{ kind: 'groups', title: 'By person', query: { group_by: 'person' }, show: 'stages' }],
    });
    shownDashboard.set({ id: 'dash.release-42', workspace: 'pm' });
    operatorSession.set({ id: 99 } as SessionRow);
    const r = render(DashboardView);
    // Pinned on another device: the page reads it from the hub.
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Release 4.2'));
    await waitFor(() => expect(r.getByText('Ana')).toBeTruthy());
    expect(r.getByText(/Pinned on .* from the agent's proposal/)).toBeTruthy();

    await fireEvent.click(r.getByTestId('widget-change'));
    await waitFor(() => expect(composerDrafts.get(99)).toBeTruthy());
    const draft = composerDrafts.get(99)!;
    expect(draft).toContain('"id": "dash.release-42"');
    expect(draft.endsWith('Change the widget "By person": ')).toBe(true);
    expect(get(destination)).toBe('control');

    // A pick holds only in the workspace it was made in.
    workspace.set('scrum');
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Sprint health'));
  });

  it('unpins, and falls back to the workspace’s own dashboard', async () => {
    workspace.set('po');
    seed({ id: 'dash.x', title: 'Mine', intro: '', widgets: [{ kind: 'stat', title: 'Open', query: {}, value: 'open' }] });
    shownDashboard.set({ id: 'dash.x', workspace: 'po' });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Mine'));
    await fireEvent.click(r.getByTestId('dashboard-unpin'));
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Epics and releases'));
    expect(get(pinnedDashboards)).toEqual([]);
    expect(hub.size).toBe(0);
  });

  it('a change Control made says so, and Undo puts the version before back', async () => {
    workspace.set('po');
    const mine: Dashboard = { id: 'dash.x', title: 'Mine', intro: '', widgets: [{ kind: 'stat', title: 'Open', query: {}, value: 'open' }] };
    const changed: Dashboard = { ...mine, title: 'Mine, with cost', widgets: [...mine.widgets, { kind: 'stat', title: 'Spent', query: {}, value: 'cost' }] };
    hub.set('dash.x', {
      id: 'dash.x', title: changed.title, spec: dashboardBlock(changed), pinned_at: 1_760_000_000, device: 'ux-agent', can_undo: true, undo: 'restore',
      previous: { id: 'dash.x', title: 'Mine', spec: dashboardBlock(mine), pinned_at: 1_760_000_000 },
    });
    shownDashboard.set({ id: 'dash.x', workspace: 'po' });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Mine, with cost'));
    expect(r.getByTestId('dashboard-by-agent').textContent).toBe('Changed by Control');
    await fireEvent.click(r.getByTestId('dashboard-undo'));
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Mine'));
    expect(r.queryByTestId('dashboard-undo')).toBeNull();
    expect(mock.mock.calls.some((c) => c[0] === 'dashboards' && (c[1] as { args: { action: string } }).args.action === 'undo')).toBe(true);
  });

  it('opens on the person’s own copy of the built-in once Control made one', async () => {
    workspace.set('po');
    const copy: Dashboard = { id: 'dash.po.mine', title: 'My releases', intro: '', widgets: [{ kind: 'stat', title: 'Open', query: {}, value: 'open' }] };
    hub.set(copy.id, { id: copy.id, title: copy.title, spec: dashboardBlock(copy), pinned_at: 1_760_000_000, device: 'ux-agent', undo: 'remove' });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('My releases'));
    // Undo of a copy Control pinned new unpins it, once the person says so.
    await fireEvent.click(r.getByTestId('dashboard-undo'));
    await fireEvent.click(r.getByTestId('dashboard-undo-keep'));
    expect(hub.size).toBe(1);
    await fireEvent.click(r.getByTestId('dashboard-undo'));
    await fireEvent.click(r.getByTestId('dashboard-undo-confirm'));
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Epics and releases'));
    expect(hub.size).toBe(0);
  });

  it('offers no Undo on a dashboard the hub says has nothing to undo', async () => {
    workspace.set('po');
    const mine: Dashboard = { id: 'dash.y', title: 'Y', intro: '', widgets: [{ kind: 'stat', title: 'Open', query: {}, value: 'open' }] };
    hub.set(mine.id, { id: mine.id, title: mine.title, spec: dashboardBlock(mine), pinned_at: 1_760_000_000, device: 'Ada’s Mac' });
    shownDashboard.set({ id: 'dash.y', workspace: 'po' });
    const r = render(DashboardView);
    await waitFor(() => expect(r.getByRole('heading', { level: 1 }).textContent).toBe('Y'));
    expect(r.queryByTestId('dashboard-undo')).toBeNull();
  });
});
