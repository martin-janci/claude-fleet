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
import { sessions, sidebarGroupBy, showBgAgents, showFriendlyNames, showRowDetails, type SessionRow } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import { orgs, scopeFilter } from './orgs';
import { trackers } from './trackers';
import { DEFAULT_WORK_FILTERS, workFilters } from './work_filters';
import { activeWorkViewId, workLayout, workViewFilters } from './work_view';
import { sessionFacets, workFacets } from './filter_facets';
import {
  FILTER_SECTIONS,
  SESSION_FILTER_SCHEMA,
  SESSION_GROUPS,
  WORK_FILTER_SCHEMA,
  WORK_FILTER_SECTIONS,
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
});

describe('the filter schema', () => {
  it('has a control for every facet either list can name', () => {
    const all = sessionFacets({
      scope: 'org:1', host: 'h1', recency: '1d', search: 'x', needsYou: true, showBgAgents: false,
      work: { ...DEFAULT_WORK_FILTERS, tracker: 1, status: 'todo', assignee: 'mine', hasSession: 'yes' },
    });
    expect(all.length).toBe(10);
    for (const f of all) expect(SESSION_FILTER_SCHEMA[f.id], f.id).toBeTruthy();
    const work = workFacets({
      org: 1, orgs: [1, 'none'], tracker: 1, status: 'open', stages: ['blocked'], status_name: 'QA Review', mine: true,
      assignee: 'Ana', has: 'active', review: true, query: 'x',
    });
    expect(work.length).toBe(11);
    for (const f of work) expect(WORK_FILTER_SCHEMA[f.id], f.id).toBeTruthy();
  });

  it('places every control on the row or under one of its list’s headings', () => {
    for (const [schema, headings] of [
      [SESSION_FILTER_SCHEMA, FILTER_SECTIONS],
      [WORK_FILTER_SCHEMA, WORK_FILTER_SECTIONS],
    ] as [Record<string, { label: string; place: string; testid: string }>, readonly string[]][]) {
      for (const c of Object.values(schema)) {
        expect(c.place === 'row' || headings.includes(c.place)).toBe(true);
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
    // There is no second row: Needs you is in the panel, Select in ⋯.
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

  it('Friendly names and Row details are switches in ⋯ that flip their prefs and persist', async () => {
    showFriendlyNames.set(true);
    showRowDetails.set(true);
    render(SidebarFilters, { props });
    await fireEvent.click(screen.getByTestId('view-options-open'));
    const menu = screen.getByTestId('view-options');
    const friendly = within(menu).getByTestId('friendly-name-toggle');
    const details = within(menu).getByTestId('toggle-row-details');
    expect(friendly.getAttribute('aria-checked')).toBe('true');
    expect(details.getAttribute('aria-checked')).toBe('true');
    await fireEvent.click(friendly);
    expect(get(showFriendlyNames)).toBe(false);
    expect(friendly.getAttribute('aria-checked')).toBe('false');
    expect(JSON.parse(localStorage.getItem('cf:pref:show-friendly-names')!)).toBe(false);
    await fireEvent.click(details);
    expect(get(showRowDetails)).toBe(false);
    expect(details.getAttribute('aria-checked')).toBe('false');
    expect(JSON.parse(localStorage.getItem('cf:pref:rows.details')!)).toBe(false);
    showFriendlyNames.set(true);
    showRowDetails.set(true);
  });

  it('Needs you, now in the panel, shows in the strip while on', () => {
    render(SidebarFilters, { props: { ...props, needsYouOnly: true } });
    expect(screen.getByTestId('active-filters').textContent).toContain('Needs you');
    expect(screen.getByTestId('filters-open').getAttribute('aria-label')).toBe('Filters, 1 active');
  });
});

describe('Work: the Filters section (board “Work · tasks with filters open”)', () => {
  const wOrgs = [
    { id: 1, name: 'Acme', color: null },
    { id: 2, name: 'Beta', color: null },
  ];
  const wTrackers = [{ id: 1, name: 'Jira (acme)', provider: 'jira', state: 'ok', org_id: 1 }];
  const props = { orgs: wOrgs, trackers: wTrackers, people: ['Ana Novak', 'Ben'], columns: ['QA Review'] };

  it('is search, then Filters and Group, while closed', async () => {
    render(WorkFiltersBar, props);
    await flush();
    for (const id of ['work-search', 'work-filters-open', 'work-group-select']) expect(screen.getByTestId(id)).toBeTruthy();
    expect(screen.queryByTestId('work-filter-panel')).toBeNull();
    expect(screen.getByTestId('work-filters-open').getAttribute('aria-expanded')).toBe('false');
  });

  it('holds every facet and the saved views under the schema’s headings', async () => {
    render(WorkFiltersBar, props);
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    const panel = screen.getByTestId('work-filter-panel');
    for (const [id, c] of Object.entries(WORK_FILTER_SCHEMA)) {
      if (c.place === 'row') expect(screen.getByTestId(c.testid), id).toBeTruthy();
      else expectUnderHeading(panel, c.testid, c.place);
    }
  });

  it('picks several organisations and statuses at once, and Clear drops them', async () => {
    render(WorkFiltersBar, props);
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await fireEvent.click(screen.getByTestId('work-filter-org-2'));
    await fireEvent.click(screen.getByTestId('work-filter-org-1'));
    await fireEvent.click(screen.getByTestId('work-filter-stage-in_progress'));
    await fireEvent.click(screen.getByTestId('work-filter-stage-blocked'));
    expect(get(workViewFilters)).toMatchObject({ orgs: [1, 2], stages: ['in_progress', 'blocked'] });
    expect(screen.getByTestId('work-filter-org-1').getAttribute('aria-pressed')).toBe('true');
    expect(screen.getByTestId('work-filters-open').getAttribute('aria-label')).toBe('Filters, 2 active');
    await fireEvent.click(screen.getByTestId('work-filter-stage-blocked'));
    expect(get(workViewFilters).stages).toEqual(['in_progress']);
    await fireEvent.click(screen.getByTestId('work-filter-panel-clear'));
    expect(get(workViewFilters)).toEqual({});
  });

  it('a saved view’s single org reads as a picked chip and folds into the set', async () => {
    workViewFilters.set({ org: 2 });
    render(WorkFiltersBar, props);
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    expect(screen.getByTestId('work-filter-org-2').getAttribute('aria-pressed')).toBe('true');
    await fireEvent.click(screen.getByTestId('work-filter-org-1'));
    expect(get(workViewFilters)).toEqual({ orgs: [1, 2] });
  });

  it('Assignee is anyone, me or one person; the live-session switch is “has: active”', async () => {
    render(WorkFiltersBar, props);
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    const who = screen.getByTestId('work-filter-assignee') as HTMLSelectElement;
    await fireEvent.change(who, { target: { value: 'me' } });
    expect(get(workViewFilters)).toMatchObject({ mine: true });
    await fireEvent.change(who, { target: { value: '@ana novak' } });
    expect(get(workViewFilters)).toMatchObject({ assignee: 'Ana Novak' });
    expect(get(workViewFilters).mine).toBeUndefined();
    await fireEvent.click(screen.getByTestId('work-filter-live'));
    expect(get(workViewFilters).has).toBe('active');
    await fireEvent.change(screen.getByTestId('work-filter-tracker'), { target: { value: 'local' } });
    expect(get(workViewFilters).tracker).toBe('local');
    // Closed, the strip names what is on.
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    const strip = screen.getByTestId('work-active-filters').textContent ?? '';
    expect(strip).toContain('Assignee: Ana Novak');
    expect(strip).toContain('Tracker: Local work');
  });

  it('More keeps the tracker column, To review and the saved views', async () => {
    render(WorkFiltersBar, props);
    await flush();
    await fireEvent.click(screen.getByTestId('work-filters-open'));
    await fireEvent.click(screen.getByTestId('work-filter-column-qa review'));
    await fireEvent.click(screen.getByTestId('work-filter-review'));
    expect(get(workViewFilters)).toMatchObject({ status_name: 'QA Review', review: true });
    expect(screen.getByTestId('work-filter-more').textContent).toContain('2');
  });

  it('Group picks List, or Grouped with each org by group, org, person, mission, account or repo', async () => {
    render(WorkFiltersBar, props);
    await flush();
    const sel = screen.getByTestId('work-group-select') as HTMLSelectElement;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual(WORK_GROUPS.map((g) => g.id));
    await fireEvent.change(sel, { target: { value: 'person' } });
    expect(get(workLayout)).toBe('grouped');
    expect(get(workViewFilters).group_by).toBe('person');
    // The task's own group is the default, so it is not sent.
    await fireEvent.change(sel, { target: { value: 'group' } });
    expect(get(workViewFilters).group_by).toBeUndefined();
    await fireEvent.change(sel, { target: { value: 'list' } });
    expect(get(workLayout)).toBe('list');
  });
});
