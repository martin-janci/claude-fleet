// The Work view's filters and saved views (work graph M14): each control
// writes the one filters object; views are applied, saved as, updated with
// their version and deleted; a lost race says so and reloads.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import WorkFiltersBar from './WorkFiltersBar.svelte';
import { activeWorkViewId, bumpWorkChanged, noteWorkChanged, workViewFilters, type WorkView } from './work_view';

const orgs = [{ id: 1, name: 'Acme', color: null }];
const trackers = [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok', org_id: 1 }];

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
  for (let i = 0; i < 10; i++) await tick();
}

/** Saved views live in the Filters panel (step 3.7): open it if needed. */
async function inPanel(testid: string): Promise<HTMLElement> {
  if (!screen.queryByTestId('work-filter-panel')) {
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await tick();
  }
  return screen.getByTestId(testid);
}

describe('WorkFiltersBar', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    workViewFilters.set({});
    activeWorkViewId.set(null);
    views = [{ id: 1, name: 'My open work', filters: { mine: true, status: 'open' }, version: 1, updated_at: 1 }];
    handlers = { work_views: () => views };
    vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
      const h = handlers[cmd];
      return h ? h((raw as { args: Record<string, unknown> } | undefined)?.args ?? {}) : null;
    });
  });

  it('re-reads the saved views on a view change, not on a placement or a session tick', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
    try {
      render(WorkFiltersBar, { orgs, trackers });
      await flush();
      const n = calls('work_views').length;
      noteWorkChanged([{ what: 'placement', task_id: 'item:1' }]);
      bumpWorkChanged('session');
      vi.advanceTimersByTime(600);
      await flush();
      expect(calls('work_views')).toHaveLength(n);
      noteWorkChanged([{ what: 'view', view_id: 1 }]);
      vi.advanceTimersByTime(600);
      await flush();
      expect(calls('work_views')).toHaveLength(n + 1);
      // A write this window made may have moved anything.
      bumpWorkChanged();
      vi.advanceTimersByTime(600);
      await flush();
      expect(calls('work_views')).toHaveLength(n + 2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('every control writes the filters; the search is debounced', async () => {
    render(WorkFiltersBar, { orgs, trackers, searchDebounceMs: 5 });
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await fireEvent.click(screen.getByTestId(`work-filter-org-${orgs[0].id}`));
    await fireEvent.change(screen.getByTestId('work-filter-tracker'), { target: { value: '1' } });
    await fireEvent.click(screen.getByTestId('work-filter-stage-in_progress'));
    await fireEvent.click(screen.getByTestId('work-filter-has-past_only'));
    await fireEvent.change(screen.getByTestId('work-filter-assignee'), { target: { value: 'me' } });
    await fireEvent.click(screen.getByTestId('work-filter-review'));
    await fireEvent.input(screen.getByTestId('work-search'), { target: { value: 'login' } });
    expect(get(workViewFilters).query).toBeUndefined();
    await new Promise((r) => setTimeout(r, 20));
    expect(get(workViewFilters)).toEqual({
      orgs: [orgs[0].id],
      tracker: 1,
      stages: ['in_progress'],
      has: 'past_only',
      mine: true,
      review: true,
      query: 'login',
    });
    // The panel counts what it holds; closed, the strip names every filter.
    expect(screen.getByTestId('work-filters-open')).toHaveAttribute('aria-label', 'Filters, 6 active');
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    expect(screen.getByTestId('facet-orgs')).toHaveTextContent(`Org: ${orgs[0].name}`);
    expect(screen.getByTestId('facet-tracker')).toHaveTextContent('Tracker: Jira (acme)');
    expect(screen.getByTestId('facet-stages')).toHaveTextContent('Status: In progress');
    // Search shows its own state: no chip for it.
    expect(screen.queryByTestId('facet-query')).toBeNull();
    expect(screen.getByTestId('facet-mine')).toBeTruthy();
    // A chip's × removes just that filter.
    await fireEvent.click(screen.getByTestId('facet-orgs'));
    expect(get(workViewFilters).orgs).toBeUndefined();
    expect(get(workViewFilters).tracker).toBe(1);
    await fireEvent.click(screen.getByTestId('work-filter-clear'));
    expect(get(workViewFilters)).toEqual({});
    expect((screen.getByTestId('work-search') as HTMLInputElement).value).toBe('');
  });

  it('a filter changing while the search is typed does not overwrite the typing', async () => {
    render(WorkFiltersBar, { orgs, trackers, searchDebounceMs: 20 });
    await flush();
    const input = screen.getByTestId('work-search') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: 'log' } });
    workViewFilters.update((f) => ({ ...f, status: 'open' }));
    await flush();
    expect(input.value).toBe('log');
    await new Promise((r) => setTimeout(r, 40));
    await flush();
    expect(get(workViewFilters)).toEqual({ status: 'open', query: 'log' });
    expect(input.value).toBe('log');
  });

  it('a search typed but not yet applied when the bar goes is applied, not lost (review r07)', async () => {
    const { unmount } = render(WorkFiltersBar, { orgs, trackers, searchDebounceMs: 10_000 });
    await flush();
    await fireEvent.input(screen.getByTestId('work-search'), { target: { value: 'login' } });
    unmount();
    expect(get(workViewFilters)).toEqual({ query: 'login' });
  });

  it('applies a view; Update is offered once the filters differ, with the view’s version', async () => {
    handlers.save_work_view = (a) => ({ ...(a.view as object), version: 2 });
    render(WorkFiltersBar, { orgs, trackers });
    await flush();
    await fireEvent.change((await inPanel('work-view-select')), { target: { value: '1' } });
    await flush();
    expect(get(activeWorkViewId)).toBe(1);
    expect(get(workViewFilters)).toEqual({ mine: true, status: 'open' });
    expect((await inPanel('work-view-update')).hasAttribute('disabled')).toBe(true);
    await fireEvent.click(screen.getByTestId('work-filter-review'));
    await flush();
    expect((await inPanel('work-view-update')).hasAttribute('disabled')).toBe(false);
    await fireEvent.click(await inPanel('work-view-update'));
    await flush();
    expect(calls('save_work_view')[0]).toEqual({
      view: { id: 1, name: 'My open work', filters: { status: 'open', mine: true, review: true }, expected_version: 1 },
    });
  });

  it('Save as… creates with expected_version 0 and selects it', async () => {
    handlers.save_work_view = (a) => {
      const v = { ...(a.view as WorkView), id: 2, version: 1 };
      views = [...views, v];
      return v;
    };
    workViewFilters.set({ has: 'none' });
    render(WorkFiltersBar, { orgs, trackers });
    await flush();
    await fireEvent.click(await inPanel('work-view-save-as'));
    await fireEvent.input((await inPanel('work-view-name')), { target: { value: 'Untouched' } });
    await fireEvent.click(await inPanel('work-view-save'));
    await flush();
    expect(calls('save_work_view')[0]).toEqual({ view: { name: 'Untouched', filters: { has: 'none' }, expected_version: 0 } });
    expect(get(activeWorkViewId)).toBe(2);
  });

  it('a lost race says so and reloads the views', async () => {
    handlers.save_work_view = () => {
      throw { code: 'E_CONFLICT', message: 'version', details: { version: 3 } };
    };
    render(WorkFiltersBar, { orgs, trackers });
    await flush();
    await fireEvent.change((await inPanel('work-view-select')), { target: { value: '1' } });
    await fireEvent.click(screen.getByTestId('work-filter-review'));
    await flush();
    await fireEvent.click(await inPanel('work-view-update'));
    await flush();
    expect(screen.getByTestId('work-view-notice').textContent).toContain('changed elsewhere');
    expect(calls('work_views').length).toBe(2);
  });

  it('Delete forgets the view', async () => {
    handlers.delete_work_view = () => {
      views = [];
      return { deleted: true };
    };
    activeWorkViewId.set(1);
    render(WorkFiltersBar, { orgs, trackers });
    await flush();
    await fireEvent.click(await inPanel('work-view-delete'));
    await flush();
    // A compare-and-set on the version the person saw.
    expect(calls('delete_work_view')[0]).toEqual({ view_id: 1, expected_version: 1 });
    expect(get(activeWorkViewId)).toBeNull();
  });

  it('a Delete that lost a race keeps the view and shows its current version, with Reload', async () => {
    handlers.delete_work_view = () => {
      views = [{ ...views[0], version: 4 }];
      throw { code: 'E_CONFLICT', message: 'view 1 was changed', details: { view_id: 1, version: 4 } };
    };
    activeWorkViewId.set(1);
    render(WorkFiltersBar, { orgs, trackers });
    await flush();
    await fireEvent.click(await inPanel('work-view-delete'));
    await flush();
    expect(get(activeWorkViewId)).toBe(1);
    const notice = screen.getByTestId('work-view-notice');
    expect(notice.textContent).toContain('changed elsewhere');
    expect(screen.getByTestId('work-conflict-current').textContent).toBe('Now: version 4');
    const before = calls('work_views').length;
    await fireEvent.click(screen.getByTestId('work-conflict-reload'));
    await flush();
    expect(calls('work_views').length).toBe(before + 1);
    // The next Delete names the version it now sees.
    handlers.delete_work_view = () => ({ deleted: true });
    await fireEvent.click(await inPanel('work-view-delete'));
    await flush();
    expect(calls('delete_work_view')[1]).toEqual({ view_id: 1, expected_version: 4 });
  });

  it('in the List layout the Archived switch is off-limits: Done shows them', async () => {
    render(WorkFiltersBar, { orgs, trackers, listLayout: true });
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await flush();
    const sw = screen.getByTestId('work-filter-archived') as HTMLButtonElement;
    expect(sw.disabled).toBe(true);
    expect(sw.title).toBe('In List view, Done shows them');
  });
});
