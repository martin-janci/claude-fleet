// File downloads (docs/superpowers/specs/2026-10-03-file-downloads-design.md):
// a file a session sent from its host, kept on the machine that owns the
// fleet (the hub when paired). The list is re-read on `download:changed`
// (ids only) and on a hub gap; nothing is patched in place.

import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { createListRace } from './row_store';
import { detectMac } from './terminal_keys';
import { dismiss, push, setToastProgress } from './toasts';

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
  /** Bytes copied so far while `fetching`. Absent from an older hub. */
  fetched_bytes?: number;
  /** A person paused the copy (G7.15); absent while it runs. */
  paused?: boolean;
}

export interface DownloadList {
  downloads: Download[];
  total_bytes: number;
  max_total_bytes: number;
  max_file_bytes: number;
}

export const downloads = writable<Download[]>([]);
/** Where this window saved each file (id → path), for Show in Finder. Per
 *  window: the hub records that a file was saved, not where. */
export const savedTo = writable<Map<number, string>>(new Map());
/** Whether the Downloads sheet is open (App mounts it): a copy's toast opens it. */
export const downloadsOpen = writable(false);
export const downloadBudget = writable<{ total: number; max: number } | null>(null);

/** Ready files nobody has saved yet: the footer's count. */
export function unseen(rows: Download[]): number {
  return rows.filter((d) => d.state === 'ready' && d.downloaded_at == null).length;
}

/** Copies this window started whose size is known: download id → the
 *  toast carrying their Progress ring (step 10.10), until they finish. */
const jobToasts = new Map<number, number>();

/** "Copying …" for a copy just asked for: with a known size the toast stays
 *  up and carries a Progress ring the re-reads move; without one it says so
 *  once and goes. */
function announceCopy(row: Download, message: string): void {
  const f = row.state === 'fetching' ? transferFraction(row) : null;
  if (f === null) {
    push({ kind: 'info', message });
    return;
  }
  jobToasts.set(
    row.id,
    push({ kind: 'info', message, sticky: true, progress: f, action: { label: 'Open', run: () => downloadsOpen.set(true) } }),
  );
}

/** Move each copy's ring; a copy that finished, failed or went drops its toast. */
function followJobs(rows: Download[]): void {
  for (const [id, toast] of jobToasts) {
    const d = rows.find((r) => r.id === id);
    const f = d && d.state === 'fetching' ? transferFraction(d) : null;
    if (f === null) {
      dismiss(toast);
      jobToasts.delete(id);
    } else if (!setToastProgress(toast, f)) {
      jobToasts.delete(id);
    }
  }
}

/** Ids this window has already announced as ready, so a re-read toasts once. */
const announced = new Set<number>();
let primed = false;

// A row this window added, changed or removed while a list was in flight
// (a copy just requested, a save, a remove) is newer than that list, and an
// older list never lands over a newer one (review r07).
const race = createListRace<number>();

export async function loadDownloads(): Promise<Result<DownloadList>> {
  const token = race.begin();
  const r = await invokeCmd<DownloadList>('list_downloads', { args: {} });
  if (!r.ok) return r;
  // An answer without the list (a mocked or unexpected reply) reads as empty
  // rather than throwing inside an event handler.
  const value = r.value ?? ({} as Partial<DownloadList>);
  const rows = Array.isArray(value.downloads) ? value.downloads : [];
  const merged = race.mergeList(get(downloads), rows, token, (d) => d.id);
  if (merged === null) return r;
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
  followJobs(rows);
  downloads.set(merged);
  downloadBudget.set({ total: value.total_bytes ?? 0, max: value.max_total_bytes ?? 0 });
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

/** Ask for a copy; the new row, or null after an error toast. */
async function requestCopy(sessionId: number, path: string, note: string | undefined, context: string): Promise<Download | null> {
  const r = await invokeCmd<Download>('send_file', {
    args: { session_id: sessionId, path, ...(note ? { note } : {}) },
  });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `${context}: ${r.error.message}` });
    return null;
  }
  race.touch(r.value.id);
  downloads.update((rows) => [r.value, ...rows.filter((d) => d.id !== r.value.id)]);
  return r.value;
}

/** "Send to downloads" from the file viewer. */
export async function sendFile(sessionId: number, path: string, note?: string): Promise<void> {
  const row = await requestCopy(sessionId, path, note, 'Send to downloads');
  if (row) announceCopy(row, `Copying ${row.name}… it appears in Downloads when ready.`);
}

/** Copy a failed file again: the same session, path and note sent anew
 *  (the session's host is read again, so a file that changed comes whole),
 *  then the failed row goes. Nothing new on the hub: it is `send_file`. */
