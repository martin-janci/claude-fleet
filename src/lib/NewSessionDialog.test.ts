import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { tick } from 'svelte';

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
        return { id: 99, tmux_name: 'dev-foo', host_alias: 'mefistos', project_id: 1, worktree_id: null, created_at: 1, last_activity_at: 1, status: 'running', notes: null, account_uuid: null };
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
});
