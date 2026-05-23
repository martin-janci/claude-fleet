import { writable, derived } from 'svelte/store';
import { invokeCmd, type Result } from './result';

export interface ProjectRow {
  id: number;
  owner: string;
  repo: string;
  base_path: string;
  last_session_at: number | null;
}

export interface WorktreeRow {
  id: number;
  project_id: number;
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

export async function bootstrapProjects(): Promise<void> {
  const r = await invokeCmd<ProjectTreeRow[]>('list_projects');
  if (r.ok) projects.set(r.value);
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
export function mergeProjectFromEvent(row: ProjectRow): void {
  projects.update((arr) => {
    const i = arr.findIndex((p) => p.project.id === row.id);
    if (i === -1) return [...arr, { project: row, worktrees: [] }];
    const next = arr.slice();
    next[i] = { project: row, worktrees: arr[i].worktrees };
    return next;
  });
}

export function mergeWorktree(row: WorktreeRow): void {
  projects.update((arr) => {
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
  });
}

export function removeWorktree(id: number): void {
  projects.update((arr) =>
    arr.map((entry) => {
      if (!entry.worktrees?.some((w) => w.id === id)) return entry;
      return { ...entry, worktrees: entry.worktrees.filter((w) => w.id !== id) };
    }),
  );
}

export interface WorktreeOccupant {
  host_alias: string;
  tmux_name: string;
}

export interface WorktreeOccupancy {
  worktree: WorktreeRow;
  occupants: WorktreeOccupant[];
}

/** List every worktree fleet knows about, each tagged with the alive Claude
 *  sessions currently using it. Pass `projectId` to scope to one project. */
export async function listWorktreeOccupancy(
  projectId: number | null = null,
): Promise<Result<WorktreeOccupancy[]>> {
  return invokeCmd<WorktreeOccupancy[]>('list_worktrees', {
    args: { project_id: projectId },
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
