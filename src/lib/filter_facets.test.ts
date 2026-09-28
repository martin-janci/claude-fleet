// The active-filter summary: one chip per filter that narrows the list, in
// the same words for the Sessions list and the Work view.
import { describe, it, expect } from 'vitest';
import { clearWorkFilterPatch, facetSentence, sessionFacets, withoutWorkFacet, workFacets } from './filter_facets';
import { DEFAULT_WORK_FILTERS, withMineReady } from './work_filters';
import { effectiveHostOf } from './hosts';

const none = {
  scope: 'all',
  host: 'all',
  recency: 'all' as const,
  search: '',
  needsYou: false,
  showBgAgents: true,
  work: DEFAULT_WORK_FILTERS,
};

describe('sessionFacets', () => {
  it('is empty when nothing narrows the list', () => {
    expect(sessionFacets(none)).toEqual([]);
  });

  it('names every filter that narrows, in reading order', () => {
    const f = sessionFacets({
      ...none,
      scope: 'org:1',
      scopeLabel: 'Acme',
      host: 'gpu-box',
      recency: '1d',
      search: '  login ',
      needsYou: true,
      showBgAgents: false,
      work: { tracker: 2, status: 'in_progress', assignee: 'mine', hasSession: 'no', archived: false },
      trackerName: (id) => (id === 2 ? 'Jira' : undefined),
    });
    expect(f.map((x) => x.label)).toEqual([
      'Needs you',
      'Org: Acme',
      'Host: gpu-box',
      'Last 1d',
      'Search: “login”',
      'Tracker: Jira',
      'Status: In progress',
      'Assigned to me',
      'Session: Past only',
      'Background agents hidden',
    ]);
  });

  it('archived rows hidden by default is not a filter the user set', () => {
    expect(sessionFacets({ ...none, work: { ...DEFAULT_WORK_FILTERS, archived: false } })).toEqual([]);
  });

  it('a tracker column reads as a column, not a status', () => {
    const f = sessionFacets({ ...none, work: { ...DEFAULT_WORK_FILTERS, status: 'name:QA Review' } });
    expect(f).toEqual([{ id: 'wf-status', label: 'Column: QA Review' }]);
  });

  it('clearing a work facet resets that one field to its default', () => {
    expect(clearWorkFilterPatch('wf-session')).toEqual({ hasSession: 'any' });
    expect(clearWorkFilterPatch('wf-mine')).toEqual({ assignee: 'all' });
    expect(clearWorkFilterPatch('host')).toBeNull();
  });
});

describe('workFacets', () => {
  it('names org, tracker, status, mine, sessions, review and search', () => {
    const f = workFacets(
      { org: 'none', tracker: 'local', status: 'open', mine: true, has: 'none', review: true, query: 'pay' },
      {},
    );
    expect(f.map((x) => x.label)).toEqual([
      'Org: Unassigned',
      'Tracker: Local work',
      'Status: Open',
      'Assigned to me',
      'Sessions: No session',
      'To review',
      'Search: “pay”',
    ]);
    expect(workFacets({ org: 3, tracker: 4 }, { orgName: () => 'Acme', trackerName: () => 'Linear' }).map((x) => x.label)).toEqual([
      'Org: Acme',
      'Tracker: Linear',
    ]);
  });

  it('defaults are not filters, and group is navigation', () => {
    expect(workFacets({ status: 'any', has: 'any', mine: false, query: ' ', group: 'x' } as never)).toEqual([]);
  });

  it('removes one facet and keeps the rest', () => {
    expect(withoutWorkFacet({ org: 1, status: 'done', query: 'x' }, 'status')).toEqual({ org: 1, query: 'x' });
  });

  it('reads as one sentence for an empty state', () => {
    expect(facetSentence([{ id: 'a', label: 'Host: x' }, { id: 'b', label: 'Last 1d' }])).toBe('Host: x, Last 1d');
  });
});

describe('the guards that keep a stale filter from emptying the list', () => {
  const list = [
    { alias: 'local', hidden: false },
    { alias: 'old', hidden: true },
  ];
  it('a remembered host that is gone or hidden reads as all', () => {
    expect(effectiveHostOf('local', list)).toBe('local');
    expect(effectiveHostOf('old', list)).toBe('all');
    expect(effectiveHostOf('removed', list)).toBe('all');
    // Before the hosts load, the remembered one stands.
    expect(effectiveHostOf('removed', [])).toBe('removed');
  });

  it('"mine" waits for the mine view to load', () => {
    const f = { ...DEFAULT_WORK_FILTERS, assignee: 'mine' as const };
    expect(withMineReady(f, false).assignee).toBe('all');
    expect(withMineReady(f, true).assignee).toBe('mine');
  });
});

describe('archived by default', () => {
  it('a v1 pref (which always stored archived: true) starts hidden; v2 is kept as is', async () => {
    const { readWorkFilters } = await import('./work_filters');
    localStorage.setItem('cf:pref:sidebar.work-filters', JSON.stringify({ ...DEFAULT_WORK_FILTERS, status: 'todo', archived: true }));
    localStorage.removeItem('cf:pref:sidebar.work-filters.v2');
    expect(readWorkFilters()).toMatchObject({ status: 'todo', archived: false });
    localStorage.setItem('cf:pref:sidebar.work-filters.v2', JSON.stringify({ ...DEFAULT_WORK_FILTERS, archived: true }));
    expect(readWorkFilters().archived).toBe(true);
    localStorage.removeItem('cf:pref:sidebar.work-filters');
    localStorage.removeItem('cf:pref:sidebar.work-filters.v2');
  });

  it('"Past only" shows past work, which is archived', async () => {
    const { effectiveWorkFilters } = await import('./work_filters');
    expect(effectiveWorkFilters({ ...DEFAULT_WORK_FILTERS, hasSession: 'no' }, [], true).archived).toBe(true);
    expect(effectiveWorkFilters({ ...DEFAULT_WORK_FILTERS, hasSession: 'no' }, [], false).archived).toBe(false);
  });

  it('recency is one rule for every row', async () => {
    const { withinRecency } = await import('./session_status');
    expect(withinRecency(1000, 'all', 5000)).toBe(true);
    expect(withinRecency(null, '1d', 5000)).toBe(false);
    expect(withinRecency(5000 - 3600, '8h', 5000)).toBe(true);
    expect(withinRecency(5000 - 9 * 3600, '8h', 5000)).toBe(false);
  });
});
