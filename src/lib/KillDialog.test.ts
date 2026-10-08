// Kill reads the worktree first (redesign step 1.7): a dirty tree is listed,
// file by file, and Clean up commits and pushes through the agent before it
// removes; a clean one is removed at once; Kill stays the caller's.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke } from '@tauri-apps/api/core';
import KillDialog from './KillDialog.svelte';
import { workLine } from './kill_check';
import { session } from './hosts_fixture';

const flush = async () => {
  for (let i = 0; i < 8; i++) {
    await Promise.resolve();
    await tick();
  }
};
const calls = (cmd: string) => vi.mocked(invoke).mock.calls.filter((c) => c[0] === cmd);

const dirty = {
  has_worktree: true,
  worktree_path: '/w/api--fix',
  branch: 'fix-login',
  upstream: 'origin/fix-login',
  dirty_files: [
    { status: 'M', path: 'src/login.ts' },
    { status: '??', path: 'notes.md' },
  ],
  unpushed_commits: 2,
  safe_to_remove: false,
  error: null,
};
const clean = { ...dirty, dirty_files: [], unpushed_commits: 0, safe_to_remove: true };

let inspections: Record<string, unknown>;
beforeEach(() => {
  inspections = {};
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string, raw?: unknown) => {
    const a = (raw as { args?: { tmux_name?: string } } | undefined)?.args ?? {};
    if (cmd === 'inspect_safe_kill') return inspections[a.tmux_name ?? ''];
    if (cmd === 'safe_kill_session') return session('mac', a.tmux_name ?? '', { id: 1, safe_kill_state: 'requested' });
    if (cmd === 'discard_kill_session') return 2;
    return null;
  });
});

describe('KillDialog', () => {
  it('lists the uncommitted files and unpushed commits of a dirty tree', async () => {
    inspections['api--fix'] = dirty;
    const onkill = vi.fn();
    render(KillDialog, { targets: [session('mac', 'api--fix', { id: 1 })], onkill, oncleaned: vi.fn(), oncancel: vi.fn() });
    expect(screen.getByTestId('kill-checking')).toBeTruthy();
    await flush();
    const row = screen.getByTestId('kill-dirty-row');
    expect(row.textContent).toContain('2 uncommitted files and 2 commits not pushed');
    expect(row.textContent).toContain('src/login.ts');
    expect(row.textContent).toContain('notes.md');
    expect(calls('inspect_safe_kill')).toHaveLength(1);
    // Kill is still one click away and stays the caller's.
    await fireEvent.click(screen.getByTestId('confirm-kill'));
    expect(onkill).toHaveBeenCalledOnce();
  });

  it('Clean up asks the agent to commit and push a dirty tree, and removes a clean one at once', async () => {
    inspections['api--fix'] = dirty;
    inspections['api--done'] = clean;
    const oncleaned = vi.fn();
    const a = session('mac', 'api--fix', { id: 1 });
    const b = session('mac', 'api--done', { id: 2 });
    render(KillDialog, { targets: [a, b], mode: 'cleanup', onkill: vi.fn(), oncleaned, oncancel: vi.fn() });
    await flush();
    expect(screen.getByRole('dialog', { name: 'Clean up 2 sessions?' })).toBeTruthy();
    expect(screen.getAllByTestId('kill-dirty-row')).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('kill-cleanup'));
    await flush();
    expect(calls('safe_kill_session').map((c) => (c[1] as { args: { tmux_name: string } }).args.tmux_name)).toEqual(['api--fix']);
    expect(calls('discard_kill_session')).toHaveLength(1);
    expect((calls('discard_kill_session')[0][1] as { force: boolean }).force).toBe(false);
    expect(calls('kill_session')).toHaveLength(0);
    const [removed, asked] = oncleaned.mock.calls[0];
    expect(removed.map((r: { id: number }) => r.id)).toEqual([2]);
    expect(asked.map((r: { id: number }) => r.id)).toEqual([1]);
  });

  it('says so when the tree is clean', async () => {
    inspections['api--done'] = clean;
    render(KillDialog, {
      targets: [session('mac', 'api--done', { id: 2 })],
      onkill: vi.fn(),
      oncleaned: vi.fn(),
      oncancel: vi.fn(),
    });
    await flush();
    expect(screen.getByTestId('kill-clean')).toBeTruthy();
  });

  it('a failed read is shown, never hidden as clean', async () => {
    inspections['api--odd'] = { ...dirty, error: 'git: not a repository' };
    render(KillDialog, { targets: [session('mac', 'api--odd', { id: 3 })], onkill: vi.fn(), oncleaned: vi.fn(), oncancel: vi.fn() });
    await flush();
    expect(screen.queryByTestId('kill-clean')).toBeNull();
    expect(screen.getByTestId('kill-unknown').textContent).toContain('not a repository');
  });

  it('words the summary', () => {
    expect(workLine({ state: 'dirty', files: [{ status: 'M', path: 'a' }], unpushed: 0, branch: null })).toBe('1 uncommitted file');
    expect(workLine({ state: 'dirty', files: [], unpushed: 1, branch: null })).toBe('1 commit not pushed');
    expect(workLine({ state: 'clean' })).toBeNull();
  });
});
