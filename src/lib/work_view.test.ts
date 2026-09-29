// The Work view's contract layer (work graph M14): the argument shapes of
// every command, the filters' normal form, sections merged from pages, the
// occurrence rules, conflicts and older hubs, and the undo of a decision.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import { sessions } from './sessions';
import { session } from './hosts_fixture';
import { knownProviderShort, providerShort } from './tracker_health';
import { linkSessionWork, confirmSessionWork, onWorkChangedDebounced, rejectWorkLink, unlinkSessionWork } from './work';
import {
  ackWorkLink,
  activeFilterCount,
  assignWorkOrg,
  buildSections,
  conflictCurrent,
  conflictNotice,
  conflictOf,
  decideWorkBatch,
  deleteWorkRule,
  deleteWorkView,
  distributeTasks,
  filtersKey,
  groupSessionLinks,
  groupSourceText,
  isOccurrenceOf,
  isOlderHub,
  mergeTasks,
  noteWorkChanged,
  noteWorkEvents,
  normalizeFilters,
  occurrenceKind,
  openTask,
  orgSourceText,
  placementNote,
  placeWork,
  providerName,
  keyPrefix,
  ruleDraftFor,
  readErrorText,
  NEWER_HUB,
  reconsiderWorkLink,
  sameFilters,
  saveWorkRule,
  saveWorkView,
  sectionFilters,
  sectionKey,
  selectedTaskId,
  sessionEventsTouchWork,
  setPrimaryWork,
  showTaskInWorkView,
  sidebarView,
  taskDetailOpen,
  toggleSidebarView,
  trackerDown,
  trackerDownLabel,
  undoOf,
  workChanged,
  bumpWorkChanged,
  workOrgImpact,
  workReview,
  workRulePreview,
  workRules,
  workSessionTasks,
  workTask,
  workTree,
  workViews,
  revealTaskRequest,
  type WorkTreePage,
} from './work_view';
import { link, task } from './work_view_fixture';

const lastCall = () => vi.mocked(invoke).mock.calls.at(-1)!;

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(null);
});

