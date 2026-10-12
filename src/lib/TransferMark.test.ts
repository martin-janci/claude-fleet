// Step 10.10: transfers other than Downloads pick their loader through
// `transferLoader`: an update in flight (Settings › Updates), a sync to the
// hosts (the Assets footer's job chip) and an import. A known size always
// picks the Progress ring; an unknown one Data rain.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import TransferMark from './TransferMark.svelte';
import JobChip from './JobChip.svelte';
import ImportDialog from './ImportDialog.svelte';
import { rowTransfer } from './transfer_loader';
import { hosts } from './hosts';

const invoke = mockedInvoke as ReturnType<typeof vi.fn>;

beforeEach(() => {
  invoke.mockReset();
  hosts.set([]);
});

describe('TransferMark', () => {
  it('a known size always picks the Progress ring, with how far it is', () => {
    for (const f of [0, 0.25, 0.5, 1]) {
      const { unmount } = render(TransferMark, { props: { fraction: f, label: 'Syncing' } });
      const mark = screen.getByTestId('transfer-mark');
      expect(mark.dataset.loader).toBe('progress-ring');
      expect(mark.getAttribute('aria-valuenow')).toBe(String(Math.round(f * 100)));
      unmount();
    }
  });

  it('an unknown size picks Data rain', () => {
    render(TransferMark, { props: { fraction: null, label: 'Importing' } });
    expect(screen.getByTestId('transfer-mark').dataset.loader).toBe('data-rain');
  });
});

describe('a sync to the hosts (JobChip)', () => {
  it('shows the Progress ring once it counts and Data rain before', async () => {
    const { rerender } = render(JobChip, { props: { label: 'Syncing', transfer: true, done: null, total: null } });
    expect(screen.getByTestId('assets-job-mark').dataset.loader).toBe('data-rain');
    await rerender({ label: 'Syncing', transfer: true, done: 3, total: 4 });
    const mark = screen.getByTestId('assets-job-mark');
    expect(mark.dataset.loader).toBe('progress-ring');
    expect(mark.getAttribute('aria-valuenow')).toBe('75');
  });

  it('a scan keeps the Orbit', async () => {
    render(JobChip, { props: { label: 'Scanning' } });
    await waitFor(() => expect(screen.getByTestId('assets-job-mark').dataset.loader).toBe('orbit'));
  });
});

describe('an update in flight (a page row)', () => {
  it('reads its transfer beside the named cell', () => {
    const row = { update: 'Installing', transfer: { column: 'update', percent: 50 } };
    expect(rowTransfer(row, 'update')).toEqual({ fraction: 0.5 });
    expect(rowTransfer(row, 'version')).toBeUndefined();
    expect(rowTransfer({ transfer: { column: 'update', percent: null } }, 'update')).toEqual({ fraction: null });
    expect(rowTransfer({ update: 'Up to date' }, 'update')).toBeUndefined();
  });
});

describe('an import', () => {
  it('shows Data rain while it reads the host, then goes', async () => {
    let answer: (v: unknown) => void = () => {};
    invoke.mockImplementationOnce(() => new Promise((r) => (answer = r)));
    render(ImportDialog, { props: { onclose: () => {}, ondone: () => {} } });
    await fireEvent.click(screen.getByTestId('import-dry-run'));
    expect(screen.getByTestId('import-transfer').dataset.loader).toBe('data-rain');
    answer({ created: [], problems: [], warnings: [], flagged_secrets: [], dry_run: true });
    await waitFor(() => expect(screen.queryByTestId('import-running')).toBeNull());
  });
});
