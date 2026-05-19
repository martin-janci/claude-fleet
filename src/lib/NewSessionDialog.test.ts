import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import NewSessionDialog from './NewSessionDialog.svelte';

const project = {
  project: { id: 7, owner: 'martin-janci', repo: 'claude-fleet', base_path: '/r/cf', last_session_at: null },
  worktrees: [
    { id: 71, project_id: 7, name: 'main', path: '/r/cf', branch: 'main' },
    { id: 72, project_id: 7, name: 'feature-x', path: '/r/cf/.worktrees/feature-x', branch: 'feature-x' },
  ],
};

beforeEach(() => {
  (mockedInvoke as ReturnType<typeof vi.fn>).mockReset();
});

describe('NewSessionDialog', () => {
  it('emits onCreate with chosen worktree and default name', async () => {
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      id: 99, tmux_name: 'dev-martin-janci-claude-fleet', host_alias: 'local',
      project_id: 7, worktree_id: 71, created_at: 1, last_activity_at: 1, status: 'running', notes: null,
    });
    (mockedInvoke as ReturnType<typeof vi.fn>).mockResolvedValueOnce([]); // list_sessions

    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await fireEvent.click(screen.getByText('Create'));

    expect(onCreate).toHaveBeenCalledOnce();
    const call = (onCreate as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(call.tmux_name).toBe('dev-martin-janci-claude-fleet');
  });

  it('emits onCancel without invoking the backend', async () => {
    const onCreate = vi.fn();
    const onCancel = vi.fn();
    render(NewSessionDialog, { props: { project, onCreate, onCancel } });
    await fireEvent.click(screen.getByText('Cancel'));
    expect(onCancel).toHaveBeenCalledOnce();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});
