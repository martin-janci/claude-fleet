// Gap plan G2.7, the FormsSession board's Commit form: "On <branch> · N ahead
// of origin", Push after commit and Amend last, wired to the existing
// `repo_commit_create { amend }` and `repo_push`.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';
import type { BranchDiff, ChangedFile } from './files';
import { clearToasts, toasts } from './toasts';
import { get } from 'svelte/store';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;
const staged: ChangedFile = { path: 'src/a.ts', status: 'modified', staged: true, orig_path: null };
const commit = (hash: string, subject: string) => ({ hash, shortHash: hash.slice(0, 7), parents: [], author: 'a', date: '2026-10-10', subject, refs: [] });

let diff: BranchDiff;
let pushFails = false;

function branchDiff(over: Partial<BranchDiff> = {}): BranchDiff {
  return {
    branch: 'fix/hub-e2e-windows',
    upstream: 'origin/fix/hub-e2e-windows',
    unpushed: [commit('4e1a9c2', 'one'), commit('3d0b8a1', 'two')],
    unpushedFiles: [],
    truncated: false,
    base: 'origin/main',
    aheadOfBase: 2,
    baseFiles: [],
    ...over,
  };
}

beforeEach(() => {
  pushFails = false;
  diff = branchDiff();
  clearToasts();
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'repo_changes':
        return [staged];
      case 'repo_branch_diff':
        return diff;
      case 'repo_push':
        if (pushFails) throw { code: 'E_GIT', message: 'rejected (non-fast-forward)' };
        return null;
      default:
        return null;
    }
  });
});

const calls = () => invoke.mock.calls.map((c) => c[0] as string).filter((c) => c === 'repo_commit_create' || c === 'repo_push');
const argsOf = (cmd: string) => invoke.mock.calls.filter((c) => c[0] === cmd).map((c) => (c[1] as { args: unknown }).args);

async function mount() {
  render(FilesPanel, { props: { session: { id: 7 } as SessionRow } });
  await screen.findByTestId('commit-head');
}
async function write(msg: string) {
  await fireEvent.input(screen.getByPlaceholderText('Commit message…'), { target: { value: msg } });
}

describe('FilesPanel commit form (G2.7)', () => {
  it('says where the commit goes', async () => {
    await mount();
    expect(screen.getByTestId('commit-head').textContent).toBe('On fix/hub-e2e-windows · 2 ahead of origin');
    expect(screen.getByTestId('commit-submit').textContent).toBe('Commit 1 file');
  });

  it('commits, then pushes when Push after commit is ticked', async () => {
    await mount();
    await write('fix(agent): flush first');
    await fireEvent.click(screen.getByTestId('commit-push-after'));
    await fireEvent.click(screen.getByTestId('commit-submit'));
    await waitFor(() => expect(calls()).toEqual(['repo_commit_create', 'repo_push']));
    expect(argsOf('repo_commit_create')).toEqual([{ session_id: 7, message: 'fix(agent): flush first', amend: false }]);
    expect(argsOf('repo_push')).toEqual([{ session_id: 7, set_upstream: false }]);
  });

  it('Amend last amends the last commit and pushes an unpushed one', async () => {
    await mount();
    await fireEvent.click(screen.getByTestId('commit-amend'));
    expect(screen.getByTestId('commit-submit').textContent).toBe('Amend last with 1 file');
    await write('fix(agent): flush before close');
    await fireEvent.click(screen.getByTestId('commit-push-after'));
    await fireEvent.click(screen.getByTestId('commit-submit'));
    await waitFor(() => expect(calls()).toEqual(['repo_commit_create', 'repo_push']));
    expect(argsOf('repo_commit_create')).toEqual([{ session_id: 7, message: 'fix(agent): flush before close', amend: true }]);
    // The box resets for the next commit.
    expect((screen.getByTestId('commit-amend') as HTMLInputElement).checked).toBe(false);
  });

  it('never pushes an amended commit the remote already has (that needs a force push)', async () => {
    diff = branchDiff({ unpushed: [] });
    await mount();
    expect(screen.getByTestId('commit-head').textContent).toBe('On fix/hub-e2e-windows · 0 ahead of origin');
    await fireEvent.click(screen.getByTestId('commit-push-after'));
    await fireEvent.click(screen.getByTestId('commit-amend'));
    const push = screen.getByTestId('commit-push-after') as HTMLInputElement;
    expect(push.disabled).toBe(true);
    expect(screen.getByTestId('commit-push-why').textContent).toContain('needs a force push');
    await write('reword');
    await fireEvent.click(screen.getByTestId('commit-submit'));
    await waitFor(() => expect(calls()).toEqual(['repo_commit_create']));
    expect(argsOf('repo_commit_create')[0]).toMatchObject({ amend: true });
  });

  it('a failed push says the commit is made', async () => {
    pushFails = true;
    await mount();
    await write('fix');
    await fireEvent.click(screen.getByTestId('commit-push-after'));
    await fireEvent.click(screen.getByTestId('commit-submit'));
    await waitFor(() => expect(get(toasts).at(-1)?.message).toContain('Committed; the push failed'));
  });

  it('a branch not on the remote yet is pushed with its upstream set', async () => {
    diff = branchDiff({ upstream: null });
    await mount();
    expect(screen.getByTestId('commit-head').textContent).toBe('On fix/hub-e2e-windows · not on the remote yet');
    await write('first');
    await fireEvent.click(screen.getByTestId('commit-push-after'));
    await fireEvent.click(screen.getByTestId('commit-submit'));
    await waitFor(() => expect(argsOf('repo_push')).toEqual([{ session_id: 7, set_upstream: true }]));
  });
});
