import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import { selectedSession, selectSession, restoreLastSession, clearSelection } from './selection';
import {
  sessions,
  mergeSession,
  removeSession,
  applySessionEvents,
  resetTombstonesForTests,
  type SessionRow,
} from './sessions';

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
    friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null,
    ...over,
  };
}

beforeEach(() => {
  localStorage.clear();
  clearSelection();
  resetTombstonesForTests();
  sessions.set([]);
});

describe('last-session persistence', () => {
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
    selectSession(null);
    // Same session, fresh DB row id after re-discovery.
    const reloaded = makeSession({ id: 99 });
    sessions.set([reloaded]);
    restoreLastSession();
    expect(get(selectedSession)).toEqual(reloaded);
  });

  it('restoreLastSession selects nothing and clears the pref when the session is gone', () => {
    selectSession(makeSession());
    selectSession(null);
    sessions.set([]); // session was killed
    restoreLastSession();
    expect(get(selectedSession)).toBeNull();
    expect(localStorage.getItem('cf:pref:session.last')).toBe(JSON.stringify(null));
  });

  it('restoreLastSession selects nothing and clears the pref when the session is a ghost', () => {
    selectSession(makeSession());
    selectSession(null);
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

// FE-2: `selectedSession` used to be a frozen snapshot written only on click,
// so SessionDetails read a stale `status` / `safe_kill_state` forever. It is
// now derived from the sessions store, keyed by the selected identity.
describe('selectedSession is derived from the sessions store', () => {
  it('reflects a session:updated merge for the selected session', () => {
    const row = makeSession({ id: 7, status: 'running', last_activity_at: 10 });
    sessions.set([row]);
    selectSession(row);
    expect(get(selectedSession)?.safe_kill_state).toBeNull();

    mergeSession({ ...row, status: 'frozen', safe_kill_state: 'requested', last_activity_at: 11 });

    const cur = get(selectedSession);
    expect(cur?.id).toBe(7);
    expect(cur?.status).toBe('frozen');
    expect(cur?.safe_kill_state).toBe('requested');
  });

  it('reflects a batched update too', () => {
    const row = makeSession({ id: 7, last_activity_at: 10 });
    sessions.set([row]);
    selectSession(row);
    applySessionEvents([
      { type: 'updated', row: { ...row, claude_status: 'working', last_activity_at: 11 } },
      { type: 'updated', row: { ...row, claude_status: 'blocked', last_activity_at: 12 } },
    ]);
    expect(get(selectedSession)?.claude_status).toBe('blocked');
  });

  it('clears the selection when the selected row is removed', () => {
    const row = makeSession({ id: 7 });
    sessions.set([row, makeSession({ id: 8, tmux_name: 'dev-bar' })]);
    selectSession(row);
    expect(get(selectedSession)?.id).toBe(7);

    removeSession(7);

    expect(get(selectedSession)).toBeNull();
    // ...and stays cleared even if a row with the same identity reappears —
    // the user must pick it again.
    sessions.set([makeSession({ id: 7 })]);
    expect(get(selectedSession)).toBeNull();
  });

  it('clears the selection when a full re-fetch no longer contains the row', () => {
    const row = makeSession({ id: 7 });
    sessions.set([row]);
    selectSession(row);
    sessions.set([makeSession({ id: 9, tmux_name: 'other' })]);
    expect(get(selectedSession)).toBeNull();
  });

  it('follows a rename of the selected row (same id, new tmux_name)', () => {
    const row = makeSession({ id: 7, tmux_name: 'old', last_activity_at: 1 });
    sessions.set([row]);
    selectSession(row);
    mergeSession({ ...row, tmux_name: 'new', last_activity_at: 2 });
    expect(get(selectedSession)?.tmux_name).toBe('new');
  });

  it('survives an id churn via the host+name fallback', () => {
    const row = makeSession({ id: 7 });
    sessions.set([row]);
    selectSession(row);
    // Re-discovery replaced the row under a fresh id in one atomic set.
    sessions.set([makeSession({ id: 70 })]);
    expect(get(selectedSession)?.id).toBe(70);
  });

  it('never matches by tmux_name alone across hosts', () => {
    const a = makeSession({ id: 1, host_alias: 'alpha', tmux_name: 'dev-same' });
    const b = makeSession({ id: 2, host_alias: 'beta', tmux_name: 'dev-same' });
    sessions.set([a, b]);
    selectSession(b);
    expect(get(selectedSession)?.host_alias).toBe('beta');
    // Host beta's row disappears; host alpha's twin must NOT be adopted.
    sessions.set([a]);
    expect(get(selectedSession)).toBeNull();
  });

  it('selectSession with a row the store has not seen yet merges it first', () => {
    const fresh = makeSession({ id: 42 });
    selectSession(fresh);
    expect(get(sessions).map((s) => s.id)).toEqual([42]);
    expect(get(selectedSession)?.id).toBe(42);
  });
});
