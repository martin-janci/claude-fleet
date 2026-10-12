// "Show its count on the rail" (gap plan G2.2): a saved view chosen in the
// Work filters (when it is saved, or later from the active view) puts its
// task count — the hub's tree total for its filters — on the rail's Work
// item, quietly; deleting the view, or the view going elsewhere, takes the
// count off again.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import AppRail from './AppRail.svelte';
import WorkFiltersBar from './WorkFiltersBar.svelte';
import { activeWorkViewId, workViewFilters, type WorkView } from './work_view';
import { railWorkView, railWorkViewId, refreshRailWorkView } from './work_rail_view';

type Handler = (args: Record<string, unknown>) => unknown;
let handlers: Record<string, Handler>;
let views: WorkView[];

function calls(cmd: string) {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === cmd)
    .map((c) => (c[1] as { args: Record<string, unknown> }).args);
}

async function flush() {
  for (let i = 0; i < 12; i++) await tick();
}

async function inPanel(testid: string): Promise<HTMLElement> {
  if (!screen.queryByTestId('work-filter-panel')) {
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await tick();
  }
  return screen.getByTestId(testid);
}

describe('a saved view’s count on the rail', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    railWorkViewId.set(null);
    railWorkView.set(null);
    workViewFilters.set({});
    activeWorkViewId.set(null);
    views = [{ id: 1, name: 'Mine', filters: { mine: true }, version: 1 }];
    handlers = {
      work_views: () => views,
      work_tree: (a) => ({
        tasks: [],
        groups: [],
        orgs: [],
        trackers: [],
        total: (a.filters as { has?: string }).has === 'none' ? 7 : 3,
      }),
      save_work_view: (a) => {
        const v = { ...(a.view as WorkView), id: 2, version: 1 };
        views = [...views, v];
        return v;
      },
      delete_work_view: () => {
        views = views.filter((v) => v.id !== 2);
        return { deleted: true };
      },
    };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });
  afterEach(() => {
    railWorkViewId.set(null);
    railWorkView.set(null);
  });

  it('Save view with “Show its count on the rail” puts its count on the Work item', async () => {
    const rail = render(AppRail, { isMac: true, onselect: () => {} });
    await flush();
    expect(rail.queryByTestId('work-rail-count')).toBeNull();

    workViewFilters.set({ has: 'none' });
    render(WorkFiltersBar, { orgs: [], trackers: [] });
    await flush();
    await fireEvent.click(await inPanel('work-view-save-as'));
    await fireEvent.input(await inPanel('work-view-name'), { target: { value: 'Untouched' } });
    await fireEvent.click(await inPanel('work-view-rail'));
    await fireEvent.click(await inPanel('work-view-save'));
    await flush();
    expect(get(railWorkViewId)).toBe(2);
    // The count is the hub's tree total for that view's own filters.
    const tree = calls('work_tree').at(-1)!;
    expect(tree).toMatchObject({ filters: { has: 'none' }, limit: 1, per_task: 0 });
    const badge = rail.getByTestId('work-rail-count');
    expect(badge.textContent).toBe('7');
    expect(badge.getAttribute('aria-label')).toBe('7 in Untouched');
    expect(badge.classList.contains('badge--quiet')).toBe(true);
    expect(rail.getByTestId('rail-work').title).toContain('Untouched: 7');

    // Deleting the view takes it off the rail.
    await fireEvent.click(await inPanel('work-view-delete'));
    await flush();
    expect(get(railWorkViewId)).toBeNull();
    expect(rail.queryByTestId('work-rail-count')).toBeNull();
  });

  it('the active view’s toggle puts it on, and takes it off, the rail', async () => {
    activeWorkViewId.set(1);
    render(WorkFiltersBar, { orgs: [], trackers: [] });
    await flush();
    const toggle = await inPanel('work-view-rail-toggle');
    expect(toggle.getAttribute('aria-pressed')).toBe('false');
    await fireEvent.click(toggle);
    await flush();
    expect(get(railWorkViewId)).toBe(1);
    await refreshRailWorkView();
    expect(get(railWorkView)).toEqual({ id: 1, name: 'Mine', count: 3 });
    expect(screen.getByTestId('work-view-rail-toggle').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(screen.getByTestId('work-view-rail-toggle'));
    await flush();
    expect(get(railWorkViewId)).toBeNull();
  });

  it('a view deleted elsewhere drops off; a failed read keeps the last count', async () => {
    railWorkViewId.set(1);
    await refreshRailWorkView();
    expect(get(railWorkView)?.count).toBe(3);
    handlers.work_tree = () => {
      throw { code: 'E_IO', message: 'hub away' };
    };
    await refreshRailWorkView();
    expect(get(railWorkView)?.count).toBe(3);
    views = [];
    await refreshRailWorkView();
    expect(get(railWorkViewId)).toBeNull();
    expect(get(railWorkView)).toBeNull();
  });
});
