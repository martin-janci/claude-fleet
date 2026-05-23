import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';
import * as sessionsModule from './sessions';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';
import { hosts } from './hosts';

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
  hosts.set([
    { alias: 'local', ssh_alias: null, reachable: true, claude_version: '2.1.145', tmux_version: '3.5a', hidden: false, last_pinged_at: 1, account_uuid: null },
    { alias: 'mefistos', ssh_alias: 'mefistos', reachable: true, claude_version: '2.1.144', tmux_version: '3.6a', hidden: false, last_pinged_at: 1, account_uuid: null },
  ]);
  localStorage.clear();
});

const project = {
  project: { id: 1, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
  worktrees: [{ id: 11, project_id: 1, name: 'main', path: '/r/cf', branch: 'main' }],
};

describe('NewSessionDialog', () => {
  it('renders one host-pick button per non-hidden host', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const picks = document.querySelectorAll('.host-pick');
    expect(picks).toHaveLength(2);
    expect(Array.from(picks).map((p) => p.textContent?.trim())).toEqual(['local', 'mefistos']);
  });

  it('defaults to last-host pref (local on first run)', async () => {
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const active = document.querySelector('.host-pick.active');
    expect(active?.textContent?.trim()).toBe('local');
  });

  it('clicking a host pick + Create sends host_alias to new_session', async () => {
    (mockedInvoke as ReturnType<typeof vi.fn>).mockImplementation(async (cmd: string) => {
      if (cmd === 'new_session') {
        return { id: 99, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null, kind: 'work', reviews_session_id: null, worktree_key: null, lost_at: null, claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null };
      }
      if (cmd === 'list_sessions') return [];
      return null;
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    const mefBtn = Array.from(document.querySelectorAll('.host-pick')).find((p) => p.textContent?.trim() === 'mefistos') as HTMLButtonElement;
    await fireEvent.click(mefBtn);
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    const newSessionCall = (mockedInvoke as ReturnType<typeof vi.fn>).mock.calls.find((c) => c[0] === 'new_session');
    expect((newSessionCall![1] as any).args.host_alias).toBe('mefistos');
  });

  it('clicking + new chip, typing a name, and clicking Create passes new_worktree and worktree_id=null', async () => {
    const newSessionAbortableSpy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 42,
        tmux_name: 'dev-martin-janci-claude-fleet--feat-test',
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
        worktree_key: null,
        lost_at: null,
        claude_session_id: null,
        claude_status: null,
        effort_level: null,
        pr_url: null,
        current_activity: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
      },
    });

    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();

    // Click the "+ new" chip
    const newChip = screen.getByTestId('new-worktree-chip');
    await fireEvent.click(newChip);
    await tick();

    // Type a worktree name
    const nameInput = screen.getByTestId('new-worktree-name');
    await fireEvent.input(nameInput, { target: { value: 'feat-test' } });
    await tick();

    // Click Create
    await fireEvent.click(screen.getByText('Create'));
    await tick();

    expect(newSessionAbortableSpy).toHaveBeenCalledOnce();
    const callArgs = newSessionAbortableSpy.mock.calls[0][0];
    expect(callArgs.new_worktree).toBe('feat-test');
    expect(callArgs.worktree_id).toBeNull();

    newSessionAbortableSpy.mockRestore();
  });

  it('defaults to a "work" session', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 1, tmux_name: 'dev-martin-janci-claude-fleet', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'work',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].kind).toBe('work');
    spy.mockRestore();
  });

  it('picking Shell passes kind="shell" and suffixes the tmux name with -term', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 2, tmux_name: 'dev-martin-janci-claude-fleet-term', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'shell',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].kind).toBe('shell');
    expect(spy.mock.calls[0][0].name).toBe('dev-martin-janci-claude-fleet-term');
    expect(spy.mock.calls[0][0].start_command).toBeNull();
    spy.mockRestore();
  });

  it('a start command typed in Shell mode is passed as start_command', async () => {
    const spy = vi.spyOn(sessionsModule, 'newSessionAbortable').mockResolvedValue({
      ok: true,
      value: {
        id: 3, tmux_name: 'dev-martin-janci-claude-fleet-term', host_alias: 'local',
        project_id: 1, worktree_id: 11, created_at: 1, last_activity_at: 1,
        status: 'running', notes: null, account_uuid: null, kind: 'shell',
        reviews_session_id: null, worktree_key: null, lost_at: null,
        claude_session_id: null, claude_status: null, effort_level: null, pr_url: null, current_activity: null, safe_kill_state: null, safe_kill_nonce: null, safe_kill_detail: null, safe_kill_requested_at: null,
      },
    });
    render(NewSessionDialog, { props: { project, onCreate: () => {}, onCancel: () => {} } });
    await tick();
    await fireEvent.click(screen.getByTestId('kind-shell'));
    await tick();
    await fireEvent.input(screen.getByTestId('start-command'), { target: { value: 'pnpm test' } });
    await tick();
    await fireEvent.click(screen.getByText('Create'));
    await tick();
    expect(spy.mock.calls[0][0].start_command).toBe('pnpm test');
    spy.mockRestore();
  });
});
