// The Downloads tab with a failed transfer (plan step 10.7's test), a copy
// in flight and a saved file; and the Notifications tab.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock('@tauri-apps/plugin-opener', () => ({ revealItemInDir: vi.fn(() => Promise.resolve()) }));

import DownloadsSheet from './DownloadsSheet.svelte';
import { _resetDownloadsForTests, savedTo, type Download } from './downloads';
import { clearNotices } from './notifications';
import { clearToasts, push, toasts } from './toasts';
import { get } from 'svelte/store';
import { expectAccessible } from './a11y_check';

const MB = 1024 * 1024;
const row = (id: number, state: string, extra: Partial<Download> = {}): Download => ({
  id,
  at: Math.floor(Date.now() / 1000),
  host_alias: 'claude-fleet-trn',
  session_id: 4,
  path: `/w/f${id}`,
  name: `f${id}.tar.gz`,
  size: 88 * MB,
  state,
  source: 'agent',
  ...extra,
});
const list = (rows: Download[]) => ({ downloads: rows, total_bytes: 0, max_total_bytes: 100 * MB, max_file_bytes: 100 * MB });

beforeEach(() => {
  invoke.mockReset();
  _resetDownloadsForTests();
  clearToasts();
  clearNotices();
});

async function open(rows: Download[], tab?: 'downloads' | 'notifications') {
  invoke.mockResolvedValueOnce(list(rows));
  render(DownloadsSheet, { onclose: () => {}, ...(tab ? { tab } : {}) });
  await tick();
  await tick();
}

describe('Downloads', () => {
  it('pauses a copy in flight and lets it go on (G7.15)', async () => {
    await open([row(5, 'fetching', { fetched_bytes: 62 * MB })]);
    expect(screen.getByTestId('download-pause').textContent).toBe('Pause');
    invoke.mockResolvedValueOnce(row(5, 'fetching', { fetched_bytes: 62 * MB, paused: true }));
    await fireEvent.click(screen.getByTestId('download-pause'));
    await tick();
    await tick();
    expect(invoke).toHaveBeenCalledWith('pause_download', { args: { id: 5, paused: true } });
    expect(screen.getByTestId('download-progress').textContent).toBe('62.0 MB of 88.0 MB · paused');
    expect(screen.getByTestId('download-pause').textContent).toBe('Resume');
    invoke.mockResolvedValueOnce(row(5, 'fetching', { fetched_bytes: 62 * MB }));
    await fireEvent.click(screen.getByTestId('download-pause'));
    await tick();
    await tick();
    expect(invoke).toHaveBeenLastCalledWith('pause_download', { args: { id: 5, paused: false } });
    expect(screen.getByTestId('download-pause').textContent).toBe('Pause');
  });

  it('says Pause needs the hub updated when the hub is older (G7.15)', async () => {
    await open([row(5, 'fetching', { fetched_bytes: 62 * MB })]);
    invoke.mockRejectedValueOnce({ code: 'E_HUB_PROTOCOL', message: 'unknown tool pause_download' });
    await fireEvent.click(screen.getByTestId('download-pause'));
    await tick();
    await tick();
    const t = get(toasts).at(-1);
    expect(t?.message).toBe('Pause: the hub needs updating first; the copy goes on meanwhile.');
    expect(screen.getByTestId('download-pause').textContent).toBe('Pause');
  });

  it('retries a failed transfer: the file is sent again and the failed row goes', async () => {
    await open([row(3, 'failed', { error: 'host went offline' })]);
    expect(screen.getByTestId('download-row').dataset.state).toBe('failed');
    expect(screen.getByText('host went offline')).toBeTruthy();
    invoke.mockResolvedValueOnce(row(9, 'fetching', { fetched_bytes: 0 })).mockResolvedValueOnce(true);
    await fireEvent.click(screen.getByTestId('download-retry'));
    await tick();
    await tick();
    expect(invoke).toHaveBeenCalledWith('send_file', { args: { session_id: 4, path: '/w/f3' } });
    expect(invoke).toHaveBeenCalledWith('remove_download', { id: 3 });
    const rows = screen.getAllByTestId('download-row');
    expect(rows.map((r) => r.dataset.state)).toEqual(['fetching']);
  });

  it('shows a copy in flight with a Progress ring and its bytes', async () => {
    await open([row(5, 'fetching', { fetched_bytes: 44 * MB })]);
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe('50');
    expect(screen.getByTestId('download-ring-pending')).toBeTruthy();
    expect(screen.queryByTestId('download-rain-pending')).toBeNull();
    expect(screen.getByTestId('download-progress').textContent).toBe('44.0 MB of 88.0 MB');
  });

  it('shows Data rain for a copy of unknown size', async () => {
    await open([row(5, 'fetching')]);
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(screen.getByTestId('download-rain-pending')).toBeTruthy();
    expect(screen.getByTestId('download-progress').textContent).toBe('copying…');
  });

  it('offers Show in … for a file saved in this window, and clears finished rows', async () => {
    savedTo.set(new Map([[6, '/Users/m/Downloads/f6.tar.gz']]));
    await open([row(6, 'ready', { downloaded_at: 1 }), row(7, 'ready')]);
    expect(screen.getAllByTestId('download-reveal')).toHaveLength(1);
    invoke.mockResolvedValueOnce(true);
    await fireEvent.click(screen.getByTestId('downloads-clear-finished'));
    await tick();
    expect(invoke).toHaveBeenCalledWith('remove_download', { id: 6 });
    expect(invoke).not.toHaveBeenCalledWith('remove_download', { id: 7 });
  });
});

