import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { sessions, loadSessions, killSession, renameSession, restartSession, newSessionAbortable } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  sessions.set([]);
});

const sample = [
  { id: 1, tmux_name: 'dev-foo', host_alias: 'local', project_id: null, worktree_id: null, created_at: 1, last_activity_at: 2, status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null },
];

describe('sessions store', () => {
  it('loadSessions populates on Ok', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample);
    const r = await loadSessions();
    expect(r.ok).toBe(true);
    expect(get(sessions)).toHaveLength(1);
  });

  it('killSession returns Ok with deleted id and removes from store', async () => {
    sessions.set(sample);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(1); // kill_session returns id
    const r = await killSession('local', 'dev-foo');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toBe(1);
    expect(get(sessions)).toHaveLength(0);
  });

  it('renameSession passes old/new and merges into store', async () => {
    const renamed = { ...sample[0], tmux_name: 'dev-bar' };
    sessions.set(sample);
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(renamed); // rename_session
    const r = await renameSession('local', 'dev-foo', 'dev-bar');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'rename_session',
      { args: { host_alias: 'local', old_name: 'dev-foo', new_name: 'dev-bar' } },
    ]);
    expect(get(sessions)[0].tmux_name).toBe('dev-bar');
  });

  it('restartSession returns Ok and merges into store', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(sample[0]); // restart_session
    const r = await restartSession('local', 'dev-foo');
    expect(r.ok).toBe(true);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'restart_session',
      { args: { host_alias: 'local', name: 'dev-foo' } },
    ]);
  });

  it('newSessionAbortable fires cancel_command on abort', async () => {
    let resolveInvoke: (v: unknown) => void = () => {};
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation((cmd: string) => {
      if (cmd === 'cancel_command') return Promise.resolve(null);
      return new Promise((res) => {
        resolveInvoke = res;
      });
    });
    const ac = new AbortController();
    const p = newSessionAbortable(
      { host_alias: 'local', project_id: 1, worktree_id: null, name: 'dev-test' },
      ac.signal,
    );
    ac.abort();
    await new Promise((r) => setTimeout(r, 0));
    const sawCancel = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.some(
      (c) => c[0] === 'cancel_command',
    );
    expect(sawCancel).toBe(true);
    resolveInvoke(null);
    await p;
  });

  it('newSessionAbortable returns E_CANCELLED if signal already aborted', async () => {
    const ac = new AbortController();
    ac.abort();
    const r = await newSessionAbortable(
      { host_alias: 'local', project_id: 1, worktree_id: null, name: 'dev-test' },
      ac.signal,
    );
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_CANCELLED');
  });
});