describe('commands: every one takes { args: { … } } with the action’s fields', () => {
  it('reads', async () => {
    await workTree({ filters: { status: 'open', query: ' login ', has: 'any' }, limit: 50 });
    expect(lastCall()).toEqual(['work_tree', { args: { filters: { status: 'open', query: 'login', archived: false }, limit: 50 } }]);
    await workTree({ filters: { org: 3, group: 'tracker:1:ABC' }, cursor: 'c1', limit: 20, per_task: 4 });
    expect(lastCall()).toEqual([
      'work_tree',
      { args: { filters: { org: 3, group: 'tracker:1:ABC', archived: false }, cursor: 'c1', limit: 20, per_task: 4 } },
    ]);
    // The hub hides archived tasks only when asked: `archived` always goes
    // on the wire, `false` without a filter and `true` when showing them.
    await workTree();
    expect(lastCall()).toEqual(['work_tree', { args: { filters: { archived: false } } }]);
    await workTree({ filters: { archived: true } });
    expect(lastCall()).toEqual(['work_tree', { args: { filters: { archived: true } } }]);
    await workTask('ref:ABC-1');
    expect(lastCall()).toEqual(['work_task', { args: { task_id: 'ref:ABC-1' } }]);
    await workSessionTasks(7);
    expect(lastCall()).toEqual(['work_session_tasks', { args: { session_id: 7 } }]);
    await workReview({ limit: 1 });
    expect(lastCall()).toEqual(['work_review', { args: { limit: 1 } }]);
    await workReview({ cursor: 'r2', limit: 50 });
    expect(lastCall()).toEqual(['work_review', { args: { cursor: 'r2', limit: 50 } }]);
    await workRules();
    expect(lastCall()).toEqual(['work_rules', { args: {} }]);
    await workViews();
    expect(lastCall()).toEqual(['work_views', { args: {} }]);
    await workOrgImpact('item:77', 0);
    expect(lastCall()).toEqual(['work_org_impact', { args: { task_id: 'item:77', org_id: 0 } }]);
    await workRulePreview({
      name: ' Payments ',
      enabled: true,
      group: 'Payments',
      conditions: { tracker_id: 1, container: 'PAY', key_prefix: ' ', title_contains: '', repo: null },
    });
    expect(lastCall()).toEqual([
      'work_rule_preview',
      {
        args: {
          rule: {
            name: 'Payments',
            enabled: true,
            conditions: { tracker_id: 1, container: 'PAY', key_prefix: null, title_contains: null, repo: null },
            group: 'Payments',
          },
        },
      },
    ]);
  });

  it('link writes answer the row and patch it in', async () => {
    const row = session('mefistos', 'api', { id: 7 });
    vi.mocked(invoke).mockResolvedValue(row);
    sessions.set([]);
    const r = await setPrimaryWork(7, 43, 42);
    expect(r.ok).toBe(true);
    expect(lastCall()).toEqual(['set_primary_work', { args: { session_id: 7, link_id: 43, expected_primary: 42 } }]);
    expect(get(sessions).map((s) => s.id)).toEqual([7]);
    await setPrimaryWork(7, 43, null);
    expect(lastCall()).toEqual(['set_primary_work', { args: { session_id: 7, link_id: 43, expected_primary: 0 } }]);
    await reconsiderWorkLink(7, 42, 4);
    expect(lastCall()).toEqual(['reconsider_work_link', { args: { session_id: 7, link_id: 42, expected_version: 4 } }]);
    await reconsiderWorkLink(7, 42);
    expect(lastCall()).toEqual(['reconsider_work_link', { args: { session_id: 7, link_id: 42 } }]);
    await ackWorkLink(7, 42, 2);
    expect(lastCall()).toEqual(['ack_work_link', { args: { session_id: 7, link_id: 42, expected_version: 2 } }]);
  });

  it('the existing link commands take primary / expected_version only when given', async () => {
    vi.mocked(invoke).mockResolvedValue(session('mefistos', 'api', { id: 7 }));
    await linkSessionWork(7, { key: 'ABC-12' });
    expect(lastCall()).toEqual(['link_session_work', { args: { session_id: 7, key: 'ABC-12' } }]);
    await linkSessionWork(7, { item_id: 12 }, { primary: false });
    expect(lastCall()).toEqual(['link_session_work', { args: { session_id: 7, item_id: 12, primary: false } }]);
    await linkSessionWork(7, { key: 'ABC-12' }, { expectedVersion: 3 });
    expect(lastCall()).toEqual(['link_session_work', { args: { session_id: 7, key: 'ABC-12', expected_version: 3 } }]);
    await confirmSessionWork(7, 42, { expectedVersion: 0 });
    expect(lastCall()).toEqual(['confirm_session_work', { args: { session_id: 7, link_id: 42, expected_version: 0 } }]);
    await confirmSessionWork(7, 42, { primary: false, expectedVersion: 3 });
    expect(lastCall()).toEqual([
      'confirm_session_work',
      { args: { session_id: 7, link_id: 42, primary: false, expected_version: 3 } },
    ]);
    await rejectWorkLink(7, 42, { expectedVersion: 3 });
    expect(lastCall()).toEqual(['reject_session_work', { args: { session_id: 7, link_id: 42, expected_version: 3 } }]);
    await unlinkSessionWork(7, 42, { expectedVersion: 5 });
    expect(lastCall()).toEqual(['unlink_session_work', { args: { session_id: 7, link_id: 42, expected_version: 5 } }]);
    await unlinkSessionWork(7, 42);
    expect(lastCall()).toEqual(['unlink_session_work', { args: { session_id: 7, link_id: 42 } }]);
  });

  it('structure writes', async () => {
    await decideWorkBatch([
      { session_id: 7, link_id: 42, decision: 'confirm', expected_version: 2, primary: false },
      { session_id: 8, link_id: 50, decision: 'reject' },
    ]);
    expect(lastCall()).toEqual([
      'decide_work_batch',
      {
        args: {
          decisions: [
            { session_id: 7, link_id: 42, decision: 'confirm', expected_version: 2, primary: false },
            { session_id: 8, link_id: 50, decision: 'reject' },
          ],
        },
      },
    ]);
    await placeWork('item:12', ' Payments ', 0, ' moved for Q4 ');
    expect(lastCall()).toEqual([
      'place_work',
      { args: { task_id: 'item:12', group: 'Payments', note: 'moved for Q4', expected_version: 0 } },
    ]);
    await placeWork('item:12', '', 3);
    expect(lastCall()).toEqual(['place_work', { args: { task_id: 'item:12', group: '', expected_version: 3 } }]);
    await assignWorkOrg('item:77', 2, 'tok');
    expect(lastCall()).toEqual(['assign_work_org', { args: { task_id: 'item:77', org_id: 2, impact_token: 'tok' } }]);
    await saveWorkRule({ id: 3, name: 'Payments', enabled: false, conditions: { tracker_id: 1, container: 'PAY' }, group: 'Payments', expected_version: 2 });
    expect(lastCall()).toEqual([
      'save_work_rule',
      {
        args: {
          rule: {
            id: 3,
            name: 'Payments',
            enabled: false,
            conditions: { tracker_id: 1, container: 'PAY', key_prefix: null, title_contains: null, repo: null },
            group: 'Payments',
            expected_version: 2,
          },
        },
      },
    ]);
    await deleteWorkRule(3, 2);
    expect(lastCall()).toEqual(['delete_work_rule', { args: { rule_id: 3, expected_version: 2 } }]);
    await saveWorkView({ name: ' Mine ', filters: { mine: true, status: 'open', group: 'x' }, expected_version: 0 });
    expect(lastCall()).toEqual([
      'save_work_view',
      { args: { view: { name: 'Mine', filters: { status: 'open', mine: true }, expected_version: 0 } } },
    ]);
    await deleteWorkView(1);
    expect(lastCall()).toEqual(['delete_work_view', { args: { view_id: 1 } }]);
    await deleteWorkView(1, 3);
    expect(lastCall()).toEqual(['delete_work_view', { args: { view_id: 1, expected_version: 3 } }]);
  });
});