describe('Notifications', () => {
  it('lists this window’s toasts and marks them read', async () => {
    push({ kind: 'error', code: 'E_SSH', message: "Couldn't move to mercury" });
    push({ kind: 'success', message: 'PR #493 merged' });
    await open([]);
    expect(screen.getByTestId('tab-notifications').textContent).toBe('Notifications (2)');
    await fireEvent.click(screen.getByTestId('tab-notifications'));
    expect(screen.getAllByTestId('notice-row').map((r) => r.dataset.kind)).toEqual(['success', 'error']);
    await fireEvent.click(screen.getByTestId('notices-mark-read'));
    expect(screen.getByTestId('tab-notifications').textContent).toBe('Notifications');
  });

  it('offers a toast’s button only while the toast is up', async () => {
    const run = vi.fn();
    push({ kind: 'info', message: 'Archived 4 sessions', action: { label: 'Undo', run } });
    await open([], 'notifications');
    await fireEvent.click(screen.getByTestId('notice-action'));
    expect(run).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('notice-action')).toBeNull();
  });
});

describe('Downloads: accessibility', () => {
  it('the Downloads sheet is accessible', async () => {
    savedTo.set(new Map([[6, '/Users/m/Downloads/f6.tar.gz']]));
    push({ kind: 'error', code: 'E_SSH', message: "Couldn't move to mercury" });
    await open([
      row(3, 'failed', { error: 'host went offline' }),
      row(5, 'fetching', { fetched_bytes: 44 * MB }),
      row(6, 'ready', { downloaded_at: 1 }),
    ]);
    await expectAccessible(screen.getByTestId('downloads-sheet'));
  });
});

// Review round 13: a failed read of the list has a next step.
describe('Downloads when the list cannot be read (review r13)', () => {
  it('says so in a sentence, and Retry reads the list again', async () => {
    invoke.mockRejectedValueOnce({ code: 'E_HUB_UNREACHABLE', message: 'connection refused' });
    render(DownloadsSheet, { onclose: () => {} });
    const err = await screen.findByTestId('downloads-error');
    expect(err.textContent).toContain("Couldn't reach the hub");
    invoke.mockResolvedValueOnce(list([row(1, 'ready')]));
    await fireEvent.click(screen.getByTestId('downloads-error-retry'));
    await tick();
    await tick();
    expect(screen.queryByTestId('downloads-error')).toBeNull();
    expect(invoke.mock.calls.filter((c) => c[0] === 'list_downloads')).toHaveLength(2);
  });
});
