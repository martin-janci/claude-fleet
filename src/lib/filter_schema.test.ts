// Redesign step 3.7: one Filters section for the Sessions list and the Work
// view in the New layout. Closed, it is one row; open, it holds every 0.5.4
// facet, each where the typed schema says.
import { render, screen, fireEvent, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { tick } from 'svelte';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import SidebarFilters from './SidebarFilters.svelte';
import WorkFiltersBar from './WorkFiltersBar.svelte';
import { sessions, sidebarGroupBy, showBgAgents, type SessionRow } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import { orgs, scopeFilter } from './orgs';
import { trackers } from './trackers';
import { uiLayout } from './prefs';
import { DEFAULT_WORK_FILTERS, workFilters } from './work_filters';
import { activeWorkViewId, workLayout, workViewFilters } from './work_view';
import { sessionFacets, workFacets } from './filter_facets';
import {
  FILTER_SECTIONS,
  SESSION_FILTER_SCHEMA,
  SESSION_GROUPS,
  WORK_FILTER_SCHEMA,
  WORK_GROUPS,
  controlsIn,
} from './filter_schema';

let nextId = 1;
function row(over: Partial<SessionRow>): SessionRow {
  return {
    id: nextId++, tmux_name: 'dev', host_alias: 'h1', project_id: 1, worktree_id: null, created_at: 1,
    last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work',
    reviews_session_id: null, worktree_key: 'main', lost_at: null, claude_session_id: null,
    claude_status: null, effort_level: null, pr_url: null, current_activity: null, context_pct: null,
    stuck_kind: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null,
    safe_kill_detail: null, safe_kill_requested_at: null, idle_since: null, stuck_since: null,
    last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
    turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null,
    context_tokens: null, context_window: null, context_source: null, context_at: null,
    context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  } as SessionRow;
}
const project = (id: number, owner: string) =>
  ({ project: { id, owner, repo: `r${id}`, base_path: `/p${id}`, last_session_at: null, adopted: false, system: false }, worktrees: [] }) as ProjectTreeRow;
const work = (key: string, item: number) => ({
  link_id: item, item_id: item, key, title: key, source: 'manual', status_category: 'todo', archived_at: null,
});
const jira = (id: number, prefix: string) => ({
  id, provider: 'jira_cloud', name: `Jira ${prefix}`, site_url: `https://${prefix}.example`,
  state: 'ok', created_at: 0, config: { key_prefixes: [prefix] },
});

const props = {
  search: '',
  recency: 'all' as never,
  needsYouOnly: false,
  loading: false,
  loadError: null,
  onRefresh: () => {},
  showSettings: false,
  onOpenSettings: () => {},
  needsYouCount: 0,
  selectMode: false,
  toggleSelectMode: () => {},
  selectedCount: 0,
  onBulkSend: () => {},
  onBulkKill: () => {},
  clearSelected: () => {},
};

async function flush() {
  for (let i = 0; i < 10; i++) await tick();
}

/** Each control the schema places in the panel sits under its heading. */
function expectUnderHeading(panel: HTMLElement, testid: string, heading: string) {
  const el = within(panel).getByTestId(testid);
  const section = el.closest('section');
  expect(section, testid).not.toBeNull();
  expect(section!.querySelector('h3')?.textContent, testid).toBe(heading);
}

beforeEach(() => {
  uiLayout.set('new');
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'work_views' ? [] : null));
  sessions.set([]);
  projects.set([project(1, 'acme'), project(2, 'beta')]);
  orgs.set([]);
  scopeFilter.set('all');
  trackers.set([]);
  sidebarGroupBy.set('project');
  showBgAgents.set(true);
  workFilters.set({ ...DEFAULT_WORK_FILTERS });
  workViewFilters.set({});
  activeWorkViewId.set(null);
  workLayout.set('list');
});
afterEach(() => {
  uiLayout.set('classic');
});

describe('the filter schema', () => {
  it('has a control for every facet either list can name', () => {
    const all = sessionFacets({
      scope: 'org:1', host: 'h1', recency: '1d', search: 'x', needsYou: true, showBgAgents: false,
      work: { ...DEFAULT_WORK_FILTERS, tracker: 1, status: 'todo', assignee: 'mine', hasSession: 'yes' },
    });
    expect(all.length).toBe(10);
    for (const f of all) expect(SESSION_FILTER_SCHEMA[f.id], f.id).toBeTruthy();
    const work = workFacets({ org: 1, tracker: 1, status: 'open', mine: true, has: 'active', review: true, query: 'x' });
    expect(work.length).toBe(7);
    for (const f of work) expect(WORK_FILTER_SCHEMA[f.id], f.id).toBeTruthy();
  });

  it('places every control on the row or under a heading both lists share', () => {
    const schemas: Record<string, (typeof SESSION_FILTER_SCHEMA)[keyof typeof SESSION_FILTER_SCHEMA]>[] = [
      SESSION_FILTER_SCHEMA,
      WORK_FILTER_SCHEMA,
    ];
    for (const schema of schemas) {
      for (const c of Object.values(schema)) {
        expect(c.place === 'row' || (FILTER_SECTIONS as readonly string[]).includes(c.place)).toBe(true);
      }
      expect(controlsIn(schema, 'row')).toHaveLength(1);
    }
  });
});

