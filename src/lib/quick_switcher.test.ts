import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import {
  buildEntries,
  rankEntries,
  contextProject,
  noteRecent,
  recentSessions,
  isSwitcherChord,
  chordLabel,
  RECENT_MAX,
} from './quick_switcher';
import type { SessionRow } from './sessions';
import type { ProjectTreeRow } from './projects';
import { host } from './hosts_fixture';

function sess(over: Partial<SessionRow> & { id: number }): SessionRow {
  return {
    tmux_name: `dev-o-r--s${over.id}`,
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: over.id,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
    reviews_session_id: null,
    worktree_key: null,
    lost_at: null,
    claude_session_id: null,
    claude_status: null,
    effort_level: null,
    pr_url: null,
    current_activity: null,
    friendly_name: null,
    safe_kill_state: null,
    safe_kill_nonce: null,
    safe_kill_detail: null,
    safe_kill_requested_at: null,
    context_pct: null,
    stuck_kind: null,
    idle_since: null,
    stuck_since: null,
    last_playbook_at: null,
    last_prompt: null,
    started_at: null,
    last_turn_at: null,
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [], model: null, context_tokens: null, context_window: null, context_source: null, context_at: null, context_stale: false, tmux_pane_id: null, pending_input: null,
    ...over,
  };
}

const projects: ProjectTreeRow[] = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 10, adopted: false, system: false },
    worktrees: [
      { id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' },
      { id: 12, project_id: 1, host_alias: 'local', name: 'blue-sirius', path: '/r/cf/.worktrees/blue-sirius', branch: 'blue-sirius' },
    ],
  },
  {
    project: { id: 2, owner: 'acme', repo: 'widgets', base_path: '/r/w', last_session_at: 20, adopted: false, system: false },
    worktrees: [],
  },
];

/** The UX agent's own project row: flagged `system`, with its session. */
const operatorProject: ProjectTreeRow = {
  project: {
    id: 9,
    owner: 'fleet',
    repo: 'operator',
    base_path: '/home/u/.claude-fleet/operator',
    last_session_at: 99,
    adopted: false,
    system: true,
  },
  worktrees: [],
};
const operatorSess = sess({
  id: 9,
  project_id: 9,
  tmux_name: 'fleet-operator',
  friendly_name: 'fleet operator',
});

const sessions: SessionRow[] = [
  sess({ id: 1, friendly_name: 'Blue sirius', worktree_id: 12, host_alias: 'mefistos', claude_status: 'working' }),
  sess({ id: 2, friendly_name: 'Fix login', tmux_name: 'dev-o-r--fix-login', worktree_id: 11 }),
  sess({ id: 3, project_id: 2, tmux_name: 'dev-acme-widgets', host_alias: 'hetzner', status: 'ghost' }),
];

beforeEach(() => {
  localStorage.clear();
  recentSessions.set([]);
});

describe('buildEntries', () => {
  it('produces a session row per session and a project row per project', () => {
    const e = buildEntries(sessions, projects);
    expect(e.filter((x) => x.kind === 'session')).toHaveLength(3);
    expect(e.filter((x) => x.kind === 'project')).toHaveLength(2);
  });

  it('labels with the friendly name and describes project · host · branch', () => {
    const e = buildEntries(sessions, projects).find((x) => x.key === 'session:1')!;
    expect(e.label).toBe('Blue sirius');
    expect(e.description).toBe('martin-janci/claude-fleet · mefistos · blue-sirius');
    expect(e.meta).toBe('working');
    expect(e.fields).toContain('dev-o-r--s1');
  });

  it('falls back to the tmux name and marks ghosts', () => {
    const e = buildEntries(sessions, projects).find((x) => x.key === 'session:3')!;
    expect(e.label).toBe('dev-acme-widgets');
    expect(e.meta).toBe('ghost');
  });
});

