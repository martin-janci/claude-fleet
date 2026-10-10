// Control's Library (Orbit Fleet redesign step 9.7, board MCViews): what is
// on the fleet's hosts that a person may want back. Four sources, one list:
// the files a person put on a host (`library_items`, Upload and prompt
// attachments, through the `library` tool), the downloads a session or a
// person sent (`downloads.ts`; a session's own are its outputs), and the repos
// the sessions work in, per host (`sessions` × `projects`). Only the first is
// new; the other two are read from where they already live.
import { derived, get, writable } from 'svelte/store';
import { invokeCmd, type IpcError, type Result } from './result';
import { downloads, loadDownloads, type Download } from './downloads';
import { sessions, type SessionRow } from './sessions';
import { projectById } from './projects';
import type { PickedFile } from './attachments';

export type LibraryKind = 'upload' | 'attachment';

/** A `library_items` row (`LibraryItemRow`). */
export interface LibraryItem {
  id: number;
  at: number;
  kind: LibraryKind | string;
  host_alias: string;
  session_id?: number;
  session_name?: string;
  path: string;
  name: string;
  size?: number;
}

export interface LibraryList {
  items: LibraryItem[];
}

export interface LibraryFile {
  path: string;
  name?: string;
  size?: number;
}

export const libraryItems = writable<LibraryItem[]>([]);

export async function loadLibrary(): Promise<Result<LibraryList>> {
  const r = await invokeCmd<LibraryList>('list_library', { args: {} });
  if (r.ok) libraryItems.set(r.value.items);
  return r;
}

/** Record files already put beside a session; the new rows join the list. */
export async function addLibraryItems(
  sessionId: number,
  kind: LibraryKind,
  files: LibraryFile[],
): Promise<Result<LibraryList>> {
  const r = await invokeCmd<LibraryList>('add_library_items', {
    args: { kind, session_id: sessionId, files },
  });
  if (r.ok) libraryItems.update((rows) => [...r.value.items.slice().reverse(), ...rows]);
  return r;
}

/** Drop a row from the Library; the file stays on its host. */
export async function removeLibraryItem(id: number): Promise<Result<boolean>> {
  const r = await invokeCmd<boolean>('remove_library_item', { id });
  if (r.ok) libraryItems.update((rows) => rows.filter((x) => x.id !== id));
  return r;
}

/** What one Library row is. */
export type EntryKind = 'output' | 'download' | 'upload' | 'attachment' | 'repo';

export interface LibraryEntry {
  key: string;
  kind: EntryKind;
  name: string;
  host: string;
  /** The session it came out of or was put beside. */
  session?: string;
  /** Seconds since the epoch; a repo's last session start. */
  at: number | null;
  size?: number;
  /** Where it is: the host path, or the repo's `owner/repo`. */
  detail: string;
  /** The row behind it, for its actions. */
  download?: Download;
  item?: LibraryItem;
}

export const ENTRY_LABEL: Record<EntryKind, string> = {
  output: 'Session output',
  download: 'Download',
  upload: 'Upload',
  attachment: 'Attachment',
  repo: 'Repo',
};

export type LibraryFilter = 'all' | 'files' | 'uploads' | 'repos';

export function matchesFilter(e: LibraryEntry, f: LibraryFilter): boolean {
  if (f === 'all') return true;
  if (f === 'repos') return e.kind === 'repo';
  if (f === 'uploads') return e.kind === 'upload' || e.kind === 'attachment';
  return e.kind === 'output' || e.kind === 'download';
}

/** Repos per host: every (host, project) a session works in, newest first. */
export function reposOf(
  rows: readonly SessionRow[],
  byId: ReadonlyMap<number, { project: { owner: string; repo: string; system?: boolean } }>,
): LibraryEntry[] {
  const seen = new Map<string, LibraryEntry>();
  for (const s of rows) {
    if (s.project_id == null || s.status === 'ghost') continue;
    const p = byId.get(s.project_id)?.project;
    if (!p || p.system) continue;
    const key = `repo:${s.host_alias}:${s.project_id}`;
    const at = s.created_at ?? null;
    const prev = seen.get(key);
    if (prev && (prev.at ?? 0) >= (at ?? 0)) continue;
    seen.set(key, {
      key,
      kind: 'repo',
      name: p.repo,
      host: s.host_alias,
      at,
      detail: `${p.owner}/${p.repo}`,
    });
  }
  return [...seen.values()];
}

