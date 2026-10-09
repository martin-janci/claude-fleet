import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
const push = vi.fn((_o: { message: string; kind?: string; action?: { label: string }; [k: string]: unknown }) => 0);
const dismiss = vi.fn();
const setToastProgress = vi.fn((..._a: unknown[]) => true);
vi.mock('./toasts', () => ({
  push: (o: { message: string }) => push(o),
  dismiss: (...a: unknown[]) => dismiss(...a),
  setToastProgress: (...a: unknown[]) => setToastProgress(...a),
}));
const reveal = vi.fn((_p: string) => Promise.resolve());
vi.mock('@tauri-apps/plugin-opener', () => ({ revealItemInDir: (p: string) => reveal(p) }));

import {
  downloads,
  finished,
  loadDownloads,
  retryDownload,
  revealLabel,
  revealSaved,
  saveDownload,
  savedTo,
  sendFile,
  transferFraction,
  transferText,
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
    dismiss.mockReset();
    setToastProgress.mockReset();
    setToastProgress.mockReturnValue(true);
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

  // Step 10.10: a copy of known size keeps its toast up with a Progress
  // ring that each re-read moves, until the copy is done.
  it('a copy of known size carries a moving ring until it is done', async () => {
    push.mockReturnValue(42);
    invoke.mockResolvedValueOnce({ ...row(9, 'fetching'), fetched_bytes: 0 });
    await sendFile(4, 'out/a.pdf');
    expect(push).toHaveBeenCalledWith(expect.objectContaining({ sticky: true, progress: 0 }));
    invoke.mockResolvedValueOnce(list([{ ...row(9, 'fetching'), fetched_bytes: 5 }]));
    await loadDownloads();
    expect(setToastProgress).toHaveBeenCalledWith(42, 0.5);
    invoke.mockResolvedValueOnce(list([row(9, 'ready')]));
    await loadDownloads();
    expect(dismiss).toHaveBeenCalledWith(42);
  });

  it('a copy of unknown size says so once, with no ring', async () => {
    invoke.mockResolvedValueOnce(row(9, 'fetching'));
    await sendFile(4, 'out/a.pdf');
    expect(push.mock.calls[0][0]).not.toHaveProperty('progress');
    expect(push.mock.calls[0][0]).not.toHaveProperty('sticky');
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

describe('downloads, progress, retry and reveal (10.7)', () => {
  beforeEach(() => {
    invoke.mockReset();
    push.mockReset();
    reveal.mockClear();
    _resetDownloadsForTests();
  });

  const MB = 1024 * 1024;
  const copying = (fetched?: number): Download => ({ ...row(5, 'fetching'), size: 88 * MB, ...(fetched === undefined ? {} : { fetched_bytes: fetched }) });

  it('shows bytes so far, then the time left once it has a rate', () => {
    expect(transferText(copying(), 0)).toBe('copying…');
    expect(transferFraction(copying())).toBeNull();
    expect(transferText(copying(40 * MB), 1_000)).toBe('40.0 MB of 88.0 MB');
    expect(transferText(copying(62 * MB), 12_000)).toBe('62.0 MB of 88.0 MB · 13 s left');
    expect(transferFraction(copying(44 * MB))).toBe(0.5);
  });

  it('retries a failed copy by sending the same file again, then drops the failed row', async () => {
    const failed: Download = { ...row(3, 'failed'), session_id: 7, note: 'the report', error: 'host went offline' };
    downloads.set([failed]);
    invoke.mockResolvedValueOnce(row(11, 'fetching')).mockResolvedValueOnce(true);
    expect(await retryDownload(failed)).toBe(true);
    expect(invoke).toHaveBeenNthCalledWith(1, 'send_file', { args: { session_id: 7, path: '/w/f3', note: 'the report' } });
    expect(invoke).toHaveBeenNthCalledWith(2, 'remove_download', { id: 3 });
    expect(get(downloads).map((d) => d.id)).toEqual([11]);
  });

  it('keeps a failed row whose retry is refused, and says why', async () => {
    const failed: Download = { ...row(3, 'failed'), session_id: 7 };
    downloads.set([failed]);
    invoke.mockRejectedValueOnce({ code: 'E_NOTFOUND', message: 'session 7 not found' });
    expect(await retryDownload(failed)).toBe(false);
    expect(get(downloads).map((d) => d.id)).toEqual([3]);
    expect(push.mock.calls[0][0]).toMatchObject({ kind: 'error', code: 'E_NOTFOUND' });
  });

  it('cannot retry a file with no session, and sends nothing', async () => {
    expect(await retryDownload(row(3, 'failed'))).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    expect(push.mock.calls[0][0].kind).toBe('error');
  });

  it('remembers where a file was saved and shows it in the file manager', async () => {
    invoke.mockResolvedValueOnce('/Users/m/Downloads/f1.pdf');
    await saveDownload(1);
    expect(get(savedTo).get(1)).toBe('/Users/m/Downloads/f1.pdf');
    expect(push.mock.calls[0][0].action?.label).toMatch(/^Show in /);
    await revealSaved(1);
    expect(reveal).toHaveBeenCalledWith('/Users/m/Downloads/f1.pdf');
  });

  it('names the file manager per platform', () => {
    expect(revealLabel({ platform: 'MacIntel' })).toBe('Show in Finder');
    expect(revealLabel({ platform: 'Win32' })).toBe('Show in Explorer');
    expect(revealLabel({ platform: 'Linux x86_64' })).toBe('Show in folder');
  });

  it('counts saved files and failed copies as finished, never an unsaved one', () => {
    expect(finished([row(1, 'ready'), row(2, 'ready', 5), row(3, 'failed'), row(4, 'fetching')]).map((d) => d.id)).toEqual([2, 3]);
  });
});
