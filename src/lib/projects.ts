import { writable, derived } from 'svelte/store';
import { invokeCmd, invokeCmdAbortable, type IpcError, type Result } from './result';

export interface ProjectRow {
  id: number;
  owner: string;
  repo: string;
  base_path: string;
  last_session_at: number | null;
  /** Registered by adopting an existing checkout already on disk (possibly
   * outside the projects root) rather than by the scan or a clone. */
  adopted: boolean;
}

export interface WorktreeRow {
  id: number;
  project_id: number;
  /** Host whose checkout this is: 'local' for the project scan's rows, a
   * remote alias for rows its EnterWorktree hook reported. The project tree
   * lists local rows only. */
  host_alias: string;
  name: string;
  path: string;
  branch: string | null;
}

export interface ProjectTreeRow {
  project: ProjectRow;
  worktrees: WorktreeRow[];
}

export const projects = writable<ProjectTreeRow[]>([]);

/** O(1) project-id -> tree-row lookup, derived once per `projects` change. */
export const projectById = derived(projects, ($p) => new Map($p.map((p) => [p.project.id, p])));

export async function loadProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export async function refreshProjects(): Promise<Result<ProjectTreeRow[]>> {
  const r = await invokeCmd<ProjectTreeRow[]>('refresh_projects');
  if (r.ok) projects.set(r.value);
  return r;
}

export function mergeProject(row: ProjectTreeRow): void {
  projects.update((arr) => {
    const i = arr.findIndex((p) => p.project.id === row.project.id);
    if (i === -1) return [...arr, row];
    const next = arr.slice();
    next[i] = row;
    return next;
  });
}

/**
 * Adapter for the `project:updated` Tauri event, which emits a bare `ProjectRow`
 * (no nested worktrees — those have their own `worktree:updated` event). If the
 * project is already in the store, update only its `project` field and preserve
 * the existing `worktrees` array. If it's new, seed an empty `worktrees: []`.
 */
function mergeProjectRow(arr: ProjectTreeRow[], row: ProjectRow): ProjectTreeRow[] {
  const i = arr.findIndex((p) => p.project.id === row.id);
  if (i === -1) return [...arr, { project: row, worktrees: [] }];
  const next = arr.slice();
  next[i] = { project: row, worktrees: arr[i].worktrees };
  return next;
}

function mergeWorktreeRow(arr: ProjectTreeRow[], row: WorktreeRow): ProjectTreeRow[] {
  const idx = arr.findIndex((p) => p.project.id === row.project_id);
  if (idx === -1) return arr;
  const entry = arr[idx];
  const wts = entry.worktrees ?? [];
  const wIdx = wts.findIndex((w) => w.id === row.id);
  const newWts =
    wIdx === -1 ? [...wts, row] : wts.map((w) => (w.id === row.id ? row : w));
  const next = arr.slice();
  next[idx] = { ...entry, worktrees: newWts };
  return next;
}

function removeWorktreeRow(arr: ProjectTreeRow[], id: number): ProjectTreeRow[] {
  if (!arr.some((entry) => entry.worktrees?.some((w) => w.id === id))) return arr;
  return arr.map((entry) => {
    if (!entry.worktrees?.some((w) => w.id === id)) return entry;
    return { ...entry, worktrees: entry.worktrees.filter((w) => w.id !== id) };
  });
}

function removeWorktree(id: number): void {
  projects.update((arr) => removeWorktreeRow(arr, id));
}

/** One backend project/worktree event, as delivered by `events.ts`. Both
 *  kinds land in the same `projects` store, so they batch together. */
export type ProjectEvent =
  | { type: 'project_updated'; row: ProjectRow }
  | { type: 'worktree_updated'; row: WorktreeRow }
  | { type: 'worktree_removed'; id: number };

