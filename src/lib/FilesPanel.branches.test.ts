import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';
import type { Branch } from './history';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const b = (name: string, over: Partial<Branch> = {}): Branch => ({
  name,
  isCurrent: false,
  isRemote: false,
  upstream: null,
  ahead: 0,
  behind: 0,
  tipHash: 'abc123',
  merged: false,
  ...over,
});

const BRANCHES: Branch[] = [
  b('fix-flake', { isCurrent: true }),
  b('main'),
  b('claude/worker-guard', { merged: true }),
  b('claude/old', { merged: true }),
  b('origin/main', { isRemote: true }),
  b('origin/claude/worker-guard', { isRemote: true, merged: true }),
];

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'repo_changes':
        return [];
      case 'repo_branches':
        return BRANCHES;
      case 'repo_delete_merged_branches':
        return { deleted: ['claude/worker-guard'], kept: ['claude/old'] };
      default:
        return null;
    }
  });
});

async function openBranches() {
  render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
  await fireEvent.click(await screen.findByText('Branches'));
  await screen.findByText('claude/worker-guard');
}

// Step 5.7 (Files board, FilesHistory): Branches flags merged branches,
// filters to them, and deletes the merged local ones in one confirmed step.
describe('FilesPanel merged branches', () => {
  it('flags merged branches and counts the merged remotes', async () => {
    await openBranches();
    const row = screen.getByText('claude/worker-guard').closest('.brow') as HTMLElement;
    expect(within(row).getByTestId('branch-merged')).toBeTruthy();
    const main = screen.getByText('main').closest('.brow') as HTMLElement;
    expect(within(main).queryByTestId('branch-merged')).toBeNull();
    expect(screen.getByText(/1 merged and safe to delete/)).toBeTruthy();
  });

  it('the Merged filter shows only merged branches, and every row stays reachable', async () => {
    await openBranches();
    expect(screen.getAllByTestId('branch-row')).toHaveLength(6);
    await fireEvent.click(screen.getByTestId('filter-merged'));
    const names = screen.getAllByTestId('branch-row').map((r) => r.querySelector('.bname')?.textContent);
    expect(names).toEqual(['claude/worker-guard', 'claude/old', 'origin/claude/worker-guard']);
    await fireEvent.click(screen.getByTestId('filter-merged'));
    expect(screen.getAllByTestId('branch-row')).toHaveLength(6);
  });

  it('Delete merged confirms first, sends only the merged locals, and says what it kept', async () => {
    await openBranches();
    await fireEvent.click(screen.getByTestId('delete-merged'));
    expect(await screen.findByTestId('confirm-delete-merged')).toBeTruthy();
    expect(invoke.mock.calls.some((c) => c[0] === 'repo_delete_merged_branches')).toBe(false);
    await fireEvent.click(screen.getByTestId('confirm-delete-merged'));
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === 'repo_delete_merged_branches');
      expect((call?.[1] as { args: { names: string[] } }).args.names).toEqual([
        'claude/worker-guard',
        'claude/old',
      ]);
    });
    expect((await screen.findByTestId('delete-merged-notice')).textContent).toContain('kept claude/old');
  });

  it('deletes the merged remote branches on their remote after a confirm (G7.10)', async () => {
    await openBranches();
    await fireEvent.click(screen.getByTestId('delete-merged-remote'));
    const confirm = await screen.findByTestId('confirm-delete-merged');
    expect(screen.getByRole('dialog').textContent).toContain('origin/claude/worker-guard');
    expect(invoke.mock.calls.some((c) => c[0] === 'repo_delete_merged_branches')).toBe(false);
    await fireEvent.click(confirm);
    await waitFor(() => {
      const call = invoke.mock.calls.find((c) => c[0] === 'repo_delete_merged_branches');
      expect((call?.[1] as { args: unknown }).args).toEqual({
        session_id: 1,
        names: [],
        remotes: ['origin/claude/worker-guard'],
      });
    });
  });

  it('no Delete merged button when no local branch is merged', async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === 'repo_branches' ? [b('main', { isCurrent: true }), b('wip')] : cmd === 'repo_changes' ? [] : null,
    );
    render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
    await fireEvent.click(await screen.findByText('Branches'));
    await screen.findByText('wip');
    expect(screen.queryByTestId('delete-merged')).toBeNull();
    expect(screen.queryByTestId('delete-merged-remote')).toBeNull();
  });
});
