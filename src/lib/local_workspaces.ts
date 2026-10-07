// Local workspace sync, Phase 1
// (docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md): one
// session's worktree kept in step, both ways, with a folder on this machine.
// A link belongs to the WORKTREE (host + project + worktree key), so every
// session on that worktree shows it. The list is re-read on
// `local_workspace:changed` (ids only); nothing is patched in place.

import { get, writable } from 'svelte/store';
import { invokeCmd, type Result } from './result';
import { push } from './toasts';

export type LocalWorkspaceState =
  | 'synced'
  | 'local_changes'
  | 'remote_changes'
  | 'syncing'
  | 'conflict'
  | 'paused'
  | 'offline'
  | 'error';

export type LocalConflictKind = 'both_modified' | 'both_added' | 'local_deleted' | 'remote_deleted';

export interface LocalConflict {
  path: string;
  kind: LocalConflictKind | string;
  detected_at: number;
  /** `local` / `remote` once a side was picked and the next pass has not
   *  carried it out yet. */
  resolution?: 'local' | 'remote' | null;
}

export interface LocalWorkspace {
  id: number;
  host_alias: string;
  owner: string;
  repo: string;
  project_id?: number | null;
  worktree_key: string;
  remote_path: string;
  local_path: string;
  session_id?: number | null;
  paused: boolean;
  excludes: string[];
  state: LocalWorkspaceState | string;
  last_sync_at?: number | null;
  last_error?: string | null;
  pending_local: number;
  pending_remote: number;
  skipped: number;
  conflicts: LocalConflict[];
  created_at: number;
}

export const localWorkspaces = writable<LocalWorkspace[]>([]);

export async function loadLocalWorkspaces(): Promise<Result<LocalWorkspace[]>> {
  const r = await invokeCmd<LocalWorkspace[]>('list_local_workspaces');
  if (r.ok) localWorkspaces.set(Array.isArray(r.value) ? r.value : []);
  return r;
}

/** Coalesce a burst of `local_workspace:changed` into one re-read. */
let pending: ReturnType<typeof setTimeout> | null = null;
export function noteLocalWorkspacesChanged(): void {
  if (pending !== null) return;
  pending = setTimeout(() => {
    pending = null;
    void loadLocalWorkspaces();
  }, 150);
}

/** The fields of a session row that name its worktree. */
export interface WorktreeOf {
  host_alias: string;
  project_id: number | null;
  worktree_key: string | null;
}

/** The link on `s`'s worktree, if there is one. A session with no
 *  `worktree_key` works in the project root, which links key as `main`. */
export function linkFor(rows: LocalWorkspace[], s: WorktreeOf): LocalWorkspace | undefined {
  if (s.project_id == null) return undefined;
  const key = s.worktree_key || 'main';
  return rows.find(
    (w) => w.host_alias === s.host_alias && w.project_id === s.project_id && w.worktree_key === key,
  );
}

export type Tone = 'ok' | 'off' | 'pending' | 'conflict' | 'idle' | 'error';

/** The dot a link shows, and its words. `undefined` = no link (grey). */
export function badgeFor(w: LocalWorkspace | undefined): { tone: Tone; label: string } {
  if (!w) return { tone: 'off', label: 'Local sync off' };
  if (w.paused) return { tone: 'idle', label: 'Paused' };
  switch (w.state) {
    case 'synced':
      return { tone: 'ok', label: 'Synced' };
    case 'conflict': {
      const n = w.conflicts.length;
      return { tone: 'conflict', label: n === 1 ? '1 conflict' : `${n} conflicts` };
    }
    case 'local_changes':
      return { tone: 'pending', label: `${w.pending_local || ''} local changes`.trim() };
    case 'remote_changes':
      return { tone: 'pending', label: `${w.pending_remote || ''} remote changes`.trim() };
    case 'syncing':
      return { tone: 'pending', label: 'Syncing' };
    case 'offline':
      return { tone: 'idle', label: 'Host offline' };
    case 'error':
      return { tone: 'error', label: 'Sync error' };
    default:
      return { tone: 'pending', label: String(w.state) };
  }
}

export const CONFLICT_KIND_LABEL: Record<string, string> = {
  both_modified: 'changed on both sides',
  both_added: 'added on both sides with different content',
  local_deleted: 'deleted locally, changed on the host',
  remote_deleted: 'changed locally, deleted on the host',
};

function replace(row: LocalWorkspace): void {
  localWorkspaces.update((rows) => {
    const i = rows.findIndex((w) => w.id === row.id);
    if (i < 0) return [...rows, row];
    const next = rows.slice();
    next[i] = row;
    return next;
  });
}

async function act(
  what: string,
  cmd: string,
  args: Record<string, unknown>,
): Promise<LocalWorkspace | null> {
  const r = await invokeCmd<LocalWorkspace>(cmd, { args });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `${what}: ${r.error.message}` });
    return null;
  }
  if (r.value && typeof r.value === 'object') replace(r.value);
  return r.value;
}

export function enableLocalWorkspace(
  sessionId: number,
  localPath: string,
  excludes: string[] = [],
): Promise<LocalWorkspace | null> {
  return act('Enable local sync', 'enable_local_workspace', {
    session_id: sessionId,
    local_path: localPath,
    excludes,
  });
}

export const pauseLocalWorkspace = (id: number) =>
  act('Pause sync', 'pause_local_workspace', { id });
export const resumeLocalWorkspace = (id: number) =>
  act('Resume sync', 'resume_local_workspace', { id });
export const syncLocalWorkspaceNow = (id: number) =>
  act('Sync now', 'sync_local_workspace_now', { id });
export const setLocalWorkspaceExcludes = (id: number, excludes: string[]) =>
  act('Excludes', 'set_local_workspace_excludes', { id, excludes });
export const resolveLocalConflict = (id: number, path: string, keep: 'local' | 'remote') =>
  act('Resolve conflict', 'resolve_local_workspace_conflict', { id, path, keep });

/** Drop the link; files on both sides stay. */
export async function disconnectLocalWorkspace(id: number): Promise<boolean> {
  const r = await invokeCmd<null>('disconnect_local_workspace', { args: { id } });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Disconnect: ${r.error.message}` });
    return false;
  }
  localWorkspaces.set(get(localWorkspaces).filter((w) => w.id !== id));
  return true;
}

/** A suggested folder for a new link: `~/fleet/<repo>` or
 *  `~/fleet/<repo>-<worktree>`. */
export function suggestedFolder(repo: string, worktreeKey: string | null): string {
  const key = worktreeKey && worktreeKey !== 'main' ? `-${worktreeKey}` : '';
  return `~/fleet/${repo}${key}`;
}

/** For tests. */
export function _resetLocalWorkspacesForTests(): void {
  if (pending !== null) clearTimeout(pending);
  pending = null;
  localWorkspaces.set([]);
}