/** Apply a burst of project + worktree events in ONE store update, in order. */
export function applyProjectEvents(events: readonly ProjectEvent[]): void {
  if (events.length === 0) return;
  projects.update((arr) => {
    let next = arr;
    for (const ev of events) {
      if (ev.type === 'project_updated') next = mergeProjectRow(next, ev.row);
      else if (ev.type === 'worktree_updated') next = mergeWorktreeRow(next, ev.row);
      else next = removeWorktreeRow(next, ev.id);
    }
    return next;
  });
}

/** Delete a git worktree on its host and drop the fleet row. The backend
 *  refuses (`E_WORKTREE_BUSY`) when an alive session uses it unless `force`. */
export async function deleteWorktree(
  worktreeId: number,
  force = false,
): Promise<Result<void>> {
  const r = await invokeCmd<void>('delete_worktree', {
    args: { worktree_id: worktreeId, force },
  });
  if (r.ok) removeWorktree(worktreeId);
  return r;
}

/** `list_host_worktrees` result: one project's worktrees as they exist on
 *  one host. `cloned: false` means the repo is not checked out there yet. */
export interface HostWorktrees {
  host_alias: string;
  project_id: number;
  cloned: boolean;
  worktrees: WorktreeRow[];
}

/** The worktrees of `projectId` on `hostAlias`. `local` answers from the
 *  DB; a remote host is scanned over SSH (one short call) and cached. */
export async function listHostWorktrees(
  hostAlias: string,
  projectId: number,
): Promise<Result<HostWorktrees>> {
  return invokeCmd<HostWorktrees>('list_host_worktrees', {
    args: { host_alias: hostAlias, project_id: projectId },
  });
}

/** Wire shape of `service::add_project::AddProjectArgs::source`
 *  (`#[serde(tag = "kind", rename_all = "snake_case")]`). */
export type AddProjectSource =
  | { kind: 'clone'; url: string }
  | { kind: 'folder'; path: string }
  | { kind: 'new'; owner: string; repo: string; create_remote: boolean; confirm?: string };

/** Wire shape of `service::add_project::GithubRepo`. */
export interface GithubRepo {
  name_with_owner: string;
  description: string | null;
  is_private: boolean;
  updated_at: string | null;
}

/**
 * Add a project fleet does not know about yet: clone a GitHub repo, adopt an
 * existing checkout, or create a new one. Cancellable via `signal` — see
 * `invokeCmdAbortable` and `AddProjectArgs::call_id`. Merges the returned
 * row into the `projects` store on success via `mergeProject`, so the
 * sidebar shows the new project without a refetch.
 */
export async function addProject(
  hostAlias: string,
  source: AddProjectSource,
  signal?: AbortSignal,
): Promise<Result<ProjectTreeRow>> {
  const r = await invokeCmdAbortable<ProjectTreeRow>(
    'add_project',
    { args: { host_alias: hostAlias, source } },
    signal,
  );
  if (r.ok) mergeProject(r.value);
  return r;
}

/** The repositories `gh` can see on `hostAlias`, for the Add-project
 *  dialog's browse mode. Read-only. */
export async function listGithubRepos(hostAlias: string): Promise<Result<GithubRepo[]>> {
  return invokeCmd<GithubRepo[]>('list_github_repos', { args: { host_alias: hostAlias } });
}

const CONFIRM_TOKEN_RE = /^[0-9a-f]{64}$/;

/**
 * The `create_remote` confirmation token carried in an `E_CONFIRM_REQUIRED`
 * error's `details.confirm` field (see `service::add_project::ConfirmTokens`),
 * for the Add-project dialog's two-step confirmation flow. `null` when the
 * error is not `E_CONFIRM_REQUIRED`, has no `details`, or the token is not a
 * well-formed 64-character hex string.
 */
export function confirmTokenOf(error: IpcError): string | null {
  if (error.code !== 'E_CONFIRM_REQUIRED') return null;
  const details = error.details;
  if (!details || typeof details !== 'object') return null;
  const confirm = (details as { confirm?: unknown }).confirm;
  return typeof confirm === 'string' && CONFIRM_TOKEN_RE.test(confirm) ? confirm : null;
}
