// Review changes: git's list with each file's side, its diff, and Ask AI /
// Commit / Discard on the files ticked.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import LocalChangesDialog from './LocalChangesDialog.svelte';
import type { LocalWorkspace } from './local_workspaces';
import { expectAccessible } from './a11y_check';

const link = {
  id: 9,
  host_alias: 'devbox',
  owner: 'acme',
  repo: 'app',
  project_id: 4,
  worktree_key: 'main',
  remote_path: '/r',
  local_path: '/l',
  paused: false,
  excludes: [],
  state: 'synced',
  pending_local: 0,
  pending_remote: 0,
  skipped: 0,
  conflicts: [],
  created_at: 1,
  local_activity: 1,
} satisfies LocalWorkspace;

const changes = {
  branch: 'feat',
  files: [
    { path: 'src/a.rs', status: 'M', staged: false, origin: 'local' },
    { path: 'src/b.rs', status: '??', staged: false, origin: 'remote' },
  ],
  activity: [{ path: 'src/a.rs', origin: 'local', change: 'modified', at: 1 }],
};

function route(over: Record<string, unknown> = {}) {
  invoke.mockImplementation((cmd: string) => {
    if (cmd in over) return Promise.resolve(over[cmd]);
    if (cmd === 'local_workspace_changes') return Promise.resolve(changes);
    if (cmd === 'local_workspace_diff')
      return Promise.resolve({ path: 'src/a.rs', diff: '@@ -1 +1 @@\n-old\n+new\n', binary: false, truncated: false });
    return Promise.resolve(link);
  });
}

beforeEach(() => invoke.mockReset());

describe('LocalChangesDialog', () => {
  it('lists the changes with their side and shows a diff', async () => {
    route();
    render(LocalChangesDialog, { link, onclose: () => {} });
    const files = await screen.findAllByTestId('lw-change');
    expect(files).toHaveLength(2);
    expect(screen.getByTestId('lw-changes-files')).toHaveTextContent('you');
    expect(screen.getByTestId('lw-changes-files')).toHaveTextContent('agent');
    await fireEvent.click(files[0]);
    expect(invoke).toHaveBeenCalledWith('local_workspace_diff', { args: { id: 9, path: 'src/a.rs' } });
    expect(await screen.findByTestId('lw-diff')).toHaveTextContent('new');
  });

  it('asks the agent about the ticked files only', async () => {
    route();
    const onclose = vi.fn();
    render(LocalChangesDialog, { link, onclose });
    await screen.findAllByTestId('lw-change');
    await fireEvent.click(screen.getByLabelText('Include src/b.rs'));
    await fireEvent.change(screen.getByTestId('lw-ask-intent'), { target: { value: 'review' } });
    await fireEvent.click(screen.getByTestId('lw-ask'));
    expect(invoke).toHaveBeenCalledWith('ask_ai_about_local_changes', {
      args: { id: 9, intent: 'review', question: null, paths: ['src/a.rs'] },
    });
    await waitFor(() => expect(onclose).toHaveBeenCalled());
  });

  it('takes an optional question with any intent', async () => {
    route();
    render(LocalChangesDialog, { link, onclose: () => {} });
    await screen.findAllByTestId('lw-change');
    await fireEvent.change(screen.getByTestId('lw-ask-intent'), { target: { value: 'review' } });
    const q = screen.getByTestId('lw-ask-question');
    expect(q).toHaveAttribute('placeholder', 'Anything to add? (optional)');
    await fireEvent.input(q, { target: { value: ' check the error paths ' } });
    await fireEvent.click(screen.getByTestId('lw-ask'));
    expect(invoke).toHaveBeenCalledWith('ask_ai_about_local_changes', {
      args: { id: 9, intent: 'review', question: 'check the error paths', paths: ['src/a.rs', 'src/b.rs'] },
    });
  });

  it('commits with a message and discards only after confirming', async () => {
    route({ commit_local_workspace: { commit: 'abc1234567' } });
    render(LocalChangesDialog, { link, onclose: () => {} });
    await screen.findAllByTestId('lw-change');
    await fireEvent.input(screen.getByTestId('lw-commit-message'), { target: { value: 'Fix a' } });
    await fireEvent.click(screen.getByTestId('lw-commit'));
    expect(invoke).toHaveBeenCalledWith('commit_local_workspace', {
      args: { id: 9, message: 'Fix a', paths: ['src/a.rs', 'src/b.rs'] },
    });
    await waitFor(() => expect(screen.getByTestId('lw-discard')).not.toBeDisabled());
    await fireEvent.click(screen.getByTestId('lw-discard'));
    expect(invoke).not.toHaveBeenCalledWith('discard_local_workspace_changes', expect.anything());
    await fireEvent.click(screen.getByTestId('lw-discard-confirm'));
    expect(invoke).toHaveBeenCalledWith('discard_local_workspace_changes', {
      args: { id: 9, paths: ['src/a.rs', 'src/b.rs'] },
    });
  });
});

describe('LocalChangesDialog accessibility (7.2)', () => {
  it('passes the axe and audit checks', async () => {
    route();
    const { container } = render(LocalChangesDialog, { link, onclose: () => {} });
    await screen.findAllByTestId('lw-change');
    await expectAccessible(container);
  });
});
