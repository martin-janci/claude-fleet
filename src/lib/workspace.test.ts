import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { RAIL_ITEMS, visibleRailItems } from './rail';
import { activeWorkViewId, workLayout, workViewFilters } from './work_view';
import {
  WORKSPACES,
  WORKSPACE_IDS,
  switchWorkspace,
  workspace,
  workspaceLanding,
  workspacePreset,
  workspaceRailItems,
} from './workspace';

describe('workspaces', () => {
  beforeEach(() => {
    localStorage.clear();
    workspace.set('developer');
    workViewFilters.set({});
    activeWorkViewId.set(null);
    workLayout.set('list');
    workspaceLanding.set(null);
  });

  it('Developer keeps the rail in the manual’s order', () => {
    expect(workspaceRailItems('developer', visibleRailItems()).map((i) => i.id)).toEqual(RAIL_ITEMS.map((i) => i.id));
  });

  it('every workspace keeps every rail item and Settings last: moved, never hidden', () => {
    for (const id of WORKSPACE_IDS) {
      const ids = workspaceRailItems(id, visibleRailItems()).map((i) => i.id);
      expect([...ids].sort(), id).toEqual(RAIL_ITEMS.map((i) => i.id).sort());
      expect(ids.at(-1), id).toBe('settings');
      expect(ids.indexOf('inbox'), `${id}: Needs you stays near the top`).toBeLessThan(2);
    }
  });

  it('features the preset’s items first, in its order', () => {
    const ids = workspaceRailItems('pm', visibleRailItems()).map((i) => i.id);
    expect(ids.slice(0, 4)).toEqual(['inbox', 'control', 'work', 'automation']);
    expect(ids.slice(4)).toEqual(['sessions', 'accounts', 'toolkit', 'settings']);
  });

  it('every preset lands on a rail item it features', () => {
    for (const w of WORKSPACES) expect(w.rail, w.id).toContain(w.landing);
  });

  it('a switch applies the preset’s Work filters on a first visit and asks to land', () => {
    workViewFilters.set({ mine: true });
    expect(switchWorkspace('scrum')).toBe(true);
    expect(get(workspace)).toBe('scrum');
    expect(get(workViewFilters)).toEqual(workspacePreset('scrum').work);
    expect(get(workLayout)).toBe('grouped');
    expect(get(workspaceLanding)?.rail).toBe('work');
    expect(localStorage.getItem('cf:pref:ui.workspace')).toBe('"scrum"');
  });

  it('each workspace keeps its own Work filters across switches', () => {
    workViewFilters.set({ mine: true });
    activeWorkViewId.set(7);
    switchWorkspace('po');
    workViewFilters.set({ group_by: 'epic', query: 'checkout' });
    switchWorkspace('developer');
    expect(get(workViewFilters)).toEqual({ mine: true });
    expect(get(activeWorkViewId)).toBe(7);
    expect(get(workLayout)).toBe('list');
    switchWorkspace('po');
    expect(get(workViewFilters)).toEqual({ group_by: 'epic', query: 'checkout' });
    expect(get(activeWorkViewId)).toBeNull();
  });

  it('switching to the active workspace does nothing', () => {
    expect(switchWorkspace('developer')).toBe(false);
    expect(get(workspaceLanding)).toBeNull();
  });
});
