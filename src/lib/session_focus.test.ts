import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { sessions, type SessionRow } from './sessions';
import { selectedSession, selectSession } from './selection';
import { sessionFocus, focusSession, clearSessionFocus } from './session_focus';
import { toasts, clearToasts } from './toasts';

function row(id: number, name: string): SessionRow {
  return {
    id,
    tmux_name: name,
    host_alias: 'local',
    project_id: 1,
    worktree_id: null,
    created_at: 1,
    last_activity_at: 1,
    status: 'running',
    notes: null,
    account_uuid: null,
    kind: 'work',
  } as SessionRow;
}

describe('focusSession', () => {
  beforeEach(() => {
    sessions.set([row(1, 'dev-a'), row(2, 'dev-b')]);
    clearSessionFocus();
    selectSession(null);
    clearToasts();
  });

  it('narrows to a session the store has and opens it', () => {
    expect(focusSession(2, 'dev-b')).toBe(true);
    expect(get(sessionFocus)).toEqual({ id: 2, label: 'dev-b' });
    expect(get(selectedSession)?.id).toBe(2);
    expect(get(toasts)).toEqual([]);
  });

  it('sets no focus for an id the store does not have, and says so', () => {
    // A tidy-up candidate killed between two refreshes of the sheet: the row
    // is gone from the store while the sheet still lists it.
    expect(focusSession(99, 'dev-gone')).toBe(false);
    expect(get(sessionFocus)).toBeNull();
    expect(get(selectedSession)).toBeNull();
    expect(get(toasts).map((t) => t.message)).toEqual([
      'dev-gone is gone: the session is no longer in the fleet.',
    ]);
  });

  it('a focus set earlier is kept when a later click names a gone session', () => {
    focusSession(1, 'dev-a');
    expect(focusSession(99, 'dev-gone')).toBe(false);
    expect(get(sessionFocus)).toEqual({ id: 1, label: 'dev-a' });
  });

  it('drops the focus when its session leaves the store', () => {
    focusSession(1, 'dev-a');
    sessions.set([row(2, 'dev-b')]);
    expect(get(sessionFocus)).toBeNull();
  });
});
