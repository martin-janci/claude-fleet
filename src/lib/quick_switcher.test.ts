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
    ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [],
    ...over,
  };
}

const projects: ProjectTreeRow[] = [
  {
    project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: 10 },
    worktrees: [
      { id: 11, project_id: 1, host_alias: 'local', name: 'main', path: '/r/cf', branch: 'main' },
      { id: 12, project_id: 1, host_alias: 'local', name: 'blue-sirius', path: '/r/cf/.worktrees/blue-sirius', branch: 'blue-sirius' },
    ],
  },
  {
    project: { id: 2, owner: 'acme', repo: 'widgets', base_path: '/r/w', last_session_at: 20 },
    worktrees: [],
  },
];

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