describe('filters', () => {
  it('drops defaults and junk', () => {
    expect(normalizeFilters({ status: 'any', has: 'any', mine: false, review: false, query: '  ' })).toEqual({});
    expect(normalizeFilters({ org: 'none', tracker: 'ref', status: 'nope', has: 'suggested', org2: 1 })).toEqual({
      org: 'none',
      tracker: 'ref',
      has: 'suggested',
    });
    expect(normalizeFilters({ org: -1, tracker: 1.5 })).toEqual({});
    expect(normalizeFilters(null)).toEqual({});
  });

  it('keys equal filters equally, whatever the field order', () => {
    expect(filtersKey({ query: 'x', org: 1 })).toBe(filtersKey({ org: 1, query: 'x', status: 'any' }));
    expect(sameFilters({ mine: true }, { mine: false })).toBe(false);
    expect(activeFilterCount({ mine: true, status: 'open', group: 'none' })).toBe(2);
  });

  it('a section loads with its org and group on top of the view', () => {
    expect(sectionFilters({ status: 'open', org: 2 }, null, 'none')).toEqual({ status: 'open', org: 'none', group: 'none' });
    expect(sectionFilters({}, 1, 'tracker:1:ABC')).toEqual({ org: 1, group: 'tracker:1:ABC' });
  });
});

describe('sections', () => {
  const page: WorkTreePage = {
    tasks: [task(), task({ task_id: 'ref:ABC-13', item_id: null, key: 'ABC-13', title: '', kind: 'ref' })],
    groups: [
      { org_id: 1, org_name: 'Acme', group: task().group, count: 5 },
      { org_id: 1, org_name: 'Acme', group: { id: 'none', label: 'No group', source: 'none' }, count: 1 },
      { org_id: null, group: { id: 'none', label: 'No group', source: 'none' }, count: 2 },
    ],
    orgs: [{ id: 1, name: 'Acme', color: '#f00' }],
    trackers: [],
    total: 8,
    next_cursor: 'n1',
  };

  it('draws every header from groups, org first, with the first page’s tasks', () => {
    const st = distributeTasks(page);
    const s = buildSections(page.groups, page.orgs, st);
    expect(s.map((o) => [o.name, o.count])).toEqual([
      ['Acme', 6],
      ['Unassigned', 2],
    ]);
    expect(s[0].color).toBe('#f00');
    expect(s[0].groups.map((g) => [g.group.id, g.tasks.length, g.more])).toEqual([
      ['tracker:1:ABC', 2, true],
      ['none', 0, true],
    ]);
    // The same group id in another org is another section.
    expect(s[1].groups[0].key).toBe(sectionKey(null, 'none'));
    expect(s[1].groups[0].key).not.toBe(s[0].groups[1].key);
  });

  it('a section loaded by itself keeps its own tasks and cursor across a first-page read', () => {
    const own = new Map([[sectionKey(1, 'none'), { tasks: [task({ task_id: 'item:99', group: { id: 'none', label: '', source: 'none' } })], cursor: null, own: true }]]);
    const st = distributeTasks(page, own);
    const s = buildSections(page.groups, page.orgs, st);
    expect(s[0].groups[1].tasks.map((t) => t.task_id)).toEqual(['item:99']);
    // Its own cursor is null: nothing more, whatever the count says.
    expect(s[0].groups[1].more).toBe(false);
  });

  it('merges pages without repeating a task, taking the newer copy', () => {
    const a = [task({ task_id: 'item:1' }), task({ task_id: 'item:2', title: 'old' })];
    const b = [task({ task_id: 'item:2', title: 'new' }), task({ task_id: 'item:3' })];
    const m = mergeTasks(a, b);
    expect(m.map((t) => t.task_id)).toEqual(['item:1', 'item:2', 'item:3']);
    expect(m[1].title).toBe('new');
  });
});