describe('rankEntries', () => {
  it('empty query: recent sessions first, then by activity, projects last', () => {
    const entries = buildEntries(sessions, projects);
    const recent = ['local/dev-o-r--fix-login'];
    const keys = rankEntries(entries, '', recent).map((e) => e.key);
    expect(keys).toEqual(['session:2', 'session:3', 'session:1', 'project:2', 'project:1']);
  });

  it('matches across friendly name, tmux name, project, host, branch, status', () => {
    const entries = buildEntries(sessions, projects);
    const top = (q: string) => rankEntries(entries, q, [])[0]?.key;
    expect(top('blue')).toBe('session:1');
    expect(top('fix-login')).toBe('session:2');
    expect(top('widgets')).toBe('session:3');
    expect(top('mefistos')).toBe('session:1');
    expect(top('working')).toBe('session:1');
    expect(top('ghost')).toBe('session:3');
  });

  it('a multi-token query narrows across fields', () => {
    const entries = buildEntries(sessions, projects);
    expect(rankEntries(entries, 'blue mef', [])[0].key).toBe('session:1');
    expect(rankEntries(entries, 'widgets hetz', []).map((e) => e.key)).toEqual(['session:3']);
  });

  it('drops rows that do not match', () => {
    const entries = buildEntries(sessions, projects);
    expect(rankEntries(entries, 'zzzz', [])).toEqual([]);
  });

  it('breaks score ties by recency', () => {
    const a = sess({ id: 7, friendly_name: 'Alpha vega' });
    const b = sess({ id: 8, friendly_name: 'Alpha vega' });
    const entries = buildEntries([a, b], projects);
    const keys = rankEntries(entries, 'alpha', ['local/dev-o-r--s7']).map((e) => e.key);
    expect(keys).toEqual(['session:7', 'session:8']);
  });
});

describe('host entries', () => {
  const hostRows = [host('mefistos'), host('hetzner', { reachable: false }), host('local')];

  it('adds a `host: <alias>` row per host with its state and session count', () => {
    const entries = buildEntries(sessions, projects, hostRows);
    const mef = entries.find((e) => e.key === 'host:mefistos')!;
    expect(mef.kind).toBe('host');
    expect(mef.label).toBe('host: mefistos');
    expect(mef.description).toBe('online · 1 session');
    expect(entries.find((e) => e.key === 'host:hetzner')!.description).toBe('offline · 1 session');
    expect(entries.find((e) => e.key === 'host:local')!.description).toBe('online · 1 session');
  });

  it('buildEntries without hosts stays sessions + projects', () => {
    expect(buildEntries(sessions, projects).some((e) => e.kind === 'host')).toBe(false);
  });

  it('never ranks a host row above a session row, even when it matches better', () => {
    const entries = buildEntries(sessions, projects, hostRows);
    // "mefistos" is an exact host alias but only a host facet of session 1.
    const ranked = rankEntries(entries, 'mefistos', []);
    const kinds = ranked.map((e) => e.kind);
    expect(kinds[0]).toBe('session');
    expect(ranked.find((e) => e.kind === 'host')?.key).toBe('host:mefistos');
    const firstHost = kinds.indexOf('host');
    expect(kinds.slice(firstHost).includes('session')).toBe(false);
  });

  it('empty query: sessions, then hosts, then projects', () => {
    const ranked = rankEntries(buildEntries(sessions, projects, hostRows), '', []);
    const kinds = ranked.map((e) => e.kind);
    expect(kinds).toEqual(['session', 'session', 'session', 'host', 'host', 'host', 'project', 'project']);
  });

  it('`host` narrows to host rows first when no session matches', () => {
    const ranked = rankEntries(buildEntries([], projects, hostRows), 'host hetz', []);
    expect(ranked[0].key).toBe('host:hetzner');
  });
});