export async function retryDownload(d: Download): Promise<boolean> {
  if (d.session_id == null) {
    push({ kind: 'error', message: `Retry: ${d.name} has no session to copy it from; send it again from its session.` });
    return false;
  }
  const row = await requestCopy(d.session_id, d.path, d.note, 'Retry');
  if (!row) return false;
  await removeDownload(d.id);
  announceCopy(row, `Copying ${row.name} again…`);
  return true;
}

/** "Show in Finder" (Explorer, the file manager) for a file this window
 *  saved. */
export async function revealSaved(id: number): Promise<void> {
  const path = get(savedTo).get(id);
  if (!path) return;
  try {
    await revealItemInDir(path);
  } catch (e) {
    push({ kind: 'error', message: `Show the file: ${e instanceof Error ? e.message : String(e)}` });
  }
}

/** The button's words on this platform. */
export function revealLabel(nav: { platform?: string; userAgent?: string } | undefined = typeof navigator === 'undefined' ? undefined : navigator): string {
  if (detectMac(nav)) return 'Show in Finder';
  if (/Win/.test(nav?.platform ?? '') || /Windows/.test(nav?.userAgent ?? '')) return 'Show in Explorer';
  return 'Show in folder';
}

/** First sight of each copy in flight (ms, bytes), for the time left. */
const firstSeen = new Map<number, { at: number; bytes: number }>();

/** What a copy in flight shows: `62.0 MB of 88.0 MB · 12 s left`. The time
 *  left needs two readings a second apart; without `fetched_bytes` (an older
 *  hub) only `copying…`. */
export function transferText(d: Download, now = Date.now()): string {
  const got = d.fetched_bytes;
  if (d.paused) {
    // The rate starts again from the next reading once it goes on.
    firstSeen.delete(d.id);
    return got === undefined ? 'paused' : `${fmtSize(got)} of ${fmtSize(d.size)} · paused`;
  }
  if (got === undefined) return 'copying…';
  const first = firstSeen.get(d.id);
  if (!first) firstSeen.set(d.id, { at: now, bytes: got });
  let left = '';
  if (first && now - first.at >= 1000 && got > first.bytes) {
    const rate = (got - first.bytes) / ((now - first.at) / 1000);
    const secs = Math.ceil((d.size - got) / rate);
    left = secs < 60 ? ` · ${secs} s left` : ` · ${Math.ceil(secs / 60)} min left`;
  }
  return `${fmtSize(got)} of ${fmtSize(d.size)}${left}`;
}

/** 0–1 of a copy in flight, or null when the count is unknown. */
export function transferFraction(d: Download): number | null {
  if (d.fetched_bytes === undefined || d.size <= 0) return null;
  return Math.min(1, d.fetched_bytes / d.size);
}

/** Finished rows "Clear finished" removes: saved files and failed copies.
 *  A ready file nobody saved yet stays. */
export function finished(rows: Download[]): Download[] {
  return rows.filter((d) => d.state === 'failed' || (d.state === 'ready' && d.downloaded_at != null));
}

/** Save a ready file through this machine's save dialog. */
export async function saveDownload(id: number): Promise<string | null> {
  const r = await invokeCmd<string | null>('save_download', { id });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Save: ${r.error.message}` });
    return null;
  }
  if (r.value) {
    const path = r.value;
    savedTo.update((m) => new Map(m).set(id, path));
    race.touch(id);
    push({ kind: 'success', message: `Saved to ${path}`, action: { label: revealLabel(), run: () => void revealSaved(id) } });
    downloads.update((rows) =>
      rows.map((d) => (d.id === id ? { ...d, downloaded_at: Math.floor(Date.now() / 1000) } : d)),
    );
  }
  return r.value;
}

/** Pause a copy in flight, or let it go on (G7.15, Toasts board "Pause").
 *  It stops between slices of the copy. */
export async function pauseDownload(id: number, paused: boolean): Promise<void> {
  const r = await invokeCmd<Download>('pause_download', { args: { id, paused } });
  if (!r.ok) {
    // A hub before contract 17 has no pause_download: the copy goes on.
    const message = r.error.code === 'E_HUB_PROTOCOL'
      ? 'the hub needs updating first; the copy goes on meanwhile.'
      : r.error.message;
    push({ kind: 'error', code: r.error.code, message: `${paused ? 'Pause' : 'Resume'}: ${message}` });
    return;
  }
  race.touch(id);
  const row = r.value;
  downloads.update((rows) => rows.map((d) => (d.id === id ? { ...d, ...row, paused: row.paused } : d)));
}

export async function removeDownload(id: number): Promise<void> {
  const r = await invokeCmd<boolean>('remove_download', { id });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Remove: ${r.error.message}` });
    return;
  }
  race.touch(id);
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
  jobToasts.clear();
  downloadsOpen.set(false);
  firstSeen.clear();
  primed = false;
  savedTo.set(new Map());
  downloads.set([]);
  downloadBudget.set(null);
}