describe('occurrences and provenance', () => {
  it('kinds: primary, secondary, suggested, past — an unknown state is never active', () => {
    expect(occurrenceKind(link())).toBe('primary');
    expect(occurrenceKind(link({ primary: false }))).toBe('secondary');
    expect(occurrenceKind(link({ state: 'suggested', primary: false }))).toBe('suggested');
    expect(occurrenceKind(link({ state: 'ended' }))).toBe('past');
    expect(occurrenceKind(link({ state: 'frozen' }))).toBe('past');
    expect(occurrenceKind(link({ state: 'rejected' }))).toBe('rejected');
    expect(isOccurrenceOf(link({ state: 'ended' }), 7)).toBe(false);
    expect(isOccurrenceOf(link({ state: 'suggested' }), 7)).toBe(true);
  });

  it('says where the org comes from', () => {
    expect(orgSourceText(task())).toBe('from tracker Jira (acme)');
    expect(orgSourceText(task({ org_source: 'item' }))).toBe('set by a person');
    expect(orgSourceText(task({ org_source: 'sessions' }))).toBe('inferred from its sessions — not a boundary');
    expect(orgSourceText(task({ org_source: 'sessions', org_mixed: true }))).toContain('span several organisations');
    expect(orgSourceText(task({ org_source: 'none' }))).toBe('no organisation');
  });

  it('says where the group comes from, and that a tracker is never edited', () => {
    const t = task();
    expect(groupSourceText(t.group, t)).toBe('from the tracker: Jira (acme) ABC');
    expect(placementNote(t.group, t)).toBe('Placing it elsewhere is local to fleet: it never changes Jira.');
    const asana = task({ provider: 'asana', tracker_name: 'Asana (acme)' });
    expect(placementNote(asana.group, asana)).toContain('never changes Asana');
    expect(groupSourceText({ id: 'label:Payments', label: 'Payments', source: 'rule', rule_id: 3 }, t, 'Payments')).toBe(
      'placed by the rule “Payments”',
    );
    expect(groupSourceText({ id: 'label:X', label: 'X', source: 'manual' }, t)).toBe('placed here by a person');
    expect(groupSourceText({ id: 'repo:acme/api', label: 'acme/api', source: 'repo' }, t)).toContain('acme/api');
    expect(groupSourceText({ id: 'key:ABC', label: 'ABC', source: 'key' }, t)).toBe('from its key prefix ABC');
    expect(groupSourceText({ id: 'none', label: '', source: 'none' }, t)).toContain('no group');
  });

  it('names a provider from the one table, "the tracker" for none or unknown', () => {
    expect(providerName('jira_dc')).toBe('Jira');
    expect(providerName('github')).toBe('GitHub');
    expect(providerName('nope')).toBe('the tracker');
    expect(providerName('constructor')).toBe('the tracker');
    expect(providerName(null)).toBe('the tracker');
    expect(knownProviderShort('linear')).toBe('Linear');
    expect(knownProviderShort('nope')).toBeNull();
    // The tracker settings' short name is unchanged: the id when unknown.
    expect(providerShort('jira_dc')).toBe('Jira');
    expect(providerShort('nope')).toBe('nope');
    expect(providerShort(undefined)).toBe('tracker');
  });

  it('reads a key prefix as the hub does', () => {
    expect(keyPrefix('ABC-12')).toBe('ABC');
    expect(keyPrefix('abc-12')).toBe('ABC');
    expect(keyPrefix('ops2.x-12')).toBe('OPS2.X');
    expect(keyPrefix('1PX-3')).toBe('1PX');
    expect(keyPrefix('ABC-12a')).toBeNull();
    expect(keyPrefix('ABC-')).toBeNull();
    expect(keyPrefix('-12')).toBeNull();
    expect(keyPrefix('ABC')).toBeNull();
    expect(keyPrefix(null)).toBeNull();
  });

  it('drafts a rule from where the task is grouped', () => {
    const none = { id: 'none', label: '', source: 'none' };
    // A key group: the group's own label is the prefix.
    const keyed = ruleDraftFor(task({ kind: 'local', tracker_id: null, key: 'abc-12', group: { id: 'key:ABC', label: 'ABC', source: 'key' } }), 'G');
    expect(keyed).toEqual({
      name: '',
      enabled: true,
      group: 'G',
      expected_version: 0,
      conditions: { tracker_id: null, container: null, key_prefix: 'ABC', repo: null, title_contains: null },
    });
    // No group: the key as the hub reads it.
    expect(ruleDraftFor(task({ kind: 'local', key: 'ops2.x-12', group: none }), '').conditions.key_prefix).toBe('OPS2.X');
    expect(ruleDraftFor(task({ kind: 'local', key: 'abc-12', group: none }), '').conditions.key_prefix).toBe('ABC');
    expect(ruleDraftFor(task({ kind: 'local', key: 'ABC-12a', group: none }), '').conditions.key_prefix).toBeNull();
    // A tracker group: its tracker and container, never a key prefix.
    const tr = ruleDraftFor(task(), 'Infra', 'Infra');
    expect(tr.name).toBe('Infra');
    expect(tr.conditions).toEqual({ tracker_id: 1, container: 'ABC', key_prefix: null, repo: null, title_contains: null });
    // A repo group: the repository, and a key prefix only when the key parses.
    const repo = { id: 'repo:acme/api', label: 'acme/api', source: 'repo' };
    expect(ruleDraftFor(task({ kind: 'local', key: 'API-7', group: repo }), 'x').conditions).toEqual({
      tracker_id: null,
      container: null,
      key_prefix: 'API',
      repo: 'acme/api',
      title_contains: null,
    });
    expect(ruleDraftFor(task({ kind: 'local', key: null, group: repo }), 'x').conditions.key_prefix).toBeNull();
    // A tracker task names its tracker whatever the group.
    expect(ruleDraftFor(task({ tracker_id: 4, group: repo }), 'x').conditions.tracker_id).toBe(4);
  });

  it('a failing tracker is "down", not "no sessions"', () => {
    expect(trackerDown(task({ tracker_state: 'unreachable' }))).toBe(true);
    expect(trackerDown(task())).toBe(false);
    expect(trackerDown(task({ kind: 'local', tracker_state: null }))).toBe(false);
    expect(trackerDownLabel(task({ tracker_state: 'unconfigured' }))).toBe('tracker: not tested yet');
    expect(trackerDownLabel(task({ tracker_state: 'auth_failed' }))).toBe('tracker: token expired or wrong');
  });

  it('groups a session’s links: primary first, past newest first', () => {
    const g = groupSessionLinks([
      link({ link_id: 1, primary: false }),
      link({ link_id: 2, primary: true }),
      link({ link_id: 3, state: 'ended', ended_at: 10 }),
      link({ link_id: 4, state: 'ended', ended_at: 20 }),
      link({ link_id: 5, state: 'suggested' }),
      link({ link_id: 6, state: 'rejected' }),
    ]);
    expect(g.active.map((l) => l.link_id)).toEqual([2, 1]);
    expect(g.past.map((l) => l.link_id)).toEqual([4, 3]);
    expect(g.suggested.map((l) => l.link_id)).toEqual([5]);
    expect(g.rejected.map((l) => l.link_id)).toEqual([6]);
  });
});

