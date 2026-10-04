import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
const push = vi.fn();
vi.mock('./toasts', () => ({ push: (...a: unknown[]) => push(...a) }));

import {
  downloads,
  loadDownloads,
  sendFile,
  unseen,
  fmtSize,
  _resetDownloadsForTests,
  type Download,
} from './downloads';

const row = (id: number, state: string, downloaded_at?: number): Download => ({
  id,
  at: 1,
  host_alias: 'gpu-1',
  path: `/w/f${id}`,
  name: `f${id}.pdf`,
  size: 10,
  state,
  source: 'agent',
  ...(downloaded_at ? { downloaded_at } : {}),
});

const list = (rows: Download[]) => ({
  downloads: rows,
  total_bytes: 10,
  max_total_bytes: 100,
  max_file_bytes: 50,
});

describe('downloads', () => {
  beforeEach(() => {
    invoke.mockReset();
    push.mockReset();
    _resetDownloadsForTests();
  });

  it('counts ready files nobody saved', () => {
    expect(unseen([row(1, 'ready'), row(2, 'ready', 5), row(3, 'fetching'), row(4, 'failed')])).toBe(1);
  });

  it('announces a file once, when it turns ready after the first read', async () => {
    invoke.mockResolvedValueOnce(list([row(1, 'ready'), row(2, 'fetching')]));
    await loadDownloads();
    expect(push).not.toHaveBeenCalled();
    invoke.mockResolvedValueOnce(list([row(1, 'ready'), row(2, 'ready')]));
    await loadDownloads();
    expect(push).toHaveBeenCalledTimes(1);
    expect(push.mock.calls[0][0].message).toContain('f2.pdf');
    invoke.mockResolvedValueOnce(list([row(1, 'ready'), row(2, 'ready')]));
    await loadDownloads();
    expect(push).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('list_downloads', { args: {} });
  });

  it('sends a file with its session and path', async () => {
    invoke.mockResolvedValueOnce(row(9, 'fetching'));
    await sendFile(4, 'out/a.pdf');
    expect(invoke).toHaveBeenCalledWith('send_file', { args: { session_id: 4, path: 'out/a.pdf' } });
    expect(get(downloads)[0].id).toBe(9);
  });

  it('formats sizes', () => {
    expect(fmtSize(12)).toBe('12 B');
    expect(fmtSize(2048)).toBe('2 KB');
    expect(fmtSize(5 * 1024 * 1024)).toBe('5.0 MB');
  });
});

describe('downloads, an unexpected answer', () => {
  it('reads a null reply as an empty list', async () => {
    _resetDownloadsForTests();
    invoke.mockReset();
    invoke.mockResolvedValueOnce(null);
    const r = await loadDownloads();
    expect(r.ok).toBe(true);
    expect(get(downloads)).toEqual([]);
  });
});