describe('recent list', () => {
  it('noteRecent moves the key to the head and caps the list', () => {
    for (let i = 0; i < RECENT_MAX + 5; i++) {
      noteRecent({ host_alias: 'h', tmux_name: `s${i}` });
    }
    const cur = get(recentSessions);
    expect(cur).toHaveLength(RECENT_MAX);
    expect(cur[0]).toBe(`h/s${RECENT_MAX + 4}`);
    noteRecent({ host_alias: 'h', tmux_name: 's3' });
    expect(get(recentSessions)[0]).toBe('h/s3');
    expect(get(recentSessions).filter((k) => k === 'h/s3')).toHaveLength(1);
  });

  it('persists under the quick-switcher.recent pref', () => {
    noteRecent({ host_alias: 'h', tmux_name: 'x' });
    expect(JSON.parse(localStorage.getItem('cf:pref:quick-switcher.recent')!)).toEqual(['h/x']);
  });
});

describe('contextProject', () => {
  it('prefers the selected session project, then the top row, then MRU project', () => {
    const entries = buildEntries(sessions, projects);
    const ranked = rankEntries(entries, 'widgets', []);
    expect(contextProject(ranked, sessions[0], projects)?.project.id).toBe(1);
    expect(contextProject(ranked, null, projects)?.project.id).toBe(2);
    expect(contextProject([], null, projects)?.project.id).toBe(2);
    expect(contextProject([], null, [])).toBeNull();
  });
});

describe('isSwitcherChord', () => {
  const ev = (over: Partial<Parameters<typeof isSwitcherChord>[0]>) => ({
    key: 'k',
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...over,
  });

  it('Linux/Windows: Ctrl+Shift+K / Ctrl+Shift+P only; plain Ctrl+K/P belong to the terminal', () => {
    expect(isSwitcherChord(ev({ ctrlKey: true, shiftKey: true, key: 'K' }), false)).toBe(true);
    expect(isSwitcherChord(ev({ ctrlKey: true, shiftKey: true, key: 'P' }), false)).toBe(true);
    expect(isSwitcherChord(ev({ ctrlKey: true, key: 'k' }), false)).toBe(false);
    expect(isSwitcherChord(ev({ ctrlKey: true, key: 'p' }), false)).toBe(false);
    expect(isSwitcherChord(ev({ metaKey: true, key: 'k' }), false)).toBe(false);
    expect(isSwitcherChord(ev({ ctrlKey: true, shiftKey: true, altKey: true, key: 'K' }), false)).toBe(false);
    expect(isSwitcherChord(ev({ ctrlKey: true, shiftKey: true, key: 'J' }), false)).toBe(false);
  });

  it('macOS: Cmd+K / Cmd+P only; Ctrl+K/P still reach the terminal', () => {
    expect(isSwitcherChord(ev({ metaKey: true, key: 'k' }), true)).toBe(true);
    expect(isSwitcherChord(ev({ metaKey: true, key: 'p' }), true)).toBe(true);
    expect(isSwitcherChord(ev({ ctrlKey: true, key: 'k' }), true)).toBe(false);
    expect(isSwitcherChord(ev({ metaKey: true, ctrlKey: true, key: 'k' }), true)).toBe(false);
    expect(isSwitcherChord(ev({ metaKey: true, shiftKey: true, key: 'K' }), true)).toBe(false);
  });

  it('chordLabel names the platform chord', () => {
    expect(chordLabel(true)).toBe('⌘K');
    expect(chordLabel(false)).toBe('Ctrl+Shift+K');
  });
});

describe('the operator\'s system project', () => {
  // The design says the operator's project row is "flagged `system` and
  // hidden from the project picker". The sweep half was built and tested;
  // nothing in src/ read the flag, so `fleet / operator` showed up as a
  // place to start an ordinary session.
  it('offers no "New session in fleet/operator" entry', () => {
    const entries = buildEntries([operatorSess], [...projects, operatorProject]);
    const projectEntries = entries.filter((e) => e.kind === 'project');
    expect(projectEntries.map((e) => e.label)).not.toContain('New session in fleet/operator');
    expect(projectEntries).toHaveLength(2);
  });

  it('still LABELS the operator session with its project, which is not a picker', () => {
    const entries = buildEntries([operatorSess], [...projects, operatorProject]);
    const row = entries.find((e) => e.kind === 'session' && e.session?.id === 9);
    expect(row?.description).toContain('fleet/operator');
  });

  it('contextProject never answers with it, not even from the agent\'s own session', () => {
    const all = [...projects, operatorProject];
    const ranked = rankEntries(buildEntries([operatorSess], all), '', []);
    expect(contextProject(ranked, operatorSess, all)?.project.system).toBe(false);
    // And with nothing else to pick from, it answers "no project" rather
    // than offering the agent's directory.
    expect(contextProject([], operatorSess, [operatorProject])).toBeNull();
  });
});

