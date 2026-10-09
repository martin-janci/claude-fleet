import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';
import type { BranchDiff, ChangedFile } from './files';
import type { Commit } from './history';
import { expectAccessible } from './a11y_check';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const f = (path: string, status = 'modified'): ChangedFile => ({ path, status, staged: false, orig_path: null });
const c = (hash: string, subject: string): Commit => ({
  hash,
  shortHash: hash.slice(0, 7),
  parents: [],
  refs: [],
  author: 'a',
  date: '2026-10-08T12:00:00Z',
  subject,
});

// A branch two commits ahead of origin and of main (the plan's test case).
const TWO_AHEAD: BranchDiff = {
  branch: 'fix-flake',
  upstream: 'origin/fix-flake',
  unpushed: [c('a41c9e2aaaa', 'fix'), c('7d02b11bbbb', 'repro')],
  unpushedFiles: [f('scripts/hub-e2e.sh'), f('scripts/repro-pair.sh', 'added')],
  truncated: false,
  base: 'origin/main',
  aheadOfBase: 2,
  baseFiles: [f('scripts/hub-e2e.sh'), f('scripts/repro-pair.sh', 'added')],
};

let branchDiff: unknown = TWO_AHEAD;
let branchFails = false;

beforeEach(() => {
  branchDiff = TWO_AHEAD;
  branchFails = false;
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, payload: { args: Record<string, unknown> }) => {
    switch (cmd) {
      case 'repo_changes':
        return [f('docs/hub.md')];
      case 'repo_branch_diff':
        if (branchFails) throw { code: 'E_HUB_PROTOCOL', message: 'tool not found: repo_branch_diff' };
        return branchDiff;
      case 'repo_range_diff':
        return { path: payload.args.path, diff: `range ${payload.args.range}`, binary: false, truncated: false };
      case 'repo_diff':
        return { path: payload.args.path, diff: 'worktree', binary: false, truncated: false };
      default:
        return null;
    }
  });
});

function mount() {
  return render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
}

// Step 5.6 (Files board): Changed shows committed work too, not only git
// status — what the branch has not pushed, and the branch against its base.
describe('FilesPanel Changed: committed groups', () => {
  it('shows Uncommitted, then not pushed, with the base group folded', async () => {
    const { container } = mount();
    const unpushed = await screen.findByTestId('group-unpushed');
    expect(screen.getByTestId('group-uncommitted').textContent).toContain('Uncommitted');
    expect(unpushed.textContent).toContain('In this branch, not pushed');
    const base = screen.getByTestId('group-base');
    expect(base.textContent).toContain('Against origin/main · 2 ahead');
    expect(base.querySelector('button')?.getAttribute('aria-expanded')).toBe('false');
    expect(screen.getAllByTestId('range-row').map((r) => r.querySelector('.name')?.textContent)).toEqual([
      'scripts/hub-e2e.sh',
      'scripts/repro-pair.sh',
    ]);
    await fireEvent.click(base.querySelector('button')!);
    expect(screen.getAllByTestId('range-row')).toHaveLength(4);
    await expectAccessible(container);
  });

  it('a not-pushed row opens that range diff, not the worktree diff', async () => {
    mount();
    await screen.findByTestId('group-unpushed');
    await fireEvent.click(screen.getAllByTestId('range-row')[0]);
    await waitFor(() => {
      const call = invoke.mock.calls.find((x) => x[0] === 'repo_range_diff');
      expect(call?.[1]).toEqual({ args: { session_id: 1, path: 'scripts/hub-e2e.sh', range: 'unpushed' } });
    });
  });

  it('Push says how many commits and where, and pushes without -u when tracked', async () => {
    mount();
    const btn = await screen.findByTestId('branch-push');
    expect(btn.textContent).toBe('Push 2 ↑');
    expect(screen.getByTestId('branch-push-bar').textContent).toContain('origin/fix-flake');
    await fireEvent.click(btn);
    await waitFor(() => {
      const call = invoke.mock.calls.find((x) => x[0] === 'repo_push');
      expect((call?.[1] as { args: { set_upstream: boolean } }).args.set_upstream).toBe(false);
    });
  });

  it('a branch no remote has yet is pushed with its upstream set', async () => {
    branchDiff = { ...TWO_AHEAD, upstream: null };
    mount();
    const btn = await screen.findByTestId('branch-push');
    expect(screen.getByTestId('branch-push-bar').textContent).toContain('not on the remote yet');
    await fireEvent.click(btn);
    await waitFor(() => {
      const call = invoke.mock.calls.find((x) => x[0] === 'repo_push');
      expect((call?.[1] as { args: { set_upstream: boolean } }).args.set_upstream).toBe(true);
    });
  });

  it('nothing to push disables Push and shows no committed groups', async () => {
    branchDiff = { ...TWO_AHEAD, unpushed: [], unpushedFiles: [], aheadOfBase: 0, baseFiles: [] };
    mount();
    expect(((await screen.findByTestId('branch-push')) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByTestId('group-unpushed')).toBeNull();
    expect(screen.queryByTestId('group-uncommitted')).toBeNull();
  });

  it('an older hub without the tool keeps the flat list', async () => {
    branchFails = true;
    mount();
    await screen.findByText('docs/hub.md');
    await waitFor(() => expect(invoke.mock.calls.some((x) => x[0] === 'repo_branch_diff')).toBe(true));
    expect(screen.queryByTestId('group-unpushed')).toBeNull();
    expect(screen.queryByTestId('branch-push-bar')).toBeNull();
  });

});