describe('Sessions: the Filters section (New layout)', () => {
  function seed() {
    sessions.set([
      row({ project_id: 1, work: work('PAY-1', 1) } as Partial<SessionRow>),
      row({ project_id: 2, work: work('OPS-2', 2) } as Partial<SessionRow>),
    ]);
    trackers.set([jira(1, 'PAY'), jira(2, 'OPS')] as never);
    sidebarGroupBy.set('work');
  }

  it('is one row while closed: search, Filters, Group and ⋯', () => {
    seed();
    render(SidebarFilters, { props });
    const section = screen.getByTestId('filters-section');
    expect(section.children).toHaveLength(1);
    const rowEl = screen.getByTestId('filters-row');
    for (const id of ['sidebar-search', 'filters-open', 'group-select', 'view-options-open']) {
      expect(within(rowEl).getByTestId(id)).toBeTruthy();
    }
    // Classic's second row (Needs you, Select) is not drawn.
    expect(screen.queryByTestId('needs-you-filter')).toBeNull();
    expect(screen.queryByTestId('scope-select')).toBeNull();
  });

  it('holds every 0.5.4 facet under the heading the schema names', async () => {
    seed();
    render(SidebarFilters, { props });
    await fireEvent.click(screen.getByTestId('filters-open'));
    const panel = screen.getByTestId('filter-panel');
    for (const [id, c] of Object.entries(SESSION_FILTER_SCHEMA)) {
      if (c.place === 'row') expect(within(screen.getByTestId('filters-row')).getByTestId(c.testid), id).toBeTruthy();
      else expectUnderHeading(panel, c.testid, c.place);
    }
    await fireEvent.click(screen.getByTestId('filter-scope-owner:beta'));
    expect(get(scopeFilter)).toBe('owner:beta');
  });

  it('Group picks the grouping, and Select is in ⋯', async () => {
    seed();
    const toggle = vi.fn();
    render(SidebarFilters, { props: { ...props, toggleSelectMode: toggle } });
    const sel = screen.getByTestId('group-select') as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(SESSION_GROUPS.map((g) => g.id));
    await fireEvent.change(sel, { target: { value: 'host' } });
    expect(get(sidebarGroupBy)).toBe('host');
    await fireEvent.click(screen.getByTestId('view-options-open'));
    expect(screen.queryByTestId('group-by-project')).toBeNull();
    await fireEvent.click(screen.getByTestId('select-mode'));
    expect(toggle).toHaveBeenCalled();
  });

  it('Needs you, now in the panel, shows in the strip while on', () => {
    render(SidebarFilters, { props: { ...props, needsYouOnly: true } });
    expect(screen.getByTestId('active-filters').textContent).toContain('Needs you');
    expect(screen.getByTestId('filters-open').getAttribute('aria-label')).toBe('Filters, 1 active');
  });
});

describe('Work: the Filters section (New layout)', () => {
  const wOrgs = [{ id: 1, name: 'Acme', color: null }];
  const wTrackers = [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok', org_id: 1 }];

  it('is one row while closed, with the same shape as the Sessions list', async () => {
    render(WorkFiltersBar, { orgs: wOrgs, trackers: wTrackers });
    await flush();
    expect(screen.getByTestId('filters-section').children).toHaveLength(1);
    const rowEl = screen.getByTestId('filters-row');
    for (const id of ['work-search', 'work-filters-open', 'work-group-select']) {
      expect(within(rowEl).getByTestId(id)).toBeTruthy();
    }
    expect(screen.queryByTestId('work-view-select')).toBeNull();
    expect(screen.queryByTestId('work-filter-mine')).toBeNull();
  });

  it('holds every 0.5.4 facet and the saved views under the schema’s headings', async () => {
    render(WorkFiltersBar, { orgs: wOrgs, trackers: wTrackers });
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    const panel = screen.getByTestId('work-filter-panel');
    for (const [id, c] of Object.entries(WORK_FILTER_SCHEMA)) {
      if (c.place === 'row') expect(within(screen.getByTestId('filters-row')).getByTestId(c.testid), id).toBeTruthy();
      else expectUnderHeading(panel, c.testid, c.place);
    }
    await fireEvent.click(screen.getByTestId('work-filter-mine'));
    expect(get(workViewFilters).mine).toBe(true);
    expect(screen.getByTestId('work-active-filters').textContent).toContain('Assigned to me');
  });

  it('Group switches List (by status) and Grouped (by organisation)', async () => {
    render(WorkFiltersBar, { orgs: wOrgs, trackers: wTrackers });
    await flush();
    const sel = screen.getByTestId('work-group-select') as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(WORK_GROUPS.map((g) => g.id));
    await fireEvent.change(sel, { target: { value: 'grouped' } });
    expect(get(workLayout)).toBe('grouped');
  });
});
