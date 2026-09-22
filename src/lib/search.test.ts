import { describe, it, expect } from 'vitest';
import { sessionMatchesSearch } from './search';
import type { SessionRow } from './sessions';

function sessionFor(overrides: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1,
    tmux_name: 'dev-foo',
    host_alias: 'local',
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
    ci_status: null,
    turn_seq: 0,
    last_stop_at: null,
    parent_session_id: null,
    tags: [],
    model: null,
    context_tokens: null,
    context_window: null,
    context_source: null,
    context_at: null,
    context_stale: false,
    tmux_pane_id: null,
    pending_input: null,
    ...overrides,
  };
}

describe('sessionMatchesSearch', () => {
  it('matches a tag when no other field does', () => {
    const s = sessionFor({ tmux_name: 'dev-foo', host_alias: 'local', tags: ['review'] });
    expect(sessionMatchesSearch(s, 'rev')).toBe(true);
  });

  it('matches tags case-insensitively', () => {
    const s = sessionFor({ tags: ['review'] });
    expect(sessionMatchesSearch(s, 'rev')).toBe(true);
    // needle is expected pre-lowercased by the caller, but an upper-case tag
    // must still match.
    const s2 = sessionFor({ tags: ['REVIEW'] });
    expect(sessionMatchesSearch(s2, 'rev')).toBe(true);
  });

  it('a session with no tags does not throw and does not match a tag-only needle', () => {
    const s = sessionFor({ tags: [], tmux_name: 'dev-foo', host_alias: 'local', friendly_name: null });
    expect(() => sessionMatchesSearch(s, 'rev')).not.toThrow();
    expect(sessionMatchesSearch(s, 'rev')).toBe(false);
  });

  it('still matches on friendly_name', () => {
    const s = sessionFor({ friendly_name: 'Fix login', tmux_name: 'dev-foo', tags: [] });
    expect(sessionMatchesSearch(s, 'login')).toBe(true);
  });

  it('still matches on tmux_name and host_alias', () => {
    expect(sessionMatchesSearch(sessionFor({ tmux_name: 'dev-foo' }), 'dev-foo')).toBe(true);
    expect(sessionMatchesSearch(sessionFor({ host_alias: 'mefistos' }), 'mefistos')).toBe(true);
  });

  it('matches on last_prompt when present', () => {
    const s = sessionFor({ last_prompt: 'fix the sidebar bug', tags: [] });
    expect(sessionMatchesSearch(s, 'sidebar')).toBe(true);
  });
});