describe('errors and undo', () => {
  it('reads a conflict and its current value', () => {
    const e = { code: 'E_CONFLICT', message: 'changed', details: { link_id: 42, version: 5, state: 'confirmed', primary: 43 } };
    expect(conflictOf(e)).toEqual({ link_id: 42, version: 5, state: 'confirmed', primary: 43 });
    expect(conflictOf({ code: 'E_CONFLICT', message: 'x' })).toEqual({});
    expect(conflictOf({ code: 'E_INVALID', message: 'x' })).toBeNull();
  });

  it('says what a conflict’s current value is, for every kind the backend sends', () => {
    // A link.
    expect(conflictCurrent({ link_id: 42, version: 5, state: 'confirmed', primary: true, ended: false })).toBe(
      'Now: confirmed · primary · version 5',
    );
    expect(conflictCurrent({ link_id: 42, version: 6, state: 'confirmed', primary: false, ended: true })).toBe(
      'Now: ended · version 6',
    );
    expect(conflictCurrent({ link_id: null, version: 0 })).toBe('Now: no link');
    // A session's primary, named by the caller when it can.
    expect(conflictCurrent({ session_id: 7, primary_link_id: 44 }, (id) => (id === 44 ? 'ABC-14 Refunds' : null))).toBe(
      'Now: primary is ABC-14 Refunds',
    );
    expect(conflictCurrent({ session_id: 7, primary_link_id: 45 })).toBe('Now: primary is link 45');
    expect(conflictCurrent({ session_id: 7, primary_link_id: null })).toBe('Now: no primary');
    // A placement, a rule, a view.
    expect(conflictCurrent({ task_id: 'item:1', version: 2, group: 'Infra' })).toBe('Now: placed in “Infra” · version 2');
    expect(conflictCurrent({ task_id: 'item:1', version: 0, group: null })).toBe('Now: not placed');
    expect(conflictCurrent({ view_id: 1, version: 4 })).toBe('Now: version 4');
    expect(conflictCurrent({})).toBeNull();
    const n = conflictNotice({ code: 'E_CONFLICT', message: 'x', details: { view_id: 1, version: 4 } }, 'The view “A”');
    expect(n).toEqual({ conflict: true, text: expect.stringContaining('The view “A” changed elsewhere'), current: 'Now: version 4' });
    expect(conflictNotice({ code: 'E_INVALID', message: 'x' }, 'it')).toBeNull();
  });

  it('an unknown work action is "Needs a newer hub"; other refusals are the hub’s sentence', () => {
    expect(isOlderHub({ code: 'E_INVALID', message: 'unknown work action: tree' })).toBe(true);
    expect(isOlderHub({ code: 'E_UNKNOWN', message: 'command work_tree not found' })).toBe(true);
    expect(isOlderHub({ code: 'E_INVALID', message: 'cursor from other filters' })).toBe(false);
    expect(readErrorText({ code: 'E_INVALID', message: 'unknown action tree' })).toBe(NEWER_HUB);
    expect(readErrorText({ code: 'E_HUB', message: 'hub unreachable' })).toBe('hub unreachable');
    expect(isOlderHub({ code: 'E_INVALID', message: 'unknown work_link action "place"; one of link, unlink' })).toBe(true);
    // Not every refusal is an older hub: an unknown host, a protocol error
    // that is not a missing tool, an unknown error.
    expect(isOlderHub({ code: 'E_INVALID', message: 'unknown host "gpu"; the action needs one' })).toBe(false);
    expect(isOlderHub({ code: 'E_HUB_PROTOCOL', message: 'the hub refused the work call: invalid params' })).toBe(false);
    expect(isOlderHub({ code: 'E_HUB_PROTOCOL', message: 'the hub refused the work call: tool not found' })).toBe(true);
    expect(isOlderHub({ code: 'E_UNKNOWN', message: 'boom' })).toBe(false);
    expect(isOlderHub({ code: 'E_UNKNOWN_COMMAND', message: 'x' })).toBe(false);
  });

  it('undoes a confirm or a reject by reconsidering, with the new version', () => {
    expect(undoOf({ session_id: 7, link_id: 42, decision: 'confirm' }, 4)).toEqual({
      session_id: 7,
      link_id: 42,
      decision: 'reconsider',
      expected_version: 4,
    });
    expect(undoOf({ session_id: 7, link_id: 42, decision: 'reject' })).toEqual({ session_id: 7, link_id: 42, decision: 'reconsider' });
    expect(undoOf({ session_id: 7, link_id: 42, decision: 'ack' })).toBeNull();
    expect(undoOf({ session_id: 7, link_id: 42, decision: 'reconsider' })).toBeNull();
  });
});

