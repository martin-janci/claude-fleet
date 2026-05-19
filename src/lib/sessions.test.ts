import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { sessions, loadSessions, killSession } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([]);
});

const sample = [
  { id: 1, tmux_name: 'dev-foo', host_alias: 'local', project_id: null, worktree_id: null, created_at: 1, last_activity_at: 2, status: 'running', notes: null },
];

describe('sessions store', () => {
  it('loadSessions populates on Ok', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample);
    const r = await loadSessions();
    expect(r.ok).toBe(true);
    expect(get(sessions)).toHaveLength(1);
  });

  it('killSession returns Ok and reloads', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(null); // kill_session
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]); // list_sessions
    const r = await killSession('dev-foo');
    expect(r.ok).toBe(true);
    expect(get(sessions)).toHaveLength(0);
  });
});