/** The four sources as one list, newest first; repos last. */
export function libraryEntries(
  items: readonly LibraryItem[],
  dls: readonly Download[],
  repos: readonly LibraryEntry[],
): LibraryEntry[] {
  const files: LibraryEntry[] = [
    ...dls
      .filter((d) => d.state !== 'failed')
      .map(
        (d): LibraryEntry => ({
          key: `dl:${d.id}`,
          kind: d.source === 'agent' ? 'output' : 'download',
          name: d.name,
          host: d.host_alias,
          session: d.session_name,
          at: d.at,
          size: d.size,
          detail: d.path,
          download: d,
        }),
      ),
    ...items.map(
      (i): LibraryEntry => ({
        key: `lib:${i.id}`,
        kind: i.kind === 'attachment' ? 'attachment' : 'upload',
        name: i.name,
        host: i.host_alias,
        session: i.session_name,
        at: i.at,
        size: i.size,
        detail: i.path,
        item: i,
      }),
    ),
  ].sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
  const sortedRepos = [...repos].sort((a, b) => (b.at ?? 0) - (a.at ?? 0));
  return [...files, ...sortedRepos];
}

export const libraryEntryList = derived(
  [libraryItems, downloads, sessions, projectById],
  ([$items, $dls, $sessions, $byId]) => libraryEntries($items, $dls, reposOf($sessions, $byId)),
);

/** Open the Library: its own rows and the downloads, read fresh. Answers
 *  the first failure, so a failed read is never shown as an empty Library. */
export async function refreshLibrary(): Promise<IpcError | null> {
  const [lib, dls] = await Promise.all([loadLibrary(), loadDownloads()]);
  if (!lib.ok) return lib.error;
  if (!dls.ok) return dls.error;
  return null;
}

/** The host paths `upload_attachments` answered, paired with what was picked. */
export function placedFiles(picked: readonly PickedFile[], remote: readonly string[]): LibraryFile[] {
  return remote.map((path, i) => ({ path, name: picked[i]?.name, size: picked[i]?.size }));
}

/**
 * Upload…: pick files on this machine, put them beside `session` (its
 * worktree's attachment folder, the composer's own path) and record them.
 * `null` when the picker was cancelled.
 */
export async function uploadToSession(session: SessionRow): Promise<Result<LibraryItem[]> | null> {
  const pickedR = await invokeCmd<PickedFile[]>('pick_attachments', {});
  if (!pickedR.ok) return pickedR;
  const picked = pickedR.value;
  if (picked.length === 0) return null;
  const placed = await invokeCmd<string[]>('upload_attachments', {
    args: {
      host_alias: session.host_alias,
      session_name: session.tmux_name,
      local_paths: picked.map((p) => p.path),
    },
  });
  if (!placed.ok) return placed;
  const r = await addLibraryItems(session.id, 'upload', placedFiles(picked, placed.value));
  return r.ok ? { ok: true, value: r.value.items } : r;
}

/** A prompt's attachments, once on the host, join the Library. Best effort:
 *  the prompt goes either way, so a refusal here changes nothing. */
export function recordAttachments(host: string, tmux: string, remote: readonly string[]): void {
  if (remote.length === 0) return;
  const s = get(sessions).find((r) => r.host_alias === host && r.tmux_name === tmux);
  if (!s) return;
  void addLibraryItems(
    s.id,
    'attachment',
    remote.map((path) => ({ path })),
  );
}

// ── table (gap plan G3.10, board MCViews) ──

/** The Library table's columns: Name ▴ / Modified / Size. */
export type LibrarySortKey = 'name' | 'modified' | 'size';
export type SortDir = 'asc' | 'desc';
export interface LibrarySort {
  key: LibrarySortKey;
  dir: SortDir;
}

/** What a column header's first press sorts by: names A to Z, the newest
 *  and the largest first. */
export const FIRST_DIR: Record<LibrarySortKey, SortDir> = { name: 'asc', modified: 'desc', size: 'desc' };

/** A header press: the same column flips, another starts at its first
 *  direction. */
export function nextSort(cur: LibrarySort, key: LibrarySortKey): LibrarySort {
  if (cur.key === key) return { key, dir: cur.dir === 'asc' ? 'desc' : 'asc' };
  return { key, dir: FIRST_DIR[key] };
}

/** Entries in the table's order. A row without the column's value (a
 *  repo's size) sorts last either way; ties keep the name order. */
export function sortEntries(rows: readonly LibraryEntry[], s: LibrarySort): LibraryEntry[] {
  const val = (e: LibraryEntry): string | number | null =>
    s.key === 'name' ? e.name.toLowerCase() : s.key === 'size' ? (e.size ?? null) : e.at;
  const sign = s.dir === 'asc' ? 1 : -1;
  return [...rows].sort((a, b) => {
    const x = val(a);
    const y = val(b);
    if (x === null || y === null) {
      if (x !== y) return x === null ? 1 : -1;
    } else if (x !== y) {
      return (x < y ? -1 : 1) * sign;
    }
    return a.name.localeCompare(b.name);
  });
}

/** The Library's layout: a sortable table or a grid of tiles. */
export type LibraryLayout = 'list' | 'grid';
