// Local workspace sync
// (docs/superpowers/specs/2026-10-07-local-workspace-sync-design.md): one
// session's worktree kept in step, both ways, with a folder on this machine.
// Phases 2 and 3 (…-local-workspace-handoff-design.md): Open in IDE, what
// each side changed, diff / commit / discard, Ask AI, handoff, conflict
// compare and keep-both.
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
  /** Who drives the worktree. */
  driver?: LocalDriver;
  driver_since?: number | null;
  /** Changes carried folder → host that nobody has handed on yet. */
  local_activity?: number;
  /** Changes carried host → folder that nobody has looked at yet. */
  remote_activity?: number;
}

export type LocalDriver = 'shared' | 'developer' | 'agent';

export type OpenApp = 'folder' | 'vscode' | 'intellij' | 'terminal';

export const OPEN_APPS: { app: OpenApp; label: string }[] = [
  { app: 'vscode', label: 'VS Code' },
  { app: 'intellij', label: 'IntelliJ IDEA' },
  { app: 'terminal', label: 'Terminal' },
  { app: 'folder', label: 'Folder' },
];

export interface LocalActivity {
  path: string;
  origin: 'local' | 'remote' | string;
  change: 'added' | 'modified' | 'deleted' | string;
  at: number;
}

export interface LocalChange {
  path: string;
  status: string;
  staged: boolean;
  orig_path?: string | null;
  /** Which side's change the sync carried: `local`, `remote`, or none. */
  origin?: 'local' | 'remote' | string | null;
}

export interface LocalChanges {
  branch: string;
  files: LocalChange[];
  activity: LocalActivity[];
}

export interface FileDiff {
  path: string;
  diff: string;
  binary: boolean;
  truncated: boolean;
}

export type AskIntent =
  | 'explain'
  | 'review'
  | 'continue'
  | 'tests'
  | 'commit'
  | 'merge'
  | 'resolve'
  | 'custom';

/** The Ask AI menu, in order. `resolve` is offered on a conflict instead. */
export const ASK_INTENTS: { intent: AskIntent; label: string }[] = [
  { intent: 'continue', label: 'Continue the task from my changes' },
  { intent: 'review', label: 'Review my changes' },
  { intent: 'explain', label: 'Explain my changes' },
  { intent: 'tests', label: 'Write tests for my changes' },
  { intent: 'commit', label: 'Commit my changes' },
  { intent: 'merge', label: 'Get the branch ready to merge' },
  { intent: 'custom', label: 'Ask a question…' },
];

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
    case 'synced': {
      const n = w.local_activity ?? 0;
      if (n > 0) {
        return { tone: 'pending', label: n === 1 ? '1 local change' : `${n} local changes` };
      }
      return { tone: 'ok', label: 'Synced' };
    }
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

// ---------------------------------------------------------------------------
// Phases 2 and 3.
// ---------------------------------------------------------------------------

async function call<T>(what: string, cmd: string, args: Record<string, unknown>): Promise<T | null> {
  const r = await invokeCmd<T>(cmd, { args });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `${what}: ${r.error.message}` });
    return null;
  }
  return r.value;
}

/** Open the link's folder in an IDE, a terminal or the file manager. */
export async function openLocalWorkspace(id: number, app: OpenApp): Promise<boolean> {
  const r = await invokeCmd<null>('open_local_workspace', { args: { id, app } });
  if (!r.ok) {
    push({ kind: 'error', code: r.error.code, message: `Open: ${r.error.message}` });
    return false;
  }
  return true;
}

export const loadLocalChanges = (id: number) =>
  call<LocalChanges>('Changes', 'local_workspace_changes', { id });

export const loadLocalDiff = (id: number, path: string) =>
  call<FileDiff>('Diff', 'local_workspace_diff', { id, path });

export const commitLocalWorkspace = (id: number, message: string, paths: string[]) =>
  call<{ commit: string }>('Commit', 'commit_local_workspace', { id, message, paths });

export const discardLocalChanges = (id: number, paths: string[]) =>
  act('Discard', 'discard_local_workspace_changes', { id, paths });

export const dismissLocalActivity = (id: number, origin?: 'local' | 'remote') =>
  act('Dismiss', 'dismiss_local_workspace_activity', { id, origin: origin ?? null });

export const compareLocalConflict = (id: number, path: string) =>
  call<FileDiff>('Compare', 'compare_local_conflict', { id, path });

export const keepBothLocalConflict = (id: number, path: string) =>
  act('Keep both', 'keep_both_local_conflict', { id, path });

export function askAiAboutLocalChanges(
  id: number,
  intent: AskIntent,
  opts: { question?: string; paths?: string[] } = {},
): Promise<LocalWorkspace | null> {
  return act('Ask AI', 'ask_ai_about_local_changes', {
    id,
    intent,
    question: opts.question ?? null,
    paths: opts.paths ?? null,
  });
}

export const setLocalWorkspaceDriver = (id: number, driver: LocalDriver) =>
  act('Handoff', 'set_local_workspace_driver', { id, driver });

export const DRIVER_LABEL: Record<string, string> = {
  shared: 'Shared',
  developer: 'You’re driving',
  agent: 'Agent is driving',
};

/** Why a link is stale, or `null`: its folder or worktree is gone, or no
 *  live session uses the worktree. `liveSessions` are the session rows the
 *  app knows (any shape with the worktree fields and a status). */
export function staleReason(
  w: LocalWorkspace,
  liveSessions: (WorktreeOf & { status?: string })[],
): string | null {
  const err = w.last_error ?? '';
  // The sync's own words (local_sync::local / ::remote).
  if (/worktree folder is missing/i.test(err)) return 'The worktree is gone on the host';
  if (/local folder .* is missing/i.test(err)) return 'The local folder is gone';
  const used = liveSessions.some(
    (s) =>
      s.status !== 'ghost' &&
      s.host_alias === w.host_alias &&
      s.project_id != null &&
      s.project_id === w.project_id &&
      (s.worktree_key || 'main') === w.worktree_key,
  );
  return used ? null : 'No session uses this worktree';
}
