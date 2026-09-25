// Work graph M5.4 / M5.5: scopes, the selector's threshold, needs-you across
// scopes, the colour bar, the chord and the one composed filter.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import type { SessionRow } from './sessions';
import { sessions } from './sessions';
import { projects, type ProjectTreeRow } from './projects';
import {
  buildScopes,
  scopeOfSession,
  ownersByProject,
  effectiveScopeOf,
  needsYouElsewhere,
  orgColorOf,
  ruleChip,
  cycleScope,
  scopeFilter,
  orgs,
  scopes,
  scopeSelectorShown,
  effectiveScope,
  orgColorById,
  loadOrgs,
  createFromSuggestion,
  type OrgDetail,
} from './orgs';
import { appChord, scopeChordLabel } from './app_views';
import { rowMatches, sessionVisible, type FilterRow } from './sidebar_index';

let nextId = 1;
function row(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: nextId++,
    tmux_name: 'dev',
    host_alias: 'h1',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: 'main',
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    context_pct: null,
    stuck_kind: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  } as SessionRow;
}

function project(id: number, owner: string, system = false): ProjectTreeRow {
  return {
    project: { id, owner, repo: `r${id}`, base_path: `/p${id}`, last_session_at: null, adopted: false, system },
    worktrees: [],
  } as ProjectTreeRow;
}

const org = (id: number, name: string, color: string | null = null): OrgDetail => ({
  id,
  name,
  color,
  created_at: 1,
  rules: [],
  hosts: [],
  trackers: [],
});

const opts = { idleSecs: 3600, now: 10_000 };

beforeEach(() => {
  sessions.set([]);
  projects.set([]);
  orgs.set([]);
  scopeFilter.set('all');
});

describe('scopes', () => {
  const owners = ownersByProject([project(1, 'acme'), project(2, 'beta'), project(3, 'local'), project(4, 'fleet', true)]);

  it('a session is in its org, else its owner, else unassigned — never `local`', () => {
    expect(scopeOfSession(row({ org_id: 7, project_id: 1 }), owners)).toBe('org:7');
    expect(scopeOfSession(row({ project_id: 1 }), owners)).toBe('owner:acme');
    expect(scopeOfSession(row({ project_id: 3 }), owners)).toBe('unassigned');
    expect(scopeOfSession(row({ project_id: 4 }), owners)).toBe('unassigned');
    expect(scopeOfSession(row({ project_id: null }), owners)).toBe('unassigned');
  });

  it('named orgs first, then owners no org covers; unassigned never counts', () => {
    const rows = [row({ project_id: 1 }), row({ project_id: 2, org_id: 9 }), row({ project_id: null })];
    expect(buildScopes(rows, [org(9, 'Company B')], owners).map((s) => s.id)).toEqual([
      'org:9',
      'owner:acme',
    ]);
    // One owner and some project-less sessions: one scope, no selector.
    expect(buildScopes([row({ project_id: 1 }), row({ project_id: null })], [], owners)).toHaveLength(1);
  });

  it('the selector appears only at two or more scopes, and a hidden one filters nothing', () => {
    projects.set([project(1, 'acme'), project(2, 'beta')]);
    sessions.set([row({ project_id: 1 })]);
    expect(get(scopeSelectorShown)).toBe(false);
    scopeFilter.set('owner:acme');
    expect(get(effectiveScope)).toBe('all');
    sessions.set([row({ project_id: 1 }), row({ project_id: 2 })]);
    expect(get(scopes).map((s) => s.label)).toEqual(['acme', 'beta']);
    expect(get(scopeSelectorShown)).toBe(true);
    expect(get(effectiveScope)).toBe('owner:acme');
    // A persisted scope that no longer exists reads as all.
    expect(effectiveScopeOf('org:99', get(scopes))).toBe('all');
    expect(effectiveScopeOf('unassigned', get(scopes))).toBe('unassigned');
  });

  it('⌘⇧O / Ctrl+Shift+O cycles through all and every scope', () => {
    const ev = (key: string, m: Partial<KeyboardEvent>) => ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...m });
    expect(appChord(ev('O', { metaKey: true, shiftKey: true }), true)).toBe('scope');
    expect(appChord(ev('o', { ctrlKey: true, shiftKey: true }), false)).toBe('scope');
    expect(appChord(ev('o', { ctrlKey: true, shiftKey: true }), true)).toBeNull();
    expect(appChord(ev('o', { metaKey: true }), true)).toBeNull();
    expect(scopeChordLabel(true)).toBe('⌘⇧O');
    projects.set([project(1, 'acme'), project(2, 'beta')]);
    sessions.set([row({ project_id: 1 }), row({ project_id: 2 })]);
    const seen = [get(effectiveScope)];
    for (let i = 0; i < 3; i++) {
      cycleScope();
      seen.push(get(effectiveScope));
    }
    expect(seen).toEqual(['all', 'owner:acme', 'owner:beta', 'all']);
  });

  it('needs-you is never hidden by scope: other scopes report their waiting sessions', () => {
    const list = buildScopes([], [org(1, 'Company A'), org(2, 'Personal')], owners);
    const of = (s: SessionRow) => scopeOfSession(s, owners);
    const rows = [
      row({ org_id: 1, claude_status: 'blocked' }),
      row({ org_id: 2, claude_status: 'blocked' }),
      row({ org_id: 2, stuck_kind: 'oom' }),
      row({ org_id: 2, claude_status: 'working' }),
    ];
    expect(needsYouElsewhere(rows, 'org:1', list, of, opts)).toEqual([
      { scope: 'org:2', label: 'Personal', count: 2 },
    ]);
    expect(needsYouElsewhere(rows, 'all', list, of, opts)).toEqual([]);
  });

  it('colour bars only when two or more orgs exist', () => {
    orgs.set([org(1, 'A', '#f00')]);
    expect(orgColorOf({ org_id: 1 }, get(orgColorById))).toBeNull();
    orgs.set([org(1, 'A', '#f00'), org(2, 'B', '#00f')]);
    expect(orgColorOf({ org_id: 1 }, get(orgColorById))).toBe('#f00');
    expect(orgColorOf({ org_id: null }, get(orgColorById))).toBeNull();
  });

  it('rules read as chips', () => {
    expect(ruleChip({ id: 1, org_id: 1, owner: 'acme' })).toBe('acme/*');
    expect(ruleChip({ id: 1, org_id: 1, owner: 'acme', repo: 'api', host_alias: 'h' })).toBe('acme/api · host: h');
    expect(ruleChip({ id: 1, org_id: 1, path_prefix: '~/w/acme' })).toBe('path: ~/w/acme');
  });
});

