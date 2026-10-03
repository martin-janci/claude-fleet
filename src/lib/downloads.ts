// File downloads (docs/superpowers/specs/2026-10-03-file-downloads-design.md):
// a file a session sent from its host, kept on the machine that owns the
// fleet (the hub when paired). The list is re-read on `download:changed`
// (ids only) and on a hub gap; nothing is patched in place.

import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { push } from './toasts';

export type DownloadState = 'fetching' | 'ready' | 'failed';

export interface Download {
  id: number;
  at: number;
  host_alias: string;
  session_id?: number;
  session_name?: string;
  path: string;
  name: string;
  size: number;
  /** A newer hub may add states; anything unknown is neither ready nor failed. */
  state: DownloadState | string;
  error?: string;
  sha256?: string;
  source: 'agent' | 'person' | string;
  note?: string;
  ready_at?: number;
  downloaded_at?: number;
  expires_at?: number;
}

export interface DownloadList {
  downloads: Download[];
  total_bytes: number;
  max_total_bytes: number;
  max_file_bytes: number;
}

export const downloads = writable<Download[]>([]);
export const downloadBudget = writable<{ total: number; max: number } | null>(null);

/** Ready files nobody has saved yet: the footer's count. */
export function unseen(rows: Download[]): number {
  return rows.filter((d) => d.state === 'ready' && d.downloaded_at == null).length;
}

/** Ids this window has already announced as ready, so a re-read toasts once. */
const announced = new Set<number>();
let primed = false;

export async function loadDownloads(): Promise<Result<DownloadList>> {
  const r = await invokeCmd<DownloadList>('list_downloads', { args: {} });
  if (!r.ok) return r;
  const rows = r.value.downloads;
  if (primed) {
    for (const d of rows) {
      if (d.state === 'ready' && d.downloaded_at == null && !announced.has(d.id)) {
        push({
          kind: 'success',
          message: `${d.name} is ready to download (${d.host_alias})`,
          action: { label: 'Save', run: () => void saveDownload(d.id) },
        });
      }
    }
  }
  for (const d of rows) if (d.state === 'ready') announced.add(d.id);
  primed = true;
  downloads.set(rows);
  downloadBudget.set({ total: r.value.total_bytes, max: r.value.max_total_bytes });
  return r;
}

/** Coalesce a burst of `download:changed` into one re-read. */
let pending: ReturnType<typeof setTimeout> | null = null;
export function noteDownloadsChanged(): void {
  if (pending !== null) return;
  pending = setTimeout(() => {
    pending = null;
    void loadDownloads();
  }, 150);
}

/** "Send to downloads" from the file viewer. */
export async function sendFile(sessionId: number, path: string, note?: string): Promise<void> {
  const r = await invokeCmd<Download>('send_file', {
    args: { session_id: sessionId, path, ...(note ? { note } : {}) },
  });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Send to downloads: ${r.error.message}` });
    return;
  }
  push({ kind: 'info', message: `Copying ${r.value.name}… it appears in Downloads when ready.` });
  downloads.update((rows) => [r.value, ...rows.filter((d) => d.id !== r.value.id)]);
}

/** Save a ready file through this machine's save dialog. */
export async function saveDownload(id: number): Promise<string | null> {
  const r = await invokeCmd<string | null>('save_download', { id });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Save: ${r.error.message}` });
    return null;
  }
  if (r.value) {
    push({ kind: 'success', message: `Saved to ${r.value}` });
    downloads.update((rows) =>
      rows.map((d) => (d.id === id ? { ...d, downloaded_at: Math.floor(Date.now() / 1000) } : d)),
    );
  }
  return r.value;
}

export async function removeDownload(id: number): Promise<void> {
  const r = await invokeCmd<boolean>('remove_download', { id });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Remove: ${r.error.message}` });
    return;
  }
  downloads.set(get(downloads).filter((d) => d.id !== id));
}

/** `1.2 MB`, `340 KB`, `12 B`. */
export function fmtSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/** For tests: forget what was announced. */
export function _resetDownloadsForTests(): void {
  announced.clear();
  primed = false;
  downloads.set([]);
  downloadBudget.set(null);
}
