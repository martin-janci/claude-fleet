// The Local workspace card in session details: Enable when the worktree has
// no link, the state, paths and conflicts when it does, and a note instead of
// controls on a desktop paired with a hub.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import LocalWorkspaceCard from './LocalWorkspaceCard.svelte';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import {
  localWorkspaces,
  _resetLocalWorkspacesForTests,
  type LocalWorkspace,
} from './local_workspaces';

const link = (over: Partial<LocalWorkspace> = {}): LocalWorkspace => ({
  id: 9,
  host_alias: 'devbox',
  owner: 'acme',
  repo: 'app',
  project_id: 4,
  worktree_key: 'main',
  remote_path: '/home/u/projects/github.com/acme/app',
  local_path: '/Users/me/fleet/app',
  paused: false,
  excludes: [],
  state: 'synced',
  last_sync_at: Math.floor(Date.now() / 1000) - 3,
  pending_local: 0,
  pending_remote: 0,
  skipped: 0,
  conflicts: [],
  created_at: 1,
  ...over,
});

const row = () => session('devbox', 'dev-app', { project_id: 4, worktree_key: null });

beforeEach(() => {
  invoke.mockReset();
  _resetLocalWorkspacesForTests();
  hubStatus.set({ ...STANDALONE });
});

describe('LocalWorkspaceCard', () => {
  it('offers Enable when the worktree has no link, and sends the folder', async () => {
    invoke.mockResolvedValue(link());
    render(LocalWorkspaceCard, { session: row() });
    const input = screen.getByTestId('lw-folder') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '/Users/me/code/app' } });
    await fireEvent.click(screen.getByTestId('lw-enable'));
    expect(invoke).toHaveBeenCalledWith('enable_local_workspace', {
      args: { session_id: expect.any(Number), local_path: '/Users/me/code/app', excludes: [] },
    });
  });

  it('forgets a typed folder when the card moves to another session', async () => {
    const view = render(LocalWorkspaceCard, { session: session('devbox', 'a', { id: 1, project_id: 4 }) });
    const input = screen.getByTestId('lw-folder') as HTMLInputElement;
    await fireEvent.input(input, { target: { value: '/Users/me/fleet/foo' } });
    await view.rerender({ session: session('devbox', 'b', { id: 2, project_id: 4 }) });
    expect((screen.getByTestId('lw-folder') as HTMLInputElement).value).toBe('');
  });

  it('shows the state, both paths and the conflicts with their two picks', async () => {
    localWorkspaces.set([
      link({
        state: 'conflict',
        conflicts: [{ path: 'src/foo.rs', kind: 'both_modified', detected_at: 1 }],
      }),
    ]);
    invoke.mockResolvedValue(link());
    render(LocalWorkspaceCard, { session: row() });
    expect(screen.getByTestId('lw-status')).toHaveTextContent('1 conflict');
    expect(screen.getByTestId('local-workspace')).toHaveTextContent('/Users/me/fleet/app');
    expect(screen.getByTestId('local-workspace')).toHaveTextContent(
      'devbox:/home/u/projects/github.com/acme/app',
    );
    expect(screen.getByTestId('lw-conflict')).toHaveTextContent('src/foo.rs');
    await fireEvent.click(screen.getByRole('button', { name: 'Keep remote' }));
    expect(invoke).toHaveBeenCalledWith('resolve_local_workspace_conflict', {
      args: { id: 9, path: 'src/foo.rs', keep: 'remote' },
    });
  });

  it('pauses and resumes', async () => {
    localWorkspaces.set([link()]);
    invoke.mockResolvedValue(link({ paused: true, state: 'paused' }));
    render(LocalWorkspaceCard, { session: row() });
    await fireEvent.click(screen.getByTestId('lw-pause'));
    expect(invoke).toHaveBeenCalledWith('pause_local_workspace', { args: { id: 9 } });
    expect(await screen.findByTestId('lw-resume')).toBeInTheDocument();
  });

  it('works on a desktop paired with a hub too', () => {
    hubStatus.set({ ...STANDALONE, remote: true, url: 'https://hub.example' });
    render(LocalWorkspaceCard, { session: row() });
    expect(screen.getByTestId('lw-enable')).toBeInTheDocument();
  });

  it('opens the folder in an IDE and hands the worktree over', async () => {
    localWorkspaces.set([link()]);
    invoke.mockResolvedValue(link({ driver: 'developer' }));
    render(LocalWorkspaceCard, { session: row() });
    await fireEvent.click(screen.getByTestId('lw-open-vscode'));
    expect(invoke).toHaveBeenCalledWith('open_local_workspace', { args: { id: 9, app: 'vscode' } });
    await fireEvent.click(screen.getByTestId('lw-take-over'));
    expect(invoke).toHaveBeenCalledWith('set_local_workspace_driver', {
      args: { id: 9, driver: 'developer' },
    });
    expect(await screen.findByTestId('lw-driver')).toHaveTextContent('You’re driving');
    expect(screen.queryByTestId('lw-take-over')).toBeNull();
  });

  it('counts each side’s changes and asks the agent to continue', async () => {
    localWorkspaces.set([link({ local_activity: 7, remote_activity: 2 })]);
    invoke.mockResolvedValue(link());
    render(LocalWorkspaceCard, { session: row() });
    expect(screen.getByTestId('lw-activity')).toHaveTextContent('7 local changes');
    expect(screen.getByTestId('lw-activity')).toHaveTextContent('2 agent changes');
    await fireEvent.click(screen.getByTestId('lw-ask-continue'));
    expect(invoke).toHaveBeenCalledWith('ask_ai_about_local_changes', {
      args: { id: 9, intent: 'continue', question: null, paths: null },
    });
  });

  it('offers Compare, Keep both and Ask AI to resolve on a conflict', async () => {
    localWorkspaces.set([
      link({
        state: 'conflict',
        conflicts: [{ path: 'src/foo.rs', kind: 'both_modified', detected_at: 1 }],
      }),
    ]);
    invoke.mockResolvedValueOnce({
      path: 'src/foo.rs',
      diff: '@@ -1 +1 @@\n-mine\n+theirs\n',
      binary: false,
      truncated: false,
    });
    render(LocalWorkspaceCard, { session: row() });
    await fireEvent.click(screen.getByTestId('lw-compare'));
    expect(invoke).toHaveBeenCalledWith('compare_local_conflict', {
      args: { id: 9, path: 'src/foo.rs' },
    });
    expect(await screen.findByTestId('lw-compare-diff')).toHaveTextContent('theirs');
    invoke.mockResolvedValue(link());
    await fireEvent.click(screen.getByTestId('lw-keep-both'));
    expect(invoke).toHaveBeenCalledWith('keep_both_local_conflict', {
      args: { id: 9, path: 'src/foo.rs' },
    });
  });

  it('runs the Liquid orbit while it combines both sides of a conflict, and only then', async () => {
    localWorkspaces.set([
      link({
        state: 'conflict',
        conflicts: [{ path: 'src/foo.rs', kind: 'both_modified', detected_at: 1 }],
      }),
    ]);
    let finish!: (v: unknown) => void;
    invoke.mockImplementation(() => new Promise((r) => (finish = r)));
    render(LocalWorkspaceCard, { session: row() });
    expect(screen.queryByTestId('lw-combining')).toBeNull();
    await fireEvent.click(screen.getByTestId('lw-ask-resolve'));
    expect(screen.getByTestId('lw-combining')).toBeInTheDocument();
    expect(await screen.findByTestId('lw-liquid', {}, { timeout: 2000 })).toHaveAttribute('data-loader', 'liquid-orbit');
    finish(null);
    await vi.waitFor(() => expect(screen.queryByTestId('lw-combining')).toBeNull());
  });
});