describe('rowMatches: the filters compose', () => {
  const base: FilterRow = {
    host: 'h1',
    scope: 'org:1',
    kind: 'work',
    trackerId: 3,
    statusCategory: 'in_progress',
    assignees: ['Ann'],
    live: true,
    archived: false,
  };

  it('no filter lets everything through', () => {
    expect(rowMatches(base, {})).toBe(true);
    expect(rowMatches(base, { host: 'all', scope: 'all', tracker: 'all', status: 'all', assignee: 'all', hasSession: 'any' })).toBe(true);
  });

  // Each dimension on its own: [filters, expected].
  const single: [Parameters<typeof rowMatches>[1], boolean][] = [
    [{ host: 'h1' }, true],
    [{ host: 'h2' }, false],
    [{ scope: 'org:1' }, true],
    [{ scope: 'org:2' }, false],
    [{ scope: 'unassigned' }, false],
    [{ tracker: 3 }, true],
    [{ tracker: 4 }, false],
    [{ status: 'in_progress' }, true],
    [{ status: 'done' }, false],
    [{ assignee: 'Ann' }, true],
    [{ assignee: 'Bob' }, false],
    [{ hasSession: 'yes' }, true],
    [{ hasSession: 'no' }, false],
    [{ archived: false }, true],
    [{ showBgAgents: false }, true],
  ];
  it.each(single)('%o → %s', (f, want) => {
    expect(rowMatches(base, f)).toBe(want);
  });

  it('every clause must hold (AND), for every pair of dimensions', () => {
    const pass: Parameters<typeof rowMatches>[1] = { host: 'h1', scope: 'org:1', tracker: 3, status: 'in_progress', assignee: 'Ann', hasSession: 'yes' };
    const fail: Parameters<typeof rowMatches>[1] = { host: 'h2', scope: 'org:2', tracker: 4, status: 'done', assignee: 'Bob', hasSession: 'no' };
    const keys = Object.keys(pass) as (keyof typeof pass)[];
    expect(rowMatches(base, pass)).toBe(true);
    for (const a of keys) {
      for (const b of keys) {
        const f = { ...pass, [a]: fail[a], [b]: fail[b] };
        expect(rowMatches(base, f), `${a}+${b}`).toBe(false);
      }
    }
  });

  it('bg rows follow the toggle, archived rows the archived switch, tickets every scope', () => {
    expect(rowMatches({ ...base, kind: 'bg' }, { showBgAgents: false })).toBe(false);
    expect(rowMatches({ ...base, archived: true, live: false }, { archived: false })).toBe(false);
    expect(rowMatches({ ...base, scope: '*' }, { scope: 'org:9' })).toBe(true);
    // A ticket has no host: the host filter does not hide it.
    expect(rowMatches({ ...base, host: null }, { host: 'h2' })).toBe(true);
  });

  it('the needs-you predicate composes with host and scope', () => {
    const blocked = row({ host_alias: 'h1', claude_status: 'blocked', org_id: 1 });
    const ok = row({ host_alias: 'h1', claude_status: 'working', org_id: 1 });
    const pred = (s: SessionRow) => s.claude_status === 'blocked';
    const scope = { id: 'org:1', of: (s: SessionRow) => `org:${s.org_id}` };
    expect(sessionVisible(blocked, 'h1', true, pred, scope)).toBe(true);
    expect(sessionVisible(ok, 'h1', true, pred, scope)).toBe(false);
    expect(sessionVisible(blocked, 'h2', true, pred, scope)).toBe(false);
    expect(sessionVisible(blocked, 'h1', true, pred, { ...scope, id: 'org:2' })).toBe(false);
    expect(sessionVisible(blocked, 'all', true, null, null)).toBe(true);
  });
});

