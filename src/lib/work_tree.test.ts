// The Work view's read side (work graph M14.2): tree building from `tree`
// pages, occurrence states, filter → request mapping, cursor paging, the
// `work:changed` patch vs full reload, session-row patching and the
// older-hub state.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import {
  EMPTY_FILTERS,
  appendPage,
  buildTree,
  countsLabel,
  createWorkTreeStore,
  endOccurrences,
  filtersToRequest,
  linkSignature,
  mergeTask,
  occurrenceKind,
  projectForTask,
  removeTask,
  sectionFilters,
  sectionKey,
  ticketOf,
  trackerDown,
  treeOccurrences,
  uiFiltersOf,
  type WorkTreeDeps,
} from './work_tree';
import type { TaskDetail, TreeOpts, TreePage, WorkTask, WorkTaskLink } from './work_view';
import type { Result } from './result';
import type { SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';

// ── fixtures ────────────────────────────────────────────────────────────────

export function link(over: Partial<WorkTaskLink> = {}): WorkTaskLink {
  return {
    link_id: 1,
    link_version: 1,
    state: 'active',
    primary: true,
    session_id: 7,
    name: 'api',
    host: 'mefistos',
    source: 'manual',
    why: 'branch abc-12',
    created_at: 1,
    needs_you: false,
    archived: false,
    resumable: true,
    cross_org: false,
    other_tasks: 0,
    ...over,
  };
}

export function task(id: string, over: Partial<WorkTask> = {}): WorkTask {
  return {
    task_id: id,
    key: id.replace(/^(item|ref):/, ''),
    title: `Task ${id}`,
    kind: 'tracker',
    unavailable: false,
    mine: false,
    org_id: 1,
    org_source: 'tracker',
    org_fenced: true,
    org_mixed: false,
    group: { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker', editable: true },
    counts: { active: 1, ended: 0, suggested: 0 },
    needs_you: false,
    review: false,
    placement_version: 0,
    sessions: [link()],
    sessions_more: 0,
    ...over,
  };
}

const G_ABC = { id: 'tracker:1:ABC', label: 'ABC', source: 'tracker', editable: true };
const G_PAY = { id: 'label:Payments', label: 'Payments', source: 'manual', editable: true };
const G_NONE = { id: 'none', label: '', source: 'none', editable: true };

function page(over: Partial<TreePage> = {}): TreePage {
  return {
    tasks: [],
    groups: [
      { org_id: 1, org_name: 'Acme', group: G_ABC, count: 3 },
      { org_id: 1, org_name: 'Acme', group: G_PAY, count: 1 },
      { group: G_NONE, count: 2 },
    ],
    orgs: [{ id: 1, name: 'Acme', color: '#f00' }],
    trackers: [{ id: 1, name: 'Jira', provider: 'jira', state: 'ok', org_id: 1 }],
    total: 6,
    generated_at: 1,
    ...over,
  };
}

const ok = <T>(value: T): Result<T> => ({ ok: true, value });

// ── pure helpers ────────────────────────────────────────────────────────────

describe('buildTree', () => {
  it('groups sections under their org in the hub order, unassigned last', () => {
    const orgs = buildTree(page());
    expect(orgs.map((o) => [o.key, o.name, o.count])).toEqual([
      ['1', 'Acme', 4],
      ['none', 'Unassigned', 2],
    ]);
    expect(orgs[0].color).toBe('#f00');
    expect(orgs[0].sections.map((s) => s.key)).toEqual(['1|tracker:1:ABC', '1|label:Payments']);
    expect(orgs[1].sections[0]).toMatchObject({ key: 'none|none', orgId: null, count: 2, cursor: undefined, tasks: [] });
  });

  it('keeps tasks already loaded under a section that still exists and drops the rest', () => {
    const first = buildTree(page());
    first[0].sections[0] = { ...first[0].sections[0], tasks: [task('item:1')], cursor: 'c1' };
    first[0].sections[1] = { ...first[0].sections[1], tasks: [task('item:9')], cursor: null };
    const next = buildTree(
      page({ groups: [{ org_id: 1, org_name: 'Acme', group: G_ABC, count: 5 }] }),
      first,
    );
    expect(next).toHaveLength(1);
    expect(next[0].sections).toHaveLength(1);
    expect(next[0].sections[0]).toMatchObject({ count: 5, cursor: 'c1' });
    expect(next[0].sections[0].tasks.map((t) => t.task_id)).toEqual(['item:1']);
  });
});

describe('occurrences', () => {
  it('maps link state and primary to how the occurrence is drawn', () => {
    expect(occurrenceKind({ state: 'active', primary: true })).toBe('primary');
    expect(occurrenceKind({ state: 'active', primary: false })).toBe('secondary');
    expect(occurrenceKind({ state: 'suggested', primary: false })).toBe('suggested');
    expect(occurrenceKind({ state: 'ended', primary: true })).toBe('past');
    expect(occurrenceKind({ state: 'rejected', primary: false })).toBe('rejected');
    // A newer hub's state never passes for a live link.
    expect(occurrenceKind({ state: 'paused', primary: true })).toBe('past');
  });

  it('the tree leaves rejected links to the details', () => {
    const t = task('item:1', {
      sessions: [link({ link_id: 1 }), link({ link_id: 2, state: 'rejected', primary: false })],
    });
    expect(treeOccurrences(t).map((l) => l.link_id)).toEqual([1]);
  });

  it('a killed session turns its live occurrences past and drops its suggestions', () => {
    const t = task('item:1', {
      counts: { active: 1, ended: 1, suggested: 1 },
      sessions: [
        link({ link_id: 1, session_id: 7 }),
        link({ link_id: 2, session_id: 7, state: 'suggested', primary: false }),
        link({ link_id: 3, session_id: undefined, state: 'ended', primary: false }),
      ],
    });
    const n = endOccurrences(t, 7);
    expect(n.sessions.map((l) => [l.link_id, l.state, l.session_id])).toEqual([
      [1, 'ended', undefined],
      [3, 'ended', undefined],
    ]);
    expect(n.counts).toEqual({ active: 0, ended: 2, suggested: 0 });
    expect(countsLabel(n)).toBe('0 active / 2 past');
    // Another session: nothing changes, same object.
    expect(endOccurrences(t, 99)).toBe(t);
  });

  it('tracker down is a failing tracker, not an untested one', () => {
    expect(trackerDown('ok')).toBe(false);
    expect(trackerDown(undefined)).toBe(false);
    expect(trackerDown('unconfigured')).toBe(false);
    expect(trackerDown('auth_failed')).toBe(true);
    expect(trackerDown('unreachable')).toBe(true);
  });
});

describe('filters', () => {
  it('leaves defaults out of the request', () => {
    expect(filtersToRequest(EMPTY_FILTERS)).toEqual({});
  });

  it('maps every filter to its contract field', () => {
    expect(
      filtersToRequest({
        org: 'none',
        tracker: 'local',
        status: 'open',
        mine: true,
        has: 'past_only',
        review: true,
        query: '  login ',
      }),
    ).toEqual({ org: 'none', tracker: 'local', status: 'open', mine: true, has: 'past_only', review: true, query: 'login' });
    expect(filtersToRequest({ ...EMPTY_FILTERS, org: 3, tracker: 2, has: 'suggested' })).toEqual({
      org: 3,
      tracker: 2,
      has: 'suggested',
    });
  });

  it("reads a saved view's filters back, ignoring its group and unknown values", () => {
    expect(uiFiltersOf({ org: 2, status: 'done', mine: true, group: 'label:x' })).toEqual({
      ...EMPTY_FILTERS,
      org: 2,
      status: 'done',
      mine: true,
    });
    expect(uiFiltersOf({ status: 'weird' as never, has: 'nope' as never })).toEqual(EMPTY_FILTERS);
  });

  it("narrows a section's request to its org and group", () => {
    expect(sectionFilters({ status: 'open' }, 1, 'tracker:1:ABC')).toEqual({ status: 'open', org: 1, group: 'tracker:1:ABC' });
    expect(sectionFilters({}, null, 'none')).toEqual({ org: 'none', group: 'none' });
    expect(sectionKey(undefined, 'none')).toBe('none|none');
  });
});

describe('mergeOne / removeOne for tasks', () => {
  it('replaces in place, appends new, removes, and never repeats a task across pages', () => {
    const a = task('item:1');
    const b = task('item:2');
    const a2 = { ...a, title: 'renamed' };
    expect(mergeTask([a, b], a2).map((t) => t.title)).toEqual(['renamed', 'Task item:2']);
    expect(mergeTask([a], b).map((t) => t.task_id)).toEqual(['item:1', 'item:2']);
    expect(removeTask([a, b], 'item:1').map((t) => t.task_id)).toEqual(['item:2']);
    const same = [a];
    expect(removeTask(same, 'item:9')).toBe(same);
    expect(appendPage([a, b], [b, task('item:3')]).map((t) => t.task_id)).toEqual(['item:1', 'item:2', 'item:3']);
  });
});

describe('linkSignature', () => {
  it('moves with the primary and the top suggestion only', () => {
    const base = { work: null, work_suggested: null } as unknown as SessionRow;
    const linked = { ...base, work: { link_id: 4, state: 'confirmed' } } as unknown as SessionRow;
    expect(linkSignature(base)).not.toBe(linkSignature(linked));
    expect(linkSignature(linked)).toBe(linkSignature({ ...linked, claude_status: 'working' } as SessionRow));
  });
});

describe('projectForTask / ticketOf', () => {
  const proj = (id: number, owner: string, repo: string, last = 0, system = false): ProjectTreeRow =>
    ({ project: { id, owner, repo, base_path: '', last_session_at: last, adopted: false, system }, worktrees: [] }) as ProjectTreeRow;
  const projects = [proj(1, 'acme', 'api', 5), proj(2, 'acme', 'web', 9), proj(3, 'fleet', 'operator', 99, true)];

  it("prefers the project of the task's newest listed session", () => {
    const rows = [{ id: 7, project_id: 1, host_alias: 'h1', last_activity_at: 10 }] as SessionRow[];
    expect(projectForTask(task('item:1'), rows, projects)).toEqual({ project: projects[0], host: 'h1' });
  });

  it('then its repository, then the most recently used project (never a system one)', () => {
    const t = task('item:1', { sessions: [], repos: ['acme/api'] });
    expect(projectForTask(t, [], projects)?.project.project.id).toBe(1);
    expect(projectForTask({ ...t, repos: [] , key: undefined }, [], projects)?.project.project.id).toBe(2);
    expect(projectForTask(t, [], [])).toBeNull();
  });

  it('a work item is a ticket for the dialog; a bare key is not', () => {
    expect(ticketOf(task('ref:ABC-1'))).toBeUndefined();
    const tk = ticketOf(task('item:12', { item_id: 12, key: 'ABC-12', status_category: 'in_progress' }));
    expect(tk).toMatchObject({ id: 12, key: 'ABC-12', status_category: 'in_progress', live_session_ids: [7] });
  });
});

// ── the store ───────────────────────────────────────────────────────────────

type TreeFn = (o: TreeOpts) => Promise<Result<TreePage>>;

function deps(tree: TreeFn, over: Partial<WorkTreeDeps> = {}): Partial<WorkTreeDeps> {
  return {
    tree: vi.fn(tree),
    task: vi.fn(async () => ({ ok: false, error: { code: 'E_NOTFOUND', message: 'gone' } }) as Result<TaskDetail>),
    views: vi.fn(async () => ok([])),
    readPrefs: () => null,
    writePrefs: () => {},
    debounceMs: 50,
    ...over,
  };
}

/** A hub fake: header calls (limit 1) answer the groups; section calls
 *  answer that section's tasks two at a time over a cursor. */
function fakeHub(tasksByGroup: Record<string, WorkTask[]>, pg: TreePage = page()): TreeFn {
  return async (o) => {
    if (o.limit === 1) return ok(pg);
    const g = o.filters?.group ?? '';
    const all = tasksByGroup[g] ?? [];
    const start = o.cursor ? Number(o.cursor) : 0;
    const n = o.cursor ? 2 : Math.min(o.limit ?? 50, 2);
    const slice = all.slice(start, start + n);
    const next = start + n < all.length ? String(start + n) : undefined;
    return ok({ ...pg, tasks: slice, next_cursor: next });
  };
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve();
}

describe('WorkTreeStore', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('loads the headers, then the open sections, and pages with Load more', async () => {
    const abc = [task('item:1'), task('item:2'), task('item:3')];
    const d = deps(fakeHub({ 'tracker:1:ABC': abc }));
    const s = createWorkTreeStore(d);
    await s.reload();
    const st = get(s);
    expect(st.status).toBe('ready');
    expect(st.total).toBe(6);
    // No pref: the first sections open by themselves.
    expect([...st.expanded]).toEqual(['1|tracker:1:ABC', '1|label:Payments', 'none|none']);
    const sec = () => get(s).orgs[0].sections[0];
    expect(sec().tasks.map((t) => t.task_id)).toEqual(['item:1', 'item:2']);
    expect(sec().cursor).toBe('2');
    // The section's request: the view's filters + its org and group.
    const call = vi.mocked(d.tree!).mock.calls.find((c) => c[0].filters?.group === 'tracker:1:ABC');
    expect(call![0].filters).toEqual({ org: 1, group: 'tracker:1:ABC' });

    await s.loadMore('1|tracker:1:ABC');
    expect(sec().tasks.map((t) => t.task_id)).toEqual(['item:1', 'item:2', 'item:3']);
    expect(sec().cursor).toBeNull();
    const withCursor = vi.mocked(d.tree!).mock.calls.find((c) => c[0].cursor === '2');
    expect(withCursor![0].filters).toEqual({ org: 1, group: 'tracker:1:ABC' });
    // The last page is loaded: another Load more asks nothing.
    const calls = vi.mocked(d.tree!).mock.calls.length;
    await s.loadMore('1|tracker:1:ABC');
    expect(vi.mocked(d.tree!).mock.calls.length).toBe(calls);
  });

  it('a filter change reloads with the new filters and drops old cursors', async () => {
    const d = deps(fakeHub({ 'tracker:1:ABC': [task('item:1'), task('item:2'), task('item:3')] }));
    const s = createWorkTreeStore(d);
    await s.reload();
    s.setFilters({ ...EMPTY_FILTERS, status: 'open', mine: true });
    await settle();
    const last = vi.mocked(d.tree!).mock.calls.filter((c) => c[0].filters?.group === 'tracker:1:ABC').pop()!;
    expect(last[0].filters).toEqual({ status: 'open', mine: true, org: 1, group: 'tracker:1:ABC' });
    expect(last[0].cursor).toBeUndefined();
  });

  it('only opened sections are read; opening one reads its first page', async () => {
    const d = deps(fakeHub({ 'label:Payments': [task('item:5')] }), { readPrefs: () => ({ expanded: [], collapsedOrgs: [], task: null }) });
    const s = createWorkTreeStore(d);
    await s.reload();
    expect(vi.mocked(d.tree!).mock.calls.length).toBe(1);
    s.toggleSection('1|label:Payments');
    await settle();
    expect(get(s).orgs[0].sections[1].tasks.map((t) => t.task_id)).toEqual(['item:5']);
  });

  it('keeps expansion and the last task per view', async () => {
    const saved: Record<string, unknown> = {};
    const d = deps(fakeHub({}), {
      readPrefs: (v) => (saved[String(v)] as never) ?? null,
      writePrefs: (v, p) => (saved[String(v)] = p),
    });
    const s = createWorkTreeStore(d);
    await s.reload();
    s.toggleSection('none|none');
    s.selectTask('item:1');
    expect(saved.adhoc).toMatchObject({ task: 'item:1' });
    expect((saved.adhoc as { expanded: string[] }).expanded).not.toContain('none|none');
    s.applyView({ id: 4, name: 'Mine', filters: { mine: true }, version: 1, updated_at: 0 });
    await settle();
    expect(get(s).view).toBe(4);
    expect(get(s).filters.mine).toBe(true);
    expect(get(s).selected).toBeNull();
    s.applyView(null);
    await settle();
    expect(get(s).selected).toBe('item:1');
  });

  it('an older hub is the needs-a-newer-hub state, not an error', async () => {
    const d = deps(async () => ({ ok: false, error: { code: 'E_INVALID', message: 'unknown work action "tree"' } }));
    const s = createWorkTreeStore(d);
    await s.reload();
    expect(get(s).status).toBe('needs_hub');
    const e = createWorkTreeStore(deps(async () => ({ ok: false, error: { code: 'E_DB', message: 'locked' } })));
    await e.reload();
    expect(get(e)).toMatchObject({ status: 'error', error: 'E_DB: locked' });
  });

  it('work:changed for one task patches it in place and re-reads only the headers', async () => {
    const t1 = task('item:1');
    const moved = task('item:1', { group: G_PAY, placement_version: 2 });
    const d = deps(fakeHub({ 'tracker:1:ABC': [t1], 'label:Payments': [task('item:5')] }), {
      task: vi.fn(async () => ok({ task: moved } as TaskDetail)),
    });
    const s = createWorkTreeStore(d);
    await s.reload();
    const sectionReads = () => vi.mocked(d.tree!).mock.calls.filter((c) => c[0].limit !== 1).length;
    const headerReads = () => vi.mocked(d.tree!).mock.calls.filter((c) => c[0].limit === 1).length;
    const before = sectionReads();
    s.onWorkChanged([{ what: 'placement', task_id: 'item:1' }]);
    s.onWorkChanged([{ what: 'placement', task_id: 'item:1' }]);
    await vi.advanceTimersByTimeAsync(60);
    await settle();
    expect(d.task).toHaveBeenCalledTimes(1);
    expect(sectionReads()).toBe(before);
    expect(headerReads()).toBe(2);
    const [abc, pay] = get(s).orgs[0].sections;
    expect(abc.tasks).toEqual([]);
    expect(pay.tasks.map((t) => t.task_id)).toEqual(['item:5', 'item:1']);
  });

  it('a resync, a rule change or an id-less change reloads the whole view', async () => {
    for (const change of [{ what: 'resync' }, { what: 'rule', rule_id: 3 }, { what: 'org' }] as const) {
      const d = deps(fakeHub({}));
      const s = createWorkTreeStore(d);
      await s.reload();
      const n = vi.mocked(d.tree!).mock.calls.filter((c) => c[0].limit === 1).length;
      s.onWorkChanged([change]);
      await vi.advanceTimersByTimeAsync(60);
      await settle();
      expect(vi.mocked(d.tree!).mock.calls.filter((c) => c[0].limit === 1).length).toBe(n + 1);
      expect(d.task).not.toHaveBeenCalled();
    }
  });

  it('a view change re-reads only the views list', async () => {
    const d = deps(fakeHub({}));
    const s = createWorkTreeStore(d);
    await s.reload();
    const n = vi.mocked(d.tree!).mock.calls.length;
    s.onWorkChanged([{ what: 'view', view_id: 1 }]);
    await vi.advanceTimersByTimeAsync(60);
    await settle();
    expect(d.views).toHaveBeenCalled();
    expect(vi.mocked(d.tree!).mock.calls.length).toBe(n);
  });

  it('session rows patch occurrences without a read; a link change re-reads its section once', async () => {
    const d = deps(fakeHub({ 'tracker:1:ABC': [task('item:1')] }));
    const s = createWorkTreeStore(d);
    await s.reload();
    const n = vi.mocked(d.tree!).mock.calls.length;
    const row = { id: 7, claude_status: 'working', work: { link_id: 1, state: 'confirmed' }, work_suggested: null } as unknown as SessionRow;
    s.onSessionEvents([{ type: 'updated', row }]);
    s.onSessionEvents([{ type: 'updated', row: { ...row, claude_status: 'idle' } as SessionRow }]);
    await vi.advanceTimersByTimeAsync(60);
    await settle();
    expect(get(s).orgs[0].sections[0].tasks[0].sessions[0].claude_status).toBe('idle');
    // First sighting after load counts as a change only for a created row.
    expect(vi.mocked(d.tree!).mock.calls.length).toBe(n);

    // The primary moves: a burst re-reads the one section showing it once.
    const moved = { ...row, work: { link_id: 9, state: 'confirmed' } } as unknown as SessionRow;
    s.onSessionEvents([{ type: 'updated', row: moved }]);
    s.onSessionEvents([{ type: 'updated', row: { ...moved, claude_status: 'working' } as SessionRow }]);
    await vi.advanceTimersByTimeAsync(60);
    await settle();
    const after = vi.mocked(d.tree!).mock.calls.slice(n);
    expect(after).toHaveLength(1);
    expect(after[0][0].filters?.group).toBe('tracker:1:ABC');

    s.onSessionEvents([{ type: 'killed', id: 7 }]);
    const occ = get(s).orgs[0].sections[0].tasks[0];
    expect(occ.sessions[0].state).toBe('ended');
    expect(occ.counts.active).toBe(0);
  });

  it('reveal opens the section holding the task and selects it', async () => {
    const t = task('item:5', { group: G_PAY });
    const d = deps(fakeHub({ 'label:Payments': [task('item:6')] }), {
      readPrefs: () => ({ expanded: [], collapsedOrgs: ['1'], task: null }),
      task: vi.fn(async () => ok({ task: t } as TaskDetail)),
    });
    const s = createWorkTreeStore(d);
    await s.reveal('item:5');
    const st = get(s);
    expect(st.selected).toBe('item:5');
    expect(st.expanded.has('1|label:Payments')).toBe(true);
    expect(st.collapsedOrgs.has('1')).toBe(false);
    expect(st.orgs[0].sections[1].tasks.map((x) => x.task_id)).toEqual(['item:5', 'item:6']);
  });
});
