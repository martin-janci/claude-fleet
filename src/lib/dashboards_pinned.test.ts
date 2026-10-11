// Persona dashboards (step 5): what pinning an agent's proposal does, what
// "Change" hands the agent, and that a stored list is checked again.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
import {
  BUILTIN_DASHBOARDS,
  MAX_PINNED,
  changePrompt,
  dashboardBlock,
  dashboardChanges,
  pinDashboard,
  pinStateOf,
  pinnedDashboards,
  unpinDashboard,
  type Dashboard,
} from './dashboards';
import { checkUiBlock } from './rich_blocks';

const release: Dashboard = {
  id: 'dash.release-42',
  title: 'Release 4.2',
  intro: 'What the release still holds.',
  widgets: [
    { kind: 'stat', title: 'Open', query: { status: 'open' }, value: 'open' },
    { kind: 'groups', title: 'By person', query: { group_by: 'person' }, show: 'stages' },
  ],
};

beforeEach(() => pinnedDashboards.set([]));

describe('pinned dashboards', () => {
  it('pins a proposal, then knows it is pinned, then offers a change as a replacement', () => {
    expect(pinStateOf(release, [])).toBe('new');
    expect(pinDashboard(release, 'You asked about 4.2.', 100)).toBe(true);
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
    pinDashboard(next, undefined, 200);
    // Replaced in place, not added.
    expect(get(pinnedDashboards).map((p) => [p.id, p.pinned_at, p.widgets.length])).toEqual([['dash.release-42', 200, 2]]);

    unpinDashboard(release.id);
    expect(get(pinnedDashboards)).toEqual([]);
  });

  it('never takes a built-in’s id, and stops at the device’s limit', () => {
    const fake = { ...release, id: BUILTIN_DASHBOARDS.scrum.id };
    expect(pinStateOf(fake, [])).toBe('builtin');
    expect(pinDashboard(fake)).toBe(false);
    for (let k = 0; k < MAX_PINNED; k++) expect(pinDashboard({ ...release, id: `d${k}` })).toBe(true);
    expect(pinStateOf(release, get(pinnedDashboards))).toBe('full');
    expect(pinDashboard(release)).toBe(false);
    // Replacing one already pinned still works when full.
    expect(pinDashboard({ ...release, id: 'd3', title: 'Renamed' })).toBe(true);
  });

  it('"Change" hands the agent the dashboard as a block it can answer with', () => {
    const text = changePrompt(release, 'By person');
    expect(text.startsWith('My pinned dashboard "Release 4.2", as it is now:')).toBe(true);
    expect(text.endsWith('Change the widget "By person": ')).toBe(true);
    const json = text.match(/```fleet-ui\n([\s\S]*?)\n```/)![1];
    const c = checkUiBlock(json);
    expect(c.ok).toBe(true);
    expect(JSON.parse(json)).toEqual(dashboardBlock(release));
  });

  it('a built-in is offered as a copy under an id of its own', () => {
    const text = changePrompt(BUILTIN_DASHBOARDS.pm);
    expect(text).toContain('keep the id dash.pm.mine');
    expect(text.endsWith('Change it: ')).toBe(true);
    const json = JSON.parse(text.match(/```fleet-ui\n([\s\S]*?)\n```/)![1]);
    expect(json.id).toBe('dash.pm.mine');
    // Every built-in passes the block check an agent's answer must pass.
    for (const d of Object.values(BUILTIN_DASHBOARDS)) {
      const c = checkUiBlock(JSON.stringify(dashboardBlock(d)));
      expect(c.ok ? [] : c.problems, d.id).toEqual([]);
    }
  });

  it('drops a stored entry that no longer checks out', async () => {
    localStorage.setItem(
      'cf:pref:ui.dashboards.pinned',
      JSON.stringify([
        { ...release, pinned_at: 5 },
        { ...release, id: 'bad', widgets: [{ kind: 'stat', title: 'X', query: { sprint: 'now' }, value: 'open' }] },
        'junk',
      ]),
    );
    vi.resetModules();
    const fresh = await import('./dashboards');
    expect(get(fresh.pinnedDashboards).map((p) => p.id)).toEqual(['dash.release-42']);
    localStorage.removeItem('cf:pref:ui.dashboards.pinned');
  });
});
