import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import FilesPanel from './FilesPanel.svelte';
import type { SessionRow } from './sessions';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

const branch = {
  name: 'feature/x',
  isCurrent: false,
  isRemote: false,
  upstream: null,
  ahead: 0,
  behind: 0,
  tipHash: 'abc123',
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'repo_changes':
        return [];
      case 'repo_branches':
        return [branch, { ...branch, name: 'main', isCurrent: true }];
      case 'repo_checkout':
        return null;
      default:
        return null;
    }
  });
});

// Switching the branch under a running agent has the same consequence as
// checking out a commit, which has always confirmed. Before this, the
// branch list did it on one click — despite the handler being called
// `confirmCheckout`.
describe('FilesPanel branch checkout', () => {
  it('asks before switching the branch, and only then calls repo_checkout', async () => {
    render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
    await fireEvent.click(await screen.findByText('Branches'));
    const row = (await screen.findByText('feature/x')).closest('.brow') as HTMLElement;
    await fireEvent.click(within(row).getByText('Checkout'));
    expect(await screen.findByTestId('confirm-checkout-branch')).toBeTruthy();
    expect(invoke.mock.calls.some((c) => c[0] === 'repo_checkout')).toBe(false);
    await fireEvent.click(screen.getByTestId('confirm-checkout-branch'));
    await waitFor(() =>
      expect(
        invoke.mock.calls.some(
          (c) => c[0] === 'repo_checkout' && (c[1] as { args: { branch: string } }).args.branch === 'feature/x',
        ),
      ).toBe(true),
    );
  });

  it('cancelling leaves the branch alone', async () => {
    render(FilesPanel, { props: { session: { id: 1 } as SessionRow } });
    await fireEvent.click(await screen.findByText('Branches'));
    const row = (await screen.findByText('feature/x')).closest('.brow') as HTMLElement;
    await fireEvent.click(within(row).getByText('Checkout'));
    await screen.findByTestId('confirm-checkout-branch');
    await fireEvent.click(screen.getByText('Cancel'));
    await waitFor(() => expect(screen.queryByTestId('confirm-checkout-branch')).toBeNull());
    expect(invoke.mock.calls.some((c) => c[0] === 'repo_checkout')).toBe(false);
  });
});