describe('stores', () => {
  it('toggles the sidebar view and remembers it', () => {
    sidebarView.set('sessions');
    toggleSidebarView();
    expect(get(sidebarView)).toBe('work');
    expect(localStorage.getItem('cf:pref:sidebar.view')).toBe('"work"');
    toggleSidebarView();
    expect(get(sidebarView)).toBe('sessions');
  });

  it('"Show in Work view" switches, selects, opens the detail and asks for a reveal', () => {
    sidebarView.set('sessions');
    taskDetailOpen.set(false);
    showTaskInWorkView('item:12');
    expect(get(sidebarView)).toBe('work');
    expect(get(selectedTaskId)).toBe('item:12');
    expect(get(taskDetailOpen)).toBe(true);
    expect(get(revealTaskRequest)?.taskId).toBe('item:12');
    openTask('item:13');
    expect(get(selectedTaskId)).toBe('item:13');
  });

  it('every successful work write bumps the tick at once; a refused one does not', async () => {
    vi.mocked(invoke).mockResolvedValue(session('mefistos', 'api', { id: 7 }));
    sessions.set([]);
    const writes: [string, () => Promise<unknown>][] = [
      ['set_primary_work', () => setPrimaryWork(7, 43, 42)],
      ['reconsider_work_link', () => reconsiderWorkLink(7, 42, 1)],
      ['ack_work_link', () => ackWorkLink(7, 42, 1)],
      ['decide_work_batch', () => decideWorkBatch([{ session_id: 7, link_id: 42, decision: 'confirm' }])],
      ['place_work', () => placeWork('item:12', 'Payments', 0)],
      ['assign_work_org', () => assignWorkOrg('item:12', 2, 'tok')],
      ['save_work_rule', () => saveWorkRule({ name: 'R', enabled: true, conditions: {}, group: 'G' })],
      ['delete_work_rule', () => deleteWorkRule(3, 1)],
      ['save_work_view', () => saveWorkView({ name: 'V', filters: {} })],
      ['delete_work_view', () => deleteWorkView(1)],
      ['link_session_work', () => linkSessionWork(7, { key: 'ABC-1' }, { primary: false })],
      ['confirm_session_work', () => confirmSessionWork(7, 42, { expectedVersion: 1 })],
      ['reject_session_work', () => rejectWorkLink(7, 42, { expectedVersion: 1 })],
      ['unlink_session_work', () => unlinkSessionWork(7, 42, { expectedVersion: 1 })],
    ];
    for (const [cmd, w] of writes) {
      const before = get(workChanged);
      await w();
      expect([cmd, get(workChanged)]).toEqual([cmd, before + 1]);
    }
    vi.mocked(invoke).mockRejectedValue({ code: 'E_CONFLICT', message: 'changed' });
    const before = get(workChanged);
    await setPrimaryWork(7, 43, 42);
    await placeWork('item:12', 'Payments', 0);
    await unlinkSessionWork(7, 42, { expectedVersion: 1 });
    expect(get(workChanged)).toBe(before);
    // Reads never bump.
    vi.mocked(invoke).mockResolvedValue(null);
    await workTree({});
    await workSessionTasks(7);
    expect(get(workChanged)).toBe(before);
  });

  it('work:changed and a work item bump the tick; tracker frames do not', () => {
    const before = get(workChanged);
    noteWorkEvents([{ type: 'tracker' }]);
    noteWorkChanged([]);
    expect(get(workChanged)).toBe(before);
    noteWorkChanged([{ what: 'placement', task_id: 'item:12' }]);
    expect(get(workChanged)).toBe(before + 1);
    noteWorkChanged([{ what: 'resync' }]);
    expect(get(workChanged)).toBe(before + 2);
    noteWorkEvents([{ type: 'item' }]);
    expect(get(workChanged)).toBe(before + 3);
  });

  it('session events touch work only when a row’s work, org or attention moved', () => {
    const plain = session('mefistos', 'plain', { id: 1 });
    const worked = session('mefistos', 'api', {
      id: 7,
      work: { link_id: 42, item_id: 12, key: 'ABC-12', title: 'Login', source: 'manual' },
    });
    const cur = [plain, worked];
    // A status tick of a session with no work, not shown: nothing.
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...plain, claude_status: 'working' } }], cur, new Set())).toBe(false);
    // Shown by the tree: working ↔ idle is too chatty to re-read on…
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...plain, claude_status: 'working' } }], cur, new Set([1]))).toBe(false);
    // …but "needs you" (blocked, stuck, failed) and alive / dead are drawn.
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...plain, claude_status: 'blocked' } }], cur, new Set([1]))).toBe(true);
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...plain, stuck_kind: 'oom' } }], cur, new Set([1]))).toBe(true);
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...plain, status: 'dead' } }], cur, new Set([1]))).toBe(true);
    // A secondary link changed: only `work_rev` says so.
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...worked, work_rev: 99 } }], cur, new Set())).toBe(true);
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...worked, claude_status: 'working' } }], cur, new Set())).toBe(false);
    // Its work moved.
    expect(
      sessionEventsTouchWork([{ type: 'updated', row: { ...worked, work: { ...worked.work!, link_id: 43 } } }], cur, new Set()),
    ).toBe(true);
    // Same row again: nothing.
    expect(sessionEventsTouchWork([{ type: 'updated', row: { ...worked } }], cur, new Set())).toBe(false);
    // Killed with work.
    expect(sessionEventsTouchWork([{ type: 'killed', id: 7 }], cur, new Set())).toBe(true);
    expect(sessionEventsTouchWork([{ type: 'killed', id: 1 }], cur, new Set())).toBe(false);
  });

  it('every attention reason the hub draws refreshes a shown row; turns and reads do not', () => {
    const plain = session('mefistos', 'plain', { id: 1, context_pct: 10 });
    const cur = [plain];
    const shown = new Set([1]);
    const touches = (row: typeof plain) => sessionEventsTouchWork([{ type: 'updated', row }], cur, shown);
    // The reasons the old hand-written check missed.
    expect(touches({ ...plain, context_pct: 97 })).toBe(true);
    expect(touches({ ...plain, stale_working_at: 1790000000 })).toBe(true);
    expect(touches({ ...plain, ci_status: 'failing' })).toBe(true);
    expect(touches({ ...plain, lost_at: 1790000000 })).toBe(true);
    // Not a reason: context rising short of red, a turn, a Stop (done /
    // unread is the sidebar's, never the tree's).
    expect(touches({ ...plain, context_pct: 40 })).toBe(false);
    expect(touches({ ...plain, claude_status: 'working' })).toBe(false);
    expect(touches({ ...plain, last_stop_at: 1790000999, last_turn_at: 1790000999 })).toBe(false);
    // One reason becoming another is a change too.
    const waiting = { ...plain, claude_status: 'blocked' as const };
    expect(
      sessionEventsTouchWork([{ type: 'updated', row: { ...waiting, claude_status: 'idle', context_pct: 97 } }], [waiting], shown),
    ).toBe(true);
  });
});

