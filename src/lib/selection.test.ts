import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { selectedSession, selectSession, restoreLastSession } from './selection';
import { sessions, type SessionRow } from './sessions';

function makeSession(over: Partial<SessionRow> = {}): SessionRow {
  return {
    id: 1,
    tmux_name: 'dev-foo',
    host_alias: 'mefistos',
    project_id: null,
    worktree_id: null,
    created_at: 0,
    last_activity_at: 0,
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
    ...over,
  };
}

describe('last-session persistence', () => {
  beforeEach(() => {
    localStorage.clear();
    selectedSession.set(null);
    sessions.set([]);
  });

  it('selectSession persists the host_alias+tmux_name identity', () => {
    selectSession(makeSession());
    expect(localStorage.getItem('cf:pref:session.last')).toBe(
      JSON.stringify({ host_alias: 'mefistos', tmux_name: 'dev-foo' }),
    );
  });

  it('selectSession(null) leaves the remembered session intact', () => {
    selectSession(makeSession());
    selectSession(null);
    expect(localStorage.getItem('cf:pref:session.last')).toBe(
      JSON.stringify({ host_alias: 'mefistos', tmux_name: 'dev-foo' }),
    );
  });

  it('restoreLastSession re-selects by stable identity even when the id changed', () => {
    selectSession(makeSession({ id: 1 }));
    selectedSession.set(null);
    // Same session, fresh DB row id after re-discovery.
    const reloaded = makeSession({ id: 99 });
    sessions.set([reloaded]);
    restoreLastSession();
    expect(get(selectedSession)).toEqual(reloaded);
  });

  it('restoreLastSession selects nothing and clears the pref when the session is gone', () => {
    selectSession(makeSession());
    selectedSession.set(null);
    sessions.set([]); // session was killed
    restoreLastSession();
    expect(get(selectedSession)).toBeNull();
    expect(localStorage.getItem('cf:pref:session.last')).toBe(JSON.stringify(null));
  });

  it('restoreLastSession selects nothing and clears the pref when the session is a ghost', () => {
    selectSession(makeSession());
    selectedSession.set(null);
    sessions.set([makeSession({ status: 'ghost' })]);
    restoreLastSession();
    expect(get(selectedSession)).toBeNull();
    expect(localStorage.getItem('cf:pref:session.last')).toBe(JSON.stringify(null));
  });

  it('restoreLastSession is a no-op when nothing was remembered', () => {
    restoreLastSession();
    expect(get(selectedSession)).toBeNull();
  });
});