describe('⌘K under the scope (work graph M5.5)', () => {
  it('sessions and tickets go through the same rowMatches; an unassigned tracker is in every scope', async () => {
    const { scopeEntries } = await import('./quick_switcher');
    const s1 = row({ org_id: 1 });
    const s2 = row({ org_id: 2 });
    const entries = [
      { kind: 'session', key: 's1', label: 'a', session: s1 },
      { kind: 'session', key: 's2', label: 'b', session: s2 },
      { kind: 'ticket', key: 't1', label: 'A-1', ticket: { id: 1, tracker_id: 10 } },
      { kind: 'ticket', key: 't2', label: 'B-1', ticket: { id: 2, tracker_id: 20 } },
      { kind: 'ticket', key: 't3', label: 'X-1', ticket: { id: 3, tracker_id: 30 } },
      { kind: 'host', key: 'h', label: 'h1' },
    ] as never[];
    const trackerOrg = new Map<number, number | null>([[10, 1], [20, 2], [30, null]]);
    const of = (s: SessionRow) => `org:${s.org_id}`;
    const keys = (scope: string) =>
      scopeEntries(entries, scope, of, trackerOrg).map((e: { key: string }) => e.key);
    expect(keys('all')).toEqual(['s1', 's2', 't1', 't2', 't3', 'h']);
    expect(keys('org:1')).toEqual(['s1', 't1', 't3', 'h']);
    expect(keys('org:2')).toEqual(['s2', 't2', 't3', 'h']);
  });
});

describe('loadOrgs', () => {
  it('a stale list answer never overwrites a newer one', async () => {
    const mocked = vi.mocked(invoke);
    let first: (v: unknown) => void = () => {};
    mocked.mockImplementationOnce(() => new Promise((r) => (first = r)));
    mocked.mockResolvedValueOnce([org(2, 'newer')]);
    const a = loadOrgs();
    const b = loadOrgs();
    await b;
    expect(get(orgs).map((o) => o.name)).toEqual(['newer']);
    first([org(1, 'older')]);
    expect((await a).ok).toBe(true);
    expect(get(orgs).map((o) => o.name)).toEqual(['newer']);
  });
});