describe('work change kinds', () => {
  it('a work:changed kind reaches a debounced reader; a burst arrives as the union', () => {
    vi.useFakeTimers();
    try {
      const seen: string[][] = [];
      const off = onWorkChangedDebounced((kinds) => seen.push([...kinds].sort()), () => 100);
      noteWorkChanged([{ what: 'rule', rule_id: 3 }]);
      vi.advanceTimersByTime(150);
      expect(seen).toEqual([['rule']]);
      noteWorkChanged([{ what: 'placement', task_id: 'item:1' }]);
      vi.advanceTimersByTime(50);
      noteWorkChanged([{ what: 'view', view_id: 2 }]);
      vi.advanceTimersByTime(150);
      expect(seen).toEqual([['rule'], ['placement', 'view']]);
      // One frame batch with several kinds is one bump carrying them all.
      noteWorkChanged([{ what: 'org' }, { what: 'resync' }]);
      vi.advanceTimersByTime(150);
      expect(seen.at(-1)).toEqual(['org', 'resync']);
      // A bump with no kind is a write this window made; an item frame moves
      // tasks.
      bumpWorkChanged();
      noteWorkEvents([{ type: 'item' }]);
      vi.advanceTimersByTime(150);
      expect(seen.at(-1)).toEqual(['local', 'placement']);
      off();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('the Work view org chord and archived tasks', () => {
  it('⌘⇧O cycles the Work view org: any → each org → unassigned → any', async () => {
    const { cycleWorkOrg, workTreeMeta, workViewFilters } = await import('./work_view');
    const { get } = await import('svelte/store');
    workTreeMeta.set({ orgs: [{ id: 1, name: 'Acme' }, { id: 2, name: 'Beta' }], trackers: [], groups: [] });
    workViewFilters.set({ status: 'open' });
    const seen = [];
    for (let i = 0; i < 4; i++) {
      cycleWorkOrg();
      seen.push(get(workViewFilters).org);
    }
    expect(seen).toEqual([1, 2, 'none', undefined]);
    expect(get(workViewFilters).status).toBe('open');
    workViewFilters.set({});
  });

  it('archived is kept and is not counted as narrowing', async () => {
    const { activeFilterCount, filtersKey, normalizeFilters } = await import('./work_view');
    expect(normalizeFilters({ archived: true })).toEqual({ archived: true });
    expect(normalizeFilters({ archived: false })).toEqual({});
    expect(filtersKey({ archived: true, org: 3 })).not.toBe(filtersKey({ org: 3 }));
    expect(activeFilterCount({ archived: true })).toBe(0);
  });
});
