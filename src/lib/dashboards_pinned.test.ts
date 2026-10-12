// Persona dashboards (steps 5 and 6): pinning goes through the hub's
// `dashboards` tool, a change is offered as a replacement, step 5's local
// pins move to the hub once, and "Change" hands the agent the block.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import {
  BUILTIN_DASHBOARDS,
  changePrompt,
  dashboardBlock,
  dashboardChanges,
  loadPinned,
  pinDashboard,
  pinStateOf,
  pinnedDashboards,
  unpinDashboard,
  type Dashboard,
} from './dashboards';
import { checkUiBlock } from './rich_blocks';

const mock = invoke as ReturnType<typeof vi.fn>;

/** The hub's side: rows by id, as `service::dashboards` keeps them. */
let rows: Map<string, { id: string; title: string; spec: Record<string, unknown>; why?: string; pinned_at: number }>;

const release: Dashboard = {
  id: 'dash.release-42',
  title: 'Release 4.2',
  intro: 'What the release still holds.',
  widgets: [
    { kind: 'stat', title: 'Open', query: { status: 'open' }, value: 'open' },
    { kind: 'groups', title: 'By person', query: { group_by: 'person' }, show: 'stages' },
  ],
};

beforeEach(() => {
  rows = new Map();
  pinnedDashboards.set([]);
  localStorage.clear();
  mock.mockReset();
  mock.mockImplementation(async (cmd: string, a: { args: { action: string; id?: string; dashboard?: Record<string, unknown>; why?: string } }) => {
    expect(cmd).toBe('dashboards');
    const { action, id, dashboard, why } = a.args;
    if (action === 'list') return [...rows.values()];
    if (action === 'pin') {
      const did = dashboard!.id as string;
      if (did.startsWith('dash.') && Object.values(BUILTIN_DASHBOARDS).some((d) => d.id === did))
        throw { code: 'E_INVALID', message: `${did} is a built-in dashboard's id` };
      const row = { id: did, title: dashboard!.title as string, spec: dashboard!, why, pinned_at: rows.get(did)?.pinned_at ?? 100 };
      rows.set(did, row);
      return row;
    }
    if (action === 'unpin') return { removed: rows.delete(id!) };
    throw new Error(action);
  });
});

describe('pinned dashboards', () => {
  it('pins on the hub, knows it is pinned, then offers a change as a replacement', async () => {
    expect(pinStateOf(release, [])).toBe('new');
    const r = await pinDashboard(release, 'You asked about 4.2.');
    expect(r.ok).toBe(true);
    expect(rows.get('dash.release-42')?.why).toBe('You asked about 4.2.');
    expect(get(pinnedDashboards)).toEqual([{ ...release, pinned_at: 100, why: 'You asked about 4.2.' }]);
    expect(pinStateOf(release, get(pinnedDashboards))).toBe('same');

    const next: Dashboard = {
      ...release,
      widgets: [
        { kind: 'stat', title: 'Open', query: { status: 'open', mine: true }, value: 'open' },
        { kind: 'stat', title: 'Spent', query: {}, value: 'cost' },
      ],
    };
    expect(pinStateOf(next, get(pinnedDashboards))).toBe('replace');
    expect(dashboardChanges(release, next)).toEqual(['changes “Open”', 'adds “Spent”', 'removes “By person”']);
    await pinDashboard(next);
    expect(get(pinnedDashboards).map((p) => [p.id, p.widgets.length])).toEqual([['dash.release-42', 2]]);

    await unpinDashboard(release.id);
    expect(get(pinnedDashboards)).toEqual([]);
    expect(rows.size).toBe(0);
  });

  it('says why the hub refused, and keeps the list as it was', async () => {
    const r = await pinDashboard({ ...release, id: BUILTIN_DASHBOARDS.scrum.id });
    expect(r.ok).toBe(false);
    expect(pinStateOf({ ...release, id: BUILTIN_DASHBOARDS.scrum.id }, [])).toBe('builtin');
    expect(get(pinnedDashboards)).toEqual([]);
  });

  it('reads what another device pinned, leaving out a row that no longer checks out', async () => {
    rows.set('dash.release-42', { id: 'dash.release-42', title: 'R', spec: dashboardBlock(release), pinned_at: 5 });
    rows.set('bad', { id: 'bad', title: 'B', spec: { kind: 'dashboard' }, pinned_at: 6 });
    await loadPinned();
    expect(get(pinnedDashboards).map((p) => p.id)).toEqual(['dash.release-42']);
  });

  it('moves step 5’s local pins to the hub once, then forgets them here', async () => {
    localStorage.setItem('cf:pref:ui.dashboards.pinned', JSON.stringify([{ ...release, pinned_at: 5, why: 'old' }, 'junk']));
    await loadPinned();
    expect([...rows.keys()]).toEqual(['dash.release-42']);
    expect(rows.get('dash.release-42')?.why).toBe('old');
    expect(get(pinnedDashboards).map((p) => p.id)).toEqual(['dash.release-42']);
    expect(JSON.parse(localStorage.getItem('cf:pref:ui.dashboards.pinned') ?? 'null')).toEqual([]);
    mock.mockClear();
    await loadPinned();
    expect(mock.mock.calls.map((c) => c[1].args.action)).toEqual(['list']);
  });

  it('"Change" hands the agent the dashboard as a block it can answer with', () => {
    const text = changePrompt(release, 'By person');
    expect(text.startsWith('My pinned dashboard "Release 4.2", as it is now:')).toBe(true);
    expect(text.endsWith('Change the widget "By person": ')).toBe(true);
    const json = text.match(/```fleet-ui\n([\s\S]*?)\n```/)![1];
    expect(checkUiBlock(json).ok).toBe(true);
    expect(JSON.parse(json)).toEqual(dashboardBlock(release));
  });

  it('a built-in is offered as a copy under an id of its own', () => {
    const text = changePrompt(BUILTIN_DASHBOARDS.pm);
    expect(text).toContain('keep the id dash.pm.mine');
    const json = JSON.parse(text.match(/```fleet-ui\n([\s\S]*?)\n```/)![1]);
    expect(json.id).toBe('dash.pm.mine');
    for (const d of Object.values(BUILTIN_DASHBOARDS)) {
      const c = checkUiBlock(JSON.stringify(dashboardBlock(d)));
      expect(c.ok ? [] : c.problems, d.id).toEqual([]);
    }
  });
});