describe('createFromSuggestion', () => {
  const mocked = vi.mocked(invoke);
  const cmds = () => mocked.mock.calls.map((c) => c[0]);

  beforeEach(() => {
    mocked.mockReset();
  });

  it('a tracker suggestion makes the org, assigns the tracker, then re-reads the list', async () => {
    mocked.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_org') return org(9, 'Beta');
      if (cmd === 'list_orgs') return [org(9, 'Beta')];
      return null;
    });
    const r = await createFromSuggestion({ name: 'Beta', tracker_id: 5, sessions: 0, reason: 'a tracker named Beta' });
    expect(r.ok && r.value.id).toBe(9);
    expect(cmds()).toEqual(['add_org', 'assign_tracker_org', 'list_orgs']);
    expect(mocked).toHaveBeenCalledWith('add_org', { args: { name: 'Beta', color: null, isolate_sessions: false } });
    expect(mocked).toHaveBeenCalledWith('assign_tracker_org', { args: { tracker_id: 5, org_id: 9 } });
    expect(get(orgs).map((o) => o.name)).toEqual(['Beta']);
  });

  it('owner and tracker together: the rule first, then the tracker, both on the new org', async () => {
    mocked.mockImplementation(async (cmd: string) => (cmd === 'add_org' ? org(9, 'Beta') : cmd === 'list_orgs' ? [] : null));
    const r = await createFromSuggestion({ name: 'Beta', owner: 'beta', tracker_id: 5, sessions: 2, reason: 'x' });
    expect(r.ok).toBe(true);
    expect(cmds()).toEqual(['add_org', 'add_org_rule', 'assign_tracker_org', 'list_orgs']);
    expect(mocked).toHaveBeenCalledWith('add_org_rule', { args: { org_id: 9, owner: 'beta' } });
    expect(mocked).toHaveBeenCalledWith('assign_tracker_org', { args: { tracker_id: 5, org_id: 9 } });
  });

  it('a refused add_org is the answer, and nothing else is tried', async () => {
    orgs.set([org(1, 'A')]);
    mocked.mockRejectedValue({ code: 'E_INVALID', message: 'an org named Beta exists' });
    const r = await createFromSuggestion({ name: 'Beta', owner: 'beta', tracker_id: 5, sessions: 0, reason: 'x' });
    expect(r).toEqual({ ok: false, error: { code: 'E_INVALID', message: 'an org named Beta exists' } });
    expect(cmds()).toEqual(['add_org']);
    expect(get(orgs).map((o) => o.name)).toEqual(['A']);
  });

  it('a refused rule is the answer: the tracker is not assigned and the list not re-read', async () => {
    mocked.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_org') return org(9, 'Beta');
      if (cmd === 'add_org_rule') throw { code: 'E_INVALID', message: 'owner beta is already ruled' };
      return null;
    });
    const r = await createFromSuggestion({ name: 'Beta', owner: 'beta', tracker_id: 5, sessions: 0, reason: 'x' });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.message).toBe('owner beta is already ruled');
    expect(cmds()).toEqual(['add_org', 'add_org_rule']);
  });

  it('a refused tracker assignment is the answer, and the list is not re-read', async () => {
    mocked.mockImplementation(async (cmd: string) => {
      if (cmd === 'add_org') return org(9, 'Beta');
      if (cmd === 'assign_tracker_org') throw { code: 'E_NOT_FOUND', message: 'tracker 5' };
      return null;
    });
    const r = await createFromSuggestion({ name: 'Beta', tracker_id: 5, sessions: 0, reason: 'x' });
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_NOT_FOUND');
    expect(cmds()).toEqual(['add_org', 'assign_tracker_org']);
  });

  it('a tracker id of 0 is still a tracker; only a missing one is skipped', async () => {
    mocked.mockImplementation(async (cmd: string) => (cmd === 'add_org' ? org(9, 'Beta') : cmd === 'list_orgs' ? [] : null));
    await createFromSuggestion({ name: 'Beta', tracker_id: 0, sessions: 0, reason: 'x' });
    expect(cmds()).toEqual(['add_org', 'assign_tracker_org', 'list_orgs']);
    mocked.mockClear();
    await createFromSuggestion({ name: 'Beta', owner: 'beta', tracker_id: null, sessions: 0, reason: 'x' });
    expect(cmds()).toEqual(['add_org', 'add_org_rule', 'list_orgs']);
  });
});
