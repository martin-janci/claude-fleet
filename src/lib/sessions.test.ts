import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import { sessions, loadSessions, killSession, renameSession, restartSession, repairSession, newSessionAbortable, newBgSession, peekSession, purgeProject, showBgAgents, resetTombstonesForTests } from './sessions';
import { formatCostMicros, formatTokens, sessionUsageTokens } from './sessions';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  resetTombstonesForTests();
  sessions.set([]);
});

describe('usage formatting', () => {
  it('formats token counts compactly', () => {
    expect(formatTokens(0)).toBe('0');
    expect(formatTokens(null)).toBe('0');
    expect(formatTokens(950)).toBe('950');
    expect(formatTokens(1_234)).toBe('1.2k');
    expect(formatTokens(123_456)).toBe('123k');
    expect(formatTokens(4_560_000)).toBe('4.56M');
    expect(formatTokens(245_000_000)).toBe('245.0M');
    expect(formatTokens(2_500_000_000)).toBe('2.50B');
  });

  it('formats estimated cost from micro-USD', () => {
    expect(formatCostMicros(0)).toBe('$0.00');
    expect(formatCostMicros(undefined)).toBe('$0.00');
    expect(formatCostMicros(5_000)).toBe('<$0.01');
    expect(formatCostMicros(1_234_567)).toBe('$1.23');
    expect(formatCostMicros(1_234_000_000)).toBe('$1,234');
  });

  it('sums every token counter and treats missing fields as zero', () => {
    expect(sessionUsageTokens({ usage_input_tokens: 1, usage_output_tokens: 2, usage_cache_write_tokens: 3, usage_cache_read_tokens: 4 })).toBe(10);
    expect(sessionUsageTokens({})).toBe(0);
  });
});

const sample = [
  { id: 1, tmux_name: 'dev-foo', host_alias: 'local', project_id: null, worktree_id: null, created_at: 1, last_activity_at: 2, status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, friendly_name: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null, context_pct: null, stuck_kind: null, idle_since: null, stuck_since: null, last_playbook_at: null, last_prompt: null, started_at: null, last_turn_at: null, ci_status: null, turn_seq: 0, last_stop_at: null, parent_session_id: null, tags: [] },
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

  it('repairSession passes the session id and returns the report untouched', async () => {
    sessions.set(sample);
    const report = {
      session_id: 1, host_alias: 'local', tmux_name: 'dev-foo', project_root: '/repo', cwd: '/repo/.worktrees/x',
      healthy: false, actions: ['git worktree prune', 'git worktree add /repo/.worktrees/x x'],
      warnings: [], branch_source: 'branch_local', tmux: 'respawned', tmux_alive: true,
      tmux_cwd_stale: false, worktree_row_updated: false, sibling_session_ids: [],
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(report); // repair_session
    const r = await repairSession(1);
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual(report);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'repair_session',
      { args: { session_id: 1, explicit: false } },
    ]);
    // Row events carry any store change; the wrapper itself merges nothing.
    expect(get(sessions)).toEqual(sample);
  });

  it('repairSession surfaces the backend E_* code on failure', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockRejectedValueOnce({
      code: 'E_REPO_MISSING',
      message: 'project repository is missing',
    });
    const r = await repairSession(1);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error.code).toBe('E_REPO_MISSING');
  });

  it('repairSession defaults to the automatic check; explicit must be asked for', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    await repairSession(3, { explicit: true });
    await repairSession(4, {});
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls).toEqual([
      ['repair_session', { args: { session_id: 3, explicit: true } }],
      ['repair_session', { args: { session_id: 4, explicit: false } }],
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

describe('newBgSession', () => {
  it('calls new_bg_session with correct args and returns result', async () => {
    const payload = { claude_session_id: 'abc-123' };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce(payload);
    const r = await newBgSession('local', 'my-session', 'Do the thing');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual(payload);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'new_bg_session',
      { args: { host_alias: 'local', name: 'my-session', prompt: 'Do the thing' } },
    ]);
  });
});

describe('peekSession', () => {
  it('calls peek_session with correct args and returns log output', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce('log output here');
    const r = await peekSession('local', 'sess-id-456');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toBe('log output here');
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'peek_session',
      { args: { host_alias: 'local', claude_session_id: 'sess-id-456' } },
    ]);
  });
});

describe('purgeProject', () => {
  it('calls purge_project with correct args and returns the report', async () => {
    const report = {
      host_alias: 'box',
      logical_path: '/home/user/my-project',
      physical_path: '/mnt/user/my-project',
      purged: ['/mnt/user/my-project'],
      not_found: ['/home/user/my-project'],
    };
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([report]);
    const r = await purgeProject(['box', 'local'], '/home/user/my-project', 42);
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.value).toEqual([report]);
    expect((mockedInvoke as ReturnType<typeof vi.fn>).mock.calls[0]).toEqual([
      'purge_project',
      {
        args: {
          host_aliases: ['box', 'local'],
          project_path: '/home/user/my-project',
          project_id: 42,
        },
      },
    ]);
  });
});

describe('showBgAgents', () => {
  it('defaults to true and persists changes to localStorage', () => {
    localStorage.clear();
    expect(get(showBgAgents)).toBe(true);
    showBgAgents.set(false);
    expect(localStorage.getItem('cf:pref:show-bg-agents')).toBe('false');
  });
});