// ---- tickets (work graph M3) -------------------------------------------------

import {
  ticketEntries as _ticketEntries,
  lookupEntry as _lookupEntry,
  placeForTicket as _placeForTicket,
  rankEntries as _rankEntries,
  type SwitcherEntry as _SwitcherEntry,
} from './quick_switcher';
import type { TicketRow as _TicketRow } from './trackers';

function _ticket(key: string, title: string, over: Partial<_TicketRow> = {}): _TicketRow {
  return {
    id: key.length,
    source: 'jira',
    key,
    title,
    status_category: 'todo',
    status_name: 'To Do',
    created_at: 1,
    updated_at: 1,
    tracker_id: 1,
    ...over,
  };
}

function _session(id: number, label: string): _SwitcherEntry {
  return {
    kind: 'session',
    key: `session:${id}`,
    label,
    description: '',
    meta: '',
    fields: [label],
    session: { id, host_alias: 'h', tmux_name: `t${id}`, last_activity_at: id } as never,
  };
}

describe('quick switcher tickets', () => {
  const tickets = _ticketEntries([
    { ticket: _ticket('ABC-1', 'Login page'), section: 'Recent' },
    { ticket: _ticket('ABC-2', 'Billing', { live_session_ids: [7] }), section: 'My work' },
    { ticket: _ticket('ABC-1', 'Login page'), section: 'My work' },
    { ticket: _ticket('ABC-3', 'Sprint thing'), section: 'Current sprint' },
  ]);

  it('one row per key, labelled with key and title, live ones jump', () => {
    expect(tickets.map((t) => t.key)).toEqual(['ticket:ABC-1', 'ticket:ABC-2', 'ticket:ABC-3']);
    expect(tickets[0].label).toBe('ABC-1 Login page');
    expect(tickets[1].meta).toBe('jump');
    expect(tickets[0].meta).toBe('start');
  });

  it('tickets rank below sessions, by section on an empty query', () => {
    const ranked = _rankEntries([...tickets, _session(1, 'login work')], '', []);
    expect(ranked.map((e) => e.key)).toEqual([
      'session:1',
      'ticket:ABC-2',
      'ticket:ABC-3',
      'ticket:ABC-1',
    ]);
    const q = _rankEntries([...tickets, _session(1, 'login work')], 'login', []);
    expect(q[0].key).toBe('session:1');
  });

  it('an exact key match ranks first, even above sessions', () => {
    const ranked = _rankEntries([...tickets, _session(1, 'abc-1 notes')], 'abc-1', []);
    expect(ranked[0].key).toBe('ticket:ABC-1');
  });

  it('a pasted URL or an unknown exact key offers a lookup row, ranked first', () => {
    const known = new Set(['ABC-1']);
    expect(_lookupEntry('https://acme.atlassian.net/browse/ABC-9', known)?.lookup).toBe(
      'https://acme.atlassian.net/browse/ABC-9',
    );
    expect(_lookupEntry('abc-9', known)?.label).toBe('Look up ABC-9');
    expect(_lookupEntry('abc-1', known)).toBeNull();
    expect(_lookupEntry('login page', known)).toBeNull();
    const row = _lookupEntry('abc-9', known)!;
    expect(_rankEntries([_session(1, 'x'), row], 'abc-9', [])[0].kind).toBe('lookup');
  });

  it('a ticket lands where its key prefix last ran', () => {
    const p1 = { project: { id: 1, owner: 'o', repo: 'a', base_path: '/a', last_session_at: 1, adopted: false, system: false }, worktrees: [] };
    const p2 = { project: { id: 2, owner: 'o', repo: 'b', base_path: '/b', last_session_at: 9, adopted: false, system: false }, worktrees: [] };
    const s = (id: number, pid: number, key: string, at: number) =>
      ({ id, project_id: pid, host_alias: `h${id}`, last_activity_at: at, work: { key } }) as never;
    const keyOf = (x: { work?: { key: string } }) => x.work?.key ?? null;
    const place = _placeForTicket('ABC-9', [s(1, 1, 'ABC-1', 5), s(2, 2, 'ZED-1', 50)], [p1, p2] as never, keyOf as never);
    expect(place?.project.project.id).toBe(1);
    expect(place?.host).toBe('h1');
    expect(_placeForTicket('QQ-1', [s(1, 1, 'ABC-1', 5)], [p1] as never, keyOf as never)).toBeNull();
  });

  it('a GitHub issue is placed by its repo, never by a Jira prefix that reads like its owner', () => {
    const p1 = { project: { id: 1, owner: 'o', repo: 'a', base_path: '/a', last_session_at: 1, adopted: false, system: false }, worktrees: [] };
    const p2 = { project: { id: 2, owner: 'o', repo: 'b', base_path: '/b', last_session_at: 9, adopted: false, system: false }, worktrees: [] };
    const s = (id: number, pid: number, key: string, at: number) =>
      ({ id, project_id: pid, host_alias: `h${id}`, last_activity_at: at, work: { key } }) as never;
    const keyOf = (x: { work?: { key: string } }) => x.work?.key ?? null;
    // `acme-corp/web#7` is not ACME-*: the Jira session must not place it…
    expect(_placeForTicket('acme-corp/web#7', [s(1, 1, 'ACME-12', 5)], [p1] as never, keyOf as never)).toBeNull();
    // …nor the other way round.
    expect(_placeForTicket('ACME-3', [s(1, 1, 'acme-corp/web#7', 5)], [p1] as never, keyOf as never)).toBeNull();
    // The same repo places it; another repo of the same owner does not.
    const place = _placeForTicket(
      'torvalds/linux#1',
      [s(1, 1, 'torvalds/linux#5', 5), s(2, 2, 'torvalds/subsurface#2', 50)],
      [p1, p2] as never,
      keyOf as never,
    );
    expect(place?.project.project.id).toBe(1);
    expect(place?.host).toBe('h1');
    // Asana tasks are one family.
    expect(
      _placeForTicket('asana:1207000000000009', [s(1, 1, 'asana:1207000000000001', 5)], [p1] as never, keyOf as never)?.host,
    ).toBe('h1');
  });
});

describe('ticket rows across providers (work graph M6)', () => {
  const t = (over: Partial<_TicketRow>): _TicketRow => ({
    id: 1,
    source: 'asana',
    key: 'asana:1207000000000001',
    title: 'Migrate login to SSO',
    status_category: 'in_progress',
    created_at: 1,
    updated_at: 1,
    tracker_id: 3,
    ...over,
  });

  it('shows an Asana key short and carries the tracker badge', () => {
    const rows = _ticketEntries([{ ticket: t({}), section: 'My work' }], new Map([[3, { icon: 'A', title: 'Asana' }]]));
    expect(rows[0].label).toBe('Asana …000001 Migrate login to SSO');
    expect(rows[0].badge).toEqual({ icon: 'A', title: 'Asana' });
    expect(rows[0].key).toBe('ticket:asana:1207000000000001');
    // No badges given (one provider): none shown.
    expect(_ticketEntries([{ ticket: t({}), section: 'My work' }])[0].badge).toBeUndefined();
  });

  it('a typed owner/repo#n is looked up as typed', () => {
    const e = _lookupEntry('acme/api#42', new Set());
    expect(e?.label).toBe('Look up acme/api#42');
    expect(e?.lookup).toBe('acme/api#42');
  });
});
